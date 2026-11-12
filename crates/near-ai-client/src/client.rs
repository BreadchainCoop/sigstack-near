//! NEAR AI Cloud HTTP client.

use crate::error::NearAiError;
use crate::types::*;
use futures::StreamExt;
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio_stream::Stream;
use tracing::{debug, instrument, warn};

/// Default retry configuration
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_INITIAL_BACKOFF_MS: u64 = 100;
const DEFAULT_MAX_BACKOFF_MS: u64 = 5000;

/// A completed chat completion, byte-for-byte as sent and received: what NEAR AI's
/// response signature covers.
#[derive(Debug, Clone)]
pub struct ChatExchange {
    /// The completion's `id`, which keys its signature.
    pub chat_id: String,
    /// The model named in the request.
    pub model: String,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
}

/// Told about every successful chat completion, after the caller has its reply.
pub trait ChatObserver: Send + Sync {
    fn observe(&self, exchange: ChatExchange);
}

/// NEAR AI Cloud client.
///
/// The API key is stored using `SecretString` to prevent accidental
/// exposure in logs or debug output.
#[derive(Clone)]
pub struct NearAiClient {
    client: Client,
    base_url: String,
    api_key: SecretString,
    model: String,
    no_aliasing: bool,
    observer: Option<Arc<dyn ChatObserver>>,
}

impl NearAiClient {
    /// Create a new NEAR AI client.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, NearAiError> {
        let client = Client::builder().timeout(timeout).build()?;

        Ok(Self {
            client,
            base_url: base_url.into(),
            api_key: SecretString::new(api_key.into()),
            model: model.into(),
            no_aliasing: false,
            observer: None,
        })
    }

    /// Send `x-no-aliasing: true`, so NEAR AI serves exactly the configured model
    /// (which must then be a canonical name, not an alias) and the model's own TEE
    /// signs the response. Without it, only the gateway signs.
    pub fn with_no_aliasing(mut self, no_aliasing: bool) -> Self {
        self.no_aliasing = no_aliasing;
        self
    }

    /// Report each successful chat completion to `observer`.
    pub fn with_observer(mut self, observer: Arc<dyn ChatObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Get the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Send a chat completion request.
    #[instrument(skip(self, messages), fields(message_count = messages.len()))]
    pub async fn chat(
        &self,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<String, NearAiError> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            temperature,
            max_tokens,
            stream: Some(false),
            tools: None,
            tool_choice: None,
        };

        // Serialize once and send these exact bytes: the response signature covers them.
        let body = serde_json::to_vec(&request)?;
        let mut post = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json");
        if self.no_aliasing {
            post = post.header("x-no-aliasing", "true");
        }
        let response = post.body(body.clone()).send().await?;
        if !response.status().is_success() {
            return Err(self.extract_error(response).await);
        }
        let bytes = response.bytes().await?.to_vec();
        let chat_response: ChatResponse = serde_json::from_slice(&bytes)?;

        if let Some(observer) = &self.observer {
            observer.observe(ChatExchange {
                chat_id: chat_response.id.clone(),
                model: request.model,
                request: body,
                response: bytes,
            });
        }

        // Extract content from response, returning error if empty
        chat_response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .filter(|content| !content.is_empty())
            .ok_or(NearAiError::EmptyResponse)
    }

    /// Transcribe audio via `POST /audio/transcriptions` (multipart; metadata-stripped file only).
    #[instrument(skip(self, audio))]
    pub async fn transcribe(
        &self,
        audio: &[u8],
        filename: &str,
        content_type: &str,
    ) -> Result<String, NearAiError> {
        let mut part =
            reqwest::multipart::Part::bytes(audio.to_vec()).file_name(filename.to_string());
        if !content_type.is_empty() {
            part = part.mime_str(content_type).map_err(NearAiError::Http)?;
        }

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "openai/whisper-large-v3")
            .text("response_format", "json");

        let response = self
            .client
            .post(format!("{}/audio/transcriptions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .multipart(form)
            .send()
            .await?;

        let body = self
            .handle_response::<AudioTranscriptionResponse>(response)
            .await?;
        let text = body.text.trim().to_string();
        if text.is_empty() {
            return Err(NearAiError::EmptyResponse);
        }
        Ok(text)
    }

    /// Send a chat completion request with tool support.
    ///
    /// Not reported to the observer, so these responses aren't verified.
    #[instrument(skip(self, messages, tools), fields(message_count = messages.len()))]
    pub async fn chat_with_tools(
        &self,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResponseWithTools, NearAiError> {
        self.warn_unverified("chat_with_tools");
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            temperature,
            max_tokens,
            stream: Some(false),
            tools: tools.map(|t| t.to_vec()),
            tool_choice: tools.map(|_| "auto".to_string()),
        };

        let url = format!("{}/chat/completions", self.base_url);
        debug!("Sending chat_with_tools request to {}", url);

        let response = match self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("HTTP request to {} failed: {}", url, e);
                return Err(e.into());
            }
        };

        debug!("Response status: {}", response.status());
        let chat_response = self.handle_response::<ChatResponse>(response).await?;

        // Extract usage before consuming response
        let usage = chat_response.usage;

        // Extract from first choice
        let choice = chat_response
            .choices
            .into_iter()
            .next()
            .ok_or(NearAiError::EmptyResponse)?;

        Ok(ChatResponseWithTools {
            content: choice.message.content,
            tool_calls: choice.message.tool_calls,
            finish_reason: choice.finish_reason.unwrap_or_default(),
            usage,
        })
    }

    /// Send a streaming chat completion request.
    ///
    /// Not reported to the observer (the stream isn't kept byte-for-byte), so these
    /// responses aren't verified.
    #[instrument(skip(self, messages), fields(message_count = messages.len()))]
    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<impl Stream<Item = Result<String, NearAiError>>, NearAiError> {
        self.warn_unverified("chat_stream");
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            temperature,
            max_tokens,
            stream: Some(true),
            tools: None,
            tool_choice: None,
        };

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(self.extract_error(response).await);
        }

        let stream = response.bytes_stream().map(|result| {
            result.map_err(NearAiError::from).map(|bytes| {
                // Parse SSE data
                let text = String::from_utf8_lossy(&bytes);
                let mut content = String::new();

                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            continue;
                        }
                        if let Ok(chunk) = serde_json::from_str::<ChatChunk>(data) {
                            if let Some(delta_content) =
                                chunk.choices.first().and_then(|c| c.delta.content.as_ref())
                            {
                                content.push_str(delta_content);
                            }
                        }
                    }
                }

                content
            })
        });

        Ok(stream)
    }

    /// List available models.
    /// Note: NEAR AI Cloud doesn't have a /models endpoint, so we return known models.
    #[instrument(skip(self))]
    pub async fn list_models(&self) -> Result<Vec<Model>, NearAiError> {
        // NEAR AI Cloud doesn't expose a /models endpoint
        // Return a list of known available models (as of Dec 2025)
        Ok(vec![
            Model {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: "deepseek".to_string(),
            },
            Model {
                id: "deepseek-ai/DeepSeek-V3.1".to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: "deepseek".to_string(),
            },
            Model {
                id: "openai/gpt-oss-120b".to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: "openai".to_string(),
            },
            Model {
                id: "Qwen/Qwen3-30B-A3B-Instruct-2507".to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: "alibaba".to_string(),
            },
            Model {
                id: "zai-org/GLM-4.6".to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: "zhipu".to_string(),
            },
        ])
    }

    /// Send a chat completion request with automatic retry and exponential backoff.
    ///
    /// Retries on transient errors (network issues, rate limits) up to `max_retries` times.
    /// Does not retry on authentication errors or empty responses.
    #[instrument(skip(self, messages), fields(message_count = messages.len()))]
    pub async fn chat_with_retry(
        &self,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        max_retries: Option<u32>,
    ) -> Result<String, NearAiError> {
        let max_retries = max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
        let mut backoff_ms = DEFAULT_INITIAL_BACKOFF_MS;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                debug!("Retry attempt {} after {}ms backoff", attempt, backoff_ms);
                sleep(Duration::from_millis(backoff_ms)).await;
                // Exponential backoff with cap
                backoff_ms = (backoff_ms * 2).min(DEFAULT_MAX_BACKOFF_MS);
            }

            match self.chat(messages.clone(), temperature, max_tokens).await {
                Ok(response) => return Ok(response),
                Err(NearAiError::Unauthorized) => return Err(NearAiError::Unauthorized),
                Err(e) => {
                    warn!("Chat request failed (attempt {}): {}", attempt + 1, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(NearAiError::Api {
            status: 0,
            message: "Max retries exceeded".into(),
        }))
    }

    /// Health check - returns true if API is reachable.
    /// Tests connectivity by sending a minimal chat completion request.
    pub async fn health_check(&self) -> bool {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message::user("ping")],
            temperature: None,
            max_tokens: Some(1),
            stream: Some(false),
            tools: None,
            tool_choice: None,
        };

        match self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    true
                } else {
                    warn!("NEAR AI health check failed: HTTP {}", response.status());
                    false
                }
            }
            Err(e) => {
                warn!("NEAR AI health check failed: {}", e);
                false
            }
        }
    }

    /// Only `chat` reports to the observer; say so loudly if another path is used
    /// while verification is on.
    fn warn_unverified(&self, method: &str) {
        if self.observer.is_some() {
            warn!("NEAR AI {method} responses are not verified; use chat for verified replies");
        }
    }

    /// Handle HTTP response, converting errors appropriately.
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, NearAiError> {
        let status = response.status();

        if status.is_success() {
            let body = response.text().await?;
            debug!("Response body: {}", &body[..body.len().min(200)]);
            serde_json::from_str(&body).map_err(NearAiError::from)
        } else {
            Err(self.extract_error(response).await)
        }
    }

    /// Extract error information from failed response.
    async fn extract_error(&self, response: reqwest::Response) -> NearAiError {
        let status = response.status();

        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                warn!("Rate limit exceeded");
                NearAiError::RateLimit
            }
            StatusCode::UNAUTHORIZED => {
                warn!("Authentication failed");
                NearAiError::Unauthorized
            }
            _ => {
                let message = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".into());
                NearAiError::Api {
                    status: status.as_u16(),
                    message,
                }
            }
        }
    }
}
