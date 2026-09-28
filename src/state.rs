//! Shared state: config (hot-reloaded), per-target health, stats, recent requests, model cache.

use crate::config::{self, Config};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The loaded config plus the file stamp (mtime, size) it was read at.
type CfgSlot = (Arc<Config>, Option<(SystemTime, u64)>);

pub struct Cool {
    pub until: Instant,
    pub fails: u32,
    pub last: String,
}

pub struct Recent {
    pub time: String,
    pub requested: String,
    pub kind: String,
    pub served_by: Option<String>,
    pub ms: u64,
    pub attempts: usize,
    pub skipped: Vec<Value>,
    pub pii: Option<Value>,
}

#[derive(Default)]
pub struct Stats {
    pub requests: u64,
    pub failovers: u64,
    pub errors: u64,
    pub served: HashMap<String, u64>,
}

pub struct AppState {
    pub path: PathBuf,
    pub upstream: String,
    pub admin_token: Option<String>,
    pub passthrough: bool,
    pub client: reqwest::Client,
    pub pii: crate::pii::Pii,
    pub started: Instant,
    cfg: Mutex<CfgSlot>,
    health: Mutex<HashMap<String, Cool>>,
    pub stats: Mutex<Stats>,
    recent: Mutex<VecDeque<Recent>>,
    models: Mutex<Vec<Value>>,
}

pub fn hms() -> String {
    let s = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        s % 86_400 / 3600,
        s % 3600 / 60,
        s % 60
    )
}

pub fn log(event: &str, fields: Value) {
    let mut v = json!({"event": event, "t": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)});
    if let (Some(o), Some(f)) = (v.as_object_mut(), fields.as_object()) {
        o.extend(f.clone());
    }
    println!("{v}");
}

impl AppState {
    pub fn new(path: PathBuf, upstream: String, admin_token: Option<String>) -> Arc<Self> {
        Self::with_pii(path, upstream, admin_token, crate::pii::Pii::from_env())
    }

    pub fn with_pii(
        path: PathBuf,
        upstream: String,
        admin_token: Option<String>,
        pii: crate::pii::Pii,
    ) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        let (cfg, mt) = match config::load(&path) {
            Ok(c) => (c, config::stamp_of(&path)),
            Err(e) => {
                log("config_error", json!({"error": e}));
                (Config::empty(), None)
            }
        };
        Arc::new(Self {
            path,
            upstream,
            admin_token,
            passthrough: !matches!(
                std::env::var("PASSTHROUGH")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "0" | "false" | "off" | "no"
            ),
            client,
            pii,
            started: Instant::now(),
            cfg: Mutex::new((Arc::new(cfg), mt)),
            health: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
            recent: Mutex::new(VecDeque::new()),
            models: Mutex::new(Vec::new()),
        })
    }

    /// Current config; re-read from disk when the file changed. A broken edit keeps the last good config.
    pub fn config(&self) -> Arc<Config> {
        self.load_if(false)
    }

    /// Re-read the file unconditionally. Used after our own writes, when a same-tick edit
    /// would leave the modification stamp unchanged.
    pub fn reload(&self) -> Arc<Config> {
        self.load_if(true)
    }

    fn load_if(&self, force: bool) -> Arc<Config> {
        let now = config::stamp_of(&self.path);
        let mut g = self.cfg.lock().unwrap();
        if now.is_some() && (force || now != g.1) {
            match config::load(&self.path) {
                Ok(c) => {
                    log(
                        "config_loaded",
                        json!({"groups": c.chains.iter().map(|c| c.0.clone()).collect::<Vec<_>>()}),
                    );
                    *g = (Arc::new(c), now);
                }
                Err(e) => {
                    log("config_error", json!({"error": e}));
                    g.1 = now; // do not re-log every request
                }
            }
        }
        g.0.clone()
    }

    pub fn version(&self) -> String {
        config::content_version(&self.path)
    }

    // ---- health -------------------------------------------------------
    pub fn in_cooldown(&self, key: &str) -> bool {
        self.health
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|c| c.until > Instant::now())
    }

    /// Cool a target down: 30s, doubling per consecutive failure up to 10 minutes (or `retry_after`).
    pub fn mark_bad(&self, key: &str, reason: &str, retry_after: Option<u64>) {
        let mut h = self.health.lock().unwrap();
        let c = h.entry(key.to_owned()).or_insert(Cool {
            until: Instant::now(),
            fails: 0,
            last: String::new(),
        });
        c.fails += 1;
        let secs = retry_after.unwrap_or_else(|| (30u64 << (c.fails - 1).min(5)).min(600));
        c.until = Instant::now() + Duration::from_secs(secs);
        c.last = reason.to_owned();
    }

    pub fn mark_good(&self, key: &str, label: &str) {
        self.health.lock().unwrap().remove(key);
        *self
            .stats
            .lock()
            .unwrap()
            .served
            .entry(label.to_owned())
            .or_insert(0) += 1;
    }

    pub fn cooling(&self) -> Value {
        let now = Instant::now();
        let h = self.health.lock().unwrap();
        let mut m = serde_json::Map::new();
        for (k, c) in h.iter().filter(|(_, c)| c.until > now) {
            m.insert(k.clone(), json!({"seconds_left": (c.until - now).as_secs(), "fails": c.fails, "last": c.last}));
        }
        Value::Object(m)
    }

    // ---- recent requests ------------------------------------------------
    pub fn note(&self, r: Recent) {
        let mut q = self.recent.lock().unwrap();
        q.push_front(r);
        q.truncate(100);
    }

    pub fn recent_json(&self) -> Value {
        json!(self
            .recent
            .lock()
            .unwrap()
            .iter()
            .map(|r| json!({"time": r.time, "requested": r.requested, "kind": r.kind, "served_by": r.served_by,
                            "ms": r.ms, "attempts": r.attempts, "skipped": r.skipped, "pii": r.pii}))
            .collect::<Vec<_>>())
    }

    // ---- upstream model list ----------------------------------------------
    pub fn models(&self) -> Vec<Value> {
        self.models.lock().unwrap().clone()
    }

    pub async fn refresh_models(&self) {
        let url = format!("{}/v1/models", self.upstream.trim_end_matches('/'));
        let res = tokio::time::timeout(Duration::from_secs(10), self.client.get(&url).send()).await;
        if let Ok(Ok(r)) = res {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(d) = v.get("data").and_then(Value::as_array) {
                    *self.models.lock().unwrap() = d.clone();
                }
            }
        }
    }

    pub fn target_spec(&self, spec: &str) -> String {
        let models = self.models.lock().unwrap();
        if let Some((p, _)) = spec.split_once('/') {
            if models
                .iter()
                .any(|m| crate::chain::provider_of(m) == Some(p))
            {
                return spec.to_owned();
            }
        }
        models
            .iter()
            .find(|m| m.get("id").and_then(Value::as_str) == Some(spec))
            .and_then(crate::chain::provider_of)
            .map(|p| format!("{p}/{spec}"))
            .unwrap_or_else(|| spec.to_owned())
    }

    /// Ids the upstream itself serves over `/v1/messages` (no translation needed for these).
    pub fn native_messages_ids(&self) -> Vec<String> {
        self.models()
            .iter()
            .filter(|m| {
                m.get("supported_endpoints")
                    .and_then(Value::as_array)
                    .is_some_and(|e| e.iter().any(|x| x == "/v1/messages"))
            })
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect()
    }
}
