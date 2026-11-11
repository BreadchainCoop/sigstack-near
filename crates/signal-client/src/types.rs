//! Signal API types.

use serde::{Deserialize, Serialize};

/// Identity row from `GET /v1/identities/{number}`.
#[derive(Debug, Clone, Deserialize)]
pub struct IdentityEntry {
    #[serde(default)]
    pub number: String,
    pub status: String,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub safety_number: Option<String>,
}

/// Incoming Signal message.
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingMessage {
    pub envelope: Envelope,
    pub account: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub source: String,
    #[serde(rename = "sourceNumber")]
    pub source_number: Option<String>,
    #[serde(rename = "sourceUuid")]
    pub source_uuid: Option<String>,
    #[serde(rename = "sourceName")]
    pub source_name: Option<String>,
    pub timestamp: i64,
    #[serde(rename = "dataMessage")]
    pub data_message: Option<DataMessage>,
    /// In-place edit. signal-cli delivers this instead of top-level `dataMessage`.
    #[serde(rename = "editMessage", default)]
    pub edit_message: Option<EditMessage>,
}

/// Signal edit envelope (`envelope.editMessage`).
#[derive(Debug, Clone, Deserialize)]
pub struct EditMessage {
    #[serde(rename = "targetSentTimestamp")]
    pub target_sent_timestamp: i64,
    #[serde(rename = "dataMessage")]
    pub data_message: DataMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataMessage {
    pub message: Option<String>,
    pub timestamp: i64,
    #[serde(rename = "groupInfo")]
    pub group_info: Option<GroupInfo>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub quote: Option<Quote>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupInfo {
    /// Internal group ID on incoming messages (`internal_id` from list groups).
    #[serde(rename = "groupId")]
    pub group_id: String,
    #[serde(rename = "groupName")]
    pub group_name: Option<String>,
}

/// Group from `GET /v1/groups/{number}` — use `id` (not `internal_id`) for `/v2/send`.
#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    pub name: String,
    pub id: String,
    #[serde(rename = "internal_id")]
    pub internal_id: String,
    /// Active members (E.164 phones and/or UUIDs), when the API includes them.
    #[serde(default)]
    pub members: Vec<String>,
    /// Pending invites (not yet accepted).
    #[serde(default)]
    pub pending_invites: Vec<String>,
    /// Pending join requests.
    #[serde(default)]
    pub pending_requests: Vec<String>,
    /// Group admins.
    #[serde(default)]
    pub admins: Vec<String>,
}

impl Group {
    /// True if `identity` (phone or UUID) appears in members or pending invites/requests.
    pub fn contains_member_or_pending(&self, identity: &str) -> bool {
        self.members
            .iter()
            .chain(self.pending_invites.iter())
            .chain(self.pending_requests.iter())
            .any(|m| identities_match(m, identity))
    }

    /// True if `identity` has been invited or has a pending join request (not yet a member).
    pub fn is_pending_for(&self, identity: &str) -> bool {
        self.pending_invites
            .iter()
            .chain(self.pending_requests.iter())
            .any(|m| identities_match(m, identity))
    }

    /// True if `identity` is an active member or admin.
    pub fn has_member_or_admin(&self, identity: &str) -> bool {
        self.members
            .iter()
            .chain(self.admins.iter())
            .any(|m| identities_match(m, identity))
    }
}

/// Compare Signal identities: exact match, or digit-only match for E.164 phones.
pub fn identities_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let da = digits_only(a);
    let db = digits_only(b);
    !da.is_empty() && da == db
}

fn digits_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Request body for `POST /v1/groups/{number}`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Response from create group.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateGroupResponse {
    pub id: String,
}

/// Request body for add/remove group members.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeGroupMembersRequest {
    pub members: Vec<String>,
}

/// Request body for `PUT /v1/groups/{number}/{groupid}`.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateGroupRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Attachment {
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub filename: Option<String>,
    pub id: String,
    pub size: Option<i64>,
    #[serde(rename = "uploadTimestamp")]
    pub upload_timestamp: Option<i64>,
}

impl Attachment {
    /// Whether this attachment is an audio/voice note.
    pub fn is_audio(&self) -> bool {
        self.content_type.starts_with("audio/")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    pub id: i64,
    pub author: Option<String>,
    #[serde(rename = "authorNumber")]
    pub author_number: Option<String>,
    #[serde(rename = "authorUuid")]
    pub author_uuid: Option<String>,
    pub text: Option<String>,
    #[serde(default)]
    pub attachments: Vec<QuotedAttachment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotedAttachment {
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub filename: Option<String>,
    pub id: Option<String>,
    pub size: Option<i64>,
    #[serde(rename = "uploadTimestamp")]
    pub upload_timestamp: Option<i64>,
    pub thumbnail: Option<Attachment>,
}

/// Quote metadata on an incoming message (user replied to another message).
#[derive(Debug, Clone)]
pub struct QuotedMessage {
    pub id: i64,
    pub author_number: Option<String>,
    pub text: Option<String>,
    /// Audio attachment from the quoted message (voice notes), when Signal includes it.
    pub audio_attachment: Option<Attachment>,
}

/// Outgoing message request (legacy shape; prefer [`SendMessageV2Request`]).
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub message: String,
    pub number: Option<String>,
    pub recipients: Option<Vec<String>>,
}

/// How signal-cli-rest-api renders an outgoing message's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextMode {
    /// Literal text, the API default.
    #[default]
    Normal,
    /// `**bold**`, `*italic*`, `~strikethrough~`, `||spoiler||` and `` `monospace` ``
    /// become Signal text styles. Only for text the bot writes itself: user or model
    /// text could contain the markers and be misrendered.
    Styled,
}

/// Outgoing message request for `/v2/send` (text + optional quote-reply).
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageV2Request {
    pub message: String,
    pub number: String,
    pub recipients: Vec<String>,
    /// Omitted for [`TextMode::Normal`], so plain sends are unchanged on the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_mode: Option<TextMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_message: Option<String>,
}

/// Send message response.
#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    pub timestamp: Option<i64>,
}

/// Account information.
#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub number: String,
    pub uuid: Option<String>,
    pub registered: bool,
}

/// Response from `POST /v1/accounts/{number}/username`.
#[derive(Debug, Clone, Deserialize)]
pub struct UsernameInfo {
    pub username: Option<String>,
    pub username_link: Option<String>,
}

/// Parsed message for bot processing.
#[derive(Debug, Clone)]
pub struct BotMessage {
    /// Sender id from the envelope (`source` — often UUID).
    pub source: String,
    /// E.164 phone when Signal includes it.
    pub source_number: Option<String>,
    /// Display name when Signal includes it.
    pub source_name: Option<String>,
    /// The message text (empty for voice-only messages).
    pub text: String,
    /// Envelope timestamp (milliseconds).
    pub timestamp: i64,
    /// `dataMessage.timestamp` — use for outbound `quote_timestamp`.
    pub message_timestamp: i64,
    /// Whether this is a group message.
    pub is_group: bool,
    /// Group ID if this is a group message.
    pub group_id: Option<String>,
    /// Optional group display name from `groupInfo`.
    pub group_name: Option<String>,
    /// The bot's phone number that received this message.
    pub receiving_account: String,
    /// Attachments on this message (voice notes, etc.).
    pub attachments: Vec<Attachment>,
    /// Quote/reply metadata if the sender quoted another message.
    pub quote: Option<QuotedMessage>,
}

impl BotMessage {
    /// Extract bot message from incoming envelope.
    ///
    /// Returns `Some` for text messages and voice notes (audio attachments).
    /// Edits (`editMessage`) are accepted only when the new body is a `!` command.
    pub fn from_incoming(msg: &IncomingMessage) -> Option<Self> {
        let from_edit = msg.envelope.data_message.is_none();
        let data = msg.envelope.data_message.as_ref().or_else(|| {
            msg.envelope
                .edit_message
                .as_ref()
                .map(|edit| &edit.data_message)
        })?;
        if from_edit
            && !data
                .message
                .as_deref()
                .unwrap_or("")
                .trim()
                .starts_with('!')
        {
            return None;
        }
        let has_text = data.message.as_ref().is_some_and(|text| !text.is_empty());
        let has_audio = data.attachments.iter().any(Attachment::is_audio);

        if !has_text && !has_audio {
            return None;
        }

        let quote = data.quote.as_ref().map(|q| QuotedMessage {
            id: q.id,
            author_number: q.author_number.clone().or_else(|| q.author.clone()),
            text: q.text.clone(),
            audio_attachment: quoted_audio_attachment(q),
        });

        Some(Self {
            source: msg.envelope.source.clone(),
            source_number: msg.envelope.source_number.clone(),
            source_name: msg.envelope.source_name.clone(),
            text: data.message.clone().unwrap_or_default(),
            timestamp: msg.envelope.timestamp,
            message_timestamp: data.timestamp,
            is_group: data.group_info.is_some(),
            group_id: data.group_info.as_ref().map(|g| g.group_id.clone()),
            group_name: data.group_info.as_ref().and_then(|g| g.group_name.clone()),
            receiving_account: msg.account.clone(),
            attachments: data.attachments.clone(),
            quote,
        })
    }

    /// Best address for group invite (`members[]`): phone, else source if it looks usable.
    pub fn invite_address(&self) -> Option<String> {
        if let Some(n) = &self.source_number {
            if n.starts_with('+') {
                return Some(n.clone());
            }
        }
        if self.source.starts_with('+') {
            return Some(self.source.clone());
        }
        // UUID may work for some signal-cli versions; prefer when no phone.
        if self.source.contains('-') && self.source.len() >= 32 {
            return Some(self.source.clone());
        }
        None
    }

    /// Display name for attribution in bridged messages.
    pub fn display_name(&self) -> String {
        self.source_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                let s = self.source_number.as_deref().unwrap_or(&self.source);
                if s.chars().count() > 16 {
                    format!("{}…", s.chars().take(12).collect::<String>())
                } else {
                    s.to_string()
                }
            })
    }

    /// Whether this message is a voice note (has at least one audio attachment).
    pub fn is_voice_note(&self) -> bool {
        self.attachments.iter().any(Attachment::is_audio)
    }

    /// Audio attachments only (voice notes).
    pub fn audio_attachments(&self) -> impl Iterator<Item = &Attachment> {
        self.attachments.iter().filter(|a| a.is_audio())
    }

    /// Primary audio attachment, if any (first audio attachment).
    pub fn primary_audio_attachment(&self) -> Option<&Attachment> {
        self.audio_attachments().next()
    }

    /// Raw reply target from the envelope (DM source or group `internal_id`).
    ///
    /// For group sends, resolve via [`SignalClient::resolve_send_recipient`] — `/v2/send`
    /// requires the `group.*` id from list groups, not `groupInfo.groupId` on receive.
    pub fn reply_target(&self) -> &str {
        self.group_id.as_deref().unwrap_or(&self.source)
    }

    /// Author identifier for quoting this message (`source` phone number).
    pub fn quote_author(&self) -> &str {
        &self.source
    }
}

fn quoted_audio_attachment(quote: &Quote) -> Option<Attachment> {
    for quoted in &quote.attachments {
        if let Some(audio) = quoted_attachment_as_audio(quoted) {
            return Some(audio);
        }
    }
    None
}

fn quoted_attachment_as_audio(quoted: &QuotedAttachment) -> Option<Attachment> {
    if let Some(thumb) = &quoted.thumbnail {
        if thumb.content_type.starts_with("audio/") && !thumb.id.is_empty() {
            return Some(thumb.clone());
        }
    }

    if let Some(id) = &quoted.id {
        if id.is_empty() {
            return None;
        }
        let content_type = quoted
            .content_type
            .as_deref()
            .filter(|ct| ct.starts_with("audio/"))
            .unwrap_or("audio/ogg");
        return Some(Attachment {
            content_type: content_type.to_string(),
            filename: quoted.filename.clone(),
            id: id.clone(),
            size: quoted.size,
            upload_timestamp: quoted.upload_timestamp,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_deserializes_members_and_pending() {
        let g: Group = serde_json::from_value(serde_json::json!({
            "name": "Main",
            "id": "group.abc=",
            "internal_id": "internal-1",
            "members": ["+15551110001", "uuid-x"],
            "pending_invites": ["+15551110002"],
            "pending_requests": [],
            "admins": ["+15551110001"]
        }))
        .unwrap();
        assert_eq!(g.members.len(), 2);
        assert!(g.contains_member_or_pending("+15551110001"));
        assert!(g.contains_member_or_pending("15551110002")); // digit match on pending
        assert!(!g.contains_member_or_pending("+19999999999"));
        assert!(g.is_pending_for("+15551110002"));
        assert!(!g.is_pending_for("+15551110001"));
        assert!(g.has_member_or_admin("+15551110001"));
        assert!(!g.has_member_or_admin("+15551110002"));
    }

    #[test]
    fn group_members_default_empty_when_omitted() {
        let g: Group = serde_json::from_value(serde_json::json!({
            "name": "Main",
            "id": "group.abc=",
            "internal_id": "internal-1"
        }))
        .unwrap();
        assert!(g.members.is_empty());
        assert!(g.pending_invites.is_empty());
    }

    #[test]
    fn identities_match_phones_and_exact_uuid() {
        assert!(identities_match("+15551234567", "15551234567"));
        assert!(identities_match("uuid-abc", "uuid-abc"));
        assert!(!identities_match("uuid-abc", "uuid-xyz"));
    }

    fn incoming(
        data_message: Option<DataMessage>,
        edit_message: Option<EditMessage>,
    ) -> IncomingMessage {
        IncomingMessage {
            envelope: Envelope {
                source: "+14155551234".into(),
                source_number: Some("+14155551234".into()),
                source_uuid: None,
                source_name: Some("Test User".into()),
                timestamp: 1677652288000,
                data_message,
                edit_message,
            },
            account: "+15555555555".into(),
        }
    }

    fn text_data(message: &str, timestamp: i64) -> DataMessage {
        DataMessage {
            message: Some(message.into()),
            timestamp,
            group_info: None,
            attachments: vec![],
            quote: None,
        }
    }

    #[test]
    fn edit_command_becomes_bot_message() {
        let incoming = incoming(
            None,
            Some(EditMessage {
                target_sent_timestamp: 1000,
                data_message: text_data("!help-threads", 1001),
            }),
        );
        let msg = BotMessage::from_incoming(&incoming).expect("edit command");
        assert_eq!(msg.text, "!help-threads");
        assert_eq!(msg.message_timestamp, 1001);
        assert_eq!(msg.receiving_account, "+15555555555");
    }

    #[test]
    fn edit_chat_without_command_is_dropped() {
        let incoming = incoming(
            None,
            Some(EditMessage {
                target_sent_timestamp: 1000,
                data_message: text_data("hola", 1001),
            }),
        );
        assert!(BotMessage::from_incoming(&incoming).is_none());
    }

    #[test]
    fn data_message_preferred_over_edit_without_command_gate() {
        let incoming = incoming(
            Some(text_data("hello", 1000)),
            Some(EditMessage {
                target_sent_timestamp: 1000,
                data_message: text_data("!help-threads", 1001),
            }),
        );
        let msg = BotMessage::from_incoming(&incoming).expect("new send");
        assert_eq!(msg.text, "hello");
        assert_eq!(msg.message_timestamp, 1000);
    }

    #[test]
    fn empty_edit_is_dropped() {
        let incoming = incoming(
            None,
            Some(EditMessage {
                target_sent_timestamp: 1000,
                data_message: text_data("", 1001),
            }),
        );
        assert!(BotMessage::from_incoming(&incoming).is_none());
    }

    #[test]
    fn edit_command_keeps_group_and_quote() {
        let incoming = incoming(
            None,
            Some(EditMessage {
                target_sent_timestamp: 1000,
                data_message: DataMessage {
                    message: Some("!help-threads".into()),
                    timestamp: 1001,
                    group_info: Some(GroupInfo {
                        group_id: "test-group-id".into(),
                        group_name: Some("Main".into()),
                    }),
                    attachments: vec![],
                    quote: Some(Quote {
                        id: 42,
                        author: None,
                        author_number: Some("+14155550000".into()),
                        author_uuid: None,
                        text: Some("prior".into()),
                        attachments: vec![],
                    }),
                },
            }),
        );
        let msg = BotMessage::from_incoming(&incoming).expect("group edit command");
        assert!(msg.is_group);
        assert_eq!(msg.group_id.as_deref(), Some("test-group-id"));
        assert_eq!(msg.group_name.as_deref(), Some("Main"));
        let quote = msg.quote.expect("quote");
        assert_eq!(quote.id, 42);
        assert_eq!(quote.author_number.as_deref(), Some("+14155550000"));
        assert_eq!(quote.text.as_deref(), Some("prior"));
    }

    #[test]
    fn edit_message_deserializes_signal_cli_shape() {
        let incoming: IncomingMessage = serde_json::from_value(serde_json::json!({
            "envelope": {
                "source": "+14155551234",
                "timestamp": 1000,
                "editMessage": {
                    "targetSentTimestamp": 1000,
                    "dataMessage": {
                        "message": "!privacy",
                        "timestamp": 1001,
                        "attachments": []
                    }
                }
            },
            "account": "+15555555555"
        }))
        .unwrap();
        let msg = BotMessage::from_incoming(&incoming).expect("json edit command");
        assert_eq!(msg.text, "!privacy");
        assert_eq!(
            incoming
                .envelope
                .edit_message
                .as_ref()
                .map(|e| e.target_sent_timestamp),
            Some(1000)
        );
    }
}
