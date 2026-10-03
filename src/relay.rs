//! Forwarding a request through an ordered list of targets, failing over on trouble.

use crate::pii::{self, Rehydrate, SseRehydrator, Tier};
use crate::pins::Event;
use crate::policy::{Mode, Policy};
use crate::shim;
use crate::state::{hms, log, AppState, Recent};
use axum::body::Body;
use axum::http::{header, HeaderMap, Response, StatusCode};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Provider is down, unauthorised, out of quota or slow: skip it and cool it down.
pub const HARD: &[u16] = &[
    401, 402, 403, 404, 408, 410, 425, 429, 500, 502, 503, 504, 520, 521, 522, 523, 524,
];
/// This target rejected this request: try the next one, but do not treat the provider as sick.
pub const SOFT: &[u16] = &[400, 413, 422];

const HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    "accept-encoding",
];

#[derive(Clone, Debug, Default)]
pub struct Target {
    pub label: String,
    pub base: String,
    /// Rewrites the request's `model` field for this target.
    pub model: Option<String>,
    /// Overrides the request path for this target.
    pub path: Option<String>,
    pub bearer: Option<String>,
    pub presence: Option<String>,
    pub skip_busy: bool,
    pub ttfb: Option<f64>,
    pub direct: bool,
    pub max_input: Option<usize>,
}

impl Target {
    pub fn direct(d: &crate::config::DirectTarget) -> Self {
        Target {
            label: d.label.clone(),
            base: d.base.clone(),
            model: d.model.clone(),
            path: d.path.clone(),
            bearer: d
                .key_env
                .as_ref()
                .and_then(|k| std::env::var(k).ok())
                .filter(|k| !k.is_empty()),
            presence: d.presence.clone(),
            skip_busy: d.skip_busy,
            ttfb: d.timeout,
            direct: true,
            max_input: d.max_input_chars,
        }
    }
}

pub struct Opts {
    /// Path used for the upstream request (and to pick the cooldown "API shape").
    pub path: String,
    pub kind: String,
    pub headers: HeaderMap,
    /// Set when the client spoke Anthropic `/v1/messages`: `payload` is already the equivalent
    /// chat-completions body and the reply is converted back. Holds the model name to echo.
    pub translate: Option<String>,
    pub plan: Option<Plan>,
}

pub fn json_response(status: u16, v: &Value, extra: &[(&str, String)]) -> Response<Body> {
    let mut b = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in extra {
        b = b.header(*k, v);
    }
    b.body(Body::from(v.to_string())).unwrap()
}

fn apply_headers(
    mut rb: reqwest::RequestBuilder,
    h: &HeaderMap,
    t: &Target,
    translating: bool,
    masked: bool,
) -> reqwest::RequestBuilder {
    let mut has_ct = false;
    for (k, v) in h {
        let n = k.as_str();
        if HOP.contains(&n)
            || n.starts_with("x-luna-")
            || (masked && pii::CONV_HEADERS.contains(&n))
        {
            continue;
        }
        // Anthropic-style client credentials mean nothing to a chat-completions upstream.
        if translating
            && (n.starts_with("anthropic-")
                || matches!(n, "x-api-key" | "authorization" | "content-type"))
        {
            continue;
        }
        // A direct upstream's own key replaces whatever the client sent.
        if t.bearer.is_some() && matches!(n, "authorization" | "x-api-key") {
            continue;
        }
        has_ct |= n == "content-type";
        rb = rb.header(k, v);
    }
    if translating || !has_ct {
        rb = rb.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(b) = &t.bearer {
        rb = rb.bearer_auth(b);
    }
    rb.header(header::ACCEPT_ENCODING, "identity")
}

fn idle_stream<S>(s: S, idle: Duration) -> impl Stream<Item = Result<Bytes, std::io::Error>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin + Send + 'static,
{
    futures_util::stream::unfold(Some(s), move |state| async move {
        let mut s = state?;
        match tokio::time::timeout(idle, s.next()).await {
            Ok(Some(Ok(b))) => Some((Ok(b), Some(s))),
            Ok(Some(Err(e))) => Some((Err(std::io::Error::other(e.to_string())), None)),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "upstream stream idle timeout",
                )),
                None,
            )),
        }
    })
}

pub struct Pii {
    pub rh: Option<Arc<Rehydrate>>,
    pub header: Option<String>,
}

fn pii_header(info: &Value) -> String {
    format!(
        "tier={}; masked={}",
        info["tier"].as_str().unwrap_or("-"),
        info["masked"].as_u64().unwrap_or(0)
    )
}

fn head(
    resp: &reqwest::Response,
    label: &str,
    attempts: usize,
    p: &Pii,
) -> axum::http::response::Builder {
    let mut b = Response::builder().status(resp.status());
    for (k, v) in resp.headers() {
        if !HOP.contains(&k.as_str()) {
            b = b.header(k, v);
        }
    }
    if let Some(h) = &p.header {
        b = b.header("x-gateway-pii", h);
    }
    b.header("x-gateway-target", label)
        .header("x-gateway-attempts", attempts.to_string())
}

async fn passthrough(
    resp: reqwest::Response,
    label: &str,
    attempts: usize,
    idle: Duration,
    wait: Duration,
    p: Pii,
) -> Response<Body> {
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let b = head(&resp, label, attempts, &p);
    let Some(rh) = p.rh else {
        return b
            .body(Body::from_stream(idle_stream(
                Box::pin(resp.bytes_stream()),
                idle,
            )))
            .unwrap();
    };
    if ct.contains("event-stream") {
        let s = Box::pin(idle_stream(Box::pin(resp.bytes_stream()), idle));
        let out = futures_util::stream::unfold(
            (s, Some(SseRehydrator::new(rh))),
            |(mut s, rz)| async move {
                let mut rz = rz?;
                match s.next().await {
                    Some(Ok(b)) => {
                        let out = Bytes::from(rz.feed(&b));
                        Some((Ok(out), (s, Some(rz))))
                    }
                    Some(Err(e)) => Some((Err(e), (s, None))),
                    None => Some((Ok(Bytes::from(rz.finish())), (s, None))),
                }
            },
        );
        return b.body(Body::from_stream(out)).unwrap();
    }
    if ct.contains("json") || ct.is_empty() {
        let data = match tokio::time::timeout(wait, resp.bytes()).await {
            Ok(Ok(d)) => d,
            _ => {
                return json_response(
                    504,
                    &json!({"error": {"message": "upstream response timed out"}}),
                    &[],
                )
            }
        };
        let body = match serde_json::from_slice::<Value>(&data) {
            Ok(mut v) => {
                rh.json(&mut v);
                Bytes::from(v.to_string())
            }
            Err(_) => data,
        };
        return b.body(Body::from(body)).unwrap();
    }
    b.body(Body::from_stream(idle_stream(
        Box::pin(resp.bytes_stream()),
        idle,
    )))
    .unwrap()
}

fn translated_stream(
    resp: reqwest::Response,
    model: String,
    label: &str,
    attempts: usize,
    idle: Duration,
    p: Pii,
) -> Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let pii_hdr = p.header.clone();
    let mut rz = p.rh.map(SseRehydrator::new);
    tokio::spawn(async move {
        let mut conv = shim::StreamConv::new(&model);
        for e in conv.start() {
            if tx.send(Ok(e)).await.is_err() {
                return;
            }
        }
        let mut s = Box::pin(resp.bytes_stream());
        loop {
            let events = match tokio::time::timeout(idle, s.next()).await {
                Ok(Some(Ok(b))) => match &mut rz {
                    Some(r) => conv.feed(&r.feed(&b)),
                    None => conv.feed(&b),
                },
                Ok(None) => {
                    let mut tail = match &mut rz {
                        Some(r) => conv.feed(&r.finish()),
                        None => Vec::new(),
                    };
                    tail.extend(conv.finish());
                    for e in tail {
                        let _ = tx.send(Ok(e)).await;
                    }
                    return;
                }
                Ok(Some(Err(err))) => {
                    for e in conv.abort(&format!("upstream stream failed: {err}")) {
                        let _ = tx.send(Ok(e)).await;
                    }
                    return;
                }
                Err(_) => {
                    for e in conv.abort("upstream stream idle timeout") {
                        let _ = tx.send(Ok(e)).await;
                    }
                    return;
                }
            };
            for e in events {
                if tx.send(Ok(e)).await.is_err() {
                    return; // client went away
                }
            }
        }
    });
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache");
    if let Some(h) = pii_hdr {
        b = b.header("x-gateway-pii", h);
    }
    b.header("x-gateway-target", label)
        .header("x-gateway-attempts", attempts.to_string())
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .unwrap()
}

async fn translated_buffered(
    resp: reqwest::Response,
    model: &str,
    label: &str,
    attempts: usize,
    wait: Duration,
    p: Pii,
) -> Response<Body> {
    let mut extra = vec![
        ("x-gateway-target", label.to_owned()),
        ("x-gateway-attempts", attempts.to_string()),
    ];
    if let Some(h) = &p.header {
        extra.push(("x-gateway-pii", h.clone()));
    }
    match tokio::time::timeout(wait, resp.bytes()).await {
        Ok(Ok(b)) => match serde_json::from_slice::<Value>(&b) {
            Ok(mut v) => {
                if let Some(rh) = &p.rh {
                    rh.json(&mut v);
                }
                json_response(200, &shim::from_openai(&v, model), &extra)
            }
            Err(_) => json_response(
                502,
                &shim::error_body(502, "upstream returned invalid JSON"),
                &extra,
            ),
        },
        _ => json_response(
            504,
            &shim::error_body(504, "upstream response timed out"),
            &extra,
        ),
    }
}

const OPTIONAL_FIELDS: &[&str] = &[
    "reasoning_effort",
    "reasoning",
    "max_completion_tokens",
    "parallel_tool_calls",
    "service_tier",
    "stream_options",
    "seed",
    "top_k",
    "logprobs",
    "top_logprobs",
    "prediction",
    "store",
    "metadata",
    "user",
    "verbosity",
    "thinking",
    "chat_template_kwargs",
];

pub fn without_rejected_fields(body: &[u8], err: &[u8]) -> Option<(Bytes, Vec<String>)> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let obj = v.as_object_mut()?;
    let err = String::from_utf8_lossy(err);
    let names: Vec<String> = OPTIONAL_FIELDS
        .iter()
        .filter(|k| obj.contains_key(**k) && err.contains(**k))
        .map(|k| (*k).to_owned())
        .collect();
    if names.is_empty() {
        return None;
    }
    for k in &names {
        let old = obj.remove(k);
        if k == "max_completion_tokens" && !obj.contains_key("max_tokens") {
            if let Some(o) = old {
                obj.insert("max_tokens".into(), o);
            }
        }
    }
    Some((Bytes::from(v.to_string()), names))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Errors worth one more try on the same model before failing over.
const RETRYABLE: &[u16] = &[408, 425, 429, 500, 502, 503, 504, 520, 521, 522, 523, 524];

pub struct Plan {
    pub group: String,
    pub policy: Policy,
    pub session: Option<String>,
    pub key: Option<String>,
    pub pin: Option<String>,
    pub explicit: Option<String>,
    pub list: Vec<String>,
}

struct Meta {
    group: String,
    served: Option<String>,
    failover: Option<(String, String, String)>,
    session: Option<String>,
    pin: &'static str,
    policy: Option<String>,
}

impl Meta {
    fn apply(&self, h: &mut HeaderMap) {
        let mut put = |k: &'static str, v: &str| {
            if let Ok(v) = header::HeaderValue::from_str(v) {
                h.insert(k, v);
            }
        };
        put("x-served-model", self.served.as_deref().unwrap_or("none"));
        put("x-served-group", &self.group);
        put("x-luna-session", self.session.as_deref().unwrap_or("-"));
        put("x-pin", self.pin);
        if let Some((from, to, why)) = &self.failover {
            put("x-failover", &format!("{from} -> {to}; reason={why}"));
        }
        if let Some(p) = &self.policy {
            put("x-gateway-policy", p);
        }
    }

    fn served_json(&self) -> Value {
        json!({"model": self.served, "group": self.group, "session": self.session, "pin": self.pin,
               "failover": self.failover.as_ref().map(|(f, t, r)| json!({"from": f, "to": t, "reason": r}))})
    }
}

fn reason_of(a: &Value) -> String {
    if let Some(s) = a.get("status").and_then(Value::as_u64) {
        return if SOFT.contains(&(s as u16)) {
            format!("rejected {s}")
        } else {
            format!("status {s}")
        };
    }
    let e = a.get("error").and_then(Value::as_str).unwrap_or("error");
    if e.starts_with("presence") {
        "presence".into()
    } else if e == "input too large" {
        "input-too-large".into()
    } else {
        e.to_owned()
    }
}

fn first_reason(attempts: &[Value], label: &str, cooling: bool) -> String {
    attempts
        .iter()
        .rev()
        .find(|a| a["target"] == label && a.get("retry").is_none())
        .map(reason_of)
        .unwrap_or_else(|| {
            if cooling {
                "cooldown".into()
            } else {
                "skipped".into()
            }
        })
}

fn header_str(h: &HeaderMap, k: &str) -> String {
    h.get(k)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

/// Test traffic (`X-Luna-Test`, or a session id starting with `test`) keeps its own cooldowns,
/// so failures it provokes never change what answers real chats.
pub fn is_test(h: &HeaderMap) -> bool {
    let v = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(str::trim);
    if v("x-luna-test").is_some_and(|t| !t.is_empty() && t != "0" && t != "false") {
        return true;
    }
    [
        "x-luna-session",
        "x-conversation-id",
        "x-hermes-session-id",
        "x-session-id",
    ]
    .iter()
    .find_map(|n| v(n).filter(|s| !s.is_empty()))
    .is_some_and(|s| s.to_ascii_lowercase().starts_with("test"))
}

pub fn is_stream(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .and_then(|v| v.get("stream").and_then(Value::as_bool))
        == Some(true)
}

/// Streams that have not started after `keepalive` seconds get their headers early, then
/// keepalives, so proxies and phone clients don't drop the connection while models fail over.
pub async fn relay(
    st: &Arc<AppState>,
    targets: Vec<Target>,
    payload: Bytes,
    o: Opts,
) -> Response<Body> {
    if !is_stream(&payload) {
        return walk(st.clone(), targets, payload, o).await;
    }
    let ka = Duration::from_secs_f64(st.config().settings.keepalive);
    let anthropic = o.translate.is_some();
    let early = Meta {
        group: o
            .plan
            .as_ref()
            .map(|p| p.group.clone())
            .unwrap_or_else(|| "-".into()),
        served: targets.first().map(|t| t.label.clone()),
        failover: None,
        session: o.plan.as_ref().and_then(|p| p.session.clone()),
        pin: "pending",
        policy: o.plan.as_ref().map(|p| p.policy.header()),
    };
    let mut task = tokio::spawn(walk(st.clone(), targets, payload, o));
    match tokio::time::timeout(ka, &mut task).await {
        Ok(Ok(r)) => return r,
        Ok(Err(_)) => {
            return json_response(
                500,
                &json!({"error": {"message": "gateway task failed"}}),
                &[],
            )
        }
        Err(_) => {}
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ka);
        tick.tick().await;
        let _ = tx
            .send(Ok(Bytes::from_static(crate::stream::KEEPALIVE)))
            .await;
        let resp = loop {
            tokio::select! {
                r = &mut task => break r.ok(),
                _ = tick.tick() => {
                    if tx.send(Ok(Bytes::from_static(crate::stream::KEEPALIVE))).await.is_err() {
                        task.abort();
                        return;
                    }
                }
            }
        };
        let Some(resp) = resp else {
            let v = json!({"message": "gateway task failed", "type": "gateway_error"});
            let _ = tx.send(Ok(crate::stream::error_event(anthropic, &v))).await;
            return;
        };
        let sse = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.contains("event-stream"));
        if resp.status().is_success() && sse {
            let mut s = resp.into_body().into_data_stream();
            while let Some(b) = s.next().await {
                let item = b.map_err(|e| std::io::Error::other(e.to_string()));
                let stop = item.is_err();
                if tx.send(item).await.is_err() || stop {
                    return;
                }
            }
            return;
        }
        let status = resp.status().as_u16();
        let model = header_str(resp.headers(), "x-served-model");
        let failover = header_str(resp.headers(), "x-failover");
        let data = axum::body::to_bytes(resp.into_body(), 65_536)
            .await
            .unwrap_or_default();
        let body: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
        let err = if body["error"]["gateway"].is_object() {
            body["error"]["gateway"].clone()
        } else {
            body["error"].clone()
        };
        let msg = err["message"]
            .as_str()
            .or(err.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&data[..data.len().min(500)]).into_owned());
        let v = json!({"message": msg, "type": err["type"].as_str().unwrap_or("gateway_exhausted"),
                       "reason": err["reason"], "model": err.get("model").cloned().unwrap_or(json!(model)),
                       "failover": failover, "status": status});
        let _ = tx.send(Ok(crate::stream::error_event(anthropic, &v))).await;
    });
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-gateway-early", "1");
    if let Some(h) = b.headers_mut() {
        early.apply(h);
    }
    b.body(Body::from_stream(
        tokio_stream::wrappers::ReceiverStream::new(rx),
    ))
    .unwrap()
}

/// Try `targets` in order (healthy ones first) until one answers, following the group's policy.
async fn walk(st: Arc<AppState>, targets: Vec<Target>, payload: Bytes, o: Opts) -> Response<Body> {
    let st = &st;
    let started = Instant::now();
    let cfg = st.config();
    let parsed: Option<Value> = serde_json::from_slice(&payload).ok();
    let req_model = parsed
        .as_ref()
        .and_then(|v| v.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_owned();
    let streaming = parsed
        .as_ref()
        .and_then(|v| v.get("stream"))
        .and_then(Value::as_bool)
        == Some(true);
    let policy = o.plan.as_ref().map(|p| p.policy.clone());
    let strict = policy.as_ref().is_some_and(|p| p.mode == Mode::Strict);
    let retries = policy.as_ref().map(|p| p.retry_same_model).unwrap_or(0);
    let cool_base = policy.as_ref().map(|p| p.cooldown_seconds).unwrap_or(30);
    let stop_on_timeout = policy.as_ref().is_some_and(|p| !p.failover_on_timeouts);
    // A streaming reply starts fast or never; a buffered one may legitimately take a while.
    let ttfb = Duration::from_secs_f64(if streaming {
        policy
            .as_ref()
            .and_then(|p| p.ttfb_seconds)
            .unwrap_or(cfg.settings.ttfb_stream)
    } else {
        policy
            .as_ref()
            .and_then(|p| p.total_timeout_seconds)
            .unwrap_or(cfg.settings.timeout)
    });
    let total = policy
        .as_ref()
        .and_then(|p| p.total_timeout_seconds)
        .map(Duration::from_secs_f64);
    let idle = Duration::from_secs_f64(cfg.settings.stream_idle);
    let ns = if is_test(&o.headers) { "test:" } else { "" };
    let family = if o.path == "/v1/messages" {
        "anthropic"
    } else {
        "openai"
    }; // cooldowns are per API shape

    let intended = targets.first().map(|t| t.label.clone()).unwrap_or_default();
    let keyed: Vec<(String, Target)> = targets
        .into_iter()
        .map(|t| (format!("{ns}{family}:{}", t.label), t))
        .collect();
    let intended_cooling = keyed.first().is_some_and(|(k, _)| st.in_cooldown(k));
    let ordered: Vec<(String, Target)> = if strict {
        keyed.into_iter().take(1).collect()
    } else {
        let (mut ordered, cooling): (Vec<_>, Vec<_>) =
            keyed.into_iter().partition(|(k, _)| !st.in_cooldown(k));
        ordered.extend(cooling); // cooled-down targets stay as a last resort
        ordered
    };
    let mut meta = Meta {
        group: o
            .plan
            .as_ref()
            .map(|p| p.group.clone())
            .unwrap_or_else(|| "-".into()),
        served: None,
        failover: None,
        session: o.plan.as_ref().and_then(|p| p.session.clone()),
        pin: "none",
        policy: policy.as_ref().map(Policy::header),
    };

    let mut attempts: Vec<Value> = Vec::new();
    let mut last: Option<(u16, Option<header::HeaderValue>, Bytes)> = None;
    let mut stopped: Option<String> = None;
    let ckey = pii::conv_key(&o.headers, parsed.as_ref());
    let mut ner_by_tier: HashMap<Tier, Result<Vec<pii::ner::Found>, String>> = HashMap::new();
    'targets: for (key, mut t) in ordered {
        if t.max_input.is_some_and(|n| payload.len() > n) {
            attempts.push(json!({"target": t.label, "error": "input too large"}));
            continue;
        }
        if let Some(node) = &t.presence {
            match st.presence.eligible(node, t.skip_busy) {
                Ok(found) => {
                    if t.base.is_empty() {
                        t.base = found.unwrap_or_default();
                    }
                }
                Err(why) => {
                    attempts.push(json!({"target": t.label, "error": format!("presence: {why}")}));
                    continue;
                }
            }
            if t.base.is_empty() {
                attempts.push(json!({"target": t.label, "error": "presence: no address"}));
                continue;
            }
        }
        let wait = t
            .ttfb
            .map(|s| ttfb.min(Duration::from_secs_f64(s)))
            .unwrap_or(ttfb);
        let tier = if st.pii.enabled {
            let spec = if !t.direct && t.base == st.upstream {
                t.model.clone().unwrap_or_else(|| req_model.clone())
            } else {
                t.label.clone()
            };
            Some(cfg.pii.tier(&st.target_spec(&spec)))
        } else {
            None
        };
        let masking = tier.is_some_and(Tier::masks);
        let mut info: Option<Value> = tier.map(|t| json!({"tier": t.name(), "masked": 0}));
        let mut rh: Option<Arc<Rehydrate>> = None;
        let body = if masking {
            let tier = tier.unwrap_or(Tier::Public);
            if let (Some(v), false) = (&parsed, ner_by_tier.contains_key(&tier)) {
                let r = st.pii.ner_find(&st.client, tier, &cfg.pii, v).await;
                ner_by_tier.insert(tier, r);
            }
            let found = ner_by_tier.get(&tier).cloned().unwrap_or(Ok(Vec::new()));
            let masked = found.and_then(|found| {
                parsed
                    .as_ref()
                    .ok_or_else(|| "body is not JSON".to_owned())
                    .and_then(|v| {
                        let mut v = v.clone();
                        if let Some(m) = &t.model {
                            v["model"] = json!(m);
                        }
                        st.pii
                            .mask_with(&ckey, &mut v, tier, &cfg.pii, &found)
                            .map(|(r, h)| (v, r, h))
                    })
            });
            match masked {
                Ok((v, r, h)) => {
                    info = Some(r.to_json(tier));
                    if !h.is_empty() {
                        rh = Some(h);
                    }
                    Bytes::from(v.to_string())
                }
                Err(e) => {
                    st.pii.note_blocked();
                    log("pii_blocked", json!({"target": t.label, "error": e}));
                    attempts.push(json!({"target": t.label, "error": "pii-blocked", "detail": e}));
                    continue;
                }
            }
        } else {
            match (&t.model, &parsed) {
                (Some(m), Some(v)) => {
                    let mut v = v.clone();
                    v["model"] = json!(m);
                    Bytes::from(v.to_string())
                }
                _ => payload.clone(),
            }
        };
        let url = format!(
            "{}{}",
            t.base.trim_end_matches('/'),
            t.path.as_deref().unwrap_or(&o.path)
        );
        let mut body = body;
        let mut dropped = false;
        let mut tries_left = if t.label == intended { retries } else { 0 };
        let mut t0;
        let resp = loop {
            t0 = Instant::now();
            let rb = apply_headers(
                st.client.post(&url).body(body.clone()),
                &o.headers,
                &t,
                o.translate.is_some(),
                masking,
            );
            let failed = match tokio::time::timeout(wait, rb.send()).await {
                Ok(Ok(r)) => {
                    let status = r.status().as_u16();
                    if !(HARD.contains(&status) || SOFT.contains(&status)) {
                        break r;
                    }
                    let retry_after = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|n| n.min(300));
                    let ct = r.headers().get(header::CONTENT_TYPE).cloned();
                    let data = r.bytes().await.unwrap_or_default();
                    let data = data.slice(..data.len().min(65_536));
                    if SOFT.contains(&status) && !dropped {
                        if let Some((nb, names)) = without_rejected_fields(&body, &data) {
                            attempts.push(json!({"target": t.label, "status": status, "error": format!("retried without {}", names.join(", ")), "retry": true}));
                            dropped = true;
                            body = nb;
                            continue;
                        }
                    }
                    let wrong_shape = status == 404 && contains(&data, b"is available via");
                    if RETRYABLE.contains(&status) && tries_left > 0 {
                        tries_left -= 1;
                        attempts.push(json!({"target": t.label, "status": status, "retry": true}));
                        let pause = retry_after.unwrap_or(1).clamp(1, 5);
                        tokio::time::sleep(Duration::from_secs(pause)).await;
                        continue;
                    }
                    if HARD.contains(&status) && !wrong_shape {
                        st.mark_bad(&key, &status.to_string(), retry_after, cool_base);
                    }
                    attempts.push(json!({"target": t.label, "status": status}));
                    last = Some((status, ct, data));
                    None
                }
                Ok(Err(e)) => Some(if e.is_timeout() { "timeout" } else { "connect" }),
                Err(_) => Some("timeout"),
            };
            if let Some(what) = failed {
                if what == "connect" && tries_left > 0 {
                    tries_left -= 1;
                    attempts.push(json!({"target": t.label, "error": what, "retry": true}));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                st.mark_bad(&key, what, None, cool_base);
                attempts.push(json!({"target": t.label, "error": what}));
                if what == "timeout" && stop_on_timeout {
                    stopped = Some(format!("timeout after {}s", wait.as_secs()));
                    break 'targets;
                }
            }
            continue 'targets;
        };
        let status = resp.status().as_u16();
        // Success.
        st.mark_good(&key, &t.label);
        let ms = t0.elapsed().as_millis() as u64;
        if attempts.iter().any(|a| a.get("retry").is_none()) {
            st.stats.lock().unwrap().failovers += 1;
        }
        meta.served = Some(if t.label == "upstream" && req_model != "-" {
            req_model.clone()
        } else {
            t.label.clone()
        });
        if t.label != intended && !intended.is_empty() {
            meta.failover = Some((
                intended.clone(),
                t.label.clone(),
                first_reason(&attempts, &intended, intended_cooling),
            ));
        }
        if let Some(p) = &o.plan {
            settle_pin(st, p, &mut meta);
        }
        log(
            "served",
            json!({"kind": o.kind, "path": o.path, "target": t.label, "status": status, "ttfb_ms": ms, "skipped": attempts, "pii": info,
                   "group": meta.group, "session": meta.session, "pin": meta.pin}),
        );
        let n = attempts.len() + 1;
        let p = Pii {
            rh,
            header: info.as_ref().map(pii_header),
        };
        st.note(Recent {
            time: hms(),
            requested: req_model,
            kind: o.kind,
            served_by: Some(t.label.clone()),
            ms,
            attempts: n,
            skipped: attempts,
            pii: info,
        });
        let anthropic = o.translate.is_some();
        let resp = match o.translate {
            None => passthrough(resp, &t.label, n, idle, ttfb, p).await,
            Some(model) if streaming => translated_stream(resp, model, &t.label, n, idle, p),
            Some(model) => translated_buffered(resp, &model, &t.label, n, ttfb, p).await,
        };
        let ka = Duration::from_secs_f64(cfg.settings.keepalive);
        return finish(resp, &meta, started, ms, ka, total, anthropic);
    }

    st.stats.lock().unwrap().errors += 1;
    let reason = stopped
        .clone()
        .unwrap_or_else(|| first_reason(&attempts, &intended, intended_cooling));
    if let Some(p) = &o.plan {
        if !intended.is_empty() {
            st.pins.event(Event {
                time: crate::pins::now(),
                group: p.group.clone(),
                session: p.session.clone().unwrap_or_default(),
                from: intended.clone(),
                to: None,
                reason: reason.clone(),
            });
        }
    }
    log(
        "exhausted",
        json!({"kind": o.kind, "path": o.path, "attempts": attempts, "group": meta.group, "session": meta.session,
               "strict": strict, "stopped": stopped}),
    );
    st.note(Recent {
        time: hms(),
        requested: req_model,
        kind: o.kind,
        served_by: None,
        ms: 0,
        attempts: attempts.len(),
        skipped: attempts.clone(),
        pii: None,
    });
    let gateway_err = if strict {
        Some((
            503,
            "gateway_strict",
            format!("strict group {}: {intended} failed ({reason})", meta.group),
        ))
    } else {
        stopped.as_ref().map(|s| {
            (
                504,
                "gateway_timeout",
                format!(
                    "group {}: {intended} did not answer ({s}); failover_on=errors",
                    meta.group
                ),
            )
        })
    };
    let mut resp = if let Some((code, kind, msg)) = gateway_err {
        let upstream_status = last.as_ref().map(|l| l.0);
        let e = json!({"message": msg, "type": kind, "reason": reason, "model": intended, "upstream_status": upstream_status});
        if o.translate.is_some() {
            let mut b = shim::error_body(code, &msg);
            b["error"]["gateway"] = e;
            json_response(code, &b, &[("x-gateway-exhausted", "1".into())])
        } else {
            json_response(
                code,
                &json!({ "error": e }),
                &[("x-gateway-exhausted", "1".into())],
            )
        }
    } else if o.translate.is_some() {
        let (code, msg) = match &last {
            Some((c, _, d)) => (
                *c,
                String::from_utf8_lossy(&d[..d.len().min(500)]).into_owned(),
            ),
            None => (
                502,
                format!("all targets failed: {}", Value::Array(attempts)),
            ),
        };
        json_response(
            code,
            &shim::error_body(code, &msg),
            &[("x-gateway-exhausted", "1".into())],
        )
    } else {
        match last {
            Some((c, ct, d)) => Response::builder()
                .status(StatusCode::from_u16(c).unwrap_or(StatusCode::BAD_GATEWAY))
                .header(
                    header::CONTENT_TYPE,
                    ct.unwrap_or_else(|| header::HeaderValue::from_static("application/json")),
                )
                .header("x-gateway-exhausted", "1")
                .body(Body::from(d))
                .unwrap(),
            None => json_response(
                502,
                &json!({"error": {"message": "all targets failed", "type": "gateway_exhausted", "reason": reason, "attempts": attempts}}),
                &[],
            ),
        }
    };
    meta.apply(resp.headers_mut());
    resp
}

fn settle_pin(st: &AppState, p: &Plan, meta: &mut Meta) {
    let served = meta.served.clone().unwrap_or_default();
    if let Some((from, to, why)) = &meta.failover {
        st.pins.event(Event {
            time: crate::pins::now(),
            group: p.group.clone(),
            session: p.session.clone().unwrap_or_default(),
            from: from.clone(),
            to: Some(to.clone()),
            reason: why.clone(),
        });
    }
    let Some(key) = p.key.as_deref().filter(|_| p.policy.mode != Mode::Failover) else {
        meta.pin = "none";
        return;
    };
    let hold = meta
        .failover
        .as_ref()
        .is_some_and(|f| matches!(f.2.as_str(), "presence" | "input-too-large"));
    let set = |source: &str| {
        st.pins.set(
            &p.group,
            key,
            &served,
            p.policy.scope,
            p.policy.ttl_minutes,
            source,
            &p.list,
        )
    };
    meta.pin = if hold && p.pin.is_some() {
        st.pins.touch(&p.group, key);
        "held"
    } else if p.explicit.as_deref() == Some(served.as_str()) {
        set("explicit");
        "explicit"
    } else if p.explicit.is_some() || (p.pin.is_some() && p.pin.as_deref() != Some(served.as_str()))
    {
        set("failover");
        "moved"
    } else if p.pin.is_some() {
        st.pins.touch(&p.group, key);
        "kept"
    } else {
        set("answered");
        "new"
    };
}

fn finish(
    resp: Response<Body>,
    meta: &Meta,
    started: Instant,
    ttfb_ms: u64,
    keepalive: Duration,
    total: Option<Duration>,
    anthropic: bool,
) -> Response<Body> {
    let (mut parts, body) = resp.into_parts();
    meta.apply(&mut parts.headers);
    let sse = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains("event-stream"));
    if !sse || !parts.status.is_success() {
        return Response::from_parts(parts, body);
    }
    let inner: crate::stream::BoxStream = Box::pin(
        body.into_data_stream()
            .map(|r| r.map_err(|e| std::io::Error::other(e.to_string()))),
    );
    let ctx = crate::stream::Ctx {
        prefix: Some(crate::stream::comment(
            "gateway-served",
            &meta.served_json(),
        )),
        model: meta.served.clone().unwrap_or_default(),
        group: meta.group.clone(),
        started,
        ttfb_ms,
        keepalive,
        total,
        anthropic,
    };
    Response::from_parts(
        parts,
        Body::from_stream(crate::stream::decorate(inner, ctx)),
    )
}

pub async fn forward_raw(
    st: &Arc<AppState>,
    method: &axum::http::Method,
    path_and_query: &str,
    headers: &HeaderMap,
    payload: Bytes,
) -> Response<Body> {
    let cfg = st.config();
    let ttfb = Duration::from_secs_f64(cfg.settings.timeout);
    let idle = Duration::from_secs_f64(cfg.settings.stream_idle);
    let url = format!("{}{}", st.upstream.trim_end_matches('/'), path_and_query);
    let path = path_and_query.split('?').next().unwrap_or(path_and_query);
    let req_model = serde_json::from_slice::<Value>(&payload)
        .ok()
        .and_then(|v| v.get("model").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "-".into());
    st.stats.lock().unwrap().requests += 1;
    let mut info: Option<Value> = None;
    let mut rh: Option<Arc<Rehydrate>> = None;
    let mut payload = payload;
    let mut masking = false;
    let parsed = serde_json::from_slice::<Value>(&payload)
        .ok()
        .filter(Value::is_object);
    if let Some(mut v) = parsed.filter(|_| st.pii.enabled) {
        let tier = cfg.pii.tier(&st.target_spec(&req_model));
        info = Some(json!({"tier": tier.name(), "masked": 0}));
        if tier.masks() {
            masking = true;
            let ckey = pii::conv_key(headers, Some(&v));
            let masked = match st.pii.ner_find(&st.client, tier, &cfg.pii, &v).await {
                Ok(found) => st.pii.mask_with(&ckey, &mut v, tier, &cfg.pii, &found),
                Err(e) => Err(e),
            };
            match masked {
                Ok((r, h)) => {
                    info = Some(r.to_json(tier));
                    if !h.is_empty() {
                        rh = Some(h);
                    }
                    payload = Bytes::from(v.to_string());
                }
                Err(e) => {
                    st.pii.note_blocked();
                    log("pii_blocked", json!({"target": "upstream", "error": e}));
                    return raw_failed(
                        st,
                        req_model,
                        format!("passthrough:{path}"),
                        path,
                        "pii-blocked",
                    );
                }
            }
        }
    }

    let mut rb = st.client.request(method.clone(), &url);
    for (k, v) in headers {
        let n = k.as_str();
        if !HOP.contains(&n)
            && !n.starts_with("x-luna-")
            && !(masking && pii::CONV_HEADERS.contains(&n))
        {
            rb = rb.header(k, v);
        }
    }
    rb = rb.header(header::ACCEPT_ENCODING, "identity");
    if !payload.is_empty() {
        rb = rb.body(payload);
    }
    let t0 = Instant::now();
    let label = "upstream";
    let kind = format!("passthrough:{path}");
    let resp = match tokio::time::timeout(ttfb, rb.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let what = if e.is_timeout() { "timeout" } else { "connect" };
            return raw_failed(st, req_model, kind, path, what);
        }
        Err(_) => return raw_failed(st, req_model, kind, path, "timeout"),
    };
    let status = resp.status().as_u16();
    let ms = t0.elapsed().as_millis() as u64;
    log(
        "served",
        json!({"kind": kind, "path": path, "target": label, "status": status, "ttfb_ms": ms, "pii": info}),
    );
    let p = Pii {
        rh,
        header: info.as_ref().map(pii_header),
    };
    st.note(Recent {
        time: hms(),
        requested: req_model,
        kind,
        served_by: Some(format!("{label} ({status})")),
        ms,
        attempts: 1,
        skipped: vec![],
        pii: info,
    });
    passthrough(resp, label, 1, idle, ttfb, p).await
}

fn raw_failed(
    st: &Arc<AppState>,
    requested: String,
    kind: String,
    path: &str,
    what: &str,
) -> Response<Body> {
    st.stats.lock().unwrap().errors += 1;
    log(
        "exhausted",
        json!({"kind": kind, "path": path, "error": what}),
    );
    st.note(Recent {
        time: hms(),
        requested,
        kind,
        served_by: None,
        ms: 0,
        attempts: 1,
        skipped: vec![json!({"target": "upstream", "error": what})],
        pii: None,
    });
    let code = if what == "timeout" { 504 } else { 502 };
    let msg = if what == "pii-blocked" {
        "pii masking failed, request not sent".to_owned()
    } else {
        format!("upstream {what} error")
    };
    json_response(code, &json!({"error": {"message": msg}}), &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_only_fields_the_error_names() {
        let body = br#"{"model":"m","reasoning_effort":"low","seed":1,"max_completion_tokens":50}"#;
        let (nb, names) =
            without_rejected_fields(body, b"reasoning_effort is not enabled for this model")
                .unwrap();
        assert_eq!(names, ["reasoning_effort"]);
        let v: Value = serde_json::from_slice(&nb).unwrap();
        assert!(v.get("reasoning_effort").is_none());
        assert_eq!(v["seed"], 1);
        let (nb, _) = without_rejected_fields(
            body,
            br#"{"loc":["body","max_completion_tokens"],"msg":"Extra inputs are not permitted"}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_slice(&nb).unwrap();
        assert_eq!(v["max_tokens"], 50);
        assert!(without_rejected_fields(body, b"bad param for this model").is_none());
        assert!(without_rejected_fields(b"not json", b"reasoning_effort").is_none());
    }
}
