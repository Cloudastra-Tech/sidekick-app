//! Coaching model provider. Claude Haiku 4.5 via the Anthropic Messages API; kept behind
//! this interface so the transport and model stay swappable.

mod anthropic;
pub mod partial_json;
mod sse;

use crate::config::env_or_dotenv;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

/// Claude requests actually sent, retries and hedges included. The API reports usage only for
/// the request whose answer is used, so this is how the extra ones show up in the call's numbers.
pub static REQUESTS_SENT: AtomicU64 = AtomicU64::new(0);

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_SUMMARY_MODEL: &str = "claude-sonnet-5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Model,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// One structured-output request. For live hints, `system` never changes during a call
/// and `turns` is append-only, so every request extends the previous request's prefix.
pub struct LlmRequest<'a> {
    pub system: &'a str,
    pub turns: &'a [Turn],
    /// JSON schema the response must match.
    pub schema: &'a serde_json::Value,
    pub max_tokens: u32,
    /// Whether to mark the prompt for caching (worth it only for repeated prefixes).
    pub cache: bool,
    /// `output_config.effort`, when the model supports it.
    pub effort: Option<&'a str>,
    /// Lower is steadier wording; `None` leaves the model's default.
    pub temperature: Option<f32>,
    pub timeout: Duration,
    /// With no answer by then, send the same request again on a fresh connection and use
    /// whichever answers first. For live hints, where a stuck request costs the moment.
    pub hedge_after: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Total prompt tokens: uncached + cache writes + cache reads.
    pub input_tokens: u64,
    /// Portion of `input_tokens` read from the prompt cache.
    pub cache_read_tokens: u64,
    /// Portion of `input_tokens` written to the prompt cache.
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug)]
pub struct LlmResponse {
    /// JSON text matching the request's schema.
    pub text: String,
    pub usage: Usage,
}

#[derive(Clone)]
pub enum LlmProvider {
    Anthropic { access: Access, model: String },
}

/// How a Claude request is authorised: with a key on this machine, or through the backend, which
/// holds the key and counts what each user spends.
#[derive(Clone)]
pub enum Access {
    Direct { api_key: String },
    Backend { url: String, token: String },
}

impl LlmProvider {
    /// The live-hint model (Claude Haiku 4.5 by default).
    pub fn from_env() -> Result<Self, String> {
        Self::anthropic_from_env("ANTHROPIC_MODEL", DEFAULT_ANTHROPIC_MODEL)
    }

    /// The post-call summary model (Claude Sonnet 5 by default).
    pub fn summary_from_env() -> Result<Self, String> {
        Self::anthropic_from_env("ANTHROPIC_SUMMARY_MODEL", DEFAULT_SUMMARY_MODEL)
    }

    fn anthropic_from_env(model_key: &str, default_model: &str) -> Result<Self, String> {
        let model = env_or_dotenv(model_key).unwrap_or_else(|| default_model.into());
        // Signed in to a workspace: the backend has the key, so this machine doesn't need one.
        let access = match crate::backend::llm_route() {
            Some((url, token)) => Access::Backend { url, token },
            None => Access::Direct {
                api_key: env_or_dotenv("ANTHROPIC_API_KEY")
                    .ok_or("ANTHROPIC_API_KEY is not set (environment or .env)")?,
            },
        };
        Ok(Self::Anthropic { access, model })
    }

    pub fn model_name(&self) -> &str {
        match self {
            Self::Anthropic { model, .. } => model,
        }
    }

    pub async fn generate(
        &self,
        client: &reqwest::Client,
        request: &LlmRequest<'_>,
    ) -> Result<LlmResponse, String> {
        match self {
            Self::Anthropic { access, model } => {
                anthropic::generate(client, access, model, request).await
            }
        }
    }

    /// Like `generate`, with the answer streamed: `on_text` gets each piece as Claude writes it.
    pub async fn stream(
        &self,
        client: &reqwest::Client,
        request: &LlmRequest<'_>,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse, String> {
        match self {
            Self::Anthropic { access, model } => {
                anthropic::stream(client, access, model, request, on_text).await
            }
        }
    }
}
