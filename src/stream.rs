//! SSE decoration: a leading `gateway-served` comment, `: keepalive` during silences, a total
//! wall-time cap and a trailing `gateway-usage` comment.

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::pin::Pin;
use std::time::{Duration, Instant};

pub type BoxStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

pub const KEEPALIVE: &[u8] = b": keepalive\n\n";

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub prompt: Option<u64>,
    pub completion: Option<u64>,
}

impl Usage {
    pub fn scan_line(&mut self, line: &str) {
        let Some(d) = line.strip_prefix("data:") else {
            return;
        };
        if !d.contains("usage") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(d.trim()) else {
            return;
        };
        for u in [&v["usage"], &v["message"]["usage"]] {
            if !u.is_object() {
                continue;
            }
            if let Some(n) = u["prompt_tokens"].as_u64().or(u["input_tokens"].as_u64()) {
                if n > 0 || self.prompt.is_none() {
                    self.prompt = Some(n);
                }
            }
            if let Some(n) = u["completion_tokens"]
                .as_u64()
                .or(u["output_tokens"].as_u64())
            {
                if n > 0 || self.completion.is_none() {
                    self.completion = Some(n);
                }
            }
        }
    }
}

pub struct Ctx {
    pub prefix: Option<Bytes>,
    pub model: String,
    pub group: String,
    pub started: Instant,
    pub ttfb_ms: u64,
    pub keepalive: Duration,
    pub total: Option<Duration>,
    pub anthropic: bool,
}

pub fn comment(name: &str, v: &Value) -> Bytes {
    Bytes::from(format!(": {name} {v}\n\n"))
}

pub fn error_event(anthropic: bool, v: &Value) -> Bytes {
    if anthropic {
        let msg = v["message"].as_str().unwrap_or("gateway error");
        let body =
            json!({"type": "error", "error": {"type": "api_error", "message": msg, "gateway": v}});
        Bytes::from(format!("event: error\ndata: {body}\n\n"))
    } else {
        Bytes::from(format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({ "error": v })
        ))
    }
}

struct St {
    inner: BoxStream,
    ctx: Ctx,
    tail: Vec<u8>,
    line: Vec<u8>,
    usage: Usage,
    done: bool,
}

impl St {
    fn at_boundary(&self) -> bool {
        self.tail.is_empty() || self.tail.ends_with(b"\n\n") || self.tail.ends_with(b"\r\n\r\n")
    }

    fn feed(&mut self, b: &[u8]) {
        self.tail.extend_from_slice(b);
        if self.tail.len() > 4 {
            self.tail.drain(..self.tail.len() - 4);
        }
        for &c in b {
            if c == b'\n' {
                let l = String::from_utf8_lossy(&self.line)
                    .trim_end_matches('\r')
                    .to_owned();
                self.usage.scan_line(&l);
                self.line.clear();
            } else if self.line.len() < 1 << 20 {
                self.line.push(c);
            }
        }
    }

    fn usage_comment(&self) -> Bytes {
        let v = json!({"model": self.ctx.model, "group": self.ctx.group,
            "prompt_tokens": self.usage.prompt, "completion_tokens": self.usage.completion,
            "ms": self.ctx.started.elapsed().as_millis() as u64, "ttfb_ms": self.ctx.ttfb_ms});
        let c = comment("gateway-usage", &v);
        if self.at_boundary() {
            c
        } else {
            let mut b = b"\n\n".to_vec();
            b.extend_from_slice(&c);
            Bytes::from(b)
        }
    }
}

/// Wraps an SSE body. Keepalives are only written between events, never inside a partial one.
pub fn decorate(inner: BoxStream, mut ctx: Ctx) -> BoxStream {
    let prefix = ctx.prefix.take();
    let st = St {
        inner,
        ctx,
        tail: Vec::new(),
        line: Vec::new(),
        usage: Usage::default(),
        done: false,
    };
    let body = futures_util::stream::unfold(st, |mut st| async move {
        if st.done {
            return None;
        }
        loop {
            let wait = match st.ctx.total {
                Some(t) => {
                    let left = t.saturating_sub(st.ctx.started.elapsed());
                    if left.is_zero() {
                        st.done = true;
                        let mut b = if st.at_boundary() {
                            Vec::new()
                        } else {
                            b"\n\n".to_vec()
                        };
                        let v = json!({"message": "stream exceeded total_timeout_seconds", "type": "gateway_timeout",
                                       "reason": "total_timeout", "model": st.ctx.model});
                        b.extend_from_slice(&error_event(st.ctx.anthropic, &v));
                        return Some((Ok(Bytes::from(b)), st));
                    }
                    left.min(st.ctx.keepalive)
                }
                None => st.ctx.keepalive,
            };
            match tokio::time::timeout(wait, st.inner.next()).await {
                Ok(Some(Ok(b))) => {
                    st.feed(&b);
                    return Some((Ok(b), st));
                }
                Ok(Some(Err(e))) => {
                    st.done = true;
                    return Some((Err(e), st));
                }
                Ok(None) => {
                    st.done = true;
                    let c = st.usage_comment();
                    return Some((Ok(c), st));
                }
                Err(_) => {
                    if st.at_boundary() {
                        return Some((Ok(Bytes::from_static(KEEPALIVE)), st));
                    }
                }
            }
        }
    });
    match prefix {
        Some(p) => Box::pin(futures_util::stream::once(async move { Ok(p) }).chain(body)),
        None => Box::pin(body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(keepalive_ms: u64) -> Ctx {
        Ctx {
            prefix: Some(comment("gateway-served", &json!({"model": "m"}))),
            model: "m".into(),
            group: "g".into(),
            started: Instant::now(),
            ttfb_ms: 5,
            keepalive: Duration::from_millis(keepalive_ms),
            total: None,
            anthropic: false,
        }
    }

    async fn collect(s: BoxStream) -> String {
        let parts: Vec<Result<Bytes, std::io::Error>> = s.collect().await;
        parts
            .into_iter()
            .map(|p| String::from_utf8_lossy(&p.unwrap()).into_owned())
            .collect()
    }

    #[tokio::test]
    async fn prefix_usage_and_keepalive_only_on_boundaries() {
        let chunks: Vec<(u64, &'static str)> = vec![
            (0, "data: {\"choices\":[]}\n\n"),
            (60, "data: {\"choices\":[],\"usa"),
            (
                60,
                "ge\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n",
            ),
            (60, "data: [DONE]\n\n"),
        ];
        let s = futures_util::stream::unfold(chunks.into_iter(), |mut it| async move {
            let (ms, c) = it.next()?;
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Some((Ok(Bytes::from_static(c.as_bytes())), it))
        });
        let out = collect(decorate(Box::pin(s), ctx(25))).await;
        assert!(
            out.starts_with(": gateway-served {\"model\":\"m\"}\n\n"),
            "{out}"
        );
        let usa = out.find("\"usa").unwrap();
        let ge = out.find("ge\":{").unwrap();
        assert!(
            !out[usa..ge].contains("keepalive"),
            "no keepalive inside a partial event: {out}"
        );
        assert!(out.contains(": keepalive\n\n"));
        assert!(out.trim_end().ends_with("}"));
        let last = out
            .lines()
            .rev()
            .find(|l| l.starts_with(": gateway-usage"))
            .unwrap();
        let v: Value = serde_json::from_str(last.trim_start_matches(": gateway-usage ")).unwrap();
        assert_eq!(v["prompt_tokens"], 7);
        assert_eq!(v["completion_tokens"], 3);
    }

    #[tokio::test]
    async fn total_cap_ends_with_error_event() {
        let s = futures_util::stream::unfold(0u8, |i| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Some((Ok(Bytes::from(format!("data: {i}\n\n"))), i.wrapping_add(1)))
        });
        let mut c = ctx(1000);
        c.total = Some(Duration::from_millis(150));
        let out = collect(decorate(Box::pin(s), c)).await;
        assert!(out.contains("gateway_timeout"), "{out}");
        assert!(out.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn usage_shapes() {
        let mut u = Usage::default();
        u.scan_line(r#"data: {"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}"#);
        u.scan_line(r#"data: {"type":"message_delta","usage":{"output_tokens":9}}"#);
        assert_eq!(u.prompt, Some(12));
        assert_eq!(u.completion, Some(9));
    }
}
