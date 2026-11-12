//! Signal CLI REST API client.

mod client;
mod error;
mod receiver;
mod types;

pub use client::SignalClient;
pub use error::SignalError;
pub use receiver::MessageReceiver;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn create_test_client(mock_server: &MockServer) -> SignalClient {
        SignalClient::new(mock_server.uri()).unwrap()
    }

    #[tokio::test]
    async fn test_health_check_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        assert!(client.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/health"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        assert!(!client.health_check().await);
    }

    #[tokio::test]
    async fn test_list_accounts() {
        let mock_server = MockServer::start().await;

        let accounts = serde_json::json!(["+15555555555", "+16666666666"]);

        Mock::given(method("GET"))
            .and(path("/v1/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&accounts))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client.list_accounts().await;

        assert!(result.is_ok());
        let accs = result.unwrap();
        assert_eq!(accs.len(), 2);
        assert_eq!(accs[0], "+15555555555");
    }

    #[tokio::test]
    async fn test_receive_messages() {
        let mock_server = MockServer::start().await;

        let messages = serde_json::json!([
            {
                "envelope": {
                    "source": "+14155551234",
                    "sourceNumber": "+14155551234",
                    "sourceName": "Test User",
                    "timestamp": 1677652288000i64,
                    "dataMessage": {
                        "message": "Hello bot!",
                        "timestamp": 1677652288000i64,
                        "groupInfo": null
                    }
                },
                "account": "+15555555555"
            }
        ]);

        // Note: + is URL-encoded as %2B
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15555555555"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&messages))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client.receive("+15555555555").await;

        assert!(result.is_ok());
        let msgs = result.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].envelope.source, "+14155551234");
    }

    #[tokio::test]
    async fn test_send_message() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "timestamp": 1677652288000i64
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client.send("+15555555555", "+14155551234", "Hello!").await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_message_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Invalid recipient"))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client.send("+15555555555", "+14155551234", "Hello!").await;

        assert!(result.is_err());
        assert!(matches!(result, Err(SignalError::SendFailed(_))));
    }

    #[tokio::test]
    async fn test_get_account() {
        let mock_server = MockServer::start().await;

        let account = serde_json::json!({
            "number": "+15555555555",
            "uuid": "test-uuid",
            "registered": true
        });

        // Note: + is URL-encoded as %2B
        Mock::given(method("GET"))
            .and(path("/v1/accounts/%2B15555555555"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&account))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let result = client.get_account("+15555555555").await;

        assert!(result.is_ok());
        let acc = result.unwrap();
        assert_eq!(acc.number, "+15555555555");
        assert!(acc.registered);
    }

    #[tokio::test]
    async fn test_bot_message_from_incoming() {
        let incoming = IncomingMessage {
            envelope: Envelope {
                source: "+14155551234".into(),
                source_number: Some("+14155551234".into()),
                source_uuid: None,
                source_name: Some("Test User".into()),
                timestamp: 1677652288000,
                data_message: Some(DataMessage {
                    message: Some("Hello bot!".into()),
                    timestamp: 1677652288000,
                    group_info: None,
                    attachments: vec![],
                    quote: None,
                }),
                edit_message: None,
            },
            account: "+15555555555".into(),
        };

        let bot_msg = BotMessage::from_incoming(&incoming);
        assert!(bot_msg.is_some());

        let msg = bot_msg.unwrap();
        assert_eq!(msg.source, "+14155551234");
        assert_eq!(msg.text, "Hello bot!");
        assert_eq!(msg.receiving_account, "+15555555555");
        assert!(!msg.is_group);
        assert!(msg.group_id.is_none());
    }

    #[tokio::test]
    async fn test_bot_message_from_group() {
        let incoming = IncomingMessage {
            envelope: Envelope {
                source: "+14155551234".into(),
                source_number: Some("+14155551234".into()),
                source_uuid: None,
                source_name: Some("Test User".into()),
                timestamp: 1677652288000,
                data_message: Some(DataMessage {
                    message: Some("Hello group!".into()),
                    timestamp: 1677652288000,
                    group_info: Some(GroupInfo {
                        group_id: "test-group-id".into(),
                        group_name: None,
                        kind: None,
                    }),
                    attachments: vec![],
                    quote: None,
                }),
                edit_message: None,
            },
            account: "+15555555555".into(),
        };

        let bot_msg = BotMessage::from_incoming(&incoming);
        assert!(bot_msg.is_some());

        let msg = bot_msg.unwrap();
        assert!(msg.is_group);
        assert_eq!(msg.group_id, Some("test-group-id".into()));
        assert_eq!(msg.reply_target(), "test-group-id");
        assert_eq!(msg.receiving_account, "+15555555555");
        assert_eq!(msg.message_timestamp, 1677652288000);
        assert!(!msg.is_voice_note());
    }

    #[tokio::test]
    async fn test_bot_message_no_data_message() {
        let incoming = IncomingMessage {
            envelope: Envelope {
                source: "+14155551234".into(),
                source_number: None,
                source_uuid: None,
                source_name: None,
                timestamp: 1677652288000,
                data_message: None,
                edit_message: None,
            },
            account: "+15555555555".into(),
        };

        let bot_msg = BotMessage::from_incoming(&incoming);
        assert!(bot_msg.is_none());
    }

    #[tokio::test]
    async fn test_download_attachment() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/attachments/test-audio-id"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-audio-bytes"))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let bytes = client.download_attachment("test-audio-id").await.unwrap();
        assert_eq!(bytes, b"fake-audio-bytes");
    }

    #[tokio::test]
    async fn test_send_quoted_reply() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "timestamp": 1677652288000i64
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let original = BotMessage {
            source: "+14155551234".into(),
            source_number: None,
            source_name: None,
            text: "Hola".into(),
            timestamp: 1677652288000,
            message_timestamp: 1677652287000,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15555555555".into(),
            attachments: vec![],
            quote: None,
        };

        client
            .reply_quoted(&original, "Hello", Some("Hola"))
            .await
            .unwrap();
    }

    // Typing on is a PUT, off a DELETE, both naming the chat to show it in.
    #[tokio::test]
    async fn typing_indicator_puts_then_deletes() {
        let mock_server = MockServer::start().await;
        for verb in ["PUT", "DELETE"] {
            Mock::given(method(verb))
                .and(path("/v1/typing-indicator/%2B15555555555"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&mock_server)
                .await;
        }

        let client = create_test_client(&mock_server).await;
        let mut original = BotMessage {
            source: "+14155551234".into(),
            source_number: None,
            source_name: None,
            text: "!verify".into(),
            timestamp: 2,
            message_timestamp: 1,
            is_group: true,
            group_id: Some("group.abc".into()),
            group_name: None,
            receiving_account: "+15555555555".into(),
            attachments: vec![],
            quote: None,
        };
        client.set_typing(&original, true).await.unwrap();
        client.set_typing(&original, false).await.unwrap();

        let bodies: Vec<serde_json::Value> = mock_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(bodies[0]["recipient"], "group.abc");

        // A failed call is an error, so dispatch can log it and carry on.
        original.receiving_account = "+10000000000".into();
        assert!(client.set_typing(&original, true).await.is_err());
    }

    // Styled sends set text_mode; plain sends leave it out, as before styled mode existed.
    #[tokio::test]
    async fn styled_replies_set_text_mode() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let original = BotMessage {
            source: "+14155551234".into(),
            source_number: None,
            source_name: None,
            text: "!verify".into(),
            timestamp: 2,
            message_timestamp: 1,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15555555555".into(),
            attachments: vec![],
            quote: None,
        };
        client
            .reply_with_mode(&original, "**a**", TextMode::Styled)
            .await
            .unwrap();
        client
            .reply_quoted_with_mode(&original, "**b**", None, TextMode::Styled)
            .await
            .unwrap();
        client.reply(&original, "c").await.unwrap();

        let bodies: Vec<serde_json::Value> = mock_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0]["text_mode"], "styled");
        assert_eq!(bodies[1]["text_mode"], "styled");
        assert_eq!(bodies[1]["quote_timestamp"], 1);
        assert!(bodies[2].get("text_mode").is_none());
    }

    #[tokio::test]
    async fn test_resolve_send_recipient_group() {
        let mock_server = MockServer::start().await;

        let groups = serde_json::json!([{
            "name": "testing signal bot",
            "id": "group.TUIzYitaQy85SmtteUpTMEo2ZE9wZ3lib0tOWVZrcDEzNFA3bDU0N1BrOD0=",
            "internal_id": "MB3b+ZC/9JkmyJS0J6dOpgyboKNYVkp134P7l547Pk8="
        }]);

        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&groups))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let message = BotMessage {
            source: "+14155551234".into(),
            source_number: None,
            source_name: None,
            text: "hi".into(),
            timestamp: 1,
            message_timestamp: 1,
            is_group: true,
            group_id: Some("MB3b+ZC/9JkmyJS0J6dOpgyboKNYVkp134P7l547Pk8=".into()),
            group_name: None,
            receiving_account: "+15555555555".into(),
            attachments: vec![],
            quote: None,
        };

        let recipient = client.resolve_send_recipient(&message).await.unwrap();
        assert_eq!(
            recipient,
            "group.TUIzYitaQy85SmtteUpTMEo2ZE9wZ3lib0tOWVZrcDEzNFA3bDU0N1BrOD0="
        );

        // Cached — list_groups should not be called again
        let recipient2 = client.resolve_send_recipient(&message).await.unwrap();
        assert_eq!(recipient2, recipient);
    }

    #[test]
    fn test_bot_message_from_voice_fixture() {
        let fixture = include_str!("../../../docs/spikes/fixtures/voice-note-dm.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();

        assert!(bot_msg.is_voice_note());
        assert!(bot_msg.text.is_empty());
        assert_eq!(bot_msg.source, "+14155559876");
        assert_eq!(bot_msg.message_timestamp, 1719000000000);

        let audio = bot_msg.primary_audio_attachment().unwrap();
        assert_eq!(audio.content_type, "audio/ogg");
        assert_eq!(audio.id, "pwtcq-example-voice-id");
    }

    #[test]
    fn test_bot_message_from_group_voice_fixture() {
        let fixture = include_str!("../../../docs/spikes/fixtures/voice-note-group.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();

        assert!(bot_msg.is_voice_note());
        assert!(bot_msg.is_group);
        assert_eq!(
            bot_msg.group_id.as_deref(),
            Some("MB3b+ZC/9JkmyJS0J6dOpgyboKNYVkp134P7l547Pk8=")
        );
    }

    #[test]
    fn test_bot_message_from_quote_fixture() {
        let fixture = include_str!("../../../docs/spikes/fixtures/text-with-quote-reply.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();

        assert_eq!(bot_msg.text, "!translate es");
        assert!(!bot_msg.is_voice_note());

        let quote = bot_msg.quote.as_ref().unwrap();
        assert_eq!(quote.id, 1718999999000);
        assert_eq!(quote.author_number.as_deref(), Some("+14155559876"));
        assert!(quote.text.as_ref().unwrap().contains("Hola a todos"));
    }

    #[test]
    fn test_bot_message_from_voice_quote_fixture() {
        let fixture = include_str!("../../../docs/spikes/fixtures/voice-with-quote-reply.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();

        assert_eq!(bot_msg.text, "!transcribe");
        assert!(!bot_msg.is_voice_note());

        let quote = bot_msg.quote.as_ref().unwrap();
        assert_eq!(quote.id, 1719000000000);
        let audio = quote.audio_attachment.as_ref().unwrap();
        assert_eq!(audio.id, "pwtcq-example-voice-id");
        assert!(audio.content_type.starts_with("audio/"));
    }

    #[test]
    fn test_bot_message_from_voice_quote_without_attachment_metadata() {
        let fixture =
            include_str!("../../../docs/spikes/fixtures/voice-with-quote-reply-no-attachment.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();

        let quote = bot_msg.quote.as_ref().unwrap();
        assert_eq!(quote.id, 1719000000000);
        assert!(quote.audio_attachment.is_none());
    }

    #[test]
    fn test_quoted_attachment_id_at_root() {
        let fixture =
            include_str!("../../../docs/spikes/fixtures/voice-with-quote-reply-root-id.json");
        let messages: Vec<IncomingMessage> = serde_json::from_str(fixture).unwrap();
        let bot_msg = BotMessage::from_incoming(&messages[0]).unwrap();
        let audio = bot_msg
            .quote
            .as_ref()
            .unwrap()
            .audio_attachment
            .as_ref()
            .unwrap();
        assert_eq!(audio.id, "root-audio-id");
        assert_eq!(audio.content_type, "audio/aac");
    }

    #[tokio::test]
    async fn test_create_group() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/groups/%2B15555555555"))
            .and(body_json(serde_json::json!({
                "name": "Language Thread Spanish",
                "members": ["+14155551234"],
                "description": "Spanish sidecar"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "group.sidecarEs=="
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "name": "Language Thread Spanish",
                    "id": "group.sidecarEs==",
                    "internal_id": "es-internal-id"
                }])),
            )
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let group = client
            .create_group(
                "+15555555555",
                "Language Thread Spanish",
                vec!["+14155551234".into()],
                Some("Spanish sidecar"),
            )
            .await
            .unwrap();

        assert_eq!(group.id, "group.sidecarEs==");
        assert_eq!(group.internal_id, "es-internal-id");
    }

    #[tokio::test]
    async fn test_create_group_retries_list_for_internal_id() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "group.sidecarEs=="
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([]))
                    .append_header("x-attempt", "1"),
            )
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "name": "Language Thread Spanish",
                    "id": "group.sidecarEs==",
                    "internal_id": "es-internal-id"
                }])),
            )
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let group = client
            .create_group(
                "+15555555555",
                "Language Thread Spanish",
                vec!["+14155551234".into()],
                None,
            )
            .await
            .unwrap();

        assert_eq!(group.internal_id, "es-internal-id");
    }

    #[tokio::test]
    async fn test_add_and_remove_members() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path(
                "/v1/groups/%2B15555555555/group.sidecarEs%3D%3D/members",
            ))
            .and(body_json(serde_json::json!({
                "members": ["+14155559876"]
            })))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        Mock::given(method("DELETE"))
            .and(path(
                "/v1/groups/%2B15555555555/group.sidecarEs%3D%3D/members",
            ))
            .and(body_json(serde_json::json!({
                "members": ["+14155559876"]
            })))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        client
            .add_members(
                "+15555555555",
                "group.sidecarEs==",
                vec!["+14155559876".into()],
            )
            .await
            .unwrap();
        client
            .remove_members(
                "+15555555555",
                "group.sidecarEs==",
                vec!["+14155559876".into()],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_update_group() {
        let mock_server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/v1/groups/%2B15555555555/group.sidecarEs%3D%3D"))
            .and(body_json(serde_json::json!({
                "name": "Spanish · Stacked"
            })))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        client
            .update_group("+15555555555", "group.sidecarEs==", "Spanish · Stacked")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_join_group() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/groups/%2B15555555555/group.pending%3D%3D/join"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        client
            .join_group("+15555555555", "group.pending==")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_trust_identity() {
        let mock_server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/v1/identities/%2B15550001111/trust/%2B15550002222"))
            .and(body_json(
                serde_json::json!({ "trust_all_known_keys": true }),
            ))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        client
            .trust_identity("+15550001111", "+15550002222")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_create_group_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/groups/%2B15555555555"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad members"))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let err = client
            .create_group("+15555555555", "Language Thread English", vec![], None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bad members"));
    }

    #[tokio::test]
    async fn test_set_username() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/accounts/%2B15555555555/username"))
            .and(body_json(serde_json::json!({ "username": "sigstack" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "username": "sigstack.54",
                "username_link": "https://signal.me/#eu/abc"
            })))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let info = client
            .set_username("+15555555555", "sigstack")
            .await
            .unwrap();
        assert_eq!(info.username.as_deref(), Some("sigstack.54"));
        assert_eq!(
            info.username_link.as_deref(),
            Some("https://signal.me/#eu/abc")
        );
    }

    #[tokio::test]
    async fn test_set_username_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/accounts/%2B15555555555/username"))
            .respond_with(ResponseTemplate::new(400).set_body_string("taken"))
            .mount(&mock_server)
            .await;

        let client = create_test_client(&mock_server).await;
        let err = client
            .set_username("+15555555555", "sigstack")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("taken"));
    }

    #[test]
    fn test_bot_message_source_fields() {
        let incoming = IncomingMessage {
            envelope: Envelope {
                source: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                source_number: Some("+14155551234".into()),
                source_uuid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
                source_name: Some("Maria".into()),
                timestamp: 1,
                data_message: Some(DataMessage {
                    message: Some("hola".into()),
                    timestamp: 1,
                    group_info: None,
                    attachments: vec![],
                    quote: None,
                }),
                edit_message: None,
            },
            account: "+15555555555".into(),
        };
        let msg = BotMessage::from_incoming(&incoming).unwrap();
        assert_eq!(msg.source_name.as_deref(), Some("Maria"));
        assert_eq!(msg.source_number.as_deref(), Some("+14155551234"));
        assert_eq!(msg.invite_address().as_deref(), Some("+14155551234"));
        assert_eq!(msg.display_name(), "Maria");
    }
}
