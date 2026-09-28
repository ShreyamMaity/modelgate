//! modelgate: failover chains and Anthropic<->OpenAI translation in front of
//! Tailscale Aperture (or any OpenAI-compatible gateway).

pub mod chain;
pub mod config;
pub mod pii;
pub mod relay;
pub mod server;
pub mod shim;
pub mod state;
