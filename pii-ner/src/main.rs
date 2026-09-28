mod model;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct App {
    model: model::Model,
    name: String,
    requests: AtomicU64,
    texts: AtomicU64,
    bytes: AtomicU64,
    busy_us: AtomicU64,
    max_texts: usize,
}

#[derive(Deserialize)]
struct Req {
    texts: Vec<String>,
}

fn env(name: &str, dflt: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| dflt.to_owned())
}

fn env_num<T: std::str::FromStr>(name: &str, dflt: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(dflt)
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

async fn ner(State(app): State<Arc<App>>, Json(req): Json<Req>) -> (StatusCode, Json<Value>) {
    if req.texts.len() > app.max_texts {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "too many texts"})),
        );
    }
    let n = req.texts.len() as u64;
    let bytes: usize = req.texts.iter().map(String::len).sum();
    let a = app.clone();
    let t0 = Instant::now();
    let res = tokio::task::spawn_blocking(move || a.model.predict(&req.texts)).await;
    let us = t0.elapsed().as_micros() as u64;
    match res {
        Ok(Ok(spans)) => {
            app.requests.fetch_add(1, Ordering::Relaxed);
            app.texts.fetch_add(n, Ordering::Relaxed);
            app.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
            app.busy_us.fetch_add(us, Ordering::Relaxed);
            let out: Vec<Vec<Value>> = spans
                .into_iter()
                .map(|v| {
                    v.into_iter()
                        .map(|s| {
                            json!([s.start, s.end, s.kind, (s.score * 1000.0).round() / 1000.0])
                        })
                        .collect()
                })
                .collect();
            (
                StatusCode::OK,
                Json(json!({"spans": out, "ms": us as f64 / 1000.0})),
            )
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "inference task failed"})),
        ),
    }
}

async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "model": app.name,
        "backend": app.model.backend(),
        "labels": app.model.labels().iter().map(|l| json!({"prompt": l.prompt, "kind": l.kind, "threshold": l.threshold})).collect::<Vec<_>>(),
        "requests": app.requests.load(Ordering::Relaxed),
        "texts": app.texts.load(Ordering::Relaxed),
        "bytes": app.bytes.load(Ordering::Relaxed),
        "busy_ms": app.busy_us.load(Ordering::Relaxed) / 1000,
        "rss_kb": rss_kb(),
    }))
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim() {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim() {}

fn probe() -> i32 {
    use std::io::{Read, Write};
    let listen = env("LISTEN", "127.0.0.1:8090");
    let port = listen.rsplit(':').next().unwrap_or("8090");
    let Ok(mut s) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) else {
        return 1;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    if s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return 1;
    }
    let mut buf = [0u8; 16];
    match s.read(&mut buf) {
        Ok(n) if buf[..n].starts_with(b"HTTP/1.1 200") => 0,
        _ => 1,
    }
}

fn bench(model: &model::Model) {
    let path = env("NER_BENCH_FILE", "bench.txt");
    let full = std::fs::read_to_string(&path).unwrap_or_default();
    let cut = |n: usize| {
        let mut n = n.min(full.len());
        while !full.is_char_boundary(n) {
            n -= 1;
        }
        full[..n].to_owned()
    };
    let loaded = rss_kb();
    let mut out = serde_json::Map::new();
    for (name, size, reps) in [("1kb", 1024usize, 9usize), ("20kb", 20480, 3)] {
        let text = vec![cut(size)];
        let mut ms: Vec<f64> = (0..reps)
            .map(|_| {
                let t0 = Instant::now();
                let _ = model.predict(&text);
                t0.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.insert(
            format!("{name}_ms"),
            json!((ms[reps / 2] * 10.0).round() / 10.0),
        );
    }
    out.insert("rss_loaded_kb".into(), json!(loaded));
    out.insert("rss_after_kb".into(), json!(rss_kb()));
    out.insert("model".into(), json!(env("NER_NAME", "")));
    println!("{}", Value::Object(out));
}

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("health") {
        std::process::exit(probe());
    }
    let model_path = env("NER_MODEL", "/models/model.onnx");
    let tok_path = env("NER_TOKENIZER", "/models/tokenizer.json");
    let thr: f32 = env_num("NER_THRESHOLD", 0.5);
    let labels = model::parse_labels(
        &env(
            "NER_LABELS",
            "name:PERSON|location address:ADDRESS|organization:ORG|location city:LOCATION",
        ),
        thr,
    );
    let opts = model::Options {
        arena: env_num("NER_ARENA", 0) == 1,
        opt_level: env_num("NER_OPT", 3),
        prepack: env_num("NER_PREPACK", 1) == 1,
        threads: env_num("NER_THREADS", 2),
        window: env_num("NER_WINDOW", 160),
        overlap: env_num("NER_OVERLAP", 24),
        batch: env_num("NER_BATCH", 4),
        max_span_words: env_num("NER_MAX_SPAN_WORDS", 30),
    };
    let config = match std::env::var("NER_CONFIG").ok().filter(|p| !p.is_empty()) {
        Some(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("{}", json!({"event": "config_failed", "error": e}));
                std::process::exit(1);
            }
        },
        None => None,
    };
    let model = match model::Model::load(&model_path, &tok_path, labels, config, opts) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", json!({"event": "load_failed", "error": e}));
            std::process::exit(1);
        }
    };
    let warm = model.predict(&["Warm up with Asha Rao in Pune.".to_owned()]);
    trim();
    if std::env::args().nth(1).as_deref() == Some("bench") {
        bench(&model);
        return;
    }
    let app = Arc::new(App {
        model,
        name: env("NER_NAME", &model_path),
        requests: AtomicU64::new(0),
        texts: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        busy_us: AtomicU64::new(0),
        max_texts: env_num("NER_MAX_TEXTS", 4096),
    });
    let listen = env("LISTEN", "127.0.0.1:8090");
    eprintln!(
        "{}",
        json!({"event": "ready", "listen": listen, "warm_ok": warm.is_ok(), "rss_kb": rss_kb()})
    );
    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/ner", post(ner))
        .layer(DefaultBodyLimit::max(env_num("NER_MAX_BODY", 8 << 20)))
        .with_state(app);
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "{}",
                json!({"event": "bind_failed", "error": e.to_string()})
            );
            std::process::exit(1);
        }
    };
    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending::<()>().await } } => {}
            }
        })
        .await;
}
