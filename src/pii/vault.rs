use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Entry {
    pub value: String,
    pub kind: String,
    pub name: Option<String>,
    pub ignore_case: bool,
    pub tool: bool,
}

#[derive(Default, Debug)]
pub struct Vault {
    pub entries: Vec<Entry>,
    pub exact: HashSet<String>,
    pub skipped: usize,
}

fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        .take(48)
        .collect()
}

fn clean_kind(s: &str) -> String {
    let k: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(24)
        .collect::<String>()
        .to_ascii_uppercase();
    if k.is_empty() {
        "SECRET".into()
    } else {
        k
    }
}

impl Vault {
    pub fn from_value(v: &Value) -> Vault {
        let mut out = Vault::default();
        let mut push = |value: &str, kind: &str, name: Option<&str>, ic: bool, tool: bool| {
            if value.chars().count() < 4 {
                out.skipped += 1;
                return;
            }
            let name = name.map(clean_name).filter(|n| !n.is_empty());
            out.exact.insert(value.to_owned());
            out.entries.push(Entry {
                value: value.to_owned(),
                kind: clean_kind(kind),
                name,
                ignore_case: ic,
                tool,
            });
        };
        if let Some(list) = v.get("entries").and_then(Value::as_array) {
            for e in list {
                let Some(value) = e.get("value").and_then(Value::as_str) else {
                    continue;
                };
                let name = e.get("name").and_then(Value::as_str);
                let kind = e.get("kind").and_then(Value::as_str).unwrap_or("SECRET");
                let ic = e
                    .get("ignore_case")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let tool = e.get("tool").and_then(Value::as_bool).unwrap_or(false);
                push(value, kind, name, ic, tool);
            }
        }
        if let Some(obj) = v.get("secrets").and_then(Value::as_object) {
            for (name, value) in obj {
                if let Some(value) = value.as_str() {
                    push(value, "SECRET", Some(name), false, false);
                }
            }
        }
        out
    }

    pub fn load(path: &Path) -> Result<Vault, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let text = text.trim_start_matches('\u{feff}').trim();
        if text.is_empty() {
            return Ok(Vault::default());
        }
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Ok(Vault::from_value(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_vault_parses() {
        let v: Value = serde_json::from_str(include_str!("../../pii-vault.example.json")).unwrap();
        let vault = Vault::from_value(&v);
        assert_eq!(vault.entries.len(), 5);
        assert_eq!(vault.entries[0].name.as_deref(), Some("maps_key"));
        assert!(vault.entries[2].ignore_case);
        assert_eq!(vault.entries[3].kind, "ADDRESS");
        let short = Vault::from_value(&serde_json::json!({"secrets": {"a": "abc", "b": "abcd"}}));
        assert_eq!((short.entries.len(), short.skipped), (1, 1));
    }
}
