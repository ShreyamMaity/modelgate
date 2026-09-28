use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const NER_KINDS: &[&str] = &["PERSON", "ADDRESS", "ORG", "LOCATION"];

const HONORIFIC: &[&str] = &[
    "mr", "mrs", "ms", "miss", "dr", "prof", "shri", "sri", "shrimati", "smt", "late", "sir",
    "madam", "ji", "da", "di", "didi", "dada", "bhaiya", "bhaiyya", "bhai", "anna", "akka", "amma",
    "appa", "aunty", "auntie", "uncle", "saab", "sahab", "babu", "bro", "chachu", "chacha", "mama",
    "mami",
];

const STOP: &[&str] = &[
    "hai",
    "hain",
    "ho",
    "hoga",
    "hogi",
    "tha",
    "thi",
    "ka",
    "ki",
    "ke",
    "ko",
    "ne",
    "se",
    "me",
    "mein",
    "main",
    "pe",
    "par",
    "aur",
    "bhi",
    "nahi",
    "nahin",
    "na",
    "kya",
    "kyu",
    "kyun",
    "kab",
    "kaha",
    "kahan",
    "kaise",
    "jo",
    "woh",
    "wo",
    "ye",
    "yeh",
    "voh",
    "iska",
    "uska",
    "uski",
    "iski",
    "mera",
    "meri",
    "mere",
    "tera",
    "teri",
    "tere",
    "apna",
    "apni",
    "hum",
    "tum",
    "aap",
    "tu",
    "mai",
    "mujhe",
    "tujhe",
    "unko",
    "isko",
    "usko",
    "kar",
    "karo",
    "karna",
    "karke",
    "kiya",
    "kiye",
    "de",
    "do",
    "dena",
    "diya",
    "diye",
    "dega",
    "degi",
    "le",
    "lo",
    "lena",
    "liya",
    "gaya",
    "gayi",
    "gaye",
    "raha",
    "rahi",
    "rahe",
    "bol",
    "bolo",
    "bola",
    "boli",
    "bhej",
    "bhejo",
    "bhejna",
    "abhi",
    "kal",
    "aaj",
    "parso",
    "yaar",
    "bas",
    "sab",
    "kuch",
    "koi",
    "ek",
    "jab",
    "tab",
    "agar",
    "lekin",
    "phir",
    "fir",
    "ab",
    "toh",
    "wala",
    "wali",
    "wale",
    "ghar",
    "baat",
    "baje",
    "din",
    "saath",
    "liye",
    "chahiye",
    "sakta",
    "sakti",
    "aa",
    "aaya",
    "aayi",
    "aayega",
    "aayegi",
    "aayenge",
    "aayengi",
    "ja",
    "jaa",
    "jana",
    "jao",
    "chal",
    "chalo",
    "pata",
    "dekh",
    "dekho",
    "haan",
    "mat",
    "sirf",
    "bahut",
    "thoda",
    "accha",
    "acha",
    "theek",
    "thik",
    "kaam",
    "paise",
    "the",
    "a",
    "an",
    "and",
    "or",
    "of",
    "to",
    "in",
    "on",
    "at",
    "for",
    "with",
    "from",
    "by",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "it",
    "this",
    "that",
    "these",
    "those",
    "my",
    "your",
    "his",
    "her",
    "our",
    "their",
    "i",
    "you",
    "he",
    "she",
    "we",
    "they",
    "him",
    "us",
    "them",
    "please",
    "thanks",
    "hi",
    "hello",
    "dear",
    "ask",
    "tell",
    "send",
    "call",
    "meet",
    "remind",
    "let",
    "can",
    "could",
    "would",
    "should",
    "will",
    "shall",
    "does",
    "did",
    "have",
    "has",
    "had",
    "not",
    "no",
    "yes",
    "ok",
    "okay",
    "today",
    "tomorrow",
    "yesterday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "via",
    "use",
    "using",
    "run",
    "check",
    "restart",
    "deploy",
    "put",
];

const IMPERATIVE: &[&str] = &[
    "find",
    "show",
    "check",
    "send",
    "get",
    "make",
    "tell",
    "list",
    "open",
    "run",
    "draft",
    "reply",
    "ask",
    "add",
    "book",
    "pay",
    "call",
    "create",
    "update",
    "delete",
    "remove",
    "search",
    "fetch",
    "read",
    "write",
    "summarise",
    "summarize",
    "schedule",
    "reschedule",
    "cancel",
    "remind",
    "email",
    "text",
    "message",
    "share",
    "forward",
    "move",
    "copy",
    "save",
    "set",
    "fix",
    "start",
    "stop",
    "play",
    "pause",
    "close",
    "give",
    "take",
    "buy",
    "order",
    "note",
    "ping",
    "sync",
    "pull",
    "push",
    "merge",
    "review",
    "approve",
    "join",
    "invite",
    "post",
    "upload",
    "download",
    "print",
    "scan",
    "track",
    "look",
    "see",
    "try",
    "help",
    "keep",
    "go",
    "turn",
    "compare",
    "explain",
    "translate",
    "plan",
    "prepare",
    "confirm",
    "sign",
    "submit",
    "attach",
    "archive",
    "mute",
    "block",
    "report",
    "what",
    "where",
    "when",
    "who",
    "why",
    "how",
    "which",
    "hey",
];

const NOT_ORG: &[&str] = &[
    "emi", "emis", "kyc", "otp", "upi", "gst", "gstin", "tds", "pan", "atm", "pdf", "faq", "eta",
    "ceo", "cto", "cfo", "coo", "hr", "qa", "ui", "ux", "api", "url", "sms", "ott", "nri", "ifsc",
    "neft", "rtgs", "imps", "pr", "prs",
];

fn common(w: &str) -> Option<bool> {
    static C: OnceLock<Vec<&'static str>> = OnceLock::new();
    let list = C.get_or_init(|| {
        include_str!("common_words.txt")
            .lines()
            .filter(|l| !l.is_empty())
            .collect()
    });
    list.binary_search_by(|l| l.trim_end_matches('*').cmp(w))
        .ok()
        .map(|i| list[i].ends_with('*'))
}

fn sentence_initial(text: &str, at: usize) -> bool {
    for c in text[..at].chars().rev() {
        match c {
            '\n' => return true,
            ' ' | '\t' | '"' | '\'' | '(' | '[' | '{' | '*' | '_' | '`' | '\u{201c}'
            | '\u{2018}' => {}
            '.' | '!' | '?' | ':' | ';' | '-' | '>' | '#' | '|' | '\u{2022}' | '\u{2013}'
            | '\u{2014}' => return true,
            _ => return false,
        }
    }
    true
}

const BUILTIN_ALLOW: &[&str] = &[
    "claude",
    "anthropic",
    "openai",
    "chatgpt",
    "gpt",
    "gemini",
    "google",
    "llama",
    "meta",
    "mistral",
    "qwen",
    "deepseek",
    "ollama",
    "github",
    "gitlab",
    "docker",
    "kubernetes",
    "tailscale",
    "cloudflare",
    "aws",
    "azure",
    "linux",
    "windows",
    "macos",
    "ubuntu",
    "python",
    "rust",
    "javascript",
    "typescript",
    "json",
    "api",
    "http",
    "https",
    "sonnet",
    "opus",
    "haiku",
    "nemotron",
    "kimi",
    "grok",
    "copilot",
    "cursor",
    "vscode",
    "slack",
    "notion",
    "gmail",
    "whatsapp",
    "telegram",
    "discord",
    "youtube",
    "linear",
];

#[derive(Clone, Debug)]
pub struct NerConfig {
    pub url: Option<String>,
    pub kinds: HashSet<String>,
    pub min_score: f32,
    pub org_min_score: f32,
    pub org_single_min_score: f32,
    pub allow: HashSet<String>,
    pub disable_groups: HashSet<String>,
    pub timeout: Duration,
    pub max_bytes: usize,
    pub batch_bytes: usize,
}

impl Default for NerConfig {
    fn default() -> Self {
        NerConfig {
            url: None,
            kinds: NER_KINDS.iter().map(|s| (*s).to_owned()).collect(),
            min_score: 0.5,
            org_min_score: 0.7,
            org_single_min_score: 0.85,
            allow: BUILTIN_ALLOW.iter().map(|s| (*s).to_owned()).collect(),
            disable_groups: HashSet::new(),
            timeout: Duration::from_secs(180),
            max_bytes: 256 << 10,
            batch_bytes: 48 << 10,
        }
    }
}

impl NerConfig {
    pub fn from_value(v: Option<&Value>) -> NerConfig {
        let mut c = NerConfig::default();
        let env_url = std::env::var("PII_NER_URL")
            .ok()
            .filter(|u| !u.trim().is_empty());
        let Some(v) = v.filter(|v| v.is_object()) else {
            c.url = env_url;
            return c;
        };
        if v.get("enabled").and_then(Value::as_bool) == Some(false) {
            return c;
        }
        c.url = v
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|u| !u.trim().is_empty())
            .or(env_url);
        if let Some(k) = v.get("kinds").and_then(Value::as_array) {
            c.kinds = k
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_uppercase)
                .filter(|k| NER_KINDS.contains(&k.as_str()))
                .collect();
        }
        if let Some(s) = v.get("min_score").and_then(Value::as_f64) {
            c.min_score = s.clamp(0.0, 1.0) as f32;
        }
        if let Some(s) = v.get("org_min_score").and_then(Value::as_f64) {
            c.org_min_score = s.clamp(0.0, 1.0) as f32;
        }
        if let Some(s) = v.get("org_single_min_score").and_then(Value::as_f64) {
            c.org_single_min_score = s.clamp(0.0, 1.0) as f32;
        }
        if let Some(a) = v.get("allow").and_then(Value::as_array) {
            for x in a.iter().filter_map(Value::as_str) {
                let x = x.trim().to_lowercase();
                if !x.is_empty() {
                    c.allow.insert(x);
                }
            }
        }
        if let Some(a) = v.get("disable_groups").and_then(Value::as_array) {
            c.disable_groups = a
                .iter()
                .filter_map(Value::as_str)
                .map(|g| g.trim().to_owned())
                .filter(|g| !g.is_empty())
                .collect();
        }
        if let Some(t) = v.get("timeout_ms").and_then(Value::as_u64) {
            c.timeout = Duration::from_millis(t.clamp(100, 600_000));
        }
        if let Some(m) = v.get("max_bytes").and_then(Value::as_u64) {
            c.max_bytes = m as usize;
        }
        c
    }

    pub fn enabled(&self) -> bool {
        self.url.is_some() && !self.kinds.is_empty()
    }

    pub fn off_for(&self, model: &str) -> bool {
        !self.disable_groups.is_empty()
            && model
                .split('|')
                .any(|m| self.disable_groups.contains(m.trim()))
    }

    pub fn summary(&self) -> Value {
        let mut kinds: Vec<&String> = self.kinds.iter().collect();
        kinds.sort();
        json!({
            "enabled": self.enabled(),
            "url": self.url,
            "kinds": kinds,
            "min_score": self.min_score,
            "org_min_score": self.org_min_score,
            "org_single_min_score": self.org_single_min_score,
            "allow": self.allow.len(),
            "disable_groups": self.disable_groups.iter().collect::<std::collections::BTreeSet<_>>(),
            "timeout_ms": self.timeout.as_millis() as u64,
            "max_bytes": self.max_bytes,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub kind: String,
    pub score: f32,
}

enum Call {
    Down(String),
    Bad(String),
}

#[derive(Default)]
struct Stats {
    bad_replies: u64,
    skipped_texts: u64,
    calls: u64,
    texts: u64,
    bytes: u64,
    ms: u64,
    errors: u64,
    hits: u64,
    last_error: Option<String>,
}

pub struct Ner {
    cache: Mutex<HashMap<u64, Arc<Vec<Hit>>>>,
    stats: Mutex<Stats>,
    pub cache_cap: usize,
}

impl Default for Ner {
    fn default() -> Self {
        Ner {
            cache: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
            cache_cap: 20_000,
        }
    }
}

fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h ^ (s.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

pub const CHUNK: usize = 4096;

pub fn chunks(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for para in t.split("\n\n") {
        if para.len() <= CHUNK {
            out.push(para);
            continue;
        }
        let mut start = 0;
        let mut last = 0;
        for (i, _) in para.match_indices('\n') {
            if i - start > CHUNK && last > start {
                out.push(&para[start..last]);
                start = last + 1;
            }
            last = i;
        }
        out.push(&para[start..]);
    }
    out
}

fn trim_edges(s: &str) -> (usize, usize) {
    let keep = |c: char| c.is_alphanumeric();
    let st = s.find(keep).unwrap_or(s.len());
    let en = s
        .rfind(keep)
        .map(|i| i + s[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(st);
    (st, en.max(st))
}

fn strip_possessive(s: &str) -> &str {
    for suf in ["'s", "\u{2019}s", "'S", "\u{2019}S", "'", "\u{2019}"] {
        if let Some(x) = s.strip_suffix(suf) {
            return x.trim_end();
        }
    }
    s
}

fn word(t: &str) -> String {
    t.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

pub fn clean(text: &str, start: usize, end: usize, kind: &str, cfg: &NerConfig) -> Option<String> {
    clean_at(text, start, end, kind, cfg).map(|(_, r)| r.to_owned())
}

fn clean_at<'a>(
    text: &'a str,
    start: usize,
    end: usize,
    kind: &str,
    cfg: &NerConfig,
) -> Option<(usize, &'a str)> {
    if start >= end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return None;
    }
    let span = &text[start..end];
    let mut toks: Vec<(usize, usize)> = Vec::new();
    let mut at = None;
    for (i, c) in span.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = at.take() {
                toks.push((s, i));
            }
        } else if at.is_none() {
            at = Some(i);
        }
    }
    if let Some(s) = at {
        toks.push((s, span.len()));
    }
    let personish = matches!(kind, "PERSON" | "LOCATION");
    let drop_edge = |t: &str| {
        let w = word(t);
        w.is_empty() || STOP.contains(&w.as_str()) || (personish && HONORIFIC.contains(&w.as_str()))
    };
    while toks.first().is_some_and(|&(s, e)| {
        drop_edge(&span[s..e]) || IMPERATIVE.contains(&word(&span[s..e]).as_str())
    }) {
        toks.remove(0);
    }
    while toks.last().is_some_and(|&(s, e)| drop_edge(&span[s..e])) {
        toks.pop();
    }
    let (first, last) = (toks.first()?.0, toks.last()?.1);
    let words: Vec<String> = toks.iter().map(|&(s, e)| word(&span[s..e])).collect();
    if kind == "PERSON"
        && words.iter().any(|w| {
            STOP.contains(&w.as_str()) || HONORIFIC.contains(&w.as_str()) && words.len() > 3
        })
    {
        return None;
    }
    if words.iter().any(|w| cfg.allow.contains(w)) {
        return None;
    }
    let inner = &span[first..last];
    let (a, b) = trim_edges(inner);
    let raw = strip_possessive(&inner[a..b]);
    let (a, b) = trim_edges(raw);
    let raw = &raw[a..b];
    if raw.chars().filter(|c| c.is_alphabetic()).count() < 3 {
        return None;
    }
    if cfg.allow.contains(&raw.to_lowercase()) {
        return None;
    }
    if personish {
        if raw.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        if words.len() == 1 && raw.len() <= 4 && raw.chars().all(|c| !c.is_lowercase()) {
            return None;
        }
    }
    Some((raw.as_ptr() as usize - text.as_ptr() as usize, raw))
}

pub fn accept(
    text: &str,
    start: usize,
    end: usize,
    kind: &str,
    score: f32,
    cfg: &NerConfig,
) -> Option<String> {
    let (at, raw) = clean_at(text, start, end, kind, cfg)?;
    let single = !raw.contains(char::is_whitespace);
    let min = match kind {
        "ORG" if single => cfg.org_single_min_score.max(cfg.min_score),
        "ORG" => cfg.org_min_score.max(cfg.min_score),
        _ => cfg.min_score,
    };
    if score < min {
        return None;
    }
    if single && matches!(kind, "PERSON" | "ORG" | "LOCATION") {
        let w = raw.to_lowercase();
        if IMPERATIVE.contains(&w.as_str()) || STOP.contains(&w.as_str()) {
            return None;
        }
        if kind == "ORG" && NOT_ORG.contains(&w.as_str()) {
            return None;
        }
        if let Some(proper) = common(&w) {
            let drop = match kind {
                "ORG" => true,
                "LOCATION" => !proper,
                _ => raw.starts_with(char::is_lowercase) || !proper && sentence_initial(text, at),
            };
            if drop {
                return None;
            }
        }
    }
    Some(raw.to_owned())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub kind: String,
    pub raw: String,
}

impl Ner {
    pub fn status(&self) -> Value {
        let s = self.stats.lock().unwrap();
        json!({
            "calls": s.calls,
            "texts": s.texts,
            "bytes": s.bytes,
            "ms": s.ms,
            "errors": s.errors,
            "bad_replies": s.bad_replies,
            "skipped_texts": s.skipped_texts,
            "cache_hits": s.hits,
            "cache_entries": self.cache.lock().unwrap().len(),
            "last_error": s.last_error,
        })
    }

    fn fail(&self, e: String) -> String {
        let mut s = self.stats.lock().unwrap();
        s.errors += 1;
        s.last_error = Some(e.clone());
        e
    }

    fn note(&self, e: String) {
        let mut s = self.stats.lock().unwrap();
        s.bad_replies += 1;
        s.last_error = Some(e);
    }

    async fn call(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
        cfg: &NerConfig,
        batch: &[&str],
    ) -> Result<Vec<Arc<Vec<Hit>>>, Call> {
        let resp = client
            .post(endpoint)
            .timeout(cfg.timeout)
            .json(&json!({ "texts": batch }))
            .send()
            .await
            .map_err(|e| Call::Down(format!("ner unavailable: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Call::Bad(format!("ner status {}", status.as_u16())));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| Call::Bad(format!("ner bad reply: {e}")))?;
        let lists = v
            .get("spans")
            .and_then(Value::as_array)
            .filter(|a| a.len() == batch.len())
            .ok_or_else(|| Call::Bad("ner reply shape".into()))?;
        let mut out = Vec::with_capacity(batch.len());
        for (t, list) in batch.iter().zip(lists) {
            let mut hits = Vec::new();
            for s in list.as_array().into_iter().flatten() {
                let (Some(a), Some(b), Some(k)) = (
                    s.get(0).and_then(Value::as_u64),
                    s.get(1).and_then(Value::as_u64),
                    s.get(2).and_then(Value::as_str),
                ) else {
                    return Err(Call::Bad("ner span shape".into()));
                };
                let (a, b) = (a as usize, b as usize);
                if b > t.len() || a >= b || !t.is_char_boundary(a) || !t.is_char_boundary(b) {
                    return Err(Call::Bad("ner span out of range".into()));
                }
                let score = s.get(3).and_then(Value::as_f64).unwrap_or(1.0) as f32;
                hits.push(Hit {
                    start: a,
                    end: b,
                    kind: k.to_ascii_uppercase(),
                    score,
                });
            }
            out.push(Arc::new(hits));
        }
        Ok(out)
    }

    pub async fn find(
        &self,
        client: &reqwest::Client,
        cfg: &NerConfig,
        kinds: &(dyn Fn(&str) -> bool + Sync),
        texts: &[String],
    ) -> Result<Vec<Found>, String> {
        let Some(url) = cfg.url.as_deref() else {
            return Ok(Vec::new());
        };
        let mut uniq: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for t in texts {
            for c in chunks(t) {
                if c.chars().any(char::is_alphabetic) && seen.insert(c) {
                    uniq.push(c);
                }
            }
        }
        let mut have: HashMap<u64, Arc<Vec<Hit>>> = HashMap::new();
        let mut miss: Vec<&str> = Vec::new();
        {
            let cache = self.cache.lock().unwrap();
            for t in &uniq {
                let h = hash(t);
                match cache.get(&h) {
                    Some(v) => {
                        have.insert(h, v.clone());
                    }
                    None => miss.push(t),
                }
            }
        }
        self.stats.lock().unwrap().hits += have.len() as u64;
        let miss_bytes: usize = miss.iter().map(|t| t.len()).sum();
        if miss_bytes > cfg.max_bytes {
            return Err(self.fail(format!("ner input too large ({miss_bytes} bytes)")));
        }
        let endpoint = format!("{}/v1/ner", url.trim_end_matches('/'));
        let mut i = 0;
        while i < miss.len() {
            let mut j = i;
            let mut bytes = 0;
            while j < miss.len()
                && (j == i || bytes + miss[j].len() <= cfg.batch_bytes)
                && j - i < 64
            {
                bytes += miss[j].len();
                j += 1;
            }
            let batch = &miss[i..j];
            let t0 = Instant::now();
            let mut fresh: Vec<(u64, Arc<Vec<Hit>>)> = Vec::with_capacity(batch.len());
            match self.call(client, &endpoint, cfg, batch).await {
                Ok(lists) => {
                    for (t, hits) in batch.iter().zip(lists) {
                        fresh.push((hash(t), hits));
                    }
                }
                Err(Call::Down(e)) => return Err(self.fail(e)),
                Err(Call::Bad(e)) => {
                    self.note(e);
                    for t in batch {
                        match self
                            .call(client, &endpoint, cfg, std::slice::from_ref(t))
                            .await
                        {
                            Ok(mut lists) => fresh.push((hash(t), lists.remove(0))),
                            Err(Call::Down(e)) => return Err(self.fail(e)),
                            Err(Call::Bad(e)) => {
                                self.note(e);
                                self.stats.lock().unwrap().skipped_texts += 1;
                                crate::state::log("ner_text_skipped", json!({"bytes": t.len()}));
                            }
                        }
                    }
                }
            }
            {
                let mut s = self.stats.lock().unwrap();
                s.calls += 1;
                s.texts += batch.len() as u64;
                s.bytes += bytes as u64;
                s.ms += t0.elapsed().as_millis() as u64;
            }
            let mut cache = self.cache.lock().unwrap();
            if cache.len() + fresh.len() > self.cache_cap {
                cache.clear();
            }
            for (h, v) in fresh {
                cache.insert(h, v.clone());
                have.insert(h, v);
            }
            i = j;
        }
        let mut out: Vec<Found> = Vec::new();
        let mut dedup = HashSet::new();
        for t in &uniq {
            let Some(hits) = have.get(&hash(t)) else {
                continue;
            };
            for h in hits.iter() {
                if !cfg.kinds.contains(&h.kind) || !kinds(&h.kind) {
                    continue;
                }
                if let Some(raw) = accept(t, h.start, h.end, &h.kind, h.score, cfg) {
                    if dedup.insert((h.kind.clone(), raw.clone())) {
                        out.push(Found {
                            kind: h.kind.clone(),
                            raw,
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(t: &str, sub: &str, kind: &str) -> Option<String> {
        let s = t.find(sub).unwrap();
        clean(t, s, s + sub.len(), kind, &NerConfig::default())
    }

    #[test]
    fn cleaning_rules() {
        assert_eq!(
            c("Arre Sourav da, bol", "Sourav da", "PERSON").as_deref(),
            Some("Sourav")
        );
        assert_eq!(
            c("Sneha Joshi's leave", "Sneha Joshi's", "PERSON").as_deref(),
            Some("Sneha Joshi")
        );
        assert_eq!(
            c(
                "of Late Shri Ramesh Chandra Pandey.",
                "Late Shri Ramesh Chandra Pandey.",
                "PERSON"
            )
            .as_deref(),
            Some("Ramesh Chandra Pandey")
        );
        assert_eq!(c("bhej do aur Rahul", "bhej do aur", "PERSON"), None);
        assert_eq!(
            c("Woh Chennai se hai", "Woh Chennai", "LOCATION").as_deref(),
            Some("Chennai")
        );
        assert_eq!(c("Ask Claude to review", "Ask Claude", "PERSON"), None);
        assert_eq!(c("from HR today", "HR", "PERSON"), None);
        assert_eq!(
            c("rahul bol raha tha", "rahul", "PERSON").as_deref(),
            Some("rahul")
        );
        assert_eq!(
            c(
                "Kal Priya Venkataraman ke ghar",
                "Priya Venkataraman ke",
                "PERSON"
            )
            .as_deref(),
            Some("Priya Venkataraman")
        );
        assert_eq!(
            c("Rao & Associates is", "Rao & Associates", "ORG").as_deref(),
            Some("Rao & Associates")
        );
        let mut cfg = NerConfig::default();
        cfg.allow.insert("zorblax".into());
        assert_eq!(clean("Zorblax 2 27B runs", 0, 12, "ORG", &cfg), None);
    }

    fn a(t: &str, sub: &str, kind: &str, score: f32) -> Option<String> {
        let s = t.find(sub).unwrap();
        accept(t, s, s + sub.len(), kind, score, &NerConfig::default())
    }

    #[test]
    fn imperative_verbs_and_common_words_are_not_entities() {
        for (t, verb) in [
            ("Find the invoice from Acme", "Find"),
            ("Check my calendar", "Check"),
            ("Send it to Priya", "Send"),
            ("Show GitHub notifications", "Show"),
            ("Get me the latest statement", "Get"),
            ("Make a note for Rahul", "Make"),
            ("Tell Suraj I will be late", "Tell"),
            ("List my open issues", "List"),
            ("Open the report", "Open"),
            ("Run the backup", "Run"),
            ("Draft a reply to Meera", "Draft"),
            ("Reply to Anjali Menon", "Reply"),
            ("- Find My: tracks lost earbuds", "Find My"),
        ] {
            for kind in ["ORG", "PERSON", "LOCATION"] {
                assert_eq!(a(t, verb, kind, 0.99), None, "{t:?} {verb} {kind}");
            }
        }
        assert_eq!(a("Please FIND it", "FIND", "ORG", 0.99), None);
        assert_eq!(a("The Invoice is due", "Invoice", "ORG", 0.99), None);
        assert_eq!(a("Meet at the Office", "Office", "LOCATION", 0.99), None);
        assert_eq!(a("Receipt came in", "Receipt", "PERSON", 0.99), None);
        assert_eq!(a("ask the manager", "manager", "PERSON", 0.99), None);
    }

    #[test]
    fn real_names_and_orgs_still_found() {
        assert_eq!(
            a("Find the invoice from Acme", "Acme", "ORG", 0.99).as_deref(),
            Some("Acme")
        );
        assert_eq!(
            a("Send it to Priya", "Priya", "PERSON", 0.88).as_deref(),
            Some("Priya")
        );
        assert_eq!(
            a(
                "Find the most recent invoice from Kingfisher Bay Agro",
                "Kingfisher Bay Agro",
                "ORG",
                0.99
            )
            .as_deref(),
            Some("Kingfisher Bay Agro")
        );
        assert_eq!(
            a(
                "Find Kingfisher Bay Agro invoices",
                "Find Kingfisher Bay Agro",
                "ORG",
                0.99
            )
            .as_deref(),
            Some("Kingfisher Bay Agro")
        );
        assert_eq!(
            a("Mail from Federal Bank", "Federal Bank", "ORG", 0.99).as_deref(),
            Some("Federal Bank")
        );
        assert_eq!(
            a("Reply to Anjali Menon", "Anjali Menon", "PERSON", 0.99).as_deref(),
            Some("Anjali Menon")
        );
        assert_eq!(
            a("We fly to Goa next week", "Goa", "LOCATION", 0.99).as_deref(),
            Some("Goa")
        );
        assert_eq!(
            a("Rose called about the rent", "Rose", "PERSON", 0.9).as_deref(),
            Some("Rose")
        );
        assert_eq!(
            a("Lunch with Baker today", "Baker", "PERSON", 0.9).as_deref(),
            Some("Baker")
        );
    }

    #[test]
    fn org_needs_more_confidence_than_person() {
        assert_eq!(a("lacks Zorvane Labs", "Zorvane Labs", "ORG", 0.6), None);
        assert_eq!(
            a("lacks Zorvane Labs", "Zorvane Labs", "ORG", 0.75).as_deref(),
            Some("Zorvane Labs")
        );
        assert_eq!(a("paid Zorvane", "Zorvane", "ORG", 0.8), None);
        assert_eq!(
            a("paid Zorvane", "Zorvane", "ORG", 0.9).as_deref(),
            Some("Zorvane")
        );
        assert_eq!(
            a("paid Zorvane", "Zorvane", "PERSON", 0.6).as_deref(),
            Some("Zorvane")
        );
        assert_eq!(a("paid Zorvane", "Zorvane", "PERSON", 0.4), None);
        assert_eq!(a("pay the EMI", "EMI", "ORG", 0.99), None);
        let cfg = NerConfig::from_value(Some(
            &json!({"org_min_score": 0.5, "org_single_min_score": 0.5}),
        ));
        assert_eq!(
            accept("paid Zorvane", 5, 12, "ORG", 0.6, &cfg).as_deref(),
            Some("Zorvane")
        );
    }

    #[test]
    fn common_word_list_loads() {
        assert_eq!(common("find"), Some(false));
        assert_eq!(common("goa"), Some(true));
        assert_eq!(common("priya"), None);
        assert_eq!(common("rahul"), None);
        assert!(sentence_initial("Hi. Find", 4));
        assert!(sentence_initial("- Find", 2));
        assert!(!sentence_initial("please Find", 7));
    }

    #[test]
    fn chunking_is_content_defined() {
        let a = format!("{}\n\nPriya Rao", "x ".repeat(10));
        let b = format!("{}\n\nPriya Rao", "y ".repeat(30));
        assert_eq!(chunks(&a)[1], chunks(&b)[1]);
        let long = "line of text\n".repeat(1000);
        let parts = chunks(&long);
        assert!(parts.len() > 2 && parts.iter().all(|p| p.len() <= CHUNK + 16));
        assert_eq!(parts.concat().len() + parts.len() - 1, long.len());
    }

    #[test]
    fn config_parse() {
        let c = NerConfig::from_value(Some(
            &json!({"url": "http://ner:8090", "kinds": ["person", "bogus", "ORG"], "allow": ["Snorkelbot"], "min_score": 0.6}),
        ));
        assert!(c.enabled());
        assert_eq!(c.kinds.len(), 2);
        assert!(c.allow.contains("snorkelbot"));
        assert!((c.min_score - 0.6).abs() < 1e-6);
        let off = NerConfig::from_value(Some(&json!({"enabled": false, "url": "http://x"})));
        assert!(!off.enabled());
    }
}
