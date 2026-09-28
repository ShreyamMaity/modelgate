use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use regex::Regex;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Mutex;
use tokenizers::Tokenizer;

#[derive(Clone, Debug)]
pub struct Label {
    pub prompt: String,
    pub kind: String,
    pub threshold: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: String,
    pub score: f32,
}

pub struct Options {
    pub max_tokens: Option<usize>,
    pub arena: bool,
    pub opt_level: u8,
    pub prepack: bool,
    pub threads: usize,
    pub window: usize,
    pub overlap: usize,
    pub batch: usize,
    pub max_span_words: usize,
}

#[derive(Clone, Debug)]
pub struct Tag {
    pub kind: Option<usize>,
    pub begin: bool,
}

pub enum Backend {
    Gliner,
    TokCls { tags: Vec<Tag>, type_ids: bool },
}

pub struct Model {
    session: Mutex<Session>,
    tok: Tokenizer,
    labels: Vec<Label>,
    backend: Backend,
    words: Regex,
    pad_id: i64,
    max_tokens: usize,
    opts: Options,
}

struct Window {
    text: usize,
    first: usize,
    n: usize,
}

pub const MAX_WORD_BYTES: usize = 64;
pub const MAX_SYMBOL_RUN: usize = 3;

#[derive(Default, Debug)]
pub struct Stats {
    pub windows: usize,
    pub failed_windows: usize,
    pub skipped_words: usize,
    pub last_error: Option<String>,
}

pub fn model_limit(config: &Value) -> usize {
    let mpe = config
        .get("max_position_embeddings")
        .and_then(Value::as_u64)
        .unwrap_or(512) as usize;
    let offset = match config.get("model_type").and_then(Value::as_str) {
        Some("xlm-roberta" | "roberta" | "camembert") => 2,
        _ => 0,
    };
    mpe.saturating_sub(offset).min(8192)
}

pub fn plan_windows(
    counts: &[usize],
    budget: usize,
    max_words: usize,
    overlap: usize,
) -> (Vec<(usize, usize)>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut first = 0;
    while first < counts.len() {
        if counts[first] > budget {
            skipped += 1;
            first += 1;
            continue;
        }
        let mut n = 0;
        let mut used = 0;
        while first + n < counts.len() && n < max_words && used + counts[first + n] <= budget {
            used += counts[first + n];
            n += 1;
        }
        out.push((first, n));
        if first + n >= counts.len() {
            break;
        }
        let mut back = 0;
        let mut back_tokens = 0;
        while back < overlap
            && back + 1 < n
            && back_tokens + counts[first + n - 1 - back] <= budget / 4
        {
            back_tokens += counts[first + n - 1 - back];
            back += 1;
        }
        first += n - back;
    }
    (out, skipped)
}

type Cand = (usize, usize, usize, f32);

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

pub fn parse_labels(spec: &str, default_thr: f32) -> Vec<Label> {
    spec.split(['|', ';'])
        .filter_map(|part| {
            let mut it = part.split(':').map(str::trim);
            let prompt = it.next().filter(|s| !s.is_empty())?.to_owned();
            let kind = it.next().filter(|s| !s.is_empty())?.to_ascii_uppercase();
            let threshold = it
                .next()
                .and_then(|t| t.parse::<f32>().ok())
                .unwrap_or(default_thr);
            Some(Label {
                prompt,
                kind,
                threshold,
            })
        })
        .collect()
}

pub fn tags_from_config(config: &Value, labels: &[Label]) -> Result<Vec<Tag>, String> {
    let map = config
        .get("id2label")
        .and_then(Value::as_object)
        .ok_or("config.json has no id2label")?;
    let mut tags = vec![
        Tag {
            kind: None,
            begin: false,
        };
        map.len()
    ];
    for (id, name) in map {
        let i: usize = id.parse().map_err(|_| format!("bad label id {id}"))?;
        let name = name.as_str().unwrap_or("O");
        let (begin, ty) = match name.split_once('-') {
            Some((p, t)) if p.len() == 1 => (p.eq_ignore_ascii_case("B"), t),
            _ => (false, name),
        };
        let kind = labels
            .iter()
            .position(|l| l.prompt.eq_ignore_ascii_case(ty));
        if i < tags.len() {
            tags[i] = Tag { kind, begin };
        }
    }
    Ok(tags)
}

pub fn greedy(mut cands: Vec<Cand>) -> Vec<Cand> {
    cands.sort_by(|a, b| {
        b.3.partial_cmp(&a.3)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let mut out: Vec<Cand> = Vec::new();
    for c in cands {
        if out.iter().all(|o| c.1 < o.0 || o.1 < c.0) {
            out.push(c);
        }
    }
    out.sort_by_key(|c| c.0);
    out
}

pub fn decode_gliner(
    logits: &[f32],
    n_words: usize,
    labels: &[Label],
    max_span: usize,
) -> Vec<Cand> {
    let c = labels.len();
    let at = |w: usize, k: usize, p: usize| sigmoid(logits[(w * c + k) * 3 + p]);
    let mut cands = Vec::new();
    for (k, label) in labels.iter().enumerate() {
        let thr = label.threshold;
        let starts: Vec<usize> = (0..n_words).filter(|&w| at(w, k, 0) > thr).collect();
        if starts.is_empty() {
            continue;
        }
        let ends: Vec<usize> = (0..n_words).filter(|&w| at(w, k, 1) > thr).collect();
        for &st in &starts {
            for &ed in ends.iter().filter(|&&e| e >= st && e - st < max_span) {
                let mut score = at(st, k, 0).min(at(ed, k, 1));
                let mut ok = true;
                for w in st..=ed {
                    let s = at(w, k, 2);
                    if s < thr {
                        ok = false;
                        break;
                    }
                    score = score.min(s);
                }
                if ok {
                    cands.push((st, ed, k, score));
                }
            }
        }
    }
    greedy(cands)
}

pub fn decode_tags(word_tags: &[(usize, f32)], tags: &[Tag], labels: &[Label]) -> Vec<Cand> {
    let mut out = Vec::new();
    let mut cur: Option<(usize, usize, usize, f32, usize)> = None;
    let close = |c: Option<(usize, usize, usize, f32, usize)>, out: &mut Vec<Cand>| {
        if let Some((st, ed, k, sum, n)) = c {
            let score = sum / n as f32;
            if score >= labels[k].threshold {
                out.push((st, ed, k, score));
            }
        }
    };
    for (w, &(t, p)) in word_tags.iter().enumerate() {
        let tag = tags.get(t).cloned().unwrap_or(Tag {
            kind: None,
            begin: false,
        });
        match (tag.kind, cur) {
            (Some(k), Some((st, _, ck, sum, n))) if ck == k && !tag.begin => {
                cur = Some((st, w, k, sum + p, n + 1));
            }
            (Some(k), c) => {
                close(c, &mut out);
                cur = Some((w, w, k, p, 1));
            }
            (None, c) => {
                close(c, &mut out);
                cur = None;
            }
        }
    }
    close(cur, &mut out);
    out
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl Model {
    pub fn load(
        model_path: &str,
        tokenizer_path: &str,
        labels: Vec<Label>,
        config: Option<Value>,
        opts: Options,
    ) -> Result<Model, String> {
        if labels.is_empty() {
            return Err("no labels configured".into());
        }
        let max_tokens = opts
            .max_tokens
            .or_else(|| config.as_ref().map(model_limit))
            .unwrap_or(512)
            .max(32);
        let session = Session::builder()
            .map_err(err)?
            .with_optimization_level(match opts.opt_level {
                0 => GraphOptimizationLevel::Disable,
                1 => GraphOptimizationLevel::Level1,
                2 => GraphOptimizationLevel::Level2,
                _ => GraphOptimizationLevel::Level3,
            })
            .map_err(err)?
            .with_intra_threads(opts.threads.max(1))
            .map_err(err)?
            .with_inter_threads(1)
            .map_err(err)?
            .with_memory_pattern(false)
            .map_err(err)?
            .with_execution_providers([ort::ep::CPU::default()
                .with_arena_allocator(opts.arena)
                .build()])
            .map_err(err)?
            .with_config_entry(
                "session.disable_prepacking",
                if opts.prepack { "0" } else { "1" },
            )
            .map_err(err)?
            .commit_from_file(model_path)
            .map_err(err)?;
        let names: Vec<String> = session
            .inputs()
            .iter()
            .map(|i| i.name().to_owned())
            .collect();
        let backend = if names.iter().any(|n| n == "words_mask") {
            Backend::Gliner
        } else {
            let cfg = config.ok_or("token classification model needs NER_CONFIG (config.json)")?;
            Backend::TokCls {
                tags: tags_from_config(&cfg, &labels)?,
                type_ids: names.iter().any(|n| n == "token_type_ids"),
            }
        };
        let mut tok = Tokenizer::from_file(tokenizer_path).map_err(err)?;
        let pad_id = tok
            .get_padding()
            .map(|p| i64::from(p.pad_id))
            .or_else(|| {
                ["[PAD]", "<pad>", "<|padding|>"]
                    .iter()
                    .find_map(|t| tok.token_to_id(t))
                    .map(i64::from)
            })
            .unwrap_or(0);
        tok.with_truncation(None).map_err(err)?;
        tok.with_padding(None);
        Ok(Model {
            session: Mutex::new(session),
            tok,
            labels,
            backend,
            words: Regex::new(r"\w+(?:[-_]\w+)*|\S").map_err(err)?,
            pad_id,
            max_tokens,
            opts,
        })
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    pub fn backend(&self) -> &'static str {
        match self.backend {
            Backend::Gliner => "gliner",
            Backend::TokCls { .. } => "token-classification",
        }
    }

    fn prompt(&self) -> Vec<String> {
        if !matches!(self.backend, Backend::Gliner) {
            return Vec::new();
        }
        let mut p = Vec::new();
        for l in &self.labels {
            p.push("<<ENT>>".to_owned());
            p.push(l.prompt.clone());
        }
        p.push("<<SEP>>".to_owned());
        p
    }

    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    fn budget(&self) -> usize {
        let prompt: usize = self
            .prompt()
            .iter()
            .map(|p| {
                self.tok
                    .encode(p.as_str(), false)
                    .map(|e| e.len())
                    .unwrap_or(4)
            })
            .sum();
        self.max_tokens.saturating_sub(prompt + 2 + 8).max(16)
    }

    pub fn predict(&self, texts: &[String]) -> Result<(Vec<Vec<Span>>, Stats), String> {
        let mut stats = Stats::default();
        let mut words: Vec<Vec<(usize, usize)>> = Vec::with_capacity(texts.len());
        for t in texts {
            let mut ws = Vec::new();
            let mut run = 0;
            for m in self.words.find_iter(t) {
                let symbol = !m.as_str().chars().any(char::is_alphanumeric);
                run = if symbol { run + 1 } else { 0 };
                if m.end() - m.start() > MAX_WORD_BYTES || run > MAX_SYMBOL_RUN {
                    stats.skipped_words += 1;
                } else {
                    ws.push((m.start(), m.end()));
                }
            }
            words.push(ws);
        }
        let budget = self.budget();
        let mut windows = Vec::new();
        for (ti, ws) in words.iter().enumerate() {
            let pieces: Vec<&str> = ws.iter().map(|&(s, e)| &texts[ti][s..e]).collect();
            let counts: Vec<usize> = match self.tok.encode_batch(pieces, false) {
                Ok(encs) => encs.iter().map(|e| e.len().max(1)).collect(),
                Err(_) => ws.iter().map(|&(s, e)| e - s).collect(),
            };
            let (plan, skipped) =
                plan_windows(&counts, budget, self.opts.window.max(8), self.opts.overlap);
            stats.skipped_words += skipped;
            windows.extend(
                plan.into_iter()
                    .map(|(first, n)| Window { text: ti, first, n }),
            );
        }
        stats.windows = windows.len();
        let mut per_text: Vec<Vec<Cand>> = vec![Vec::new(); texts.len()];
        for group in windows.chunks(self.opts.batch.max(1)) {
            self.run_safe(texts, &words, group, &mut per_text, &mut stats);
        }
        let spans = per_text
            .into_iter()
            .enumerate()
            .map(|(ti, cands)| {
                greedy(cands)
                    .into_iter()
                    .map(|(st, ed, k, score)| Span {
                        start: words[ti][st].0,
                        end: words[ti][ed].1,
                        kind: self.labels[k].kind.clone(),
                        score,
                    })
                    .collect()
            })
            .collect();
        Ok((spans, stats))
    }

    fn run_safe(
        &self,
        texts: &[String],
        words: &[Vec<(usize, usize)>],
        group: &[Window],
        out: &mut [Vec<Cand>],
        stats: &mut Stats,
    ) {
        match self.run_group(texts, words, group) {
            Ok(found) => {
                for (ti, c) in found {
                    out[ti].push(c);
                }
            }
            Err(e) => {
                stats.last_error = Some(e);
                if group.len() > 1 {
                    for w in group {
                        self.run_safe(texts, words, std::slice::from_ref(w), out, stats);
                    }
                } else if group[0].n > 1 {
                    let w = &group[0];
                    let h = w.n / 2;
                    let a = Window {
                        text: w.text,
                        first: w.first,
                        n: h,
                    };
                    let b = Window {
                        text: w.text,
                        first: w.first + h,
                        n: w.n - h,
                    };
                    self.run_safe(texts, words, &[a], out, stats);
                    self.run_safe(texts, words, &[b], out, stats);
                } else {
                    stats.failed_windows += 1;
                }
            }
        }
    }

    fn run_group(
        &self,
        texts: &[String],
        words: &[Vec<(usize, usize)>],
        group: &[Window],
    ) -> Result<Vec<(usize, Cand)>, String> {
        let prompt = self.prompt();
        let mut found = Vec::new();
        let mut encs = Vec::with_capacity(group.len());
        for w in group {
            let t = &texts[w.text];
            let mut seq: Vec<&str> = prompt.iter().map(String::as_str).collect();
            for &(s, e) in &words[w.text][w.first..w.first + w.n] {
                seq.push(&t[s..e]);
            }
            let enc = self.tok.encode(seq, true).map_err(err)?;
            if enc.len() > self.max_tokens {
                return Err(format!(
                    "window of {} tokens exceeds {}",
                    enc.len(),
                    self.max_tokens
                ));
            }
            let mut wm = Vec::with_capacity(enc.len());
            let mut prev: Option<u32> = None;
            for wid in enc.get_word_ids() {
                let v = match wid {
                    Some(x) if (*x as usize) >= prompt.len() && Some(*x) != prev => {
                        (*x as usize - prompt.len() + 1) as i64
                    }
                    _ => 0,
                };
                wm.push(v);
                prev = *wid;
            }
            encs.push((
                enc.get_ids()
                    .iter()
                    .map(|&i| i64::from(i))
                    .collect::<Vec<i64>>(),
                wm,
            ));
        }
        let b = encs.len();
        let len = encs.iter().map(|e| e.0.len()).max().unwrap_or(0);
        let mut ids = vec![self.pad_id; b * len];
        let mut mask = vec![0i64; b * len];
        let mut wmask = vec![0i64; b * len];
        let mut lens = vec![0i64; b];
        for (i, (e, wm)) in encs.iter().enumerate() {
            ids[i * len..i * len + e.len()].copy_from_slice(e);
            mask[i * len..i * len + e.len()].fill(1);
            wmask[i * len..i * len + wm.len()].copy_from_slice(wm);
            lens[i] = group[i].n as i64;
        }
        let mut session = self.session.lock().map_err(|_| "session poisoned")?;
        let mut inputs: Vec<(Cow<str>, SessionInputValue)> = ort::inputs![
            "input_ids" => Tensor::from_array(([b, len], ids)).map_err(err)?,
            "attention_mask" => Tensor::from_array(([b, len], mask)).map_err(err)?,
        ];
        match &self.backend {
            Backend::Gliner => {
                inputs.push((
                    "words_mask".into(),
                    Tensor::from_array(([b, len], wmask.clone()))
                        .map_err(err)?
                        .into(),
                ));
                inputs.push((
                    "text_lengths".into(),
                    Tensor::from_array(([b, 1], lens)).map_err(err)?.into(),
                ));
            }
            Backend::TokCls { type_ids: true, .. } => {
                inputs.push((
                    "token_type_ids".into(),
                    Tensor::from_array(([b, len], vec![0i64; b * len]))
                        .map_err(err)?
                        .into(),
                ));
            }
            Backend::TokCls { .. } => {}
        }
        let outputs = session.run(inputs).map_err(err)?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(err)?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        match &self.backend {
            Backend::Gliner => {
                if dims.len() != 4 || dims[0] != b || dims[2] != self.labels.len() || dims[3] != 3 {
                    return Err(format!("unexpected logits shape {dims:?}"));
                }
                let per = dims[1] * dims[2] * 3;
                for (i, w) in group.iter().enumerate() {
                    let slice = &data[i * per..(i + 1) * per];
                    for (st, ed, k, sc) in decode_gliner(
                        slice,
                        w.n.min(dims[1]),
                        &self.labels,
                        self.opts.max_span_words,
                    ) {
                        found.push((w.text, (w.first + st, w.first + ed, k, sc)));
                    }
                }
            }
            Backend::TokCls { tags, .. } => {
                if dims.len() != 3 || dims[0] != b || dims[1] != len || dims[2] != tags.len() {
                    return Err(format!("unexpected logits shape {dims:?}"));
                }
                let nl = dims[2];
                for (i, w) in group.iter().enumerate() {
                    let mut word_tags = vec![(0usize, 0f32); w.n];
                    for (pos, &wm) in wmask[i * len..(i + 1) * len].iter().enumerate() {
                        if wm == 0 || wm as usize > w.n {
                            continue;
                        }
                        let row = &data[(i * len + pos) * nl..(i * len + pos + 1) * nl];
                        let mx = row.iter().cloned().fold(f32::MIN, f32::max);
                        let sum: f32 = row.iter().map(|x| (x - mx).exp()).sum();
                        let (arg, best) = row
                            .iter()
                            .enumerate()
                            .fold((0, f32::MIN), |a, (j, &x)| if x > a.1 { (j, x) } else { a });
                        word_tags[wm as usize - 1] = (arg, (best - mx).exp() / sum);
                    }
                    for (st, ed, k, sc) in decode_tags(&word_tags, tags, &self.labels) {
                        found.push((w.text, (w.first + st, w.first + ed, k, sc)));
                    }
                }
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_parse() {
        let l = parse_labels("name:person|location address:ADDRESS:0.4", 0.5);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].kind, "PERSON");
        assert_eq!(l[1].prompt, "location address");
        assert!((l[1].threshold - 0.4).abs() < 1e-6);
        assert!(parse_labels("bad", 0.5).is_empty());
    }

    #[test]
    fn gliner_decode_and_overlap() {
        let labels = parse_labels("name:PERSON", 0.5);
        let mut l = vec![-5.0; 4 * 3];
        l[3] = 5.0;
        l[3 + 2] = 5.0;
        l[6 + 1] = 5.0;
        l[6 + 2] = 5.0;
        let out = decode_gliner(&l, 4, &labels, 12);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].0, out[0].1), (1, 2));
        let g = greedy(vec![(0, 2, 0, 0.9), (1, 3, 0, 0.95), (4, 4, 0, 0.6)]);
        assert_eq!(
            g.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(),
            vec![(1, 3), (4, 4)]
        );
    }

    #[test]
    fn windows_respect_token_budget() {
        let counts = vec![3, 1, 700, 2, 2, 2, 400, 1, 1];
        let (plan, skipped) = plan_windows(&counts, 500, 160, 24);
        assert_eq!(skipped, 1);
        for &(f, n) in &plan {
            assert!(n >= 1);
            assert!(counts[f..f + n].iter().sum::<usize>() <= 500, "{plan:?}");
            assert!(!(f..f + n).contains(&2));
        }
        let covered: std::collections::HashSet<usize> =
            plan.iter().flat_map(|&(f, n)| f..f + n).collect();
        for i in 0..counts.len() {
            assert_eq!(covered.contains(&i), i != 2, "word {i} {plan:?}");
        }
        let many = vec![7usize; 5000];
        let (plan, _) = plan_windows(&many, 500, 160, 24);
        assert!(plan.iter().all(|&(_, n)| n * 7 <= 500));
        assert!(plan
            .windows(2)
            .all(|w| w[1].0 > w[0].0 && w[1].0 <= w[0].0 + w[0].1));
        assert_eq!(plan.last().map(|&(f, n)| f + n), Some(5000));
        let (plan, skipped) = plan_windows(&[], 500, 160, 24);
        assert!(plan.is_empty() && skipped == 0);
    }

    #[test]
    fn model_limits() {
        assert_eq!(
            model_limit(&json!({"model_type": "xlm-roberta", "max_position_embeddings": 514})),
            512
        );
        assert_eq!(
            model_limit(&json!({"model_type": "bert", "max_position_embeddings": 512})),
            512
        );
        assert_eq!(model_limit(&json!({})), 512);
    }

    #[test]
    #[ignore]
    fn pathological_inputs_with_real_model() {
        let dir = std::env::var("NER_TEST_MODEL_DIR").expect("NER_TEST_MODEL_DIR");
        let cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{dir}/config.json")).unwrap())
                .unwrap();
        let m = Model::load(
            &format!("{dir}/model_int8.onnx"),
            &format!("{dir}/tokenizer.json"),
            parse_labels("PER:PERSON|ORG:ORG|LOC:LOCATION", 0.5),
            Some(cfg),
            Options {
                max_tokens: None,
                arena: false,
                opt_level: 3,
                prepack: true,
                threads: 2,
                window: 160,
                overlap: 24,
                batch: 4,
                max_span_words: 30,
            },
        )
        .unwrap();
        let tail = " Please call Priya Venkataraman tomorrow.";
        let b64: String = (0..60_000)
            .map(|i| (b'A' + (i * 7 % 26) as u8) as char)
            .collect();
        let url = format!("https://example.com/a?{}", "q=x%2Fy&".repeat(3000));
        let json_min = format!(
            "{{{}}}",
            (0..3000)
                .map(|i| format!("\"k{i}\":[{i},\"v{i}\"]"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let cjk = "\u{6f22}\u{5b57}".repeat(4000);
        let deva = "\u{0915}\u{093e}\u{0932}".repeat(3000);
        let emoji = "\u{1f600}".repeat(3000);
        let punct = "!?.,;:".repeat(3000);
        let hex = "deadbeef ".repeat(4000);
        let mixed = (0..2000)
            .map(|i| format!("x{i}y-{i}_z"))
            .collect::<Vec<_>>()
            .join(" ");
        let texts: Vec<String> = [b64, url, json_min, cjk, deva, emoji, punct, hex, mixed]
            .into_iter()
            .map(|t| t + tail)
            .collect();
        let (spans, stats) = m.predict(&texts).unwrap();
        assert_eq!(stats.failed_windows, 0, "{stats:?}");
        assert!(stats.last_error.is_none(), "{stats:?}");
        for (t, sp) in texts.iter().zip(&spans) {
            assert!(
                sp.iter()
                    .any(|s| t[s.start..s.end].contains("Venkataraman")),
                "missed name after {:?}: {sp:?}",
                t.chars().take(20).collect::<String>()
            );
        }
    }

    #[test]
    fn bio_decode() {
        let labels = parse_labels("PER:PERSON|LOC:LOCATION", 0.5);
        let cfg = json!({"id2label": {"0": "O", "1": "B-PER", "2": "I-PER", "3": "B-LOC", "4": "I-LOC", "5": "B-MISC"}});
        let tags = tags_from_config(&cfg, &labels).unwrap();
        let wt = [
            (0, 0.9),
            (1, 0.9),
            (2, 0.8),
            (1, 0.9),
            (0, 0.9),
            (4, 0.7),
            (5, 0.9),
            (3, 0.3),
        ];
        let out = decode_tags(&wt, &tags, &labels);
        assert_eq!(
            out.iter().map(|c| (c.0, c.1, c.2)).collect::<Vec<_>>(),
            vec![(1, 2, 0), (3, 3, 0), (5, 5, 1)]
        );
    }
}
