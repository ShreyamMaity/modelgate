//! Turning the `model` string a client sends into an ordered list of pinned targets.
//!
//! * `smart`                         a configured group
//! * `a|b|c`                         an ad-hoc chain; each piece may be any of the forms below
//! * `provider/*`                    every model that provider serves (from the upstream model list)
//! * `provider/model`                one pinned model
//! * a plain model id                a single model, no failover (returns `None`)

use serde_json::Value;

const MAX_TARGETS: usize = 40;
const MAX_DEPTH: u8 = 4;
const PREFIX: &str = "modelgate/";

pub fn provider_of(m: &Value) -> Option<&str> {
    m.get("metadata")?.get("provider")?.get("id")?.as_str()
}

pub fn expand(
    part: &str,
    chains: &[(String, Vec<String>)],
    models: &[Value],
    depth: u8,
) -> Vec<String> {
    if depth < MAX_DEPTH {
        if let Some((_, list)) = chains.iter().find(|(n, _)| n == part) {
            return list
                .iter()
                .flat_map(|e| expand(e, chains, models, depth + 1))
                .collect();
        }
    }
    if let Some(prov) = part.strip_suffix("/*") {
        return models
            .iter()
            .filter(|m| provider_of(m) == Some(prov))
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(|id| format!("{prov}/{id}"))
            .collect();
    }
    vec![part.to_owned()]
}

/// Strip the optional `modelgate/` prefix that model pickers may add.
pub fn bare(model: &str) -> &str {
    model.strip_prefix(PREFIX).unwrap_or(model)
}

pub fn resolve(
    model: &str,
    chains: &[(String, Vec<String>)],
    models: &[Value],
) -> Option<Vec<String>> {
    let name = bare(model);
    let parts: Vec<&str> = name
        .split('|')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() == 1 && !chains.iter().any(|(n, _)| n == parts[0]) && !parts[0].ends_with("/*") {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for p in parts {
        for t in expand(bare(p), chains, models, 0) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out.truncate(MAX_TARGETS);
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chains() -> Vec<(String, Vec<String>)> {
        vec![
            ("fast".into(), vec!["a/1".into(), "b/2".into()]),
            ("smart".into(), vec!["c/3".into()]),
            ("nested".into(), vec!["fast".into(), "ollama/*".into()]),
        ]
    }
    fn models() -> Vec<Value> {
        vec![
            json!({"id": "m1", "metadata": {"provider": {"id": "ollama"}}}),
            json!({"id": "m2", "metadata": {"provider": {"id": "ollama"}}}),
            json!({"id": "x", "metadata": {"provider": {"id": "gem"}}}),
        ]
    }

    #[test]
    fn resolves_every_form() {
        let (c, m) = (chains(), models());
        assert_eq!(resolve("fast", &c, &m).unwrap(), ["a/1", "b/2"]);
        assert_eq!(resolve("modelgate/smart", &c, &m).unwrap(), ["c/3"]);
        assert_eq!(
            resolve("ollama/*", &c, &m).unwrap(),
            ["ollama/m1", "ollama/m2"]
        );
        assert_eq!(
            resolve("smart|ollama/*|c/3|fast", &c, &m).unwrap(),
            ["c/3", "ollama/m1", "ollama/m2", "a/1", "b/2"]
        );
        assert_eq!(
            resolve("nested", &c, &m).unwrap(),
            ["a/1", "b/2", "ollama/m1", "ollama/m2"],
            "groups nest"
        );
        assert!(
            resolve("ollama/gpt-oss:120b", &c, &m).is_none(),
            "single pinned model is not a chain"
        );
        assert!(resolve("plain-model", &c, &m).is_none());
    }

    #[test]
    fn cycles_terminate() {
        let c = vec![
            ("a".to_owned(), vec!["b".to_owned()]),
            ("b".to_owned(), vec!["a".to_owned(), "z/1".to_owned()]),
        ];
        let r = resolve("a", &c, &[]).unwrap();
        assert!(r.contains(&"z/1".to_owned()));
    }
}
