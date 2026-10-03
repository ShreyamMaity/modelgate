use modelgate::state::{log, spawn_pin_flush, spawn_presence, AppState};
use modelgate::{config, server};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
modelgate - failover chains for Tailscale Aperture (or any OpenAI-compatible gateway)

USAGE:
    modelgate [--config <file>] [--listen <addr>] [--upstream <url>]

Every option can also be set with an environment variable:
    CONFIG       path to the JSON config file          (default ./chains.json, created if missing)
    LISTEN       address to listen on                  (default 127.0.0.1:8080)
    UPSTREAM     the gateway to forward to             (default: \"upstream\" in the config, else http://ai)
    ADMIN_TOKEN  if set, config changes need `Authorization: Bearer <token>`
    PASSTHROUGH  0 to answer 404 for /v1/* paths the gateway doesn't handle itself
                 (default: forward them to the upstream unchanged, e.g. /v1/embeddings)
    PII_MODE     off to disable the PII egress layer   (default on: non-local targets get placeholders)
    PII_VAULT    JSON file of known secrets/PII that are always masked (never sent raw to masked tiers)
    PII_TTL_SECS how long a conversation's placeholder map is kept (default 21600)
    PII_REHYDRATE_TOOLS  0 to leave placeholders in tool-call arguments instead of real values
    PRESENCE_URL tailmesh-style hub; targets with \"presence\": \"<node>\" are used only while
                 GET <url>/presence says use_bonsai for that node (else skipped, no cooldown)
    PINS_FILE    where sticky-group pins and failover events are kept (default pins.json next to CONFIG)
    PRESENCE_POLL_MS / PRESENCE_MAX_AGE_MS / PRESENCE_TIMEOUT_MS   (defaults 1500 / 5000 / 800)

Docs: README.md    Config UI: http://<listen>/config    Activity: http://<listen>/_gateway/
";

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return;
    }
    if std::env::args().any(|a| a == "-V" || a == "--version") {
        println!("modelgate {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let path = PathBuf::from(
        arg("--config")
            .or_else(|| std::env::var("CONFIG").ok())
            .unwrap_or_else(|| "chains.json".into()),
    );
    let listen = arg("--listen")
        .or_else(|| std::env::var("LISTEN").ok())
        .unwrap_or_else(|| "127.0.0.1:8080".into());

    // First run: create an empty config so the /config page has something to edit.
    if !path.exists() {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, "{\n  \"chains\": {}\n}\n");
    }
    let upstream = arg("--upstream")
        .or_else(|| std::env::var("UPSTREAM").ok())
        .or_else(|| config::load(&path).ok().and_then(|c| c.upstream))
        .unwrap_or_else(|| "http://ai".into());
    let token = std::env::var("ADMIN_TOKEN").ok().filter(|t| !t.is_empty());

    let st = AppState::new(path.clone(), upstream.clone(), token.clone());
    // Keep the upstream model list fresh; wildcards (`provider/*`) and the config UI depend on it.
    let bg = st.clone();
    tokio::spawn(async move {
        loop {
            bg.refresh_models().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    spawn_presence(st.clone());
    spawn_pin_flush(st.clone());

    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    log(
        "start",
        json!({"listen": listen, "upstream": upstream, "config": path.display().to_string(), "admin_token": token.is_some()}),
    );
    if token.is_none() && !listen.starts_with("127.") && !listen.starts_with("localhost") {
        eprintln!("note: listening on {listen} without ADMIN_TOKEN - anyone who can reach this port can change your config");
    }
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    };
    let flush = st.clone();
    if let Err(e) = axum::serve(listener, server::router(st))
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
    flush.pins.flush();
}
