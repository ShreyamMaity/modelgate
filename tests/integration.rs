//! End-to-end tests: a mock upstream + the real gateway, talking over real sockets.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::Response;
use modelgate::state::AppState;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- mock upstream
fn jr(status: u16, v: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}

fn sse(chunks: Vec<Value>) -> Response<Body> {
    let mut s: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    s.push_str("data: [DONE]\n\n");
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(s))
        .unwrap()
}

async fn chat(model: &str, stream: bool) -> Response<Body> {
    match model {
        "dead/x" => return jr(402, json!({"error": "payment required"})),
        "flaky/x" => return jr(503, json!({"error": "unavailable"})),
        "wrong/x" => {
            return jr(
                404,
                json!({"error": {"message": "model is available via openai_chat, not anthropic_messages"}}),
            )
        }
        "picky/x" => return jr(400, json!({"error": "bad param for this model"})),
        "slow/x" => tokio::time::sleep(Duration::from_secs(6)).await,
        _ => {}
    }
    let usage = json!({"prompt_tokens": 7, "completion_tokens": 3});
    if model == "tool/x" {
        return if stream {
            sse(vec![
                json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "Read", "arguments": "{\"pa"}}]}}]}),
                json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"a\"}"}}]}}]}),
                json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
                json!({"choices": [], "usage": usage}),
            ])
        } else {
            jr(
                200,
                json!({"choices": [{"message": {"content": null, "tool_calls": [{"id": "call_1", "function": {"name": "Read", "arguments": "{\"path\":\"a\"}"}}]}, "finish_reason": "tool_calls"}], "usage": usage}),
            )
        };
    }
    let text = format!("pong from {model}");
    if stream {
        let (a, b) = text.split_at(5);
        sse(vec![
            json!({"choices": [{"delta": {"content": a}}]}),
            json!({"choices": [{"delta": {"content": b}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
            json!({"choices": [], "usage": usage}),
        ])
    } else {
        jr(
            200,
            json!({"choices": [{"message": {"content": text}, "finish_reason": "stop"}], "usage": usage}),
        )
    }
}

async fn mock(req: Request) -> Response<Body> {
    let (parts, body) = req.into_parts();
    let raw = to_bytes(body, 1 << 22).await.unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let model = v["model"].as_str().unwrap_or("").to_owned();
    let hdr = |n: &str| {
        parts
            .headers
            .get(n)
            .and_then(|x| x.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    match (parts.method.as_str(), parts.uri.path()) {
        ("GET", "/v1/models") => jr(
            200,
            json!({"object": "list", "data": [
            {"id": "m1", "supported_endpoints": ["/v1/chat/completions"], "metadata": {"provider": {"id": "p1"}}},
            {"id": "m2", "supported_endpoints": ["/v1/chat/completions"], "metadata": {"provider": {"id": "p1"}}},
            {"id": "nat", "supported_endpoints": ["/v1/messages", "/v1/chat/completions"], "metadata": {"provider": {"id": "hosted"}}}]}),
        ),
        ("POST", "/v1/messages") => jr(200, json!({"native": true, "model": model})),
        ("POST", "/v1/embeddings") => jr(
            200,
            json!({"auth": hdr("authorization"), "x_api_key": hdr("x-api-key"), "model": model}),
        ),
        ("POST", "/v1/chat/completions") => chat(&model, v["stream"] == true).await,
        (m, "/v1/files") => jr(
            200,
            json!({"method": m, "query": parts.uri.query().unwrap_or(""), "custom": hdr("x-custom"),
                   "auth": hdr("authorization"), "conn": hdr("connection")}),
        ),
        ("POST", "/v1/audio/transcriptions") => {
            let sum: u64 = raw.iter().map(|b| u64::from(*b)).sum();
            jr(
                200,
                json!({"len": raw.len(), "sum": sum, "ct": hdr("content-type")}),
            )
        }
        ("GET", "/v1/blob") => Response::builder()
            .header("content-type", "application/octet-stream")
            .header("x-upstream", "yes")
            .body(Body::from((0u8..=255).collect::<Vec<u8>>()))
            .unwrap(),
        ("POST", "/v1/slow-sse") => {
            let s = futures_util::stream::unfold(0u8, |i| async move {
                if i >= 2 {
                    return None;
                }
                if i == 1 {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                Some((
                    Ok::<_, std::io::Error>(bytes::Bytes::from(format!(
                        "data: {i}

"
                    ))),
                    i + 1,
                ))
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(s))
                .unwrap()
        }
        ("POST", "/v1/limited") => jr(429, json!({"error": {"message": "slow down"}})),
        _ => jr(404, json!({"error": "not found"})),
    }
}

// ---------------------------------------------------------------- harness
struct H {
    base: String,
    mock: String,
    dir: PathBuf,
    http: reqwest::Client,
}

impl Drop for H {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

async fn serve(router: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    format!("http://{addr}")
}

async fn start(mut config: Value, token: Option<&str>) -> H {
    let mock_url = serve(axum::Router::new().fallback(mock)).await;
    let dir = std::env::temp_dir().join(format!("modelgate-it-{}", fastrand::u64(..)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chains.json");
    config["ttfb_stream"] = config.get("ttfb_stream").cloned().unwrap_or(json!(20));
    std::fs::write(&path, config.to_string()).unwrap();
    let st: Arc<AppState> = AppState::new(path, mock_url.clone(), token.map(str::to_owned));
    st.refresh_models().await;
    let base = serve(modelgate::server::router(st)).await;
    H {
        base,
        mock: mock_url,
        dir,
        http: reqwest::Client::new(),
    }
}

impl H {
    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    async fn chat(&self, model: &str) -> reqwest::Response {
        self.post(
            "/v1/chat/completions",
            json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await
    }
}

fn hdr(r: &reqwest::Response, name: &str) -> String {
    r.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

// ---------------------------------------------------------------- failover
#[tokio::test]
async fn fails_over_and_remembers() {
    let h = start(
        json!({"chains": {"c": ["dead/x", "flaky/x", "good/x"]}}),
        None,
    )
    .await;
    let r = h.chat("c").await;
    assert_eq!(r.status(), 200);
    assert_eq!(hdr(&r, "x-gateway-target"), "good/x");
    assert_eq!(
        hdr(&r, "x-gateway-attempts"),
        "3",
        "dead and flaky were tried first"
    );
    assert_eq!(
        r.json::<Value>().await.unwrap()["choices"][0]["message"]["content"],
        "pong from good/x"
    );
    let again = h.chat("c").await;
    assert_eq!(
        hdr(&again, "x-gateway-attempts"),
        "1",
        "failed targets are cooling down and skipped"
    );
    let status: Value = h
        .http
        .get(format!("{}/_gateway/status", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(status["cooling_down"]
        .as_object()
        .unwrap()
        .keys()
        .any(|k| k.ends_with("dead/x")));
    assert_eq!(status["failovers"], 1);
}

#[tokio::test]
async fn exhausted_returns_last_upstream_error() {
    let h = start(json!({"chains": {"c": ["dead/x", "flaky/x"]}}), None).await;
    let r = h.chat("c").await;
    assert_eq!(r.status(), 503, "the last error is surfaced verbatim");
    assert_eq!(hdr(&r, "x-gateway-exhausted"), "1");
}

#[tokio::test]
async fn soft_failure_moves_on_without_cooldown() {
    let h = start(json!({"chains": {"c": ["picky/x", "good/x"]}}), None).await;
    assert_eq!(hdr(&h.chat("c").await, "x-gateway-attempts"), "2");
    assert_eq!(
        hdr(&h.chat("c").await, "x-gateway-attempts"),
        "2",
        "a 400 is this request's fault, so picky/x is retried each time"
    );
}

#[tokio::test]
async fn plain_model_passes_through_unchanged() {
    let h = start(json!({"chains": {}}), None).await;
    let r = h.chat("anything/goes").await;
    assert_eq!(hdr(&r, "x-gateway-target"), "upstream");
    assert_eq!(
        r.json::<Value>().await.unwrap()["choices"][0]["message"]["content"],
        "pong from anything/goes"
    );
}

#[tokio::test]
async fn adhoc_chain_and_wildcard() {
    let h = start(json!({"chains": {}}), None).await;
    assert_eq!(
        hdr(&h.chat("dead/x|good/x").await, "x-gateway-target"),
        "good/x"
    );
    assert_eq!(
        hdr(&h.chat("p1/*").await, "x-gateway-target"),
        "p1/m1",
        "wildcard expands from the upstream model list"
    );
    assert_eq!(
        hdr(&h.chat("modelgate/dead/x|good/x").await, "x-gateway-target"),
        "good/x",
        "the optional modelgate/ prefix is stripped"
    );
}

#[tokio::test]
async fn slow_stream_start_fails_over() {
    let h = start(
        json!({"ttfb_stream": 1, "chains": {"c": ["slow/x", "good/x"]}}),
        None,
    )
    .await;
    let t = Instant::now();
    let r = h
        .post(
            "/v1/chat/completions",
            json!({"model": "c", "stream": true, "messages": []}),
        )
        .await;
    assert_eq!(hdr(&r, "x-gateway-target"), "good/x");
    assert!(
        t.elapsed() < Duration::from_secs(4),
        "gave up on the slow target after ~1s, not 6s"
    );
    assert!(r.text().await.unwrap().contains("data: [DONE]"));
}

// ---------------------------------------------------------------- anthropic translation
#[tokio::test]
async fn anthropic_non_streaming() {
    let h = start(json!({"chains": {"c": ["dead/x", "good/x"]}}), None).await;
    let r = h.post("/v1/messages", json!({"model": "c", "max_tokens": 50, "system": "be brief", "messages": [{"role": "user", "content": "hi"}]})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(hdr(&r, "x-gateway-target"), "good/x");
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["type"], "message");
    assert_eq!(v["model"], "c");
    assert_eq!(v["content"][0]["text"], "pong from good/x");
    assert_eq!(v["stop_reason"], "end_turn");
    assert_eq!(v["usage"], json!({"input_tokens": 7, "output_tokens": 3}));
}

#[tokio::test]
async fn anthropic_streaming_text() {
    let h = start(json!({"chains": {"c": ["good/x"]}}), None).await;
    let r = h.post("/v1/messages", json!({"model": "c", "max_tokens": 50, "stream": true, "messages": [{"role": "user", "content": "hi"}]})).await;
    assert_eq!(hdr(&r, "content-type"), "text/event-stream");
    let t = r.text().await.unwrap();
    for needle in [
        "event: message_start",
        "\"text\":\"pong \"",
        "\"text\":\"from good/x\"",
        "event: message_delta",
        "\"stop_reason\":\"end_turn\"",
        "event: message_stop",
    ] {
        assert!(t.contains(needle), "missing {needle} in {t}");
    }
}

#[tokio::test]
async fn anthropic_tool_calls_both_modes() {
    let h = start(json!({"chains": {"t": ["tool/x"]}}), None).await;
    let body = |stream: bool| {
        json!({"model": "t", "max_tokens": 50, "stream": stream, "messages": [{"role": "user", "content": "read a"}],
        "tools": [{"name": "Read", "description": "read", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}]})
    };
    let v: Value = h
        .post("/v1/messages", body(false))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["content"][0],
        json!({"type": "tool_use", "id": "call_1", "name": "Read", "input": {"path": "a"}})
    );
    assert_eq!(v["stop_reason"], "tool_use");
    let t = h
        .post("/v1/messages", body(true))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        t.contains("\"type\":\"tool_use\"")
            && t.contains("input_json_delta")
            && t.contains("\"stop_reason\":\"tool_use\""),
        "{t}"
    );
}

#[tokio::test]
async fn anthropic_native_models_pass_through_and_errors_are_anthropic_shaped() {
    let h = start(json!({"chains": {"bad": ["dead/x"]}}), None).await;
    let v: Value = h
        .post(
            "/v1/messages",
            json!({"model": "nat", "max_tokens": 5, "messages": []}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["native"], true,
        "a model the upstream serves natively over /v1/messages is not translated"
    );
    let r = h
        .post(
            "/v1/messages",
            json!({"model": "bad", "max_tokens": 5, "messages": []}),
        )
        .await;
    assert_eq!(r.status(), 402);
    let e: Value = r.json().await.unwrap();
    assert_eq!(e["type"], "error");
    assert!(e["error"]["message"].as_str().unwrap().contains("payment"));
}

#[tokio::test]
async fn anthropic_wrong_shape_404_does_not_cool_down() {
    let h = start(json!({"chains": {"c": ["wrong/x", "good/x"]}}), None).await;
    let body =
        json!({"model": "c", "max_tokens": 5, "messages": [{"role": "user", "content": "hi"}]});
    assert_eq!(
        hdr(
            &h.post("/v1/messages", body.clone()).await,
            "x-gateway-attempts"
        ),
        "2"
    );
    let status: Value = h
        .http
        .get(format!("{}/_gateway/status", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        status["cooling_down"].as_object().unwrap().is_empty(),
        "wrong API shape is not a sick provider"
    );
}

#[tokio::test]
async fn count_tokens_estimates() {
    let h = start(json!({"chains": {}}), None).await;
    let v: Value = h
        .post(
            "/v1/messages/count_tokens",
            json!({"model": "x", "messages": [{"role": "user", "content": "hello there"}]}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(v["input_tokens"].as_u64().unwrap() > 3);
}

// ---------------------------------------------------------------- routes (non-text)
#[tokio::test]
async fn routes_use_own_key_and_hide_client_credentials() {
    std::env::set_var("MODELGATE_TEST_KEY", "sk-route-secret");
    let mock = serve(axum::Router::new().fallback(mock)).await;
    let h = start(
        json!({"chains": {}, "routes": {"/v1/embeddings": {"targets": [
        {"base": "http://127.0.0.1:1", "label": "down"},
        {"base": mock, "model": "emb-1", "key_env": "MODELGATE_TEST_KEY"}]}}}),
        None,
    )
    .await;
    let r = h
        .http
        .post(format!("{}/v1/embeddings", h.base))
        .header("authorization", "Bearer client-secret")
        .header("x-api-key", "client-key")
        .json(&json!({"model": "ignored", "input": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        v["auth"], "Bearer sk-route-secret",
        "the route's own key replaces the client's"
    );
    assert_eq!(v["x_api_key"], "");
    assert_eq!(v["model"], "emb-1");
}

// ---------------------------------------------------------------- catalogue + pages
#[tokio::test]
async fn models_list_and_pages() {
    let h = start(json!({"chains": {"smart": ["good/x"]}}), None).await;
    let v: Value = h
        .http
        .get(format!("{}/v1/models", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let data = v["data"].as_array().unwrap();
    let smart = data.iter().find(|m| m["id"] == "smart").unwrap();
    assert_eq!(smart["metadata"]["provider"]["id"], "modelgate");
    assert!(data
        .iter()
        .filter(|m| m["id"] != "smart")
        .all(|m| m["supported_endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "/v1/messages")));
    for page in ["/config", "/_gateway/", "/health"] {
        assert_eq!(
            h.http
                .get(format!("{}{page}", h.base))
                .send()
                .await
                .unwrap()
                .status(),
            200,
            "{page}"
        );
    }
    assert_eq!(
        h.http
            .get(format!("{}/nope", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

// ---------------------------------------------------------------- admin API
#[tokio::test]
async fn config_save_reload_history_restore() {
    let h = start(
        json!({"chains": {"a": ["dead/x", "good/x"]}, "routes": {}}),
        None,
    )
    .await;
    let get = || async {
        h.http
            .get(format!("{}/_gateway/config", h.base))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let cfg = get().await;
    let put = |body: Value| {
        h.http
            .put(format!("{}/_gateway/config", h.base))
            .json(&body)
            .send()
    };

    assert_eq!(
        put(json!({"chains": {"bad name!": []}, "version": cfg["version"]}))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        put(json!({"chains": {}, "settings": {"timeout": 99999}, "version": cfg["version"]}))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        put(json!({"chains": {"a": []}, "version": "1"}))
            .await
            .unwrap()
            .status(),
        409,
        "stale version"
    );

    let ok = put(json!({"chains": {"zz": ["good/x"], "a": ["dead/x", "good/x"]}, "settings": {"timeout": 33}, "version": cfg["version"]})).await.unwrap();
    assert_eq!(ok.status(), 200);
    let new = get().await;
    assert_eq!(
        new["chains"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["zz", "a"],
        "order is kept"
    );
    assert_eq!(new["settings"]["timeout"], 33);
    assert_eq!(
        hdr(&h.chat("zz").await, "x-gateway-target"),
        "good/x",
        "the new group works immediately"
    );
    assert_eq!(
        put(json!({"chains": {}, "version": cfg["version"]}))
            .await
            .unwrap()
            .status(),
        409,
        "the old version can no longer save"
    );

    let hist: Value = h
        .http
        .get(format!("{}/_gateway/config/history", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hist.as_array().unwrap().len(), 1);
    let file = hist[0]["file"].as_str().unwrap();
    let bad = h
        .post(
            "/_gateway/config/restore",
            json!({"file": "../../etc/passwd"}),
        )
        .await;
    assert_eq!(bad.status(), 404);
    assert_eq!(
        h.post("/_gateway/config/restore", json!({"file": file}))
            .await
            .status(),
        200
    );
    assert_eq!(
        get().await["chains"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["a"],
        "restored the original"
    );
}

#[tokio::test]
async fn admin_token_and_origin_guard() {
    let h = start(json!({"chains": {}}), Some("s3cret")).await;
    let url = format!("{}/_gateway/config", h.base);
    let cfg: Value = h.http.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(
        cfg["auth_required"], true,
        "reads stay open and tell the UI to ask for a token"
    );
    let body = json!({"chains": {"x": []}, "version": cfg["version"]});
    assert_eq!(
        h.http.put(&url).json(&body).send().await.unwrap().status(),
        401
    );
    assert_eq!(
        h.http
            .put(&url)
            .bearer_auth("wrong")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let host = h.base.trim_start_matches("http://");
    let cross = h
        .http
        .put(&url)
        .bearer_auth("s3cret")
        .header("origin", "http://evil.example")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross.status(),
        403,
        "another website cannot write even with a token"
    );
    let same = h
        .http
        .put(&url)
        .bearer_auth("s3cret")
        .header("origin", format!("http://{host}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(same.status(), 200);
    assert_eq!(
        h.post("/_gateway/test", json!({"entries": ["good/x"]}))
            .await
            .status(),
        401,
        "the test endpoint spends tokens, so it is guarded too"
    );
}

#[tokio::test]
async fn test_endpoint_reports_each_entry() {
    let h = start(json!({"chains": {"g": ["good/x", "dead/x"]}}), None).await;
    let v: Value = h
        .post("/_gateway/test", json!({"entries": ["g", "p1/*"]}))
        .await
        .json()
        .await
        .unwrap();
    let res = v["results"].as_array().unwrap();
    let status = |t: &str| {
        res.iter()
            .find(|r| r["target"] == t)
            .map(|r| r["status"].clone())
    };
    assert_eq!(status("good/x"), Some(json!(200)));
    assert_eq!(status("dead/x"), Some(json!(402)));
    assert_eq!(status("p1/m2"), Some(json!(200)), "wildcards are expanded");
}

#[tokio::test]
async fn broken_config_edit_keeps_last_good() {
    let h = start(json!({"chains": {"c": ["good/x"]}}), None).await;
    assert_eq!(hdr(&h.chat("c").await, "x-gateway-target"), "good/x");
    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(h.dir.join("chains.json"), "{ this is not json").unwrap();
    assert_eq!(
        hdr(&h.chat("c").await, "x-gateway-target"),
        "good/x",
        "a bad hand-edit must not take the gateway down"
    );
    let _ = &h.mock;
}

#[tokio::test]
async fn unknown_v1_paths_pass_through_unchanged() {
    let h = start(json!({"chains": {}}), None).await;
    for m in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let r = h
            .http
            .request(m.clone(), format!("{}/v1/files?limit=2&after=abc", h.base))
            .header("x-custom", "kept")
            .header("authorization", "Bearer client-key")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(hdr(&r, "x-gateway-target"), "upstream");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["method"], m.as_str());
        assert_eq!(v["query"], "limit=2&after=abc");
        assert_eq!(v["custom"], "kept");
        assert_eq!(
            v["auth"], "Bearer client-key",
            "client auth is forwarded as-is"
        );
        assert_eq!(v["conn"], "", "hop-by-hop headers are stripped");
    }
    let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let sum: u64 = body.iter().map(|b| u64::from(*b)).sum();
    let r = h
        .http
        .post(format!("{}/v1/audio/transcriptions", h.base))
        .header("content-type", "multipart/form-data; boundary=xyz")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["len"], body.len());
    assert_eq!(v["sum"], sum);
    assert_eq!(v["ct"], "multipart/form-data; boundary=xyz");
    let r = h
        .http
        .get(format!("{}/v1/blob", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(hdr(&r, "x-upstream"), "yes");
    assert_eq!(hdr(&r, "content-type"), "application/octet-stream");
    assert_eq!(
        r.bytes().await.unwrap().to_vec(),
        (0u8..=255).collect::<Vec<u8>>()
    );
    let r = h.post("/v1/limited", json!({"model": "m1"})).await;
    assert_eq!(r.status(), 429);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["message"],
        "slow down"
    );
    let r = h.post("/v1/nothing-here", json!({})).await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "not found");
    let r = h
        .http
        .get(format!("{}/admin", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(hdr(&r, "x-gateway-target"), "");
    let recent: Value = h
        .http
        .get(format!("{}/_gateway/recent", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        recent.to_string().contains("passthrough:/v1/files"),
        "passthrough requests show up on the activity page"
    );
}

#[tokio::test]
async fn passthrough_streams_instead_of_buffering() {
    let h = start(json!({"chains": {}}), None).await;
    let t0 = Instant::now();
    let r = h.post("/v1/slow-sse", json!({})).await;
    assert_eq!(hdr(&r, "content-type"), "text/event-stream");
    let mut s = r.bytes_stream();
    use futures_util::StreamExt;
    let first = s.next().await.unwrap().unwrap();
    assert_eq!(
        &first[..],
        b"data: 0

"
    );
    assert!(
        t0.elapsed() < Duration::from_millis(1200),
        "first chunk must arrive before the upstream finishes"
    );
    let second = s.next().await.unwrap().unwrap();
    assert_eq!(
        &second[..],
        b"data: 1

"
    );
    assert!(t0.elapsed() >= Duration::from_millis(1400));
}
