pub mod detect;
pub mod rehydrate;
pub mod vault;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use axum::http::HeaderMap;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub use rehydrate::{Rehydrate, SseRehydrator};
use vault::Vault;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    Local,
    TrustedRaw,
    TrustedMasked,
    Public,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Local => "local",
            Tier::TrustedRaw => "trusted_raw",
            Tier::TrustedMasked => "trusted_masked",
            Tier::Public => "public",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "local" => Some(Tier::Local),
            "trusted_raw" => Some(Tier::TrustedRaw),
            "trusted_masked" => Some(Tier::TrustedMasked),
            "public" => Some(Tier::Public),
            _ => None,
        }
    }

    pub fn masks(self) -> bool {
        matches!(self, Tier::TrustedMasked | Tier::Public)
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub default: Tier,
    rules: Vec<(String, Tier)>,
    kinds: HashMap<Tier, HashSet<String>>,
    disabled: HashSet<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            default: Tier::Public,
            rules: Vec::new(),
            kinds: HashMap::new(),
            disabled: HashSet::new(),
        }
    }
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn glob(pat: &str, s: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == s;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !s.starts_with(first) || s.len() < first.len() + last.len() || !s.ends_with(last) {
        return false;
    }
    let mut rest = &s[first.len()..s.len() - last.len()];
    for p in &parts[1..parts.len() - 1] {
        match rest.find(p) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    true
}

impl Policy {
    pub fn from_value(v: Option<&Value>) -> Policy {
        let mut p = Policy::default();
        let Some(v) = v.filter(|v| v.is_object()) else {
            return p;
        };
        if let Some(t) = v
            .get("default")
            .and_then(Value::as_str)
            .and_then(Tier::parse)
        {
            p.default = t;
        }
        if let Some(tiers) = v.get("tiers").and_then(Value::as_object) {
            for (name, pats) in tiers {
                if let Some(t) = Tier::parse(name) {
                    for pat in strs(Some(pats)) {
                        p.rules.push((pat, t));
                    }
                }
            }
        }
        if let Some(k) = v.get("kinds").and_then(Value::as_object) {
            for (name, list) in k {
                if let Some(t) = Tier::parse(name) {
                    p.kinds.insert(
                        t,
                        strs(Some(list))
                            .into_iter()
                            .map(|s| s.to_ascii_uppercase())
                            .collect(),
                    );
                }
            }
        }
        p.disabled = strs(v.get("disable"))
            .into_iter()
            .map(|s| s.to_ascii_uppercase())
            .collect();
        p
    }

    pub fn tier(&self, spec: &str) -> Tier {
        let mut best: Option<(usize, Tier)> = None;
        for (pat, t) in &self.rules {
            if glob(pat, spec) {
                let score = pat.chars().filter(|c| *c != '*').count();
                best = match best {
                    Some((s, bt)) if s > score || (s == score && bt >= *t) => Some((s, bt)),
                    _ => Some((score, *t)),
                };
            }
        }
        best.map(|b| b.1).unwrap_or(self.default)
    }

    pub fn allows(&self, tier: Tier, kind: &str) -> bool {
        if self.disabled.contains(kind) {
            return false;
        }
        self.kinds.get(&tier).is_none_or(|k| k.contains(kind))
    }

    pub fn summary(&self) -> Value {
        let mut tiers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (pat, t) in &self.rules {
            tiers.entry(t.name()).or_default().push(pat);
        }
        json!({
            "default": self.default.name(),
            "tiers": tiers,
            "kinds": self.kinds.iter().map(|(t, k)| (t.name().to_owned(), json!(k.iter().collect::<Vec<_>>()))).collect::<Map<_, _>>(),
            "disable": self.disabled.iter().collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub entities: usize,
    pub occurrences: usize,
    pub kinds: BTreeMap<String, usize>,
}

impl Report {
    pub fn to_json(&self, tier: Tier) -> Value {
        json!({"tier": tier.name(), "masked": self.entities, "occurrences": self.occurrences, "kinds": self.kinds})
    }
}

struct Conv {
    by_norm: HashMap<String, String>,
    by_raw: HashMap<String, (String, String)>,
    rev: HashMap<String, String>,
    next: HashMap<String, u32>,
    seen: Instant,
}

impl Conv {
    fn new() -> Conv {
        Conv {
            by_norm: HashMap::new(),
            by_raw: HashMap::new(),
            rev: HashMap::new(),
            next: HashMap::new(),
            seen: Instant::now(),
        }
    }
}

pub fn letters(mut n: u32) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

type VaultSlot = (Option<(SystemTime, u64)>, Result<Arc<Vault>, String>);

#[derive(Default)]
pub struct Totals {
    pub requests: u64,
    pub entities: u64,
    pub blocked: u64,
}

pub struct Pii {
    pub enabled: bool,
    pub tool_rehydrate: bool,
    pub ttl: Duration,
    pub max_convs: usize,
    pub max_entries: usize,
    vault_path: Option<PathBuf>,
    vault: Mutex<VaultSlot>,
    convs: Mutex<HashMap<String, Conv>>,
    pub totals: Mutex<Totals>,
}

fn env_flag(name: &str, dflt: bool) -> bool {
    match std::env::var(name)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" => false,
        "1" | "true" | "on" | "yes" => true,
        _ => dflt,
    }
}

fn stamp(p: &std::path::Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())))
}

const SKIP_KEYS: &[&str] = &[
    "model",
    "role",
    "type",
    "id",
    "tool_call_id",
    "call_id",
    "item_id",
    "name",
    "object",
    "finish_reason",
    "stop_reason",
    "tool_choice",
    "status",
    "encrypted_content",
    "signature",
    "media_type",
    "detail",
    "format",
    "service_tier",
    "reasoning_effort",
    "previous_response_id",
    "user",
    "voice",
];

fn skip_string(key: Option<&str>, s: &str) -> bool {
    if key.is_some_and(|k| SKIP_KEYS.contains(&k)) || s.len() < 3 {
        return true;
    }
    if s.starts_with("data:") {
        return true;
    }
    s.len() > 512
        && !s.bytes().any(|b| b.is_ascii_whitespace())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

fn visit_strings(v: &Value, key: Option<&str>, f: &mut dyn FnMut(&str)) {
    match v {
        Value::String(s) => {
            if skip_string(key, s) {
                return;
            }
            if key == Some("arguments") {
                if let Ok(inner @ (Value::Object(_) | Value::Array(_))) =
                    serde_json::from_str::<Value>(s)
                {
                    visit_strings(&inner, None, f);
                    return;
                }
            }
            f(s)
        }
        Value::Array(a) => a.iter().for_each(|x| visit_strings(x, key, f)),
        Value::Object(o) => o.iter().for_each(|(k, x)| visit_strings(x, Some(k), f)),
        _ => {}
    }
}

fn edit_strings(v: &mut Value, key: Option<&str>, f: &mut dyn FnMut(&str) -> Option<String>) {
    match v {
        Value::String(s) => {
            if skip_string(key, s) {
                return;
            }
            if key == Some("arguments") {
                if let Ok(mut inner @ (Value::Object(_) | Value::Array(_))) =
                    serde_json::from_str::<Value>(s)
                {
                    let before = inner.clone();
                    edit_strings(&mut inner, None, f);
                    if inner != before {
                        *s = inner.to_string();
                    }
                    return;
                }
            }
            if let Some(n) = f(s) {
                *s = n;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| edit_strings(x, key, f)),
        Value::Object(o) => o
            .iter_mut()
            .for_each(|(k, x)| edit_strings(x, Some(k.as_str()), f)),
        _ => {}
    }
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn bounded(text: &str, s: usize, e: usize) -> bool {
    let first = text[s..e].chars().next();
    let last = text[s..e].chars().next_back();
    let before = text[..s].chars().next_back();
    let after = text[e..].chars().next();
    !(is_word(first) && is_word(before) || is_word(last) && is_word(after))
}

fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| p.as_str())
                    .map(str::to_owned)
                    .or_else(|| p.get("content").map(text_of))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub const CONV_HEADERS: &[&str] = &[
    "x-conversation-id",
    "x-claude-code-session-id",
    "x-session-id",
];

pub fn conv_key(h: &HeaderMap, body: Option<&Value>) -> String {
    for name in CONV_HEADERS {
        if let Some(v) = h
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
        {
            return format!("h:{}", fnv(&[name, v]));
        }
    }
    let Some(b) = body else {
        return "anon".into();
    };
    let mut sys = String::new();
    let mut user = String::new();
    for k in ["system", "instructions"] {
        if let Some(v) = b.get(k) {
            sys.push_str(&text_of(v));
        }
    }
    let msgs = b
        .get("messages")
        .or_else(|| b.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    match &msgs {
        Value::String(s) => user = s.clone(),
        Value::Array(a) => {
            for m in a {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("");
                let content = m.get("content").map(text_of).unwrap_or_default();
                if matches!(role, "system" | "developer") && sys.is_empty() {
                    sys = content;
                } else if role == "user" && user.is_empty() {
                    user = content;
                }
            }
        }
        _ => {}
    }
    format!("b:{}", fnv(&[&sys, &user]))
}

impl Pii {
    pub fn new(enabled: bool, vault_path: Option<PathBuf>) -> Pii {
        Pii {
            enabled,
            tool_rehydrate: true,
            ttl: Duration::from_secs(6 * 3600),
            max_convs: 2000,
            max_entries: 5000,
            vault_path,
            vault: Mutex::new((None, Ok(Arc::new(Vault::default())))),
            convs: Mutex::new(HashMap::new()),
            totals: Mutex::new(Totals::default()),
        }
    }

    pub fn from_env() -> Pii {
        let mode = std::env::var("PII_MODE").unwrap_or_default();
        let enabled = !matches!(
            mode.to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        );
        let vault = std::env::var("PII_VAULT")
            .ok()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let mut p = Pii::new(enabled, vault);
        p.tool_rehydrate = env_flag("PII_REHYDRATE_TOOLS", true);
        if let Some(t) = std::env::var("PII_TTL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
        {
            p.ttl = Duration::from_secs(t.clamp(60, 7 * 86400));
        }
        if enabled {
            detect::warm();
        }
        p
    }

    pub fn vault(&self) -> Result<Arc<Vault>, String> {
        let Some(path) = &self.vault_path else {
            return self.vault.lock().unwrap().1.clone();
        };
        let now = stamp(path);
        let mut g = self.vault.lock().unwrap();
        if now.is_none() {
            g.0 = None;
            g.1 = Err("vault file missing".into());
        } else if g.0 != now || g.1.is_err() {
            g.1 = Vault::load(path).map(Arc::new);
            g.0 = now;
            crate::state::log(
                "pii_vault_loaded",
                match &g.1 {
                    Ok(v) => json!({"entries": v.entries.len(), "skipped_short": v.skipped}),
                    Err(e) => json!({"error": e}),
                },
            );
        }
        g.1.clone()
    }

    pub fn set_vault(&self, v: Vault) {
        *self.vault.lock().unwrap() = (None, Ok(Arc::new(v)));
    }

    pub fn status(&self) -> Value {
        let vault = match self.vault() {
            Ok(v) => {
                json!({"ok": true, "entries": v.entries.len(), "named": v.entries.iter().filter(|e| e.name.is_some()).count(), "tool_usable": v.entries.iter().filter(|e| e.tool && e.name.is_some()).count()})
            }
            Err(e) => json!({"ok": false, "error": e}),
        };
        let t = self.totals.lock().unwrap();
        json!({
            "enabled": self.enabled,
            "tool_rehydrate": self.tool_rehydrate,
            "ttl_secs": self.ttl.as_secs(),
            "vault_configured": self.vault_path.is_some(),
            "vault": vault,
            "conversations": self.convs.lock().unwrap().len(),
            "masked_requests": t.requests,
            "masked_entities": t.entities,
            "blocked_attempts": t.blocked,
        })
    }

    pub fn note_blocked(&self) {
        self.totals.lock().unwrap().blocked += 1;
    }

    fn assign(&self, conv: &mut Conv, kind: &str, raw: &str) -> Result<String, String> {
        if let Some((ph, _)) = conv.by_raw.get(raw) {
            return Ok(ph.clone());
        }
        let key = format!("{kind}\u{0}{}", detect::normalize(kind, raw));
        let ph = match conv.by_norm.get(&key) {
            Some(ph) => ph.clone(),
            None => {
                if conv.rev.len() >= self.max_entries {
                    return Err("pii map full".into());
                }
                let n = conv.next.entry(kind.to_owned()).or_insert(0);
                let ph = format!("<{kind}_{}>", letters(*n));
                *n += 1;
                conv.by_norm.insert(key, ph.clone());
                conv.rev.insert(ph.clone(), raw.to_owned());
                ph
            }
        };
        conv.by_raw
            .insert(raw.to_owned(), (ph.clone(), kind.to_owned()));
        Ok(ph)
    }

    fn assign_vault(&self, conv: &mut Conv, e: &vault::Entry) -> Result<String, String> {
        match &e.name {
            Some(n) => {
                let ph = format!("<SECRET:{n}>");
                conv.rev
                    .entry(ph.clone())
                    .or_insert_with(|| e.value.clone());
                Ok(ph)
            }
            None => self.assign(conv, &e.kind, &e.value),
        }
    }

    pub fn mask(
        &self,
        key: &str,
        body: &mut Value,
        tier: Tier,
        policy: &Policy,
    ) -> Result<(Report, Arc<Rehydrate>), String> {
        if !body.is_object() {
            return Err("body is not a JSON object".into());
        }
        let vault = self.vault()?;
        let allow = |k: &str| policy.allows(tier, k);
        let mut convs = self.convs.lock().unwrap();
        let now = Instant::now();
        let ttl = self.ttl;
        convs.retain(|_, c| now.duration_since(c.seen) < ttl);
        if !convs.contains_key(key) && convs.len() >= self.max_convs {
            if let Some(old) = convs
                .iter()
                .min_by_key(|(_, c)| c.seen)
                .map(|(k, _)| k.clone())
            {
                convs.remove(&old);
            }
        }
        let conv = convs.entry(key.to_owned()).or_insert_with(Conv::new);
        conv.seen = now;

        let mut found: Vec<(&'static str, String)> = Vec::new();
        let mut spans = Vec::new();
        visit_strings(body, None, &mut |s| {
            spans.clear();
            detect::detect_into(s, &allow, &mut spans);
            for sp in &spans {
                let raw = &s[sp.start..sp.end];
                if !vault.exact.contains(raw) {
                    found.push((sp.kind, raw.to_owned()));
                }
            }
        });
        for (kind, raw) in &found {
            self.assign(conv, kind, raw)?;
        }

        let mut pats: Vec<String> = Vec::new();
        let mut meta: Vec<(String, String)> = Vec::new();
        for (raw, (ph, kind)) in &conv.by_raw {
            if allow(kind) {
                pats.push(raw.clone());
                meta.push((ph.clone(), kind.clone()));
            }
        }
        let mut vault_cs: Vec<usize> = Vec::new();
        let mut vault_ci: Vec<usize> = Vec::new();
        for (i, e) in vault.entries.iter().enumerate() {
            if e.ignore_case {
                vault_ci.push(i);
            } else {
                vault_cs.push(i);
            }
        }
        let n_conv = pats.len();
        for i in &vault_cs {
            pats.push(vault.entries[*i].value.clone());
        }
        let build = |p: &[String], ci: bool| -> Result<Option<AhoCorasick>, String> {
            if p.is_empty() {
                return Ok(None);
            }
            AhoCorasickBuilder::new()
                .match_kind(MatchKind::LeftmostLongest)
                .ascii_case_insensitive(ci)
                .build(p)
                .map(Some)
                .map_err(|e| e.to_string())
        };
        let ac_cs = build(&pats, false)?;
        let ci_pats: Vec<String> = vault_ci
            .iter()
            .map(|i| vault.entries[*i].value.clone())
            .collect();
        let ac_ci = build(&ci_pats, true)?;

        let mut vault_ph: HashMap<usize, String> = HashMap::new();
        let mut err: Option<String> = None;
        let mut report = Report::default();
        let mut used: HashMap<String, String> = HashMap::new();
        if ac_cs.is_some() || ac_ci.is_some() {
            edit_strings(body, None, &mut |s| {
                let mut hits: Vec<(usize, usize, String, String)> = Vec::new();
                if let Some(ac) = &ac_ci {
                    for m in ac.find_iter(s) {
                        let idx = vault_ci[m.pattern().as_usize()];
                        if !bounded(s, m.start(), m.end()) {
                            continue;
                        }
                        let ph = match vault_ph.get(&idx) {
                            Some(p) => p.clone(),
                            None => match self.assign_vault(conv, &vault.entries[idx]) {
                                Ok(p) => {
                                    vault_ph.insert(idx, p.clone());
                                    p
                                }
                                Err(e) => {
                                    err = Some(e);
                                    continue;
                                }
                            },
                        };
                        hits.push((m.start(), m.end(), ph, vault.entries[idx].kind.clone()));
                    }
                }
                if let Some(ac) = &ac_cs {
                    for m in ac.find_iter(s) {
                        let (st, en) = (m.start(), m.end());
                        if hits.iter().any(|h| st < h.1 && h.0 < en) || !bounded(s, st, en) {
                            continue;
                        }
                        let pid = m.pattern().as_usize();
                        let (ph, kind) = if pid < n_conv {
                            meta[pid].clone()
                        } else {
                            let idx = vault_cs[pid - n_conv];
                            let ph = match vault_ph.get(&idx) {
                                Some(p) => p.clone(),
                                None => match self.assign_vault(conv, &vault.entries[idx]) {
                                    Ok(p) => {
                                        vault_ph.insert(idx, p.clone());
                                        p
                                    }
                                    Err(e) => {
                                        err = Some(e);
                                        continue;
                                    }
                                },
                            };
                            (ph, vault.entries[idx].kind.clone())
                        };
                        hits.push((st, en, ph, kind));
                    }
                }
                if hits.is_empty() {
                    return None;
                }
                hits.sort_by_key(|h| h.0);
                let mut out = String::with_capacity(s.len());
                let mut at = 0;
                for (st, en, ph, kind) in hits {
                    out.push_str(&s[at..st]);
                    out.push_str(&ph);
                    at = en;
                    report.occurrences += 1;
                    used.entry(ph).or_insert(kind);
                }
                out.push_str(&s[at..]);
                Some(out)
            });
        }
        if let Some(e) = err {
            return Err(e);
        }
        report.entities = used.len();
        for kind in used.values() {
            *report.kinds.entry(kind.clone()).or_insert(0) += 1;
        }
        let mut tool_extra = HashMap::new();
        for e in vault.entries.iter().filter(|e| e.tool) {
            if let Some(n) = &e.name {
                tool_extra.insert(format!("<SECRET:{n}>"), e.value.clone());
            }
        }
        let rh = Rehydrate {
            text: conv.rev.clone(),
            tool_extra,
            tools: self.tool_rehydrate,
        };
        drop(convs);
        if report.entities > 0 {
            let mut t = self.totals.lock().unwrap();
            t.requests += 1;
            t.entities += report.entities as u64;
        }
        Ok((report, Arc::new(rh)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pii() -> Pii {
        Pii::new(true, None)
    }

    fn chat(text: &str) -> Value {
        json!({"model": "x", "messages": [{"role": "system", "content": "be brief"}, {"role": "user", "content": text}]})
    }

    #[test]
    fn letters_sequence() {
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
        assert_eq!(letters(27), "AB");
    }

    #[test]
    fn tiers_most_specific_wins() {
        let p = Policy::from_value(Some(&json!({
            "tiers": {"local": ["pc/*"], "trusted_raw": ["anthropic/*"], "trusted_masked": ["openai/*"], "public": ["pc/risky"]}
        })));
        assert_eq!(p.tier("pc/bonsai"), Tier::Local);
        assert_eq!(p.tier("pc/risky"), Tier::Public);
        assert_eq!(p.tier("anthropic/claude"), Tier::TrustedRaw);
        assert_eq!(p.tier("openai/gpt"), Tier::TrustedMasked);
        assert_eq!(p.tier("dahl/x"), Tier::Public);
        let q = Policy::from_value(Some(
            &json!({"tiers": {"local": ["a/*"], "public": ["a/*"]}}),
        ));
        assert_eq!(q.tier("a/b"), Tier::Public);
        assert!(glob("*x*", "abxcd"));
        assert!(!glob("a*z", "abc"));
    }

    #[test]
    fn consistent_across_turns_and_kinds() {
        let p = pii();
        let pol = Policy::default();
        let mut a = chat("card 4111 1111 1111 1111, mail jane@example.com, password: hunter2!");
        let (r, _) = p.mask("c1", &mut a, Tier::Public, &pol).unwrap();
        let s = a.to_string();
        assert!(s.contains("<CARD_A>") && s.contains("<EMAIL_A>") && s.contains("<PASSWORD_A>"));
        assert!(!s.contains("4111") && !s.contains("jane@") && !s.contains("hunter2"));
        assert_eq!(r.entities, 3);
        let mut b = json!({"model": "x", "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "card 4111 1111 1111 1111, mail jane@example.com, password: hunter2!"},
            {"role": "assistant", "content": "noted hunter2! for JANE@example.com and 4111111111111111"},
            {"role": "user", "content": "also bob@example.org"}]});
        p.mask("c1", &mut b, Tier::Public, &pol).unwrap();
        let s = b.to_string();
        assert!(
            s.contains("noted <PASSWORD_A> for <EMAIL_A> and <CARD_A>"),
            "{s}"
        );
        assert!(s.contains("<EMAIL_B>"));
        let mut c = chat("mail bob@example.org");
        p.mask("c2", &mut c, Tier::Public, &pol).unwrap();
        assert!(
            c.to_string().contains("<EMAIL_A>"),
            "separate conversation starts over"
        );
    }

    #[test]
    fn masks_tool_args_and_results() {
        let p = pii();
        let mut v = json!({"model": "x", "messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "send", "arguments": "{\"to\":\"jane@example.com\",\"n\":1}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "sent to jane@example.com from +91 98765 43210"}]});
        p.mask("t", &mut v, Tier::Public, &Policy::default())
            .unwrap();
        let args = v["messages"][1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        let parsed: Value = serde_json::from_str(args).unwrap();
        assert_eq!(parsed["to"], "<EMAIL_A>");
        assert_eq!(
            v["messages"][2]["content"],
            "sent to <EMAIL_A> from <PHONE_A>"
        );
        assert_eq!(
            v["messages"][1]["tool_calls"][0]["function"]["name"],
            "send"
        );
    }

    #[test]
    fn vault_named_and_case_insensitive() {
        let p = pii();
        p.set_vault(Vault::from_value(&json!({"entries": [
            {"name": "maps key", "value": "FAKEMAPSKEY123", "tool": true},
            {"kind": "name", "value": "Jane Fakename", "ignore_case": true}
        ]})));
        let mut v = chat("use FAKEMAPSKEY123 for jane fakename and JANE FAKENAME");
        let (r, rh) = p
            .mask("v", &mut v, Tier::Public, &Policy::default())
            .unwrap();
        assert_eq!(
            v["messages"][1]["content"],
            "use <SECRET:mapskey> for <NAME_A> and <NAME_A>"
        );
        assert_eq!(r.entities, 2);
        assert_eq!(rh.text.get("<SECRET:mapskey>").unwrap(), "FAKEMAPSKEY123");
    }

    #[test]
    fn kinds_filter_for_trusted_masked() {
        let p = pii();
        let pol = Policy::from_value(Some(&json!({"kinds": {"trusted_masked": ["CARD"]}})));
        let mut v = chat("4111 1111 1111 1111 and jane@example.com");
        p.mask("k", &mut v, Tier::TrustedMasked, &pol).unwrap();
        assert_eq!(v["messages"][1]["content"], "<CARD_A> and jane@example.com");
    }

    #[test]
    fn missing_vault_fails_closed() {
        let p = Pii::new(true, Some(PathBuf::from("/nonexistent/pii-vault.json")));
        let mut v = chat("hi");
        assert!(p
            .mask("x", &mut v, Tier::Public, &Policy::default())
            .is_err());
        assert!(p
            .mask("x", &mut json!([1]), Tier::Public, &Policy::default())
            .is_err());
    }

    #[test]
    fn vault_recovers_after_error_without_file_change() {
        let dir = std::env::temp_dir().join(format!("mg-vault-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.json");
        std::fs::write(&path, "{\"entries\": [x}").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let p = Pii::new(true, Some(path.clone()));
        assert!(p.vault().is_err());
        std::fs::write(&path, "{\"entries\": []}").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(p.vault().is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn conversation_keys() {
        let mut h = HeaderMap::new();
        let a = chat("first");
        let k1 = conv_key(&h, Some(&a));
        let mut longer = a.clone();
        longer["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role": "assistant", "content": "x"}));
        assert_eq!(k1, conv_key(&h, Some(&longer)));
        assert_ne!(k1, conv_key(&h, Some(&chat("other"))));
        h.insert("x-conversation-id", "abc".parse().unwrap());
        assert_eq!(conv_key(&h, Some(&a)), conv_key(&h, Some(&chat("other"))));
    }

    #[test]
    fn skips_images_and_ids() {
        let p = pii();
        let b64 = "4111111111111111".repeat(40);
        let mut v = json!({"model": "4111 1111 1111 1111", "messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{b64}")}},
            {"type": "text", "text": "x"}]}]});
        let before = v.clone();
        p.mask("i", &mut v, Tier::Public, &Policy::default())
            .unwrap();
        assert_eq!(v, before);
    }

    fn prompt_20kb() -> Value {
        let para = "The quarterly report covers revenue, hiring plans and infrastructure costs for the next cycle. ";
        let mut msgs = vec![json!({"role": "system", "content": "You are a helpful assistant."})];
        let pii = [
            "card 4111 1111 1111 1111",
            "mail jane.fake@example.com",
            "call +91 98765 43210",
            "PAN ABCPE1234F",
            "password: Hunter2!x",
            "key sk-proj-AbCdEfGhIjKlMnOpQrStUvWx12",
        ];
        for i in 0..20 {
            let mut t = para.repeat(10);
            t.push_str(pii[i % pii.len()]);
            msgs.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": t}));
        }
        json!({"model": "x", "messages": msgs})
    }

    #[test]
    #[ignore]
    fn bench_mask_20kb() {
        let p = pii();
        let pol = Policy::default();
        let base = prompt_20kb();
        let size = base.to_string().len();
        let mut v = base.clone();
        let t0 = Instant::now();
        p.mask("warm", &mut v, Tier::Public, &pol).unwrap();
        let first = t0.elapsed();
        let n = 300;
        let t0 = Instant::now();
        let mut rh = None;
        for _ in 0..n {
            let mut v = base.clone();
            rh = Some(p.mask("warm", &mut v, Tier::Public, &pol).unwrap().1);
        }
        let per = t0.elapsed() / n;
        let t0 = Instant::now();
        for _ in 0..n {
            let _ = base.clone();
        }
        let clone = t0.elapsed() / n;
        let rh = rh.unwrap();
        let mut masked = base.clone();
        p.mask("warm", &mut masked, Tier::Public, &pol).unwrap();
        let t0 = Instant::now();
        for _ in 0..n {
            let mut m = masked.clone();
            rh.json(&mut m);
        }
        let re = t0.elapsed() / n;
        println!("BENCH body={size} bytes first_call={first:?} mask_per_request={per:?} (json clone alone {clone:?}) rehydrate_20kb={re:?}");
    }
}
