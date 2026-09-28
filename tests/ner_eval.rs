use modelgate::pii::{detect, Pii, Policy};
use serde_json::{json, Value};
use std::io::Write;

fn word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn occurrences(text: &str, raw: &str) -> Vec<(usize, usize)> {
    let lt = text.to_lowercase();
    let lr = raw.to_lowercase();
    if lt.len() != text.len() || lr.is_empty() {
        return text
            .match_indices(raw)
            .map(|(i, m)| (i, i + m.len()))
            .collect();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lt[from..].find(&lr) {
        let s = from + i;
        let e = s + lr.len();
        if !(word(text[..s].chars().next_back()) || word(text[e..].chars().next())) {
            out.push((s, e));
        }
        from = e;
    }
    out
}

fn chars(text: &str, b: usize) -> usize {
    text[..b].chars().count()
}

#[tokio::test]
#[ignore]
async fn ner_eval_against_live_sidecar() {
    let url = std::env::var("NER_EVAL_URL").expect("NER_EVAL_URL");
    let set: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("NER_EVAL_SET").expect("NER_EVAL_SET")).unwrap(),
    )
    .unwrap();
    let allow: Vec<String> = std::env::var("NER_EVAL_ALLOW")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_owned())
        .collect();
    let policy = Policy::from_value(Some(&json!({"ner": {"url": url, "allow": allow}})));
    let pii = Pii::new(true, None);
    let client = reqwest::Client::new();
    let mut out =
        std::fs::File::create(std::env::var("NER_EVAL_OUT").expect("NER_EVAL_OUT")).unwrap();
    for case in set.as_array().unwrap() {
        let text = case["text"].as_str().unwrap().to_owned();
        let found = pii
            .ner
            .find(&client, &policy.ner, &|_| true, std::slice::from_ref(&text))
            .await
            .unwrap();
        let mut preds: Vec<(usize, usize, String)> = Vec::new();
        for f in &found {
            for (s, e) in occurrences(&text, &f.raw) {
                preds.push((s, e, f.kind.clone()));
            }
        }
        for sp in detect::detect(&text) {
            if sp.kind == "ADDRESS" {
                preds.retain(|p| !(p.0 < sp.end && sp.start < p.1 && p.2 != "PERSON"));
                preds.push((sp.start, sp.end, "ADDRESS".into()));
            }
        }
        let preds: Vec<Value> = preds
            .into_iter()
            .map(|(s, e, k)| json!([chars(&text, s), chars(&text, e), k]))
            .collect();
        writeln!(out, "{}", json!({"text": text, "preds": preds})).unwrap();
    }
}
