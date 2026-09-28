use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Default)]
pub struct Rehydrate {
    pub text: HashMap<String, String>,
    pub tool_extra: HashMap<String, String>,
    pub tools: bool,
    pub names: Vec<(String, String)>,
    ac: OnceLock<Option<(AhoCorasick, usize)>>,
    fz: OnceLock<Fuzzy>,
}

type Fuzzy = (Vec<(String, usize)>, HashSet<String>);

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn alpha_words(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut st: Option<usize> = None;
    let mut prev: Option<char> = None;
    for (i, c) in s.char_indices() {
        if c.is_alphabetic() {
            if st.is_none() && !prev.is_some_and(|p| p.is_alphanumeric() || p == '_') {
                st = Some(i);
            }
        } else if let Some(a) = st.take() {
            if !(c.is_ascii_digit() || c == '_') {
                out.push((a, i));
            }
        }
        prev = Some(c);
    }
    if let Some(a) = st {
        out.push((a, s.len()));
    }
    out
}

const NAME_SKIP: &[&str] = &[
    "id",
    "model",
    "role",
    "type",
    "object",
    "finish_reason",
    "stop_reason",
    "name",
    "call_id",
    "tool_call_id",
    "item_id",
    "status",
    "signature",
    "system_fingerprint",
];

const MAX_PH: usize = 100;

fn ph_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')
}

pub fn split_hold(text: &str) -> (&str, &str) {
    if let Some(i) = text.rfind('<') {
        let tail = &text[i..];
        if !tail.contains('>') && tail.len() <= MAX_PH && tail[1..].chars().all(ph_char) {
            return (&text[..i], tail);
        }
    }
    (text, "")
}

fn partial_of(s: &str, pat: &str) -> bool {
    !s.is_empty() && s.len() < pat.len() && pat[..s.len()].eq_ignore_ascii_case(s)
}

pub fn split_hold_escaped(text: &str) -> (&str, &str) {
    if let Some(i) = text.rfind('<') {
        let tail = &text[i..];
        let body = &tail[1..];
        let k = body.find(|c: char| !ph_char(c)).unwrap_or(body.len());
        let rest = &body[k..];
        if !tail.contains('>')
            && tail.len() <= MAX_PH
            && (rest.is_empty() || partial_of(rest, "\\u003e"))
        {
            return (&text[..i], tail);
        }
    }
    for n in (1..=5).rev() {
        if text.len() >= n && text.is_char_boundary(text.len() - n) {
            let t = &text[text.len() - n..];
            if partial_of(t, "\\u003c") {
                return (&text[..text.len() - n], t);
            }
        }
    }
    (text, "")
}

pub fn unescape_angles(s: &str) -> Option<String> {
    if !s.contains("\\u003") {
        return None;
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    let mut changed = false;
    while i < b.len() {
        if b[i] == b'\\' {
            if b.get(i + 1) == Some(&b'\\') {
                i += 2;
                continue;
            }
            if i + 6 <= b.len() && b[i + 1..i + 5].eq_ignore_ascii_case(b"u003") {
                let rep = match b[i + 5].to_ascii_lowercase() {
                    b'c' => Some('<'),
                    b'e' => Some('>'),
                    _ => None,
                };
                if let Some(c) = rep {
                    out.push_str(&s[last..i]);
                    out.push(c);
                    i += 6;
                    last = i;
                    changed = true;
                    continue;
                }
            }
        }
        i += 1;
    }
    if !changed {
        return None;
    }
    out.push_str(&s[last..]);
    Some(out)
}

impl Rehydrate {
    pub fn new(
        text: HashMap<String, String>,
        tool_extra: HashMap<String, String>,
        tools: bool,
        names: Vec<(String, String)>,
    ) -> Rehydrate {
        Rehydrate {
            text,
            tool_extra,
            tools,
            names,
            ac: OnceLock::new(),
            fz: OnceLock::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.names.is_empty() && (self.tool_extra.is_empty() || !self.tools)
    }

    fn names_ac(&self) -> Option<&(AhoCorasick, usize)> {
        self.ac
            .get_or_init(|| {
                if self.names.is_empty() {
                    return None;
                }
                let max = self.names.iter().map(|n| n.0.len()).max().unwrap_or(0);
                AhoCorasickBuilder::new()
                    .match_kind(MatchKind::LeftmostLongest)
                    .ascii_case_insensitive(true)
                    .build(self.names.iter().map(|n| n.0.as_str()))
                    .ok()
                    .map(|a| (a, max))
            })
            .as_ref()
    }

    fn fuzzy(&self) -> &Fuzzy {
        self.fz.get_or_init(|| {
            let mut parts = Vec::new();
            let mut known = HashSet::new();
            for (i, (sur, raw)) in self.names.iter().enumerate() {
                for w in sur.split_whitespace().chain(raw.split_whitespace()) {
                    known.insert(w.to_lowercase());
                }
                if sur.len() >= 5 && sur.chars().all(char::is_alphabetic) {
                    parts.push((sur.to_lowercase(), i));
                }
            }
            (parts, known)
        })
    }

    fn fuzzy_hit(&self, w: &str) -> Option<usize> {
        let (parts, known) = self.fuzzy();
        let lw = w.to_lowercase();
        if lw.chars().count() < 5 || known.contains(&lw) {
            return None;
        }
        let first = lw.chars().next();
        let mut best: Option<(usize, usize)> = None;
        let mut tie = false;
        for (p, idx) in parts {
            if p.chars().next() != first {
                continue;
            }
            let lim = if p.chars().count() >= 8 { 2 } else { 1 };
            let d = edit_distance(&lw, p);
            if d == 0 || d > lim {
                continue;
            }
            match best {
                None => best = Some((d, *idx)),
                Some((bd, _)) if d < bd => {
                    best = Some((d, *idx));
                    tie = false;
                }
                Some((bd, bi)) if d == bd && self.names[bi].1 != self.names[*idx].1 => tie = true,
                _ => {}
            }
        }
        best.filter(|_| !tie).map(|b| b.1)
    }

    fn names_scan(&self, s: &str, escape: bool) -> Option<String> {
        let (ac, _) = self.names_ac()?;
        let mut hits: Vec<(usize, usize, String)> = Vec::new();
        for m in ac.find_iter(s) {
            let (st, en) = (m.start(), m.end());
            if super::bounded(s, st, en) {
                let raw = &self.names[m.pattern().as_usize()].1;
                hits.push((st, en, super::surrogate::apply_case(raw, &s[st..en])));
            }
        }
        if !self.fuzzy().0.is_empty() {
            let mut fz = Vec::new();
            for (st, en) in alpha_words(s) {
                if hits.iter().any(|h| st < h.1 && h.0 < en) {
                    continue;
                }
                if let Some(i) = self.fuzzy_hit(&s[st..en]) {
                    fz.push((
                        st,
                        en,
                        super::surrogate::apply_case(&self.names[i].1, &s[st..en]),
                    ));
                }
            }
            if !fz.is_empty() {
                hits.extend(fz);
                hits.sort_by_key(|h| h.0);
            }
        }
        if hits.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(s.len());
        let mut at = 0;
        for (st, en, v) in hits {
            out.push_str(&s[at..st]);
            if escape {
                let q = serde_json::to_string(&v).unwrap_or_default();
                out.push_str(&q[1..q.len() - 1]);
            } else {
                out.push_str(&v);
            }
            at = en;
        }
        out.push_str(&s[at..]);
        Some(out)
    }

    pub fn names_cut(&self, text: &str, tool: bool) -> usize {
        if tool && !self.tools {
            return text.len();
        }
        let Some((_, max)) = self.names_ac() else {
            return text.len();
        };
        let mut lo = text.len().saturating_sub(max + 1);
        while !text.is_char_boundary(lo) {
            lo += 1;
        }
        let mut prev = text[..lo].chars().next_back();
        for (off, c) in text[lo..].char_indices() {
            let i = lo + off;
            let start =
                c.is_alphanumeric() && !prev.is_some_and(|p| p.is_alphanumeric() || p == '_');
            prev = Some(c);
            if !start {
                continue;
            }
            let tail = &text[i..];
            if self.names.iter().any(|(sur, _)| {
                tail.len() <= sur.len()
                    && sur.is_char_boundary(tail.len())
                    && sur[..tail.len()].eq_ignore_ascii_case(tail)
            }) {
                return i;
            }
        }
        if let Some(&(st, en)) = alpha_words(text).last() {
            let firsts: HashSet<Option<char>> =
                self.fuzzy().0.iter().map(|p| p.0.chars().next()).collect();
            if en == text.len()
                && en - st <= 32
                && firsts.contains(&text[st..].chars().next().map(|c| c.to_ascii_lowercase()))
            {
                return st;
            }
        }
        text.len()
    }

    fn get(&self, ph: &str, tool: bool) -> Option<&str> {
        if tool {
            if !self.tools {
                return None;
            }
            return self
                .text
                .get(ph)
                .or_else(|| self.tool_extra.get(ph))
                .map(String::as_str);
        }
        self.text.get(ph).map(String::as_str)
    }

    pub fn text(&self, s: &str, tool: bool, escape: bool) -> Option<String> {
        self.text_opt(s, tool, escape, true)
    }

    fn text_opt(&self, s: &str, tool: bool, escape: bool, names: bool) -> Option<String> {
        let named = if names && !self.names.is_empty() && (!tool || self.tools) {
            self.names_scan(s, escape)
        } else {
            None
        };
        let cur = named.as_deref().unwrap_or(s);
        self.tags(cur, tool, escape).or(named)
    }

    fn tags(&self, s: &str, tool: bool, escape: bool) -> Option<String> {
        if escape {
            if let Some(u) = unescape_angles(s) {
                return self.scan(&u, tool, escape);
            }
        }
        self.scan(s, tool, escape)
    }

    fn scan(&self, s: &str, tool: bool, escape: bool) -> Option<String> {
        if !s.contains('<') {
            return None;
        }
        let mut out = String::with_capacity(s.len());
        let mut at = 0;
        let mut changed = false;
        let mut i = 0;
        while let Some(off) = s[i..].find('<') {
            let st = i + off;
            let mut we = s.len().min(st + MAX_PH);
            while !s.is_char_boundary(we) {
                we -= 1;
            }
            let window = &s[st..we];
            let end = window[1..].find(|c: char| !ph_char(c)).map(|e| e + 1);
            match end.filter(|e| window.as_bytes()[*e] == b'>') {
                Some(e) => {
                    let ph = &s[st..st + e + 1];
                    if let Some(v) = self.get(ph, tool) {
                        out.push_str(&s[at..st]);
                        if escape {
                            let q = serde_json::to_string(v).unwrap_or_default();
                            out.push_str(&q[1..q.len() - 1]);
                        } else {
                            out.push_str(v);
                        }
                        at = st + e + 1;
                        changed = true;
                    }
                    i = st + e + 1;
                }
                None => i = st + 1,
            }
        }
        if !changed {
            return None;
        }
        out.push_str(&s[at..]);
        Some(out)
    }

    pub fn json(&self, v: &mut Value) {
        self.walk(v, None, false);
    }

    fn walk(&self, v: &mut Value, key: Option<&str>, tool: bool) {
        match v {
            Value::String(s) => {
                let (t, esc) = match key {
                    Some("arguments" | "partial_json") => (true, true),
                    _ => (tool, false),
                };
                let names = !key.is_some_and(|k| NAME_SKIP.contains(&k));
                if let Some(n) = self.text_opt(s, t, esc, names) {
                    *s = n;
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.walk(x, key, tool)),
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    let t = tool || (k == "input" && x.is_object());
                    self.walk(x, Some(k.as_str()), t);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
enum Flush {
    Chat {
        ci: i64,
        field: String,
    },
    ChatTool {
        ci: i64,
        ti: i64,
    },
    Anth {
        idx: i64,
        dtype: String,
        field: String,
    },
    Resp {
        template: Value,
    },
}

#[derive(Debug)]
struct Pending {
    text: String,
    tool: bool,
    flush: Flush,
}

pub struct SseRehydrator {
    rh: Arc<Rehydrate>,
    buf: Vec<u8>,
    lines: Vec<String>,
    pending: BTreeMap<String, Pending>,
    chat_tpl: Value,
}

fn ev(event: Option<&str>, data: &Value) -> String {
    match event {
        Some(e) => format!("event: {e}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
}

impl SseRehydrator {
    pub fn new(rh: Arc<Rehydrate>) -> Self {
        Self {
            rh,
            buf: Vec::new(),
            lines: Vec::new(),
            pending: BTreeMap::new(),
            chat_tpl: json!({}),
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = String::new();
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw)
                .trim_end_matches(['\n', '\r'])
                .to_owned();
            if line.is_empty() {
                let lines = std::mem::take(&mut self.lines);
                self.event(lines, &mut out);
            } else {
                self.lines.push(line);
            }
        }
        out.into_bytes()
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = String::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            self.lines.push(
                String::from_utf8_lossy(&rest)
                    .trim_end_matches(['\n', '\r'])
                    .to_owned(),
            );
        }
        if !self.lines.is_empty() {
            let lines = std::mem::take(&mut self.lines);
            self.event(lines, &mut out);
        }
        self.flush_all(&mut out);
        out.into_bytes()
    }

    fn process(&mut self, key: String, new: &str, fin: bool, tool: bool, flush: Flush) -> String {
        let mut text = self
            .pending
            .remove(&key)
            .map(|p| p.text)
            .unwrap_or_default();
        text.push_str(new);
        if tool {
            if let Some(u) = unescape_angles(&text) {
                text = u;
            }
        }
        let (emit, hold) = if fin {
            (text.as_str(), "")
        } else {
            let (e, _) = if tool {
                split_hold_escaped(&text)
            } else {
                split_hold(&text)
            };
            text.split_at(e.len().min(self.rh.names_cut(&text, tool)))
        };
        let out = self
            .rh
            .text(emit, tool, tool)
            .unwrap_or_else(|| emit.to_owned());
        if !hold.is_empty() {
            self.pending.insert(
                key,
                Pending {
                    text: hold.to_owned(),
                    tool,
                    flush,
                },
            );
        }
        out
    }

    fn take_rest(&mut self, key: &str) -> Option<(String, Pending)> {
        let p = self.pending.remove(key)?;
        let t = self
            .rh
            .text(&p.text, p.tool, p.tool)
            .unwrap_or_else(|| p.text.clone());
        Some((t, p))
    }

    fn flush_event(&self, rest: String, flush: &Flush, out: &mut String) {
        match flush {
            Flush::Chat { ci, field } => {
                let mut c = self.chat_tpl.clone();
                c["choices"] = json!([{"index": ci, "delta": {field.as_str(): rest}}]);
                out.push_str(&ev(None, &c));
            }
            Flush::ChatTool { ci, ti } => {
                let mut c = self.chat_tpl.clone();
                c["choices"] = json!([{"index": ci, "delta": {"tool_calls": [{"index": ti, "function": {"arguments": rest}}]}}]);
                out.push_str(&ev(None, &c));
            }
            Flush::Anth { idx, dtype, field } => {
                let d = json!({"type": "content_block_delta", "index": idx, "delta": {"type": dtype, field.as_str(): rest}});
                out.push_str(&ev(Some("content_block_delta"), &d));
            }
            Flush::Resp { template } => {
                let mut t = template.clone();
                t["delta"] = json!(rest);
                let ty = t
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                out.push_str(&ev(Some(&ty), &t));
            }
        }
    }

    fn flush_where(&mut self, pred: &dyn Fn(&str) -> bool, out: &mut String) {
        let keys: Vec<String> = self.pending.keys().filter(|k| pred(k)).cloned().collect();
        for k in keys {
            if let Some((rest, p)) = self.take_rest(&k) {
                self.flush_event(rest, &p.flush, out);
            }
        }
    }

    fn flush_all(&mut self, out: &mut String) {
        self.flush_where(&|_| true, out);
    }

    fn event(&mut self, lines: Vec<String>, out: &mut String) {
        let data: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|d| d.strip_prefix(' ').unwrap_or(d))
            .collect();
        if data.is_empty() {
            for l in &lines {
                out.push_str(l);
                out.push('\n');
            }
            out.push('\n');
            return;
        }
        let joined = data.join("\n");
        if joined.trim() == "[DONE]" {
            self.flush_all(out);
            out.push_str("data: [DONE]\n\n");
            return;
        }
        let Ok(mut v) = serde_json::from_str::<Value>(&joined) else {
            for l in &lines {
                out.push_str(l);
                out.push('\n');
            }
            out.push('\n');
            return;
        };
        let mut pre = String::new();
        self.transform(&mut v, &mut pre);
        out.push_str(&pre);
        let mut wrote = false;
        for l in &lines {
            if l.starts_with("data:") {
                if !wrote {
                    out.push_str(&format!("data: {v}\n"));
                    wrote = true;
                }
            } else {
                out.push_str(l);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    fn transform(&mut self, v: &mut Value, pre: &mut String) {
        if v.get("choices").is_some_and(Value::is_array) {
            self.chat(v);
            return;
        }
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        match ty.as_str() {
            "content_block_delta" => {
                let idx = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                let dtype = v["delta"]["type"].as_str().unwrap_or("").to_owned();
                let (field, tool) = match dtype.as_str() {
                    "input_json_delta" => ("partial_json", true),
                    "thinking_delta" => ("thinking", false),
                    _ => ("text", false),
                };
                if let Some(s) = v["delta"]
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    let key = format!("a{idx}:{field}");
                    let f = Flush::Anth {
                        idx,
                        dtype,
                        field: field.to_owned(),
                    };
                    let n = self.process(key, &s, false, tool, f);
                    v["delta"][field] = json!(n);
                }
            }
            "content_block_stop" => {
                let idx = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                let prefix = format!("a{idx}:");
                self.flush_where(&|k| k.starts_with(&prefix), pre);
            }
            t if t.starts_with("response.") && t.ends_with(".delta") => {
                if let Some(s) = v.get("delta").and_then(Value::as_str).map(str::to_owned) {
                    let tool = t.contains("arguments");
                    let key = resp_key(v, t.trim_end_matches(".delta"));
                    let mut template = v.clone();
                    template["delta"] = json!("");
                    let n = self.process(key, &s, false, tool, Flush::Resp { template });
                    v["delta"] = json!(n);
                } else {
                    self.rh.json(v);
                }
            }
            t if t.starts_with("response.") && t.ends_with(".done") => {
                let key = resp_key(v, t.trim_end_matches(".done"));
                self.flush_where(&|k| k == key, pre);
                self.rh.json(v);
            }
            "message_stop" | "response.completed" => {
                self.flush_all(pre);
                self.rh.json(v);
            }
            _ => self.rh.json(v),
        }
    }

    fn chat(&mut self, v: &mut Value) {
        let mut tpl = serde_json::Map::new();
        for k in ["id", "object", "created", "model", "system_fingerprint"] {
            if let Some(x) = v.get(k) {
                tpl.insert(k.to_owned(), x.clone());
            }
        }
        self.chat_tpl = Value::Object(tpl);
        let Some(choices) = v.get_mut("choices").and_then(Value::as_array_mut) else {
            return;
        };
        for (pos, ch) in choices.iter_mut().enumerate() {
            let ci = ch
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(pos as i64);
            let fin = ch.get("finish_reason").is_some_and(|f| !f.is_null());
            if ch.get("message").is_some() {
                self.rh.json(ch);
                continue;
            }
            if !ch.get("delta").is_some_and(Value::is_object) {
                if !fin {
                    continue;
                }
                ch["delta"] = json!({});
            }
            for field in ["content", "reasoning_content", "reasoning", "refusal"] {
                let key = format!("c{ci}:{field}");
                let cur = ch["delta"]
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if cur.is_none() && !(fin && self.pending.contains_key(&key)) {
                    continue;
                }
                let f = Flush::Chat {
                    ci,
                    field: field.to_owned(),
                };
                let n = self.process(key, cur.as_deref().unwrap_or(""), fin, false, f);
                ch["delta"][field] = json!(n);
            }
            let mut seen: Vec<i64> = Vec::new();
            if let Some(tcs) = ch["delta"]
                .get_mut("tool_calls")
                .and_then(Value::as_array_mut)
            {
                for (tpos, tc) in tcs.iter_mut().enumerate() {
                    let ti = tc
                        .get("index")
                        .and_then(Value::as_i64)
                        .unwrap_or(tpos as i64);
                    seen.push(ti);
                    let key = format!("c{ci}:t{ti}");
                    if let Some(a) = tc["function"]
                        .get("arguments")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    {
                        let n = self.process(key, &a, fin, true, Flush::ChatTool { ci, ti });
                        tc["function"]["arguments"] = json!(n);
                    }
                }
            }
            if fin {
                let prefix = format!("c{ci}:t");
                let keys: Vec<String> = self
                    .pending
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                for k in keys {
                    let ti: i64 = k[prefix.len()..].parse().unwrap_or(0);
                    if let Some((rest, _)) = self.take_rest(&k) {
                        let d = &mut ch["delta"];
                        if !d.get("tool_calls").is_some_and(Value::is_array) {
                            d["tool_calls"] = json!([]);
                        }
                        if let Some(arr) = d["tool_calls"].as_array_mut() {
                            if seen.contains(&ti) {
                                if let Some(tc) = arr
                                    .iter_mut()
                                    .find(|t| t.get("index").and_then(Value::as_i64) == Some(ti))
                                {
                                    let cur = tc["function"]["arguments"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_owned();
                                    tc["function"]["arguments"] = json!(cur + &rest);
                                }
                            } else {
                                arr.push(json!({"index": ti, "function": {"arguments": rest}}));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn resp_key(v: &Value, family: &str) -> String {
    let s = |k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
    format!(
        "r{}:{}:{}:{family}",
        s("item_id"),
        s("output_index"),
        s("content_index")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rh() -> Arc<Rehydrate> {
        let mut text = HashMap::new();
        text.insert("<CARD_A>".to_owned(), "4111 1111 1111 1111".to_owned());
        text.insert("<PASSWORD_A>".to_owned(), "p\"w\\1".to_owned());
        let mut tool_extra = HashMap::new();
        tool_extra.insert("<SECRET:maps>".to_owned(), "FAKEKEY".to_owned());
        Arc::new(Rehydrate::new(text, tool_extra, true, Vec::new()))
    }

    fn run(r: &mut SseRehydrator, chunks: &[&str]) -> String {
        let mut out = Vec::new();
        for c in chunks {
            out.extend(r.feed(c.as_bytes()));
        }
        out.extend(r.finish());
        String::from_utf8(out).unwrap()
    }

    fn chat_text(sse: &str) -> (String, String) {
        let (mut text, mut args) = (String::new(), String::new());
        for l in sse.lines() {
            let Some(d) = l.strip_prefix("data: ") else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(d) else {
                continue;
            };
            for ch in v["choices"].as_array().into_iter().flatten() {
                text.push_str(ch["delta"]["content"].as_str().unwrap_or(""));
                for tc in ch["delta"]["tool_calls"].as_array().into_iter().flatten() {
                    args.push_str(tc["function"]["arguments"].as_str().unwrap_or(""));
                }
            }
        }
        (text, args)
    }

    fn names() -> Arc<Rehydrate> {
        Arc::new(Rehydrate::new(
            HashMap::new(),
            HashMap::new(),
            true,
            vec![
                ("Chaitanya Bhandari".into(), "Priya Venkataraman".into()),
                ("Chaitanya".into(), "Priya".into()),
                ("Bhandari".into(), "Venkataraman".into()),
                (
                    "12, Palm Grove Apartments, Surat 514522".into(),
                    "Flat 12, Lake View Road, Kochi 682020".into(),
                ),
            ],
        ))
    }

    #[test]
    fn surrogates_exact_partial_case_fuzzy() {
        let r = names();
        assert_eq!(
            r.text(
                "CHAITANYA BHANDARI and chaitanya's dog; Chaituya called Bhandri",
                false,
                false
            )
            .unwrap(),
            "PRIYA VENKATARAMAN and priya's dog; Priya called Venkataraman"
        );
        assert!(r
            .text("Chapter one, Chaitra and Bhanu at the cafe", false, false)
            .is_none());
        assert_eq!(edit_distance("chaituya", "chaitanya"), 2);
    }

    #[test]
    fn surrogates_stream_split_everywhere() {
        let src =
            "Chaitanya Bhandari's parcel: 12, Palm Grove Apartments, Surat 514522. Bye Chaituya!";
        let want = "Priya Venkataraman's parcel: Flat 12, Lake View Road, Kochi 682020. Bye Priya!";
        for size in 1..12 {
            let cs: Vec<char> = src.chars().collect();
            let mut sse = String::new();
            for c in cs.chunks(size) {
                let piece: String = c.iter().collect();
                sse.push_str(&format!(
                    "data: {}\n\n",
                    json!({"choices": [{"index": 0, "delta": {"content": piece}}]})
                ));
            }
            sse.push_str(&format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            ));
            let mut z = SseRehydrator::new(names());
            let out = run(&mut z, &[&sse]);
            assert_eq!(chat_text(&out).0, want, "chunk size {size}");
        }
    }

    #[test]
    fn text_and_escape() {
        let r = rh();
        assert_eq!(
            r.text("x <CARD_A> y <NOPE_A> <", false, false).unwrap(),
            "x 4111 1111 1111 1111 y <NOPE_A> <"
        );
        assert_eq!(
            r.text("{\"p\":\"<PASSWORD_A>\"}", true, true).unwrap(),
            "{\"p\":\"p\\\"w\\\\1\"}"
        );
        assert!(r.text("<SECRET:maps>", false, false).is_none());
        assert_eq!(r.text("<SECRET:maps>", true, false).unwrap(), "FAKEKEY");
        assert_eq!(split_hold("abc <CAR"), ("abc ", "<CAR"));
        assert_eq!(split_hold("a < b"), ("a < b", ""));
    }

    #[test]
    fn buffered_json_tools() {
        let r = rh();
        let mut v = json!({"choices": [{"message": {"content": "card <CARD_A>", "tool_calls": [{"function": {"name": "f", "arguments": "{\"k\":\"<SECRET:maps>\",\"p\":\"<PASSWORD_A>\"}"}}]}}]});
        r.json(&mut v);
        assert_eq!(
            v["choices"][0]["message"]["content"],
            "card 4111 1111 1111 1111"
        );
        let a: Value = serde_json::from_str(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(a["k"], "FAKEKEY");
        assert_eq!(a["p"], "p\"w\\1");
        let mut m = json!({"content": [{"type": "text", "text": "<CARD_A>"}, {"type": "tool_use", "input": {"key": "<SECRET:maps>"}}]});
        r.json(&mut m);
        assert_eq!(m["content"][1]["input"]["key"], "FAKEKEY");
        let no = Arc::new(Rehydrate::new(
            HashMap::new(),
            r.tool_extra.clone(),
            false,
            Vec::new(),
        ));
        let mut m2 = json!({"input": {"key": "<SECRET:maps>"}});
        no.json(&mut m2);
        assert_eq!(m2["input"]["key"], "<SECRET:maps>");
    }

    #[test]
    fn chat_stream_split_placeholders() {
        let full = "pay <CARD_A> now, pw <PASSWORD_A>.";
        let args = "{\"key\":\"<SECRET:maps>\"}";
        for size in 1..8 {
            let mut chunks: Vec<String> = Vec::new();
            let cs: Vec<char> = full.chars().collect();
            for piece in cs.chunks(size) {
                let p: String = piece.iter().collect();
                chunks.push(format!(
                    "data: {}\n\n",
                    json!({"id": "x", "choices": [{"index": 0, "delta": {"content": p}}]})
                ));
            }
            let ac: Vec<char> = args.chars().collect();
            for piece in ac.chunks(size) {
                let p: String = piece.iter().collect();
                chunks.push(format!("data: {}\n\n", json!({"id": "x", "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": p}}]}}]})));
            }
            chunks.push(format!(
                "data: {}\n\n",
                json!({"id": "x", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            ));
            chunks.push("data: [DONE]\n\n".into());
            let wire: String = chunks.concat();
            let bytes: Vec<String> = wire
                .as_bytes()
                .chunks(7)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect();
            let refs: Vec<&str> = bytes.iter().map(String::as_str).collect();
            let out = run(&mut SseRehydrator::new(rh()), &refs);
            let (t, a) = chat_text(&out);
            assert_eq!(t, "pay 4111 1111 1111 1111 now, pw p\"w\\1.", "size {size}");
            assert_eq!(a, "{\"key\":\"FAKEKEY\"}", "size {size}");
            assert!(out.trim_end().ends_with("data: [DONE]"));
        }
    }

    #[test]
    fn chat_stream_flushes_unterminated_tail() {
        let out = run(
            &mut SseRehydrator::new(rh()),
            &["data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a <CARD_\"}}]}\n\n"],
        );
        assert_eq!(chat_text(&out).0, "a <CARD_");
        let out = run(
            &mut SseRehydrator::new(rh()),
            &[
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a <CARD_\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"A> b\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
        assert_eq!(chat_text(&out).0, "a 4111 1111 1111 1111 b");
    }

    #[test]
    fn anthropic_stream() {
        let evs = [
            (
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            (
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "card <CA"}}),
            ),
            (
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "RD_A> ok <CARD"}}),
            ),
            (
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            (
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"k\":\"<SECRET:"}}),
            ),
            (
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "maps>\"}"}}),
            ),
            (
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 1}),
            ),
            ("message_stop", json!({"type": "message_stop"})),
        ];
        let wire: String = evs
            .iter()
            .map(|(e, d)| format!("event: {e}\ndata: {d}\n\n"))
            .collect();
        let out = run(&mut SseRehydrator::new(rh()), &[&wire]);
        let (mut text, mut pj) = (String::new(), String::new());
        for l in out.lines() {
            if let Some(d) = l.strip_prefix("data: ") {
                let v: Value = serde_json::from_str(d).unwrap();
                text.push_str(v["delta"]["text"].as_str().unwrap_or(""));
                pj.push_str(v["delta"]["partial_json"].as_str().unwrap_or(""));
            }
        }
        assert_eq!(text, "card 4111 1111 1111 1111 ok <CARD");
        assert_eq!(pj, "{\"k\":\"FAKEKEY\"}");
        let stop0 = out
            .find("\"type\":\"content_block_stop\",\"index\":0")
            .unwrap();
        assert!(
            out[..stop0].contains("\"text\":\"<CARD\""),
            "flush lands before the block closes"
        );
    }

    #[test]
    fn responses_stream() {
        let evs = [
            json!({"type": "response.output_text.delta", "item_id": "m1", "output_index": 0, "content_index": 0, "delta": "x <CARD"}),
            json!({"type": "response.output_text.delta", "item_id": "m1", "output_index": 0, "content_index": 0, "delta": "_A> y <CA"}),
            json!({"type": "response.output_text.done", "item_id": "m1", "output_index": 0, "content_index": 0, "text": "x <CARD_A> y <CA"}),
            json!({"type": "response.function_call_arguments.delta", "item_id": "f1", "output_index": 1, "delta": "{\"k\":\"<SECRET:ma"}),
            json!({"type": "response.function_call_arguments.delta", "item_id": "f1", "output_index": 1, "delta": "ps>\"}"}),
            json!({"type": "response.function_call_arguments.done", "item_id": "f1", "output_index": 1, "arguments": "{\"k\":\"<SECRET:maps>\"}"}),
        ];
        let wire: String = evs
            .iter()
            .map(|d| format!("event: {}\ndata: {d}\n\n", d["type"].as_str().unwrap()))
            .collect();
        let out = run(&mut SseRehydrator::new(rh()), &[&wire]);
        let (mut text, mut args, mut done_text, mut done_args) =
            (String::new(), String::new(), String::new(), String::new());
        for l in out.lines() {
            if let Some(d) = l.strip_prefix("data: ") {
                let v: Value = serde_json::from_str(d).unwrap();
                match v["type"].as_str().unwrap() {
                    "response.output_text.delta" => text.push_str(v["delta"].as_str().unwrap()),
                    "response.function_call_arguments.delta" => {
                        args.push_str(v["delta"].as_str().unwrap())
                    }
                    "response.output_text.done" => {
                        done_text = v["text"].as_str().unwrap().to_owned()
                    }
                    "response.function_call_arguments.done" => {
                        done_args = v["arguments"].as_str().unwrap().to_owned()
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(text, "x 4111 1111 1111 1111 y <CA");
        assert_eq!(done_text, "x 4111 1111 1111 1111 y <CA");
        assert_eq!(args, "{\"k\":\"FAKEKEY\"}");
        assert_eq!(done_args, "{\"k\":\"FAKEKEY\"}");
    }

    #[test]
    fn go_escaped_angles_in_tool_args() {
        let r = rh();
        let mut v = json!({"choices": [{"message": {"tool_calls": [{"function": {"name": "Bash", "arguments": "{\"command\":\"echo \\u003cCARD_A\\u003e\"}"}}]}}]});
        r.json(&mut v);
        let a: Value = serde_json::from_str(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(a["command"], "echo 4111 1111 1111 1111");
        assert!(r.text("x \\\\u003cCARD_A>", true, true).is_none());
        assert_eq!(unescape_angles("a \\u003Cb \\u003E").unwrap(), "a <b >");
        let wide = format!("<{}", "\u{e9}".repeat(120));
        assert!(r.text(&wide, false, false).is_none());
        assert!(r.text(&wide, true, true).is_none());
    }

    #[test]
    fn go_escaped_tool_args_split_everywhere() {
        let args = "{\"k\":\"\\u003cSECRET:maps\\u003e and \\u003cPASSWORD_A\\u003e\"}";
        for size in 1..12 {
            let cs: Vec<char> = args.chars().collect();
            let mut wire = String::new();
            for piece in cs.chunks(size) {
                let p: String = piece.iter().collect();
                wire.push_str(&format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": p}}]}}]})));
            }
            wire.push_str("data: [DONE]\n\n");
            let out = run(&mut SseRehydrator::new(rh()), &[&wire]);
            let (_, a) = chat_text(&out);
            let v: Value =
                serde_json::from_str(&a).unwrap_or_else(|e| panic!("size {size}: {e} in {a}"));
            assert_eq!(v["k"], "FAKEKEY and p\"w\\1", "size {size}");
        }
        let evs = ["{\"k\":\"\\u00", "3cSECRET:maps", "\\u003", "e\"}"];
        let wire: String = evs
            .iter()
            .map(|p| format!("event: content_block_delta\ndata: {}\n\n", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": p}})))
            .chain(std::iter::once("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_owned()))
            .collect();
        let out = run(&mut SseRehydrator::new(rh()), &[&wire]);
        let mut pj = String::new();
        for l in out.lines() {
            if let Some(d) = l.strip_prefix("data: ") {
                let v: Value = serde_json::from_str(d).unwrap();
                pj.push_str(v["delta"]["partial_json"].as_str().unwrap_or(""));
            }
        }
        let v: Value = serde_json::from_str(&pj).unwrap();
        assert_eq!(v["k"], "FAKEKEY");
    }
}
