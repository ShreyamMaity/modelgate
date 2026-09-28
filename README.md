# modelgate

[![Rust](https://img.shields.io/badge/Rust-9A4F2B?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tailscale](https://img.shields.io/badge/Tailscale-008080?logo=tailscale&logoColor=white)](https://tailscale.com/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/shreyamMaity/modelgate)](https://github.com/shreyamMaity/modelgate/stargazers)
[![GitHub forks](https://img.shields.io/github/forks/shreyamMaity/modelgate)](https://github.com/shreyamMaity/modelgate/network)

A tiny single-binary proxy (~3.5 MB, ~8 MB RAM) that sits in front of [Tailscale Aperture](https://tailscale.com/docs/aperture) — or any OpenAI-compatible gateway — and adds what Aperture doesn't: ordered **failover chains**, **ad-hoc model routing**, and **Anthropic ↔ OpenAI translation**. One base URL for every client, no per-provider keys to manage.

## Features

- **Automatic failover** — ordered chains of models; when the first is down, out of quota, rate-limited or slow, the next one answers and the broken one is cooled down instead of being retried on every request.
- **Any model from Claude Code** — Claude Code only speaks Anthropic's `/v1/messages` format; modelgate translates it (streaming and tool calls included), so every chat model behind your gateway works.
- **Web UI** — `/config`: create groups, pick models from a searchable list, drag to reorder, test entries, save. Changes apply live.
- **Ad-hoc chains in the model name** — `--model "provider-a/big|provider-b/*"`, no config edit.
- **Live activity** — `/_gateway/` shows the model each request asked for versus the one that served it.
- **Sub-10 MB footprint** — single static binary, no build step to run.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | [Rust](https://www.rust-lang.org/) (2021 edition) |
| Web framework | [Axum](https://axum.rs/) 0.8 |
| Async runtime | [Tokio](https://tokio.rs/) |
| HTTP client | [Reqwest](https://crates.io/crates/reqwest) (rustls) |
| Serialization | [serde_json](https://crates.io/crates/serde_json) (preserve_order) |
| Gateway | [Tailscale Aperture](https://tailscale.com/docs/aperture) (or any OpenAI-compatible gateway) |
| UI | Vanilla HTML/CSS/JS (no build step) |

## Prerequisites

- A gateway to forward to — [Tailscale Aperture](https://tailscale.com/docs/aperture) (recommended) or any OpenAI-compatible endpoint — with the models you want to chain already registered.
- [Tailscale](https://tailscale.com/) on the node running modelgate (Aperture identifies callers by tailnet identity).
- [Docker](https://docs.docker.com/) for the container path, or a prebuilt binary, or a [Rust](https://www.rust-lang.org/) toolchain to build.

## Quick Start

modelgate must run on a machine that can reach Aperture — normally a node on your tailnet. It listens on `127.0.0.1:8080` by default; expose it to the tailnet with `tailscale serve`.

**Prebuilt binary** — grab the latest asset from [GitHub Releases](https://github.com/shreyamMaity/modelgate/releases):

```sh
UPSTREAM=http://<aperture-tailnet-ip> ./modelgate
#   config UI:   https://<this-node>.<tailnet>.ts.net/config
#   activity:    https://<this-node>.<tailnet>.ts.net/_gateway/
tailscale serve --bg 8080
```

Then open `/config`, create a group (say `smart`), add a few models, press **Save**. Point a client at it — the base URL is the **modelgate** service URL (the one `tailscale serve` created), *not* the Aperture URL:

```sh
# Claude Code
ANTHROPIC_BASE_URL=https://<modelgate-host>.<tailnet>.ts.net \
  ANTHROPIC_API_KEY=unused \
  claude --model smart

# any OpenAI-style client
curl https://<modelgate-host>.<tailnet>.ts.net/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"smart","messages":[{"role":"user","content":"hello"}]}'
```

### The `model` string decides the route

| You send | What happens |
|---|---|
| `smart` | your group `smart`: tried top to bottom |
| `provider/model` | exactly that model, pinned, no failover |
| `a\|b\|c` | an ad-hoc chain; each piece can be a group, a `provider/model`, or `provider/*` |
| `provider/*` | every model that provider serves |
| a plain model id | forwarded untouched to the upstream (its own routing) |

Groups can contain other groups and wildcards (nesting up to 4 levels, at most 40 targets per chain). A `modelgate/` prefix (`modelgate/smart`) is accepted too, for clients that add a provider name.

## How routing works

- **Failover.** Connect errors, timeouts, and `401 402 403 404 408 410 425 429 5xx` → try the next target and cool the failed one down (30 s, doubling to 10 min, or `Retry-After`). `400 413 422` → try the next, no cooldown. A `404` saying the model "is available via …, not …" (wrong API shape) → try the next, no cooldown.
- **Order.** Healthy targets are always tried first; cooled-down ones remain a last resort. If every target fails, the last upstream error is returned as-is (reshaped as an Anthropic error for `/v1/messages`).
- **Streams.** Once a stream has started it cannot switch providers — failover only applies before the first byte.


**Optional fields a provider rejects.** When a target answers 400/422 and the error names an optional request field the body carries (`reasoning_effort`, `max_completion_tokens`, `parallel_tool_calls`, `seed`, `service_tier`, `stream_options`, ...), the same target is retried once without it (`max_completion_tokens` becomes `max_tokens`). Mixed chains then keep working when a client always sends a field only some providers accept. The activity page shows `retried without <field>`.
## Docker Deployment

The image is built on every push to the default branch and pushed to [ghcr.io](https://ghcr.io) (multi-arch, amd64/arm64); versioned tags are published on `v*` releases.

```sh
docker pull ghcr.io/shreyammaity/modelgate:latest
docker run -d --name modelgate \
  -e UPSTREAM=http://<aperture-tailnet-ip> \
  -e ADMIN_TOKEN=<random> \
  -v $(pwd)/data:/data \
  -p 127.0.0.1:8080:8080 \
  ghcr.io/shreyammaity/modelgate:latest
tailscale serve --bg 8080
```

Or, from this repo, `docker compose up -d` (set `UPSTREAM` to your Aperture tailnet IP first). The compose file ships with `build: .` so it works before the image is published; once the repo is public, swap to `image: ghcr.io/shreyammaity/modelgate:latest` and drop `build:`.

### systemd

See [`contrib/modelgate.service`](contrib/modelgate.service) — install the binary to `/usr/local/bin/modelgate`, drop the unit into `/etc/systemd/system/`, `systemctl enable --now modelgate`, then `tailscale serve --bg 8080`.

## Configuration

One JSON file (`chains.json`, created on first run; re-read whenever it changes; a broken hand-edit is ignored and the last good config keeps serving; the UI keeps the last 40 versions under `history/`). See [`chains.example.json`](chains.example.json).

```json
{
  "upstream": "http://ai",
  "paid_providers": ["aperture"],
  "chains": {
    "fast": ["provider-a/small-model", "provider-b/small-model"],
    "smart": ["provider-a/large-model", "provider-b/large-model", "provider-c/*"]
  },
  "routes": {
    "/v1/embeddings": {
      "targets": [
        { "label": "openai", "base": "https://api.openai.com",
          "model": "text-embedding-3-small", "key_env": "OPENAI_API_KEY" }
      ]
    }
  }
}
```

| Key | Default | Meaning |
|---|---|---|
| `chains` | `{}` | groups: `{"name": ["provider/model", "provider/*", "other-group"]}` |
| `upstream` | `http://ai` | the gateway to forward to (env `UPSTREAM` wins) |
| `ttfb_stream` | `20` | seconds to wait for the first byte of a streaming reply before failing over |
| `timeout` | `90` | seconds to wait for a non-streaming reply |
| `stream_idle` | `120` | longest silence tolerated between chunks of a stream |
| `max_tokens_cap` | `32768` | cap applied to `max_tokens` on `/v1/messages` requests |
| `paid_providers` | `["aperture"]` | providers the UI marks "paid" |
| `routes` | `{}` | non-text endpoints sent to direct upstreams, with the same failover |
| `targets` | `{}` | named direct chat upstreams (your own boxes) usable as chain entries, see below |
| `pii` | all targets `public` | trust tiers for the PII egress layer, see below |

**Embeddings, images, audio.** Aperture only routes text generation. `routes` sends other endpoints straight to providers: keys are read from the environment variable named by `key_env`, never from the config file.

**Your own machines in a chain.** `targets` names OpenAI-compatible servers that are not behind the upstream, for example a llama.cpp or MLX server on another box. A name works anywhere a `provider/model` does: in a group, in an ad-hoc `a|b` chain, alone as the `model`, and `local/*` expands to every target named `local/...`. They are listed in `/v1/models` and the config UI, and the UI's Test button calls them directly.

```json
"targets": {
  "local/small": { "base": "http://10.0.0.5:8121", "model": "small-model", "timeout": 4 },
  "local/big":   { "presence": "desk-pc", "model": "big-model", "timeout": 3, "skip_busy": false }
},
"chains": { "sub": ["local/small", "provider-a/small-model"] }
```

| Target key | Meaning |
|---|---|
| `base` | server URL (optional with `presence`: the address the presence hub reports is used) |
| `model` | rewrites the request's `model` for this target |
| `key_env` | env var holding a bearer key |
| `timeout` | seconds to wait for this target's first byte (the lower of this and the global limit) |
| `presence` | node name: the target is used only while the presence hub says `use_bonsai: true` for it; otherwise it is skipped at once, with no cooldown, and the activity page shows `presence: <reason>` |
| `skip_busy` | with `presence`, also skip while the node reports its server busy |
| `max_input_chars` | skip this target at once (no cooldown) when the request body is longer, for small local models whose prefill is slow |

Presence comes from a hub that serves `GET /presence` as `{"nodes": {"<node>": {"use_bonsai": bool, "route": "bonsai", "reason": "...", "bonsai": {"host": "...", "port": 8081, "busy": bool}}}}`. Set `PRESENCE_URL` and the gateway polls `GET <url>/presence` every 1.5 s. A node that is gone, unknown, not serving, busy with other GPU work, or whose data is older than 5 s is skipped. `/_gateway/status` shows the cached view under `presence`. Put your own targets in the `local` PII tier (`"local": ["local/*"]`) so they receive raw data.

**Everything else under `/v1`.** Any `/v1/*` request the gateway doesn't handle itself (not a chain, not a `route`) is forwarded to the upstream unchanged: same method, path, query string, body and headers, with the reply streamed back as it arrives. So `/v1/embeddings`, `/v1/images/generations`, `/v1/audio/*` and friends work through the same base URL whenever the upstream serves them. Set `PASSTHROUGH=0` to answer 404 instead.

## PII egress layer

Every outgoing attempt is checked against the trust tier of the target it is about to try. A chain that fails over from a model on your own machine to a hosted one masks the hosted attempt, even inside the same request.

| Tier | Gets | Use for |
|---|---|---|
| `local` | raw request | models on hardware you own |
| `trusted_raw` | raw request | first-party APIs you trust with everything |
| `trusted_masked` | placeholders (optionally only for some kinds) | first-party APIs you trust with some things |
| `public` (default) | placeholders for every detected kind | everything else |

```json
"pii": {
  "default": "public",
  "tiers": {
    "local": ["my-box/*"],
    "trusted_raw": [],
    "trusted_masked": ["gemini/*", "anthropic-direct/*"]
  },
  "kinds": { "trusted_masked": ["CARD", "AADHAAR", "PAN", "BANK_ACCOUNT", "PASSWORD", "PIN", "PRIVATE_KEY"] },
  "disable": []
}
```

Patterns are globs matched against `provider/model` (a plain model id is resolved to its provider from the upstream model list; `routes` targets match on their `label`). The most specific pattern wins; on a tie the stricter tier wins.

**What is detected:** payment cards (Luhn), Aadhaar (Verhoeff), PAN, IFSC, UPI ids, bank account numbers next to "A/c"/"account", phone numbers (+91 and international), emails, UPI/ATM PINs and CVVs, passwords after `password:`/`pwd=`/`password is`, credentials in URLs, JWTs, private key blocks, and API keys/tokens (Anthropic, OpenAI-style `sk-`, GitHub, AWS, Google, Slack, Stripe, Hugging Face, GitLab, Groq, npm, Tailscale, Telegram bot, `Bearer`, `api_key=`). Plus exact matches from a local vault file (`PII_VAULT`), for values no pattern can find: names, addresses, the password of a bank statement PDF. `GET /_gateway/pii` lists the kinds, the tiers and the vault size (never values).

**Placeholders** are stable per conversation: `<CARD_A>`, `<EMAIL_B>`, `<PHONE_A>`, `<SECRET:maps_key>` for named vault entries. The conversation is identified by an `X-Conversation-Id` header (or Claude Code's `X-Claude-Code-Session-Id`), else by a hash of the system prompt and first user message. The map lives in memory only, for `PII_TTL_SECS`.

**Replies are rehydrated** on the way back: buffered JSON, OpenAI chat SSE, Anthropic SSE and Responses API SSE, including placeholders split across chunks. Tool-call arguments are rehydrated too, JSON-escaped, so the agent that executes the tool gets the real value while the model only ever saw the placeholder. A model can only get placeholders that were issued in its own conversation rehydrated, plus named vault entries marked `"tool": true`. `PII_REHYDRATE_TOOLS=0` leaves tool arguments masked.

**Fail closed:** if masking fails for a masked tier (vault file missing or unreadable, body not JSON, map full), that target is skipped and the next one is tried; nothing is sent raw. Non-JSON bodies on raw passthrough paths (multipart audio uploads) are not inspected.

**Nothing raw is logged.** The activity page and logs show the tier and counts per kind only.

Vault format (see [`pii-vault.example.json`](pii-vault.example.json)):

```json
{
  "entries": [
    { "name": "maps_key", "value": "FAKE-KEY-VALUE-0000", "tool": true },
    { "kind": "NAME", "value": "Jane Example", "ignore_case": true }
  ]
}
```

### Names, addresses, organisations and places

Addresses with an Indian PIN code are found by a built-in rule. For free-form names, organisations, places and addresses without a PIN, run the optional `pii-ner` sidecar ([`pii-ner/`](pii-ner)): a small Rust server that runs a token-classification or GLiNER ONNX model through ONNX Runtime and answers `POST /v1/ner`. modelgate only calls it for masked tiers (never for `local` or `trusted_raw`), caches results per paragraph, and fails closed: if the sidecar is down or too slow, masked targets are skipped and only raw-tier targets can serve the request.

```json
"pii": {
  "ner": {
    "url": "http://pii-ner:8090",
    "kinds": ["PERSON", "ADDRESS", "ORG", "LOCATION"],
    "min_score": 0.5,
    "allow": ["MyProject", "MyBot"],
    "disable_groups": ["my-code-group"],
    "timeout_ms": 180000,
    "max_bytes": 262144
  },
  "placeholder_style": { "default": "surrogate", "ORG": "tag" }
}
```

The sidecar reads `NER_MODEL` (ONNX file), `NER_TOKENIZER` (`tokenizer.json`), `NER_CONFIG` (`config.json`, for token-classification models), `NER_LABELS` (model label to kind, for example `PER:PERSON|ORG:ORG|LOC:LOCATION`, or GLiNER prompts such as `name:PERSON|location address:ADDRESS`), `NER_THREADS` (default 2) and `LISTEN` (default `127.0.0.1:8090`). `pii-ner bench` prints latency for 1 KB and 20 KB of text from `NER_BENCH_FILE` plus RSS.

`allow` lists words that are never masked (your own tool and project names); common AI and dev product names are allowed already. `PII_NER_URL` sets the URL when the config has none.

`disable_groups` turns NER off for requests to those groups (for example a coding group, where identifiers like "FizzBuzz" must reach the model unchanged). Vault entries and the pattern detectors still mask those requests.

Names, organisations, places and handles (from the vault or NER) are not masked inside URLs, hostnames and email addresses, so `https://yourname.dev` or `*.yourname.dev` reach the model intact. Secrets inside them, such as a URL password, a `?token=` value, a vault secret or a whole email address, are still masked.

**Surrogates.** With `placeholder_style` `surrogate` (the default for `PERSON`, `NAME`, `ORG`, `LOCATION` and `ADDRESS`), names are replaced by realistic fake ones instead of `<PERSON_A>`: the model reads a normal sentence and keeps its grammar, which helps quality. Surrogates are stable per conversation and name part: "Priya Venkataraman" and a later "Priya" share the same fake first name. A surrogate is never a word already present in the conversation; if one later shows up as real text, it is replaced. On the way back surrogates are matched case-insensitively, as first or last name alone, with possessives ("Kavya's"), and across streamed chunks. Secrets, cards, keys and other shaped data always use tags. Set `"placeholder_style": "tag"` to use tags everywhere.

If the model writes a surrogate name, place or organisation in Devanagari or Bengali script, it is still mapped back. Every surrogate word has a built-in transliteration with common spelling variants (inherent vowel dropped or kept, nukta, vowel length, anusvara, Bengali ya), and those forms are matched on the way back too, including across streamed chunks and with Bengali case endings. A single Indic word whose consonant skeleton (vowels, aspiration, doubling and similar sounds ignored, at least three consonants) equals a surrogate's is mapped back as well, which catches English-style respellings. A form that already appears in the request as real text is not used.

## Environment Variables

| Variable | Default | Meaning |
|---|---|---|
| `CONFIG` | `./chains.json` | path to the JSON config file (created if missing) |
| `LISTEN` | `127.0.0.1:8080` | address to listen on |
| `UPSTREAM` | config `upstream`, else `http://ai` | the gateway to forward to |
| `ADMIN_TOKEN` | unset | if set, config changes need `Authorization: Bearer <token>` |
| `PASSTHROUGH` | on | `0` answers 404 for unhandled `/v1/*` paths instead of forwarding them to the upstream |
| `PII_MODE` | on | `off` sends every request unmodified |
| `PII_VAULT` | unset | JSON file of exact values that are always masked; re-read when it changes |
| `PII_TTL_SECS` | `21600` | how long a conversation's placeholder map is kept |
| `PII_REHYDRATE_TOOLS` | on | `0` leaves placeholders in tool-call arguments |
| `PII_NER_URL` | unset | `pii-ner` sidecar used when `pii.ner.url` is not set |
| `PRESENCE_URL` | unset | presence hub for targets with `presence`; unset means those targets are always skipped |
| `PRESENCE_POLL_MS` | `1500` | how often the hub is polled |
| `PRESENCE_MAX_AGE_MS` | `5000` | older presence data counts as absent |
| `PRESENCE_TIMEOUT_MS` | `800` | per-poll timeout |

The same options can be set as flags: `--config`, `--listen`, `--upstream`. `modelgate --help` for the full list.

## Project Structure

```
src/
  main.rs       # entrypoint: args/env, config bootstrap, server wiring
  server.rs     # axum router: /v1/*, /_gateway/*, /config
  relay.rs      # failover chain engine, cooldowns, streaming relay
  shim.rs       # Anthropic <-> OpenAI translation (streaming + tool calls)
  chain.rs      # chain expansion, wildcards, nesting
  config.rs     # chains.json load/save, versioning, history
  state.rs      # runtime state: cooling_down, served, model list
  presence.rs   # presence hub poller that gates targets on another machine
  pii/          # egress masking: detectors, vault, per-conversation maps, rehydration
assets/
  config.html   # the /config UI (vanilla HTML/CSS/JS)
  dashboard.html
contrib/
  modelgate.service   # systemd unit
chains.example.json
pii-ner/        # optional NER sidecar (separate crate and image)
tests/
```

## Security

- The UI can spend tokens (`Test entries`) and change your routing. **Set `ADMIN_TOKEN`** if anyone else can reach the port — writes then need `Authorization: Bearer <token>` (the UI asks once per browser session). Reads and inference are not authenticated; keep the port on the tailnet.
- Config writes refuse cross-origin requests, so a web page can't drive your gateway through your browser.
- Client credentials (`x-api-key`, `Authorization`, `anthropic-*`) are dropped when translating `/v1/messages`; the upstream identity is the tailnet node.
- Without the NER sidecar the PII layer is pattern based and will miss free-form personal data (a name it has not been told about); put those in the vault. With it, recall is high but not perfect: a lone first name in a short message can still slip through. Tool-call rehydration means a hosted model can make your agent run a tool with a real value it never saw, so keep tool permissions on the agent side tight.

## License

MIT — see [LICENSE](LICENSE).

> Not affiliated with or endorsed by Tailscale. "Aperture" is Tailscale's product; modelgate only talks to its public HTTP API.
