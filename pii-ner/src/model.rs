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
    opts: Options,
}

struct Window {
    text: usize,
    first: usize,
    n: usize,
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

    pub fn predict(&self, texts: &[String]) -> Result<Vec<Vec<Span>>, String> {
        let words: Vec<Vec<(usize, usize)>> = texts
            .iter()
            .map(|t| {
                self.words
                    .find_iter(t)
                    .map(|m| (m.start(), m.end()))
                    .collect()
            })
            .collect();
        let win = self.opts.window.max(8);
        let step = win.saturating_sub(self.opts.overlap).max(1);
        let mut windows = Vec::new();
        for (ti, ws) in words.iter().enumerate() {
            let mut first = 0;
            while first < ws.len() {
                let n = win.min(ws.len() - first);
                windows.push(Window { text: ti, first, n });
                if first + n >= ws.len() {
                    break;
                }
                first += step;
            }
        }
        let mut per_text: Vec<Vec<Cand>> = vec![Vec::new(); texts.len()];
        let prompt = self.prompt();
        for group in windows.chunks(self.opts.batch.max(1)) {
            let mut encs = Vec::with_capacity(group.len());
            for w in group {
                let t = &texts[w.text];
                let mut seq: Vec<&str> = prompt.iter().map(String::as_str).collect();
                for &(s, e) in &words[w.text][w.first..w.first + w.n] {
                    seq.push(&t[s..e]);
                }
                let enc = self.tok.encode(seq, true).map_err(err)?;
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
                    if dims.len() != 4
                        || dims[0] != b
                        || dims[2] != self.labels.len()
                        || dims[3] != 3
                    {
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
                            per_text[w.text].push((w.first + st, w.first + ed, k, sc));
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
                            per_text[w.text].push((w.first + st, w.first + ed, k, sc));
                        }
                    }
                }
            }
        }
        Ok(per_text
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
            .collect())
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
