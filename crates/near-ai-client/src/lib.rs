//! NEAR AI Cloud client with OpenAI-compatible API.

mod client;
mod error;
mod types;
pub mod verify;

pub use client::{ChatExchange, ChatObserver, NearAiClient};
pub use error::NearAiError;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn create_test_client(mock_server: &MockServer) -> NearAiClient {
        NearAiClient::new(
            "test-api-key",
            mock_server.uri(),
            "test-model",
            Duration::from_secs(30),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_chat_success() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 15,
                "total_tokens": 25
            }
        });

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let messages = vec![Message::user("Hello")];

        let result = client.chat(messages, Some(0.7), None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello! How can I help you?");
    }

    #[tokio::test]
    async fn test_transcribe_audio() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .and(header("Authorization", "Bearer test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": " Hello from NEAR\n"
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let text = client
            .transcribe(b"fake-audio", "voice.ogg", "audio/ogg")
            .await
            .unwrap();
        assert_eq!(text, "Hello from NEAR");

        let received = mock_server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&received[0].body);
        assert!(body.contains("filename=\"voice.ogg\""));
        assert!(body.contains("openai/whisper-large-v3"));
        assert!(!body.contains("+1555"));
        assert!(!body.contains("group_id"));
        assert!(!body.contains("display_name"));
    }

    #[tokio::test]
    async fn test_transcribe_empty_fails() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "  "
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let err = client
            .transcribe(b"fake-audio", "voice.m4a", "audio/aac")
            .await
            .unwrap_err();
        assert!(matches!(err, NearAiError::EmptyResponse));
    }

    #[tokio::test]
    async fn test_chat_empty_response() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "test-model",
            "choices": [],
            "usage": null
        });

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let messages = vec![Message::user("Hello")];

        let result = client.chat(messages, Some(0.7), None).await;
        assert!(matches!(result, Err(NearAiError::EmptyResponse)));
    }

    #[tokio::test]
    async fn test_chat_rate_limit() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let messages = vec![Message::user("Hello")];

        let result = client.chat(messages, Some(0.7), None).await;
        assert!(matches!(result, Err(NearAiError::RateLimit)));
    }

    #[tokio::test]
    async fn test_chat_unauthorized() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let messages = vec![Message::user("Hello")];

        let result = client.chat(messages, Some(0.7), None).await;
        assert!(matches!(result, Err(NearAiError::Unauthorized)));
    }

    #[tokio::test]
    async fn test_list_models() {
        // list_models returns a hardcoded list (NEAR AI doesn't have /models endpoint)
        let mock_server = MockServer::start().await;
        let client = create_test_client(&mock_server).await;
        let result = client.list_models().await;

        assert!(result.is_ok());
        let models = result.unwrap();
        assert_eq!(models.len(), 5);
        assert_eq!(models[0].id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[tokio::test]
    async fn test_health_check_success() {
        // health_check calls POST /chat/completions to verify connectivity
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        assert!(client.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_with_retry_success_on_first_try() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Success on first try"
                },
                "finish_reason": "stop"
            }]
        });

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let messages = vec![Message::user("Hello")];

        let result = client.chat_with_retry(messages, None, None, Some(3)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success on first try");
    }

    #[tokio::test]
    async fn test_chat_with_retry_retries_empty_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "1",
                "object": "chat.completion",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "" },
                    "finish_reason": "stop"
                }]
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "2",
                "object": "chat.completion",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "Hola" },
                    "finish_reason": "stop"
                }]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client
            .chat_with_retry(vec![Message::user("hi")], None, None, Some(2))
            .await;
        assert_eq!(result.unwrap(), "Hola");
    }

    #[tokio::test]
    async fn test_message_constructors() {
        let system = Message::system("You are a helpful assistant");
        assert!(matches!(system.role, Role::System));
        assert_eq!(
            system.content,
            Some("You are a helpful assistant".to_string())
        );
        assert_eq!(system.tool_calls, None);
        assert_eq!(system.tool_call_id, None);

        let user = Message::user("Hello");
        assert!(matches!(user.role, Role::User));
        assert_eq!(user.content, Some("Hello".to_string()));
        assert_eq!(user.tool_calls, None);

        let assistant = Message::assistant("Hi there!");
        assert!(matches!(assistant.role, Role::Assistant));
        assert_eq!(assistant.content, Some("Hi there!".to_string()));
        assert_eq!(assistant.tool_calls, None);

        let tool_calls = vec![ToolCall {
            id: "call-1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
        }];
        let with_tools = Message::assistant_with_tool_calls(None, tool_calls.clone());
        assert!(matches!(with_tools.role, Role::Assistant));
        assert_eq!(with_tools.content, None);
        assert_eq!(with_tools.tool_calls, Some(tool_calls));

        let tool = Message::tool_result("call-1", "ok");
        assert!(matches!(tool.role, Role::Tool));
        assert_eq!(tool.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(tool.content.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn test_model_getter() {
        let mock_server = MockServer::start().await;
        let client = create_test_client(&mock_server).await;
        assert_eq!(client.model(), "test-model");
    }

    struct Recorder(std::sync::Mutex<Vec<ChatExchange>>);

    impl ChatObserver for Recorder {
        fn observe(&self, exchange: ChatExchange) {
            self.0.lock().unwrap().push(exchange);
        }
    }

    #[tokio::test]
    async fn chat_reports_the_exact_bytes_to_the_observer() {
        let mock_server = MockServer::start().await;
        let body = r#"{"id":"chat-1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"Bonjour"},"finish_reason":"stop"}]}"#;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("x-no-aliasing", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let recorder = std::sync::Arc::new(Recorder(Default::default()));
        let client = create_test_client(&mock_server)
            .await
            .with_no_aliasing(true)
            .with_observer(recorder.clone());
        let reply = client
            .chat(vec![Message::user("Hello")], None, None)
            .await
            .unwrap();
        assert_eq!(reply, "Bonjour");

        let sent = mock_server.received_requests().await.unwrap()[0].body.clone();
        let seen = recorder.0.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].chat_id, "chat-1");
        assert_eq!(seen[0].model, "test-model");
        assert_eq!(seen[0].response, body.as_bytes());
        assert_eq!(seen[0].request, sent);
    }

    #[tokio::test]
    async fn chat_sends_no_aliasing_header_only_when_asked() {
        let mock_server = MockServer::start().await;
        Mock::given(header_exists("x-no-aliasing"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "c", "object": "chat.completion", "created": 1, "model": "m",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}}]
            })))
            .mount(&mock_server)
            .await;
        let client = create_test_client(&mock_server).await;
        client
            .chat(vec![Message::user("hi")], None, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn chat_with_tools_returns_tool_calls() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "1",
                "object": "chat.completion",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client
            .chat_with_tools(vec![Message::user("hi")], None, None, None)
            .await
            .unwrap();
        assert_eq!(result.finish_reason, "tool_calls");
        assert_eq!(result.tool_calls.as_ref().unwrap()[0].id, "call_1");
    }

    #[tokio::test]
    async fn chat_stream_yields_delta_content() {
        let mock_server = MockServer::start().await;
        let chunk = serde_json::json!({
            "id": "1",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "test-model",
            "choices": [{"index": 0, "delta": {"content": "Hi"}, "finish_reason": null}]
        });
        let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let mut stream = client
            .chat_stream(vec![Message::user("hi")], None, None)
            .await
            .unwrap();
        use futures::StreamExt;
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&chunk.unwrap());
        }
        assert_eq!(out, "Hi");
    }

    #[tokio::test]
    async fn health_check_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;
        let client = create_test_client(&mock_server).await;
        assert!(!client.health_check().await);
    }
}
