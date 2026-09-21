//! Anthropic Messages API <-> OpenAI Chat Completions translation.
//!
//! Lets clients that only speak `/v1/messages` (Claude Code) use any chat-completions
//! model. Everything here is pure: no I/O, so it is unit-testable.

use bytes::Bytes;
use serde_json::{json, Map, Value};

pub fn rid(prefix: &str) -> String {
    format!("{prefix}{:024x}", fastrand::u128(..) & ((1u128 << 96) - 1))
}

fn stop_reason(finish: Option<&str>) -> &'static str {
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        _ => "end_turn",
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

fn image_part(b: &Value) -> Option<Value> {
    let s = b.get("source")?;
    match s.get("type")?.as_str()? {
        "base64" => {
            let mt = s
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = s.get("data").and_then(Value::as_str).unwrap_or("");
            Some(
                json!({"type": "image_url", "image_url": {"url": format!("data:{mt};base64,{data}")}}),
            )
        }
        "url" => Some(
            json!({"type": "image_url", "image_url": {"url": s.get("url").and_then(Value::as_str).unwrap_or("")}}),
        ),
        _ => None,
    }
}

/// Anthropic `/v1/messages` request -> OpenAI `/v1/chat/completions` request.
pub fn to_openai(a: &Value, max_tokens_cap: u64) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    let system = a.get("system").map(text_of).unwrap_or_default();
    if !system.is_empty() {
        msgs.push(json!({"role": "system", "content": system}));
    }
    for m in a
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = m.get("content").unwrap_or(&Value::Null);
        let blocks = match content {
            Value::String(s) => {
                msgs.push(json!({"role": role, "content": s}));
                continue;
            }
            Value::Array(b) => b,
            _ => continue,
        };
        if role == "assistant" {
            let calls: Vec<Value> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|b| {
                    let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
                    json!({
                        "id": b.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| rid("call_")),
                        "type": "function",
                        "function": {"name": b.get("name").and_then(Value::as_str).unwrap_or(""), "arguments": input.to_string()}
                    })
                })
                .collect();
            let text: String = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if !text.is_empty() || !calls.is_empty() {
                let mut msg = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                msgs.push(msg);
            }
            continue;
        }
        // User turn: tool results become `tool` messages and must come first.
        let mut parts: Vec<Value> = Vec::new();
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let mut body = b.get("content").map(text_of).unwrap_or_default();
                    if b.get("is_error").and_then(Value::as_bool) == Some(true) {
                        body = format!("Error: {body}");
                    }
                    msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": b.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
                        "content": body
                    }));
                }
                Some("text") => {
                    if let Some(t) = b
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                    {
                        parts.push(json!({"type": "text", "text": t}));
                    }
                }
                Some("image") => parts.extend(image_part(b)),
                _ => {}
            }
        }
        if !parts.is_empty() {
            let only_text = parts.iter().all(|p| p["type"] == "text");
            let content = if only_text {
                json!(parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"))
            } else {
                json!(parts)
            };
            msgs.push(json!({"role": "user", "content": content}));
        }
    }

    let requested = a.get("max_tokens").and_then(Value::as_u64).unwrap_or(4096);
    let mut o = Map::new();
    o.insert(
        "model".into(),
        a.get("model").cloned().unwrap_or(Value::Null),
    );
    o.insert("messages".into(), json!(msgs));
    o.insert("max_tokens".into(), json!(requested.min(max_tokens_cap)));
    for k in ["temperature", "top_p"] {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            o.insert(k.into(), v.clone());
        }
    }
    if let Some(stop) = a
        .get("stop_sequences")
        .filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
    {
        o.insert("stop".into(), stop.clone());
    }
    let tools: Vec<Value> = a
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| t.get("input_schema").is_some() || matches!(t.get("type").and_then(Value::as_str), None | Some("custom")))
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.get("name").and_then(Value::as_str).unwrap_or(""),
                "description": t.get("description").and_then(Value::as_str).unwrap_or(""),
                "parameters": t.get("input_schema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}}))
            }})
        })
        .collect();
    if !tools.is_empty() {
        o.insert("tools".into(), json!(tools));
        let tc = a.get("tool_choice").cloned().unwrap_or(Value::Null);
        match tc.get("type").and_then(Value::as_str) {
            Some("any") => {
                o.insert("tool_choice".into(), json!("required"));
            }
            Some("tool") => {
                o.insert("tool_choice".into(), json!({"type": "function", "function": {"name": tc.get("name").and_then(Value::as_str).unwrap_or("")}}));
            }
            Some("none") => {
                o.insert("tool_choice".into(), json!("none"));
            }
            _ => {}
        }
    }
    if a.get("stream").and_then(Value::as_bool) == Some(true) {
        o.insert("stream".into(), json!(true));
        o.insert("stream_options".into(), json!({"include_usage": true}));
    }
    Value::Object(o)
}

/// OpenAI chat completion -> Anthropic message.
pub fn from_openai(o: &Value, model: &str) -> Value {
    let ch = o
        .get("choices")
        .and_then(|c| c.get(0))
        .cloned()
        .unwrap_or(Value::Null);
    let m = ch.get("message").cloned().unwrap_or(Value::Null);
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(t) = m
        .get("content")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        blocks.push(json!({"type": "text", "text": t}));
    }
    for tc in m
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let f = tc.get("function").cloned().unwrap_or(Value::Null);
        let input = f
            .get("arguments")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(if s.is_empty() { "{}" } else { s }).ok())
            .unwrap_or_else(|| json!({}));
        blocks.push(json!({
            "type": "tool_use",
            "id": tc.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| rid("toolu_")),
            "name": f.get("name").and_then(Value::as_str).unwrap_or(""),
            "input": input
        }));
    }
    let has_tool = blocks.iter().any(|b| b["type"] == "tool_use");
    if blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": ""}));
    }
    let u = o.get("usage").cloned().unwrap_or(Value::Null);
    json!({
        "id": rid("msg_"), "type": "message", "role": "assistant", "model": model, "content": blocks,
        "stop_reason": if has_tool { "tool_use" } else { stop_reason(ch.get("finish_reason").and_then(Value::as_str)) },
        "stop_sequence": null,
        "usage": {
            "input_tokens": u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
        }
    })
}

pub fn error_body(status: u16, message: &str) -> Value {
    let kind = match status {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        _ => "api_error",
    };
    json!({"type": "error", "error": {"type": kind, "message": message}})
}

fn ev(name: &str, v: Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {v}\n\n"))
}

#[derive(PartialEq)]
enum Open {
    Nothing,
    Text,
    Tool(i64),
}

/// Incremental converter: feed it raw OpenAI SSE bytes, get Anthropic SSE events back.
pub struct StreamConv {
    model: String,
    idx: i64,
    open: Open,
    finish: Option<String>,
    usage: Value,
    chars: u64,
    saw_tool: bool,
    done: bool,
    ended: bool,
    buf: Vec<u8>,
}

impl StreamConv {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.into(),
            idx: -1,
            open: Open::Nothing,
            finish: None,
            usage: Value::Null,
            chars: 0,
            saw_tool: false,
            done: false,
            ended: false,
            buf: Vec::new(),
        }
    }

    pub fn start(&self) -> Vec<Bytes> {
        vec![ev(
            "message_start",
            json!({"type": "message_start", "message": {
            "id": rid("msg_"), "type": "message", "role": "assistant", "model": self.model, "content": [],
            "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}}),
        )]
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.line(&line, &mut out);
        }
        out
    }

    fn close_block(&mut self, out: &mut Vec<Bytes>) {
        if self.open != Open::Nothing {
            out.push(ev(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": self.idx}),
            ));
            self.open = Open::Nothing;
        }
    }

    fn line(&mut self, raw: &[u8], out: &mut Vec<Bytes>) {
        let line = String::from_utf8_lossy(raw);
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return;
        }
        if self.done {
            return;
        }
        let Ok(j) = serde_json::from_str::<Value>(data) else {
            return;
        };
        if j.get("usage").is_some_and(|u| !u.is_null()) {
            self.usage = j["usage"].clone();
        }
        for ch in j
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let dl = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(text) = dl
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                if self.open != Open::Text {
                    self.close_block(out);
                    self.idx += 1;
                    self.open = Open::Text;
                    out.push(ev("content_block_start", json!({"type": "content_block_start", "index": self.idx, "content_block": {"type": "text", "text": ""}})));
                }
                self.chars += text.len() as u64;
                out.push(ev("content_block_delta", json!({"type": "content_block_delta", "index": self.idx, "delta": {"type": "text_delta", "text": text}})));
            }
            for tc in dl
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let f = tc.get("function").cloned().unwrap_or(Value::Null);
                let k = Open::Tool(tc.get("index").and_then(Value::as_i64).unwrap_or(0));
                if self.open != k {
                    self.close_block(out);
                    self.idx += 1;
                    self.open = k;
                    self.saw_tool = true;
                    out.push(ev("content_block_start", json!({"type": "content_block_start", "index": self.idx, "content_block": {
                        "type": "tool_use",
                        "id": tc.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| rid("toolu_")),
                        "name": f.get("name").and_then(Value::as_str).unwrap_or(""), "input": {}}})));
                }
                if let Some(args) = f
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                {
                    self.chars += args.len() as u64;
                    out.push(ev("content_block_delta", json!({"type": "content_block_delta", "index": self.idx, "delta": {"type": "input_json_delta", "partial_json": args}})));
                }
            }
            if let Some(fr) = ch.get("finish_reason").and_then(Value::as_str) {
                self.finish = Some(fr.to_owned());
            }
        }
    }

    /// Flush and close the message (idempotent).
    pub fn finish(&mut self) -> Vec<Bytes> {
        if self.ended {
            return Vec::new();
        }
        self.ended = true;
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            self.line(&rest, &mut out);
        }
        if self.idx == -1 {
            // A valid message needs at least one content block.
            self.idx = 0;
            self.open = Open::Text;
            out.push(ev("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})));
        }
        self.close_block(&mut out);
        let stop = if self.saw_tool {
            "tool_use"
        } else {
            stop_reason(self.finish.as_deref())
        };
        let out_tokens = self
            .usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .unwrap_or(self.chars / 4);
        out.push(ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null},
            "usage": {"input_tokens": self.usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0), "output_tokens": out_tokens}})));
        out.push(ev("message_stop", json!({"type": "message_stop"})));
        out
    }

    /// The upstream died mid-stream: close what is open and report an error event.
    pub fn abort(&mut self, message: &str) -> Vec<Bytes> {
        if self.ended {
            return Vec::new();
        }
        self.ended = true;
        let mut out = Vec::new();
        self.close_block(&mut out);
        out.push(ev("error", error_body(502, message)));
        out
    }
}

/// Rough input-token estimate for `/v1/messages/count_tokens` (~3.5 chars per token).
pub fn estimate_tokens(a: &Value) -> u64 {
    let picked =
        json!({"system": a.get("system"), "messages": a.get("messages"), "tools": a.get("tools")});
    ((picked.to_string().len() as f64) / 3.5).max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &[Bytes]) -> String {
        b.iter()
            .map(|x| String::from_utf8_lossy(x).into_owned())
            .collect()
    }

    #[test]
    fn request_translation() {
        let req = json!({
            "model": "smart", "max_tokens": 64000, "stream": true,
            "system": [{"type": "text", "text": "You are terse."}],
            "tools": [{"name": "Read", "description": "read", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}],
            "tool_choice": {"type": "any"},
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "read a.txt"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}, {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"path": "a.txt"}}]},
                {"role": "user", "content": [{"type": "text", "text": "thanks"}, {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "hello"}]}]}
            ]
        });
        let o = to_openai(&req, 32768);
        let roles: Vec<&str> = o["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            ["system", "user", "assistant", "tool", "user"],
            "tool result must precede the user text"
        );
        assert_eq!(
            o["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a.txt\"}"
        );
        assert_eq!(o["messages"][3]["tool_call_id"], "toolu_1");
        assert_eq!(o["messages"][3]["content"], "hello");
        assert_eq!(o["max_tokens"], 32768);
        assert_eq!(o["tool_choice"], "required");
        assert_eq!(o["tools"][0]["function"]["name"], "Read");
        assert_eq!(o["stream_options"]["include_usage"], true);
    }

    #[test]
    fn image_and_error_result() {
        let req = json!({"model": "m", "messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AAA"}},
            {"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": "boom"}]}]});
        let o = to_openai(&req, 100);
        assert_eq!(o["messages"][0]["content"], "Error: boom");
        assert_eq!(
            o["messages"][1]["content"][0]["image_url"]["url"],
            "data:image/jpeg;base64,AAA"
        );
    }

    #[test]
    fn response_translation() {
        let resp = json!({"choices": [{"message": {"content": "hi", "tool_calls": [{"id": "c1", "function": {"name": "Read", "arguments": "{\"path\":\"x\"}"}}]}, "finish_reason": "tool_calls"}],
                          "usage": {"prompt_tokens": 11, "completion_tokens": 5}});
        let a = from_openai(&resp, "smart");
        assert_eq!(a["content"][0]["type"], "text");
        assert_eq!(a["content"][1]["input"], json!({"path": "x"}));
        assert_eq!(a["stop_reason"], "tool_use");
        assert_eq!(a["usage"], json!({"input_tokens": 11, "output_tokens": 5}));
        let empty = from_openai(
            &json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}]}),
            "m",
        );
        assert_eq!(empty["content"][0]["text"], "");
        assert_eq!(empty["stop_reason"], "max_tokens");
    }

    fn sse(objs: &[Value]) -> Vec<u8> {
        let mut s: String = objs.iter().map(|o| format!("data: {o}\n\n")).collect();
        s.push_str("data: [DONE]\n\n");
        s.into_bytes()
    }

    #[test]
    fn stream_text_then_tool() {
        let data = sse(&[
            json!({"choices": [{"delta": {"content": "Hel"}}]}),
            json!({"choices": [{"delta": {"content": "lo"}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c9", "function": {"name": "Read", "arguments": "{\"pa"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"b\"}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 20, "completion_tokens": 9}}),
        ]);
        let mut c = StreamConv::new("smart");
        let mut all = c.start();
        // Feed in awkward 7-byte slices to prove line buffering works across chunk boundaries.
        for piece in data.chunks(7) {
            all.extend(c.feed(piece));
        }
        all.extend(c.finish());
        let t = text(&all);
        let names: Vec<&str> = t
            .lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect();
        assert_eq!(
            names,
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert!(t.contains("\"stop_reason\":\"tool_use\""));
        assert!(t.contains("\"output_tokens\":9"));
        assert!(t.contains("\"partial_json\":\"th\\\":\\\"b\\\"}\""));
    }

    #[test]
    fn stream_empty_is_still_valid() {
        let mut c = StreamConv::new("m");
        let mut all = c.start();
        all.extend(c.feed(&sse(&[
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ])));
        all.extend(c.finish());
        let t = text(&all);
        assert_eq!(t.matches("event: content_block_start").count(), 1);
        assert!(t.contains("message_stop"));
        assert!(c.finish().is_empty(), "finish is idempotent");
    }

    #[test]
    fn stream_abort_reports_error() {
        let mut c = StreamConv::new("m");
        let mut all = c.start();
        all.extend(c.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n"));
        all.extend(c.abort("upstream died"));
        let t = text(&all);
        assert!(
            t.contains("event: content_block_stop")
                && t.contains("event: error")
                && !t.contains("message_stop")
        );
    }

    #[test]
    fn token_estimate() {
        assert!(
            estimate_tokens(&json!({"messages": [{"role": "user", "content": "hello there"}]})) > 5
        );
    }
}
