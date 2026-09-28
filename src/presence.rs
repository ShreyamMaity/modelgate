use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub use_bonsai: bool,
    pub route: String,
    pub reason: String,
    pub busy: bool,
    pub base: Option<String>,
}

#[derive(Default)]
struct Snapshot {
    nodes: HashMap<String, Node>,
    at: Option<Instant>,
    error: Option<String>,
}

pub struct Presence {
    url: Mutex<Option<String>>,
    pub interval: Duration,
    pub max_age: Duration,
    pub fetch_timeout: Duration,
    state: Mutex<Snapshot>,
}

fn env_ms(key: &str, dflt: u64) -> Duration {
    Duration::from_millis(
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(dflt),
    )
}

impl Presence {
    pub fn new(url: Option<String>) -> Self {
        Self {
            url: Mutex::new(url.filter(|u| !u.is_empty())),
            interval: Duration::from_millis(1500),
            max_age: Duration::from_millis(5000),
            fetch_timeout: Duration::from_millis(800),
            state: Mutex::new(Snapshot::default()),
        }
    }

    pub fn from_env() -> Self {
        let mut p = Self::new(std::env::var("PRESENCE_URL").ok());
        p.interval = env_ms("PRESENCE_POLL_MS", 1500);
        p.max_age = env_ms("PRESENCE_MAX_AGE_MS", 5000);
        p.fetch_timeout = env_ms("PRESENCE_TIMEOUT_MS", 800);
        p
    }

    pub fn url(&self) -> Option<String> {
        self.url.lock().unwrap().clone()
    }

    pub fn set_url(&self, url: Option<String>) {
        *self.url.lock().unwrap() = url.filter(|u| !u.is_empty());
    }

    pub fn update(&self, body: &Value) -> usize {
        let mut nodes = HashMap::new();
        if let Some(obj) = body.get("nodes").and_then(Value::as_object) {
            for (name, n) in obj {
                let b = n.get("bonsai").filter(|b| b.is_object());
                let base = b.and_then(|b| {
                    let host = b.get("host")?.as_str()?;
                    let port = b.get("port")?.as_u64()?;
                    Some(format!("http://{host}:{port}"))
                });
                nodes.insert(
                    name.clone(),
                    Node {
                        use_bonsai: n.get("use_bonsai").and_then(Value::as_bool) == Some(true),
                        route: n
                            .get("route")
                            .and_then(Value::as_str)
                            .unwrap_or("skip")
                            .to_owned(),
                        reason: n
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        busy: b.and_then(|b| b.get("busy")).and_then(Value::as_bool) == Some(true),
                        base,
                    },
                );
            }
        }
        let count = nodes.len();
        let mut g = self.state.lock().unwrap();
        *g = Snapshot {
            nodes,
            at: Some(Instant::now()),
            error: None,
        };
        count
    }

    fn note_error(&self, e: String) {
        self.state.lock().unwrap().error = Some(e);
    }

    pub fn eligible(&self, node: &str, skip_busy: bool) -> Result<Option<String>, String> {
        if self.url().is_none() {
            return Err("no presence source".into());
        }
        let g = self.state.lock().unwrap();
        match g.at {
            Some(t) if t.elapsed() <= self.max_age => {}
            _ => return Err("presence stale".into()),
        }
        let Some(n) = g.nodes.get(node) else {
            return Err(format!("{node} unknown"));
        };
        if !n.use_bonsai || n.route != "bonsai" {
            let why = if n.reason.is_empty() {
                n.route.clone()
            } else {
                n.reason.clone()
            };
            return Err(format!("{node} {why}"));
        }
        if skip_busy && n.busy {
            return Err(format!("{node} busy"));
        }
        Ok(n.base.clone())
    }

    pub async fn poll_once(&self, client: &reqwest::Client) -> Result<usize, String> {
        let Some(url) = self.url() else {
            return Err("no presence source".into());
        };
        let url = format!("{}/presence", url.trim_end_matches('/'));
        let res = tokio::time::timeout(self.fetch_timeout, async {
            let r = client.get(&url).send().await.map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!("status {}", r.status().as_u16()));
            }
            r.json::<Value>().await.map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|_| Err("timeout".into()));
        match res {
            Ok(v) => Ok(self.update(&v)),
            Err(e) => {
                self.note_error(e.clone());
                Err(e)
            }
        }
    }

    pub fn status(&self) -> Value {
        let g = self.state.lock().unwrap();
        let nodes: serde_json::Map<String, Value> = g
            .nodes
            .iter()
            .map(|(k, n)| {
                (
                    k.clone(),
                    json!({"use_bonsai": n.use_bonsai, "route": n.route, "reason": n.reason, "busy": n.busy, "base": n.base}),
                )
            })
            .collect();
        json!({
            "source": self.url(),
            "age_ms": g.at.map(|t| t.elapsed().as_millis() as u64),
            "max_age_ms": self.max_age.as_millis() as u64,
            "last_error": g.error,
            "nodes": nodes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(use_bonsai: bool, route: &str, reason: &str, busy: bool) -> Value {
        json!({"nodes": {"pc": {"use_bonsai": use_bonsai, "route": route, "reason": reason,
            "bonsai": {"host": "10.1.2.3", "port": 8081, "serving": true, "busy": busy}}}})
    }

    #[test]
    fn eligible_only_when_hub_says_bonsai() {
        let p = Presence::new(Some("http://hub".into()));
        assert_eq!(p.eligible("pc", false).unwrap_err(), "presence stale");
        p.update(&body(true, "bonsai", "ok", false));
        assert_eq!(
            p.eligible("pc", false).unwrap().as_deref(),
            Some("http://10.1.2.3:8081")
        );
        p.update(&body(false, "cloud", "gpu busy (game.exe)", false));
        assert_eq!(
            p.eligible("pc", false).unwrap_err(),
            "pc gpu busy (game.exe)"
        );
        p.update(&body(false, "skip", "", false));
        assert_eq!(p.eligible("pc", false).unwrap_err(), "pc skip");
        assert_eq!(p.eligible("mac", false).unwrap_err(), "mac unknown");
    }

    #[test]
    fn busy_only_matters_when_asked() {
        let p = Presence::new(Some("http://hub".into()));
        p.update(&body(true, "bonsai", "ok", true));
        assert!(p.eligible("pc", false).is_ok());
        assert_eq!(p.eligible("pc", true).unwrap_err(), "pc busy");
    }

    #[test]
    fn stale_and_missing_source_skip() {
        let mut p = Presence::new(Some("http://hub".into()));
        p.max_age = Duration::from_millis(30);
        p.update(&body(true, "bonsai", "ok", false));
        assert!(p.eligible("pc", false).is_ok());
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(p.eligible("pc", false).unwrap_err(), "presence stale");
        let q = Presence::new(None);
        q.update(&body(true, "bonsai", "ok", false));
        assert_eq!(q.eligible("pc", false).unwrap_err(), "no presence source");
    }

    #[test]
    fn status_reports_nodes() {
        let p = Presence::new(Some("http://hub".into()));
        p.update(&body(true, "bonsai", "ok", false));
        let s = p.status();
        assert_eq!(s["nodes"]["pc"]["use_bonsai"], true);
        assert_eq!(s["source"], "http://hub");
    }
}
