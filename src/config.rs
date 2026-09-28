//! Config file: parsing, validation, atomic saves with history.
//!
//! The file is a JSON object. Unknown keys are preserved on save, so hand-added
//! settings (e.g. `routes`) survive edits made from the web UI.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const SETTING_BOUNDS: &[(&str, f64, f64)] = &[
    ("timeout", 5.0, 600.0),
    ("ttfb_stream", 3.0, 120.0),
    ("stream_idle", 10.0, 600.0),
    ("max_tokens_cap", 256.0, 200_000.0),
];
pub const HISTORY_KEEP: usize = 40;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Total wait for a non-streaming reply (seconds).
    pub timeout: f64,
    /// Wait for the first byte of a streaming reply (seconds).
    pub ttfb_stream: f64,
    /// Longest silence allowed between chunks once a stream has started (seconds).
    pub stream_idle: f64,
    /// Upper bound applied to `max_tokens` on translated requests.
    pub max_tokens_cap: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: 90.0,
            ttfb_stream: 20.0,
            stream_idle: 120.0,
            max_tokens_cap: 32768,
        }
    }
}

/// A direct (non-Aperture) upstream used by `routes`, e.g. for embeddings or images.
#[derive(Clone, Debug)]
pub struct DirectTarget {
    pub label: String,
    pub base: String,
    pub model: Option<String>,
    pub path: Option<String>,
    /// Name of an environment variable holding a bearer key (never stored in the config file).
    pub key_env: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The whole file, so unknown keys round-trip.
    pub root: Value,
    /// Groups in file order.
    pub chains: Vec<(String, Vec<String>)>,
    pub settings: Settings,
    pub routes: Vec<(String, Vec<DirectTarget>)>,
    /// Providers shown with a "paid" badge in the UI.
    pub paid_providers: Vec<String>,
    pub upstream: Option<String>,
    pub pii: crate::pii::Policy,
}

impl Config {
    pub fn empty() -> Self {
        Self::from_value(json!({ "chains": {} }))
    }

    pub fn from_value(root: Value) -> Self {
        let mut chains = Vec::new();
        if let Some(obj) = root.get("chains").and_then(Value::as_object) {
            for (name, list) in obj {
                let items = list
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                chains.push((name.clone(), items));
            }
        }
        let d = Settings::default();
        let num = |k: &str, dflt: f64| root.get(k).and_then(Value::as_f64).unwrap_or(dflt);
        let settings = Settings {
            timeout: num("timeout", d.timeout),
            ttfb_stream: num("ttfb_stream", d.ttfb_stream),
            stream_idle: num("stream_idle", d.stream_idle),
            max_tokens_cap: num("max_tokens_cap", d.max_tokens_cap as f64) as u64,
        };
        let mut routes = Vec::new();
        if let Some(obj) = root.get("routes").and_then(Value::as_object) {
            for (path, r) in obj {
                let targets = r
                    .get("targets")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| {
                                let base = t.get("base")?.as_str()?.to_owned();
                                let s =
                                    |k: &str| t.get(k).and_then(Value::as_str).map(str::to_owned);
                                Some(DirectTarget {
                                    label: s("label").unwrap_or_else(|| base.clone()),
                                    base,
                                    model: s("model"),
                                    path: s("path"),
                                    key_env: s("key_env"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                routes.push((path.clone(), targets));
            }
        }
        let paid_providers = root
            .get("paid_providers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_else(|| vec!["aperture".to_owned()]);
        let upstream = root
            .get("upstream")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let pii = crate::pii::Policy::from_value(root.get("pii"));
        Config {
            root,
            chains,
            settings,
            routes,
            paid_providers,
            upstream,
            pii,
        }
    }

    pub fn chain(&self, name: &str) -> Option<&Vec<String>> {
        self.chains.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

pub fn valid_group_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Groups in file order: (name, entries).
pub type Groups = Vec<(String, Vec<String>)>;

/// Validate the body of a config save. Returns the cleaned groups and settings.
pub fn validate(body: &Value) -> Result<(Groups, Map<String, Value>), String> {
    let chains = body
        .get("chains")
        .and_then(Value::as_object)
        .filter(|c| c.len() <= 60)
        .ok_or("chains must be an object with at most 60 groups")?;
    let mut clean = Vec::new();
    for (name, targets) in chains {
        if !valid_group_name(name) {
            return Err(format!(
                "bad group name {name:?}: use letters, digits . _ - (max 40)"
            ));
        }
        let arr = targets
            .as_array()
            .filter(|a| a.len() <= 40)
            .ok_or_else(|| format!("group {name:?}: at most 40 entries"))?;
        let mut items: Vec<String> = Vec::new();
        for t in arr {
            let s = t
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 200 && !s.contains(['\n', '|']));
            let s = s.ok_or_else(|| format!("group {name:?}: bad entry {t}"))?;
            if !items.iter().any(|x| x == s) {
                items.push(s.to_owned());
            }
        }
        clean.push((name.clone(), items));
    }
    let mut settings = Map::new();
    if let Some(s) = body.get("settings").and_then(Value::as_object) {
        for (key, lo, hi) in SETTING_BOUNDS {
            if let Some(v) = s.get(*key) {
                let n = v.as_f64().filter(|n| (*lo..=*hi).contains(n));
                let n = n.ok_or_else(|| format!("{key} must be a number between {lo} and {hi}"))?;
                settings.insert((*key).to_owned(), v.clone());
                let _ = n;
            }
        }
    }
    Ok((clean, settings))
}

/// Cheap change detector for hot reload: modification time plus size.
pub fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())))
}

/// Opaque version string for optimistic concurrency: a hash of the file's contents. (Not the
/// modification time: two writes inside one clock tick would look identical.)
pub fn content_version(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return "0".into();
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
    for b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}');
    let root: Value = serde_json::from_str(text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Config::from_value(root))
}

pub fn history_dir(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("history")
}

fn stamp() -> String {
    // UTC timestamp without a date-time dependency (civil-from-days algorithm).
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Atomically write a new config. The previous file is kept in `history/` (newest 40).
pub fn save(
    path: &Path,
    chains: &[(String, Vec<String>)],
    settings: &Map<String, Value>,
    note: &str,
) -> Result<(), String> {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_else(|| json!({}));
    if path.exists() {
        let dir = history_dir(path);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let copy = serde_json::to_string_pretty(&root).unwrap_or_default();
        std::fs::write(dir.join(format!("chains-{}-{note}.json", stamp())), copy)
            .map_err(|e| e.to_string())?;
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        files.sort_by_key(|f| f.file_name());
        let excess = files.len().saturating_sub(HISTORY_KEEP);
        for f in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(f.path());
        }
    }
    let obj = root.as_object_mut().ok_or("config root is not an object")?;
    let mut groups = Map::new();
    for (name, list) in chains {
        groups.insert(name.clone(), json!(list));
    }
    obj.insert("chains".into(), Value::Object(groups));
    for (k, v) in settings {
        obj.insert(k.clone(), v.clone());
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&root).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names() {
        assert!(valid_group_name("smart-2.fast_x"));
        assert!(!valid_group_name("bad name"));
        assert!(!valid_group_name("a/b"));
        assert!(!valid_group_name(""));
        assert!(!valid_group_name(&"x".repeat(41)));
    }

    #[test]
    fn validates_and_dedupes() {
        let (c, s) = validate(
            &json!({"chains": {"a": ["x/1", " x/1 ", "y/2"]}, "settings": {"timeout": 30}}),
        )
        .unwrap();
        assert_eq!(
            c,
            vec![("a".to_owned(), vec!["x/1".to_owned(), "y/2".to_owned()])]
        );
        assert_eq!(s["timeout"], 30);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(validate(&json!({"chains": {"bad name": []}})).is_err());
        assert!(validate(&json!({"chains": {"a": ["x|y"]}})).is_err());
        assert!(validate(&json!({"chains": {"a": [""]}})).is_err());
        assert!(validate(&json!({"chains": {}, "settings": {"timeout": 99999}})).is_err());
        assert!(validate(&json!({"chains": {}, "settings": {"timeout": "x"}})).is_err());
        assert!(validate(&json!({})).is_err());
    }

    #[test]
    fn preserves_order_and_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("modelgate-test-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("chains.json");
        std::fs::write(
            &p,
            r#"{"chains":{"z":["a/1"],"a":["b/2"]},"routes":{"/v1/embeddings":{"targets":[]}}}"#,
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(
            cfg.chains.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["z", "a"]
        );
        save(
            &p,
            &[("m".into(), vec!["q/1".into()]), ("b".into(), vec![])],
            &Map::new(),
            "save",
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(
            cfg.chains.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["m", "b"]
        );
        assert_eq!(
            cfg.routes.len(),
            1,
            "unknown/extra keys must survive a save"
        );
        assert_eq!(std::fs::read_dir(history_dir(&p)).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn stamp_format() {
        let s = stamp();
        assert_eq!(s.len(), 15);
        assert_eq!(s.as_bytes()[8], b'-');
    }
}
