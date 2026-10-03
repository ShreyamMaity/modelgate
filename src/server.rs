//! HTTP surface: inference endpoints, the `/config` UI and its admin API.

use crate::chain;
use crate::config::{self, Config};
use crate::pins::Pins;
use crate::policy::{self, Mode, Policy};
use crate::relay::{forward_raw, json_response, relay, Opts, Plan, Target};
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
            log(
                "admin_refused",
                json!({"reason": "cross-origin", "origin": origin, "host": host}),
            );
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
            log("admin_refused", json!({"reason": "token"}));
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
        ..Default::default()
    }
}

fn resolve(st: &AppState, cfg: &Config, model: &str) -> Option<Vec<String>> {
    let mut models = st.models();
    models.extend(target_models(cfg));
    chain::resolve(model, &cfg.chains, &models).or_else(|| {
        let name = chain::bare(model);
        cfg.target(name).map(|_| vec![name.to_owned()])
    })
}

fn target_models(cfg: &Config) -> Vec<Value> {
    cfg.targets
        .iter()
        .map(|t| {
            let prov = t.label.split_once('/').map(|p| p.0).unwrap_or("direct");
            let id = t.label.split_once('/').map(|p| p.1).unwrap_or(&t.label);
            json!({
                "id": id, "display_name": t.label, "object": "model", "owned_by": prov,
                "supported_endpoints": ["/v1/chat/completions", "/v1/messages"],
                "metadata": {"provider": {"id": prov, "name": "", "description": "", "requires_client_auth": false, "upstream": "direct"}},
                "direct": true, "presence": t.presence,
            })
        })
        .collect()
}

const SESSION_HEADERS: &[&str] = &[
    "x-luna-session",
    "x-conversation-id",
    "x-hermes-session-id",
    "x-session-id",
    "x-claude-code-session-id",
];

pub fn session_of(h: &HeaderMap) -> Option<String> {
    SESSION_HEADERS.iter().find_map(|n| {
        let v = h.get(*n)?.to_str().ok()?.trim();
        if v.is_empty() {
            return None;
        }
        Some(
            v.chars()
                .take(200)
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '@' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect(),
        )
    })
}

fn known_model(st: &AppState, cfg: &Config, spec: &str) -> bool {
    cfg.target(spec).is_some()
        || st.models().iter().any(|m| {
            let id = m.get("id").and_then(Value::as_str).unwrap_or("");
            id == spec
                || chain::provider_of(m).is_some_and(|p| {
                    spec.strip_prefix(p)
                        .and_then(|r| r.strip_prefix('/'))
                        .is_some_and(|r| r == id)
                })
        })
}

/// Orders a group's targets for this request (explicit pin, then session pin, then the list)
/// and builds the policy plan. Models that are not groups get no plan.
#[allow(clippy::result_large_err)]
fn plan_for(
    st: &AppState,
    cfg: &Config,
    model: &str,
    h: &HeaderMap,
    list: Vec<String>,
) -> Result<(Vec<String>, Option<Plan>), Response<Body>> {
    let name = chain::bare(model);
    if cfg.chain(name).is_none() {
        return Ok((list, None));
    }
    let policy = cfg.policy(name);
    let session = session_of(h);
    let explicit = h
        .get("x-luna-pin")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if let Some(e) = &explicit {
        if !list.contains(e) && !known_model(st, cfg, e) {
            return Err(json_response(
                400,
                &json!({"error": {"type": "gateway_bad_pin", "message": format!("X-Luna-Pin {e:?} is not a model of group {name} or a known model")}}),
                &[],
            ));
        }
    }
    let key = if policy.mode == Mode::Failover {
        None
    } else {
        Pins::key(policy.scope, session.as_deref())
    };
    let pin = key
        .as_deref()
        .and_then(|k| st.pins.get(name, k, &list, policy.scope))
        .map(|p| p.model);
    let mut ordered = list.clone();
    if let Some(first) = explicit.clone().or_else(|| pin.clone()) {
        ordered.retain(|x| *x != first);
        ordered.insert(0, first);
    }
    Ok((
        ordered,
        Some(Plan {
            group: name.to_owned(),
            policy,
            session,
            key,
            pin,
            explicit,
            list,
        }),
    ))
}

fn query(uri: &axum::http::Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (pct(k) == key).then(|| pct(v)).filter(|v| !v.is_empty())
    })
}

fn pct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(n) => {
                        out.push(n);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_guard(st: &AppState, h: &HeaderMap) -> Option<Response<Body>> {
    let tok = st.admin_token.as_ref()?;
    let given = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    (!ct_eq(given, tok)).then(|| err(401, "admin token required"))
}

fn group_json(st: &AppState, cfg: &Config, name: &str, list: &[String]) -> Value {
    let expanded = resolve(st, cfg, name).unwrap_or_else(|| list.to_vec());
    json!({"name": name, "models": list, "expanded": expanded, "policy": cfg.policy(name).to_json(),
           "policy_set": cfg.root.get("policies").and_then(|p| p.get(name)).is_some()})
}

fn groups_list(st: &AppState, cfg: &Config) -> Response<Body> {
    let groups: Vec<Value> = cfg
        .chains
        .iter()
        .map(|(n, l)| group_json(st, cfg, n, l))
        .collect();
    json_response(
        200,
        &json!({"version": st.version(), "groups": groups, "defaults": Policy::default().to_json(), "fields": policy::fields()}),
        &[],
    )
}

fn pins_get(st: &AppState, uri: &axum::http::Uri, h: &HeaderMap) -> Response<Body> {
    if let Some(r) = read_guard(st, h) {
        return r;
    }
    let group = query(uri, "group");
    let session = query(uri, "session");
    json_response(
        200,
        &st.pins.json(group.as_deref(), session.as_deref()),
        &[],
    )
}

fn pins_put(st: &AppState, h: &HeaderMap, raw: &Bytes) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let v: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let cfg = st.config();
    let Some(group) = s("group").filter(|g| cfg.chain(g).is_some()) else {
        return field_err("group", "group must name an existing group");
    };
    let policy = cfg.policy(&group);
    let Some(model) = s("model").filter(|m| !m.is_empty()) else {
        return field_err("model", "model is required");
    };
    let list = resolve(st, &cfg, &group).unwrap_or_default();
    if !list.contains(&model) {
        return field_err("model", "model must be one of the group's models");
    }
    let session = s("session");
    let Some(key) = Pins::key(policy.scope, session.as_deref()) else {
        return field_err(
            "session",
            "session is required for conversation-scoped groups",
        );
    };
    st.pins.set(
        &group,
        &key,
        &model,
        policy.scope,
        policy.ttl_minutes,
        "api",
        &list,
    );
    log(
        "pin_set",
        json!({"group": group, "session": key, "model": model}),
    );
    json_response(
        200,
        &json!({"ok": true, "pins": st.pins.json(Some(&group), None)["pins"]}),
        &[],
    )
}

fn pins_delete(st: &AppState, uri: &axum::http::Uri, h: &HeaderMap) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let Some(group) = query(uri, "group") else {
        return field_err("group", "group is required");
    };
    let n = st.pins.clear(&group, query(uri, "session").as_deref());
    json_response(200, &json!({"ok": true, "cleared": n}), &[])
}

fn field_err(field: &str, msg: &str) -> Response<Body> {
    json_response(400, &json!({"error": msg, "field": field}), &[])
}

/// Single-group create/update/delete, applied atomically against the file on disk.
fn group_write(
    st: &AppState,
    h: &HeaderMap,
    method: &Method,
    target: Option<String>,
    raw: &Bytes,
) -> Response<Body> {
    if let Some(r) = admin_guard(st, h) {
        return r;
    }
    let body: Value = if raw.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(raw) {
            Ok(v) => v,
            Err(_) => return field_err("body", "invalid json"),
        }
    };
    let _lock = st.write_lock.lock().unwrap();
    let cfg = st.reload();
    if let Some(v) = body.get("version").filter(|v| !v.is_null()) {
        let v = v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string());
        if v != st.version() {
            return json_response(
                409,
                &json!({"error": "The config was changed somewhere else. Reload to see it.", "version": st.version()}),
                &[],
            );
        }
    }
    let mut groups = cfg.chains.clone();
    let mut pols = cfg.policies();
    let models = match body.get("models") {
        None | Some(Value::Null) => None,
        Some(m) => match m.as_array() {
            Some(a) if !a.is_empty() => Some(a.clone()),
            _ => return field_err("models", "models must be a non-empty list"),
        },
    };
    let policy = match body.get("policy") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(p) => match policy::validate(p) {
            Ok(m) => Some(Some(m)),
            Err((f, m)) => return field_err(&format!("policy.{f}"), &m),
        },
    };
    let (status, name, note) = match (method, target) {
        (&Method::POST, None) => {
            let Some(name) = body.get("name").and_then(Value::as_str).map(str::trim) else {
                return field_err("name", "name is required");
            };
            if !config::valid_group_name(name) {
                return field_err("name", "use letters, digits . _ - (max 40)");
            }
            if groups.iter().any(|(n, _)| n == name) {
                return json_response(
                    409,
                    &json!({"error": format!("group {name} already exists"), "field": "name"}),
                    &[],
                );
            }
            let Some(models) = models else {
                return field_err("models", "models must be a non-empty list");
            };
            groups.push((name.to_owned(), json_strings(&models)));
            if let Some(Some(p)) = policy {
                pols.insert(name.to_owned(), Value::Object(p));
            }
            (201, name.to_owned(), "group-create")
        }
        (&Method::PUT, Some(old)) => {
            let Some(i) = groups.iter().position(|(n, _)| *n == old) else {
                return err(404, "no such group");
            };
            let name = body
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or(&old)
                .to_owned();
            if name != old {
                if !config::valid_group_name(&name) {
                    return field_err("name", "use letters, digits . _ - (max 40)");
                }
                if groups.iter().any(|(n, _)| *n == name) {
                    return json_response(
                        409,
                        &json!({"error": format!("group {name} already exists"), "field": "name"}),
                        &[],
                    );
                }
                for (_, l) in groups.iter_mut() {
                    for e in l.iter_mut() {
                        if *e == old {
                            e.clone_from(&name);
                        }
                    }
                }
                groups[i].0.clone_from(&name);
                if let Some(p) = pols.remove(&old) {
                    pols.insert(name.clone(), p);
                }
                st.pins.clear(&old, None);
            }
            if let Some(m) = models {
                groups[i].1 = json_strings(&m);
            }
            match policy {
                Some(Some(p)) => {
                    pols.insert(name.clone(), Value::Object(p));
                }
                Some(None) => {
                    pols.remove(&name);
                }
                None => {}
            }
            (200, name, "group-update")
        }
        (&Method::DELETE, Some(old)) => {
            if !groups.iter().any(|(n, _)| *n == old) {
                return err(404, "no such group");
            }
            groups.retain(|(n, _)| *n != old);
            pols.remove(&old);
            st.pins.clear(&old, None);
            (200, old, "group-delete")
        }
        _ => return err(405, "method not allowed"),
    };
    let obj: serde_json::Map<String, Value> =
        groups.iter().map(|(n, l)| (n.clone(), json!(l))).collect();
    let groups = match config::validate(&json!({ "chains": obj })) {
        Ok((g, _)) => g,
        Err(e) => {
            log("config_rejected", json!({"error": e, "via": note}));
            return field_err("models", &e);
        }
    };
    if let Err(e) = config::save_with(
        &st.path,
        &groups,
        &serde_json::Map::new(),
        Some(&pols),
        note,
    ) {
        return err(500, &format!("could not save: {e}"));
    }
    log(
        "config_saved",
        json!({"groups": groups.iter().map(|c| c.0.clone()).collect::<Vec<_>>(), "via": note, "group": name}),
    );
    let cfg = st.reload();
    let group = cfg
        .chain(&name)
        .map(|l| group_json(st, &cfg, &name, l))
        .unwrap_or(Value::Null);
    json_response(
        status,
        &json!({"ok": true, "version": st.version(), "group": group}),
        &[],
    )
}

fn json_strings(a: &[Value]) -> Vec<String> {
    a.iter()
        .map(|v| v.as_str().unwrap_or("").to_owned())
        .collect()
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
                "defaults": {"timeout": 90, "ttfb_stream": 20, "stream_idle": 120, "max_tokens_cap": 32768, "keepalive": 10},
                "policies": cfg.policies(),
                "policy_defaults": Policy::default().to_json(),
                "policy_fields": policy::fields(),
                "paid_providers": cfg.paid_providers,
                "auth_required": st.admin_token.is_some(),
            }),
            &[],
        ),
        (&Method::GET, "/_gateway/config/history") => history(&st),
        (&Method::GET, "/_gateway/groups" | "/_gateway/groups/") => groups_list(&st, &cfg),
        (&Method::POST, "/_gateway/groups" | "/_gateway/groups/") => {
            group_write(&st, &parts.headers, &Method::POST, None, &raw)
        }
        (m @ (&Method::PUT | &Method::DELETE), p) if p.starts_with("/_gateway/groups/") => {
            let name = pct(&p["/_gateway/groups/".len()..]);
            group_write(&st, &parts.headers, m, Some(name), &raw)
        }
        (&Method::GET, "/_gateway/pins") => pins_get(&st, &parts.uri, &parts.headers),
        (&Method::PUT | &Method::POST, "/_gateway/pins") => pins_put(&st, &parts.headers, &raw),
        (&Method::DELETE, "/_gateway/pins") => pins_delete(&st, &parts.uri, &parts.headers),
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
            "targets": cfg.targets.iter().map(|t| json!({"name": t.label, "base": t.base, "model": t.model, "presence": t.presence})).collect::<Vec<_>>(),
            "presence": st.presence.status(),
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
    data.extend(target_models(cfg));
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
        Err(e) => {
            log("config_rejected", json!({"error": e}));
            return err(400, &e);
        }
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
    let policies = match body.get("policies").filter(|p| !p.is_null()) {
        Some(p) => match config::validate_policies(p, &chains) {
            Ok(m) => Some(m),
            Err(e) => {
                log("config_rejected", json!({"error": e}));
                return field_err("policies", &e);
            }
        },
        None => None,
    };
    let _lock = st.write_lock.lock().unwrap();
    if let Err(e) = config::save_with(&st.path, &chains, &settings, policies.as_ref(), "save") {
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
    let policies = old
        .get("policies")
        .and_then(|p| config::validate_policies(p, &chains).ok());
    let _lock = st.write_lock.lock().unwrap();
    if let Err(e) = config::save_with(
        &st.path,
        &chains,
        &settings,
        policies.as_ref(),
        "before-restore",
    ) {
        return err(500, &format!("could not restore: {e}"));
    }
    st.reload();
    json_response(200, &json!({"ok": true, "version": st.version()}), &[])
}

async fn test_one(st: Arc<AppState>, spec: String) -> Value {
    let direct = st.config().target(&spec).map(Target::direct);
    let (base, model, bearer) = match &direct {
        Some(t) => {
            let mut base = t.base.clone();
            if let Some(node) = &t.presence {
                match st.presence.eligible(node, t.skip_busy) {
                    Ok(found) if base.is_empty() => base = found.unwrap_or_default(),
                    Ok(_) => {}
                    Err(why) => {
                        return json!({"target": spec, "status": format!("skipped: {why}"), "ms": 0})
                    }
                }
            }
            (
                base,
                t.model.clone().unwrap_or_else(|| spec.clone()),
                t.bearer.clone(),
            )
        }
        None => (st.upstream.clone(), spec.clone(), None),
    };
    let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
    let body = json!({"model": model, "max_tokens": 8, "messages": [{"role": "user", "content": "say ok"}]});
    let mut rb = st.client.post(&url).json(&body);
    if let Some(b) = bearer {
        rb = rb.bearer_auth(b);
    }
    let t0 = Instant::now();
    let status = match tokio::time::timeout(Duration::from_secs(25), rb.send()).await {
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
    let mut models = st.models();
    models.extend(target_models(cfg));
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

fn targets_for(st: &AppState, cfg: &Config, list: &[String]) -> Vec<Target> {
    list.iter()
        .map(|s| match cfg.target(s) {
            Some(d) => Target::direct(d),
            None => upstream_target(st, Some(s)),
        })
        .collect()
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
    match resolve(st, cfg, &model) {
        Some(list) => {
            let (list, plan) = match plan_for(st, cfg, &model, h, list) {
                Ok(x) => x,
                Err(r) => return r,
            };
            relay(
                st,
                targets_for(st, cfg, &list),
                raw,
                Opts {
                    path: path.to_owned(),
                    kind: format!("chain:{model}"),
                    headers: h.clone(),
                    translate: None,
                    plan,
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
                    path: path.to_owned(),
                    kind: "passthrough".into(),
                    headers: h.clone(),
                    translate: None,
                    plan: None,
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
    let chain_list = resolve(st, cfg, &model);
    if chain_list.is_none() && st.native_messages_ids().contains(&model) {
        return relay(
            st,
            vec![upstream_target(st, None)],
            raw,
            Opts {
                path: "/v1/messages".into(),
                kind: "passthrough".into(),
                headers: h.clone(),
                translate: None,
                plan: None,
            },
        )
        .await;
    }
    let body = Bytes::from(shim::to_openai(&a, cfg.settings.max_tokens_cap).to_string());
    let (targets, kind, plan) = match chain_list {
        Some(list) => {
            let (list, plan) = match plan_for(st, cfg, &model, h, list) {
                Ok(x) => x,
                Err(r) => return r,
            };
            (
                targets_for(st, cfg, &list),
                format!("anthropic:chain:{}", chain::bare(&model)),
                plan,
            )
        }
        None => (
            vec![upstream_target(st, None)],
            "anthropic:direct".to_owned(),
            None,
        ),
    };
    relay(
        st,
        targets,
        body,
        Opts {
            path: "/v1/chat/completions".into(),
            kind,
            headers: h.clone(),
            translate: Some(model),
            plan,
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
    let targets = list.iter().map(Target::direct).collect();
    relay(
        st,
        targets,
        raw,
        Opts {
            path: path.to_owned(),
            kind: format!("route:{path}"),
            headers: h.clone(),
            translate: None,
            plan: None,
        },
    )
    .await
}

#[allow(dead_code)]
fn _status_ok(_: StatusCode) {}
