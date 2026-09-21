//! Forwarding a request through an ordered list of targets, failing over on trouble.

use crate::shim;
use crate::state::{hms, log, AppState, Recent};
use axum::body::Body;
use axum::http::{header, HeaderMap, Response, StatusCode};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
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

#[derive(Clone, Debug)]
pub struct Target {
    pub label: String,
    pub base: String,
    /// Rewrites the request's `model` field for this target.
    pub model: Option<String>,
    /// Overrides the request path for this target.
    pub path: Option<String>,
    pub bearer: Option<String>,
}

pub struct Opts<'a> {
    /// Path used for the upstream request (and to pick the cooldown "API shape").
    pub path: &'a str,
    pub kind: String,
    pub headers: &'a HeaderMap,
    /// Set when the client spoke Anthropic `/v1/messages`: `payload` is already the equivalent
    /// chat-completions body and the reply is converted back. Holds the model name to echo.
    pub translate: Option<String>,
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
) -> reqwest::RequestBuilder {
    let mut has_ct = false;
    for (k, v) in h {
        let n = k.as_str();
        if HOP.contains(&n) {
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

fn passthrough(
    resp: reqwest::Response,
    label: &str,
    attempts: usize,
    idle: Duration,
) -> Response<Body> {
    let mut b = Response::builder().status(resp.status());
    for (k, v) in resp.headers() {
        if !HOP.contains(&k.as_str()) {
            b = b.header(k, v);
        }
    }
    b.header("x-gateway-target", label)
        .header("x-gateway-attempts", attempts.to_string())
        .body(Body::from_stream(idle_stream(
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
) -> Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
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
                Ok(Some(Ok(b))) => conv.feed(&b),
                Ok(None) => {
                    for e in conv.finish() {
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
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-gateway-target", label)
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
) -> Response<Body> {
    let extra = [
        ("x-gateway-target", label.to_owned()),
        ("x-gateway-attempts", attempts.to_string()),
    ];
    match tokio::time::timeout(wait, resp.bytes()).await {
        Ok(Ok(b)) => match serde_json::from_slice::<Value>(&b) {
            Ok(v) => json_response(200, &shim::from_openai(&v, model), &extra),
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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Try `targets` in order (healthy ones first) until one answers.
pub async fn relay(
    st: &Arc<AppState>,
    targets: Vec<Target>,
    payload: Bytes,
    o: Opts<'_>,
) -> Response<Body> {
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
    // A streaming reply starts fast or never; a buffered one may legitimately take a while.
    let ttfb = Duration::from_secs_f64(if streaming {
        cfg.settings.ttfb_stream
    } else {
        cfg.settings.timeout
    });
    let idle = Duration::from_secs_f64(cfg.settings.stream_idle);
    let family = if o.path == "/v1/messages" {
        "anthropic"
    } else {
        "openai"
    }; // cooldowns are per API shape

    let keyed: Vec<(String, Target)> = targets
        .into_iter()
        .map(|t| (format!("{family}:{}", t.label), t))
        .collect();
    let (mut ordered, cooling): (Vec<_>, Vec<_>) =
        keyed.into_iter().partition(|(k, _)| !st.in_cooldown(k));
    ordered.extend(cooling); // cooled-down targets stay as a last resort

    let mut attempts: Vec<Value> = Vec::new();
    let mut last: Option<(u16, Option<header::HeaderValue>, Bytes)> = None;
    for (key, t) in ordered {
        let body = match (&t.model, &parsed) {
            (Some(m), Some(v)) => {
                let mut v = v.clone();
                v["model"] = json!(m);
                Bytes::from(v.to_string())
            }
            _ => payload.clone(),
        };
        let url = format!(
            "{}{}",
            t.base.trim_end_matches('/'),
            t.path.as_deref().unwrap_or(o.path)
        );
        let rb = apply_headers(
            st.client.post(&url).body(body),
            o.headers,
            &t,
            o.translate.is_some(),
        );
        let t0 = Instant::now();
        let resp = match tokio::time::timeout(ttfb, rb.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                st.mark_bad(&key, "connect", None);
                attempts.push(json!({"target": t.label, "error": if e.is_timeout() { "timeout" } else { "connect" }}));
                continue;
            }
            Err(_) => {
                st.mark_bad(&key, "timeout", None);
                attempts.push(json!({"target": t.label, "error": "timeout"}));
                continue;
            }
        };
        let status = resp.status().as_u16();
        if HARD.contains(&status) || SOFT.contains(&status) {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| n.min(300));
            let ct = resp.headers().get(header::CONTENT_TYPE).cloned();
            let data = resp.bytes().await.unwrap_or_default();
            let data = data.slice(..data.len().min(65_536));
            // "model X is available via openai_chat, not anthropic_messages": wrong API shape for this
            // target, not a sick provider - skip it without a cooldown.
            let wrong_shape = status == 404 && contains(&data, b"is available via");
            if HARD.contains(&status) && !wrong_shape {
                st.mark_bad(&key, &status.to_string(), retry_after);
            }
            attempts.push(json!({"target": t.label, "status": status}));
            last = Some((status, ct, data));
            continue;
        }
        // Success.
        st.mark_good(&key, &t.label);
        let ms = t0.elapsed().as_millis() as u64;
        if !attempts.is_empty() {
            st.stats.lock().unwrap().failovers += 1;
        }
        log(
            "served",
            json!({"kind": o.kind, "path": o.path, "target": t.label, "status": status, "ttfb_ms": ms, "skipped": attempts}),
        );
        let n = attempts.len() + 1;
        st.note(Recent {
            time: hms(),
            requested: req_model,
            kind: o.kind,
            served_by: Some(t.label.clone()),
            ms,
            attempts: n,
            skipped: attempts,
        });
        return match o.translate {
            None => passthrough(resp, &t.label, n, idle),
            Some(model) if streaming => translated_stream(resp, model, &t.label, n, idle),
            Some(model) => translated_buffered(resp, &model, &t.label, n, ttfb).await,
        };
    }

    st.stats.lock().unwrap().errors += 1;
    log(
        "exhausted",
        json!({"kind": o.kind, "path": o.path, "attempts": attempts}),
    );
    st.note(Recent {
        time: hms(),
        requested: req_model,
        kind: o.kind,
        served_by: None,
        ms: 0,
        attempts: attempts.len(),
        skipped: attempts.clone(),
    });
    if o.translate.is_some() {
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
        return json_response(
            code,
            &shim::error_body(code, &msg),
            &[("x-gateway-exhausted", "1".into())],
        );
    }
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
            &json!({"error": {"message": "all targets failed", "attempts": attempts}}),
            &[],
        ),
    }
}
