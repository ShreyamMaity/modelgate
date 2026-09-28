//! HTTP surface: inference endpoints, the `/config` UI and its admin API.

use crate::chain;
use crate::config::{self, Config};
use crate::relay::{forward_raw, json_response, relay, Opts, Target};
use crate::shim;
use crate::state::{log, AppState};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, Response, StatusCode};
use axum::Router;
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONFIG_HTML: &str = include_str!("../assets/config.html");
const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const TEXT_PATHS: &[&str] = &["/v1/chat/completions", "/v1/completions", "/v1/responses"];
const MAX_BODY: usize = 64 * 1024 * 1024;

pub fn router(st: Arc<AppState>) -> Router {
    Router::new().fallback(handle).with_state(st)
}

fn html(body: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

fn err(status: u16, msg: &str) -> Response<Body> {
    json_response(status, &json!({"error": msg}), &[])
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Writes need a same-origin request and, when configured, the admin token.
/// Returns the refusal to send, or `None` if the request may proceed.
fn admin_guard(st: &AppState, h: &HeaderMap) -> Option<Response<Body>> {
    if let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = h
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin.split_once("://").map(|x| x.1) != Some(host) {
            return Some(err(403, "cross-origin request refused"));
        }
    }
    if let Some(tok) = &st.admin_token {
        let given = h
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !ct_eq(given, tok) {
            return Some(err(401, "admin token required"));
        }
    }
    None
}

fn upstream_target(st: &AppState, spec: Option<&str>) -> Target {
    Target {
        label: spec.unwrap_or("upstream").to_owned(),
        base: st.upstream.clone(),
        model: spec.map(str::to_owned),
        path: None,
        bearer: None,
    }
}

async fn handle(State(st): State<Arc<AppState>>, req: Request) -> Response<Body> {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_owned();
    let raw = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return err(413, "request body too large"),
    };
    let cfg = st.config();
    match (&parts.method, path.as_str()) {
        (&Method::GET, "/health") => json_response(200, &json!({"ok": true}), &[]),
        (&Method::GET, "/config" | "/config/") => html(CONFIG_HTML),
        (&Method::GET, "/_gateway" | "/_gateway/") => html(DASHBOARD_HTML),
        (&Method::GET, "/_gateway/recent") => json_response(200, &st.recent_json(), &[]),
        (&Method::GET, "/_gateway/status") => status(&st, &cfg),
        (&Method::GET, "/_gateway/pii") => json_response(
            200,
            &json!({"status": st.pii.status(), "policy": cfg.pii.summary(), "kinds": crate::pii::detect::KINDS}),
            &[],
        ),
        (&Method::GET, "/_gateway/config") => json_response(
            200,
            &json!({
                "chains": cfg.root.get("chains").cloned().unwrap_or_else(|| json!({})),
                "version": st.version(),
                "settings": config::SETTING_BOUNDS.iter().filter_map(|(k, _, _)| cfg.root.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect::<serde_json::Map<_, _>>(),
                "defaults": {"timeout": 90, "ttfb_stream": 20, "stream_idle": 120, "max_tokens_cap": 32768},
                "paid_providers": cfg.paid_providers,
                "auth_required": st.admin_token.is_some(),
            }),
            &[],
        ),
        (&Method::GET, "/_gateway/config/history") => history(&st),
        (&Method::PUT, "/_gateway/config") => save_config(&st, &parts.headers, &raw),
        (&Method::POST, "/_gateway/config/restore") => restore(&st, &parts.headers, &raw),
        (&Method::POST, "/_gateway/test") => test(&st, &cfg, &parts.headers, &raw).await,
        (&Method::GET, "/v1/models" | "/models") => {
            json_response(200, &models_list(&st, &cfg), &[])
        }
        (&Method::POST, "/v1/messages/count_tokens") => match serde_json::from_slice::<Value>(&raw)
        {
            Ok(v) => json_response(
                200,
                &json!({"input_tokens": shim::estimate_tokens(&v)}),
                &[],
            ),
            Err(_) => json_response(400, &shim::error_body(400, "invalid json"), &[]),
        },
        (&Method::POST, "/v1/messages") => messages(&st, &cfg, &parts.headers, raw).await,
        (&Method::POST, p) if TEXT_PATHS.contains(&p) => {
            text(&st, &cfg, &parts.headers, p, raw).await
        }
        (&Method::POST, p) if cfg.routes.iter().any(|(rp, _)| rp == p) => {
            route(&st, &cfg, &parts.headers, p, raw).await
        }
        (m, p) if st.passthrough && p.starts_with("/v1/") => {
            let pq = parts
                .uri
                .path_and_query()
                .map(|x| x.as_str().to_owned())
                .unwrap_or_else(|| path.clone());
            forward_raw(&st, m, &pq, &parts.headers, raw).await
        }
        _ => json_response(
            404,
            &json!({"error": {"message": format!("unknown route {path}")}}),
            &[],
        ),
    }
}

fn status(st: &AppState, cfg: &Config) -> Response<Body> {
    let s = st.stats.lock().unwrap();
    json_response(
        200,
        &json!({
            "uptime_s": st.started.elapsed().as_secs(), "requests": s.requests, "failovers": s.failovers, "errors": s.errors,
            "served": s.served, "cooling_down": st.cooling(), "chains": cfg.root.get("chains"),
            "routes": cfg.routes.iter().map(|r| r.0.clone()).collect::<Vec<_>>(), "upstream": st.upstream,
            "pii": st.pii.status(),
        }),
        &[],
    )
}

fn models_list(st: &AppState, cfg: &Config) -> Value {
    let mut data: Vec<Value> = cfg
        .chains
        .iter()
        .map(|(name, list)| {
            // Same shape as the upstream's own entries: aperture-cli rejects models without
            // metadata.provider.id, and the provider block must be identical across our models.
            json!({
                "id": name, "display_name": name, "pricing_display_name": name, "object": "model", "owned_by": "modelgate",
                "supported_endpoints": ["/v1/chat/completions", "/v1/messages"],
                "context_window_tokens": 131072, "max_output_tokens": 32768,
                "metadata": {"provider": {"id": "modelgate", "name": "", "description": "", "requires_client_auth": false, "upstream": "default"}},
                "chain": list,
            })
        })
        .collect();
    for mut m in st.models() {
        // The gateway can serve any chat model to Anthropic-style clients.
        if let Some(eps) = m
            .get_mut("supported_endpoints")
            .and_then(Value::as_array_mut)
        {
            if !eps.iter().any(|e| e == "/v1/messages") {
                eps.push(json!("/v1/messages"));
            }
        }
        data.push(m);
    }
    json!({"object": "list", "data": data})
}

fn history(st: &AppState) -> Response<Body> {
    let dir = config::history_dir(&st.path);
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|f| f.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| b.cmp(a));
    let out: Vec<Value> = files
        .iter()
        .filter_map(|name| {
            let v: Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join(name)).ok()?).ok()?;
            let groups = v
                .get("chains")
                .and_then(Value::as_object)
                .map(|o| o.len())
                .unwrap_or(0);
            // chains-YYYYMMDD-HHMMSS-note.json
            let rest = name.strip_prefix("chains-")?.strip_suffix(".json")?;
            let (d, rest) = rest.split_once('-')?;
            let (t, note) = rest.split_once('-').unwrap_or((rest, ""));
            let when = format!(
                "{}-{}-{} {}:{}:{} UTC",
                d.get(..4)?,
                d.get(4..6)?,
                d.get(6..8)?,
                t.get(..2)?,
                t.get(2..4)?,
                t.get(4..6)?
            );
            Some(json!({"file": name, "groups": groups, "note": note, "when": when}))
        })
        .collect();
    json_response(200, &json!(out), &[])
}

fn save_config(st: &AppState, h: &HeaderMap, raw: &Bytes) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let Ok(body) = serde_json::from_slice::<Value>(raw) else {
        return err(400, "invalid json");
    };
    let (chains, settings) = match config::validate(&body) {
        Ok(x) => x,
        Err(e) => return err(400, &e),
    };
    if body.get("version").map(|v| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    }) != Some(st.version())
    {
        return json_response(
            409,
            &json!({"error": "The config was changed somewhere else. Reload to see it.", "version": st.version()}),
            &[],
        );
    }
    if let Err(e) = config::save(&st.path, &chains, &settings, "save") {
        return err(500, &format!("could not save: {e}"));
    }
    log(
        "config_saved",
        json!({"groups": chains.iter().map(|c| c.0.clone()).collect::<Vec<_>>()}),
    );
    st.reload();
    json_response(200, &json!({"ok": true, "version": st.version()}), &[])
}

fn restore(st: &AppState, h: &HeaderMap, raw: &Bytes) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let name = serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.get("file").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let safe = name.starts_with("chains-")
        && name.ends_with(".json")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let file = config::history_dir(&st.path).join(&name);
    let old: Option<Value> = if safe {
        std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
    } else {
        None
    };
    let Some(old) = old else {
        return err(404, "no such history entry");
    };
    let settings: serde_json::Map<String, Value> = config::SETTING_BOUNDS
        .iter()
        .filter_map(|(k, _, _)| old.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    let (chains, settings) =
        match config::validate(&json!({"chains": old.get("chains"), "settings": settings})) {
            Ok(x) => x,
            Err(e) => return err(400, &e),
        };
    if let Err(e) = config::save(&st.path, &chains, &settings, "before-restore") {
        return err(500, &format!("could not restore: {e}"));
    }
    st.reload();
    json_response(200, &json!({"ok": true, "version": st.version()}), &[])
}

async fn test_one(st: Arc<AppState>, spec: String) -> Value {
    let url = format!("{}/v1/chat/completions", st.upstream.trim_end_matches('/'));
    let body = json!({"model": spec, "max_tokens": 8, "messages": [{"role": "user", "content": "say ok"}]});
    let t0 = Instant::now();
    let status = match tokio::time::timeout(
        Duration::from_secs(25),
        st.client.post(&url).json(&body).send(),
    )
    .await
    {
        Ok(Ok(r)) => {
            let s = r.status().as_u16();
            let _ = r.bytes().await;
            json!(s)
        }
        Ok(Err(_)) => json!("ConnectError"),
        Err(_) => json!("Timeout"),
    };
    json!({"target": spec, "status": status, "ms": t0.elapsed().as_millis() as u64})
}

async fn test(st: &Arc<AppState>, cfg: &Config, h: &HeaderMap, raw: &Bytes) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let entries: Option<Vec<String>> = serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("entries").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .filter(|e: &Vec<String>| e.len() <= 60);
    let Some(entries) = entries else {
        return err(400, "entries must be a list of up to 60 strings");
    };
    let models = st.models();
    let mut specs: Vec<String> = Vec::new();
    for e in &entries {
        for t in chain::expand(e, &cfg.chains, &models, 0) {
            if !specs.contains(&t) {
                specs.push(t);
            }
        }
    }
    let truncated = specs.len() > 40;
    specs.truncate(40);
    let mut results = Vec::new();
    for batch in specs.chunks(8) {
        results.extend(
            futures_util::future::join_all(batch.iter().map(|s| test_one(st.clone(), s.clone())))
                .await,
        );
    }
    json_response(
        200,
        &json!({"results": results, "truncated": truncated}),
        &[],
    )
}

fn model_of(raw: &[u8]) -> String {
    serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.get("model").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}

fn targets_for(st: &AppState, list: &[String]) -> Vec<Target> {
    list.iter().map(|s| upstream_target(st, Some(s))).collect()
}

async fn text(
    st: &Arc<AppState>,
    cfg: &Config,
    h: &HeaderMap,
    path: &str,
    raw: Bytes,
) -> Response<Body> {
    st.stats.lock().unwrap().requests += 1;
    let model = model_of(&raw);
    match chain::resolve(&model, &cfg.chains, &st.models()) {
        Some(list) => {
            relay(
                st,
                targets_for(st, &list),
                raw,
                Opts {
                    path,
                    kind: format!("chain:{model}"),
                    headers: h,
                    translate: None,
                },
            )
            .await
        }
        // Not a virtual model: transparent passthrough to the upstream.
        None => {
            relay(
                st,
                vec![upstream_target(st, None)],
                raw,
                Opts {
                    path,
                    kind: "passthrough".into(),
                    headers: h,
                    translate: None,
                },
            )
            .await
        }
    }
}

/// Anthropic clients (Claude Code). Models the upstream serves natively over `/v1/messages` pass
/// straight through; everything else is translated to chat-completions and back.
async fn messages(st: &Arc<AppState>, cfg: &Config, h: &HeaderMap, raw: Bytes) -> Response<Body> {
    st.stats.lock().unwrap().requests += 1;
    let Ok(a) = serde_json::from_slice::<Value>(&raw) else {
        return json_response(400, &shim::error_body(400, "invalid json"), &[]);
    };
    let model = a
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let chain_list = chain::resolve(&model, &cfg.chains, &st.models());
    if chain_list.is_none() && st.native_messages_ids().contains(&model) {
        return relay(
            st,
            vec![upstream_target(st, None)],
            raw,
            Opts {
                path: "/v1/messages",
                kind: "passthrough".into(),
                headers: h,
                translate: None,
            },
        )
        .await;
    }
    let body = Bytes::from(shim::to_openai(&a, cfg.settings.max_tokens_cap).to_string());
    let (targets, kind) = match &chain_list {
        Some(list) => (
            targets_for(st, list),
            format!("anthropic:chain:{}", chain::bare(&model)),
        ),
        None => (
            vec![upstream_target(st, None)],
            "anthropic:direct".to_owned(),
        ),
    };
    relay(
        st,
        targets,
        body,
        Opts {
            path: "/v1/chat/completions",
            kind,
            headers: h,
            translate: Some(model),
        },
    )
    .await
}

/// Non-text endpoints (embeddings, images, audio) sent straight to configured direct upstreams.
async fn route(
    st: &Arc<AppState>,
    cfg: &Config,
    h: &HeaderMap,
    path: &str,
    raw: Bytes,
) -> Response<Body> {
    st.stats.lock().unwrap().requests += 1;
    let Some((_, list)) = cfg.routes.iter().find(|(p, _)| p == path) else {
        return err(404, "no such route");
    };
    let targets = list
        .iter()
        .map(|d| Target {
            label: d.label.clone(),
            base: d.base.clone(),
            model: d.model.clone(),
            path: d.path.clone(),
            bearer: d
                .key_env
                .as_ref()
                .and_then(|k| std::env::var(k).ok())
                .filter(|k| !k.is_empty()),
        })
        .collect();
    relay(
        st,
        targets,
        raw,
        Opts {
            path,
            kind: format!("route:{path}"),
            headers: h,
            translate: None,
        },
    )
    .await
}

#[allow(dead_code)]
fn _status_ok(_: StatusCode) {}
