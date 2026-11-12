//! Signal HTTP client.

use crate::error::SignalError;
use crate::types::*;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, instrument, warn};
use urlencoding::encode;

type GroupCache = Arc<RwLock<HashMap<(String, String), String>>>;

/// Signal CLI REST API client.
///
/// Supports multi-account operations - can send/receive for any registered account.
#[derive(Clone)]
pub struct SignalClient {
    client: Client,
    base_url: String,
    group_cache: GroupCache,
}

impl SignalClient {
    /// Create a new Signal client.
    pub fn new(base_url: impl Into<String>) -> Result<Self, SignalError> {
        let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

        Ok(Self {
            client,
            base_url: base_url.into(),
            group_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// List all registered accounts.
    #[instrument(skip(self))]
    pub async fn list_accounts(&self) -> Result<Vec<String>, SignalError> {
        let response = self
            .client
            .get(format!("{}/v1/accounts", self.base_url))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        let accounts: Vec<String> = response.json().await?;
        debug!("Found {} registered accounts", accounts.len());
        Ok(accounts)
    }

    /// List Signal groups for an account.
    #[instrument(skip(self))]
    pub async fn list_groups(&self, phone_number: &str) -> Result<Vec<Group>, SignalError> {
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .get(format!("{}/v1/groups/{}", self.base_url, encoded_number))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        let groups: Vec<Group> = response.json().await?;
        debug!("Found {} groups for {}", groups.len(), phone_number);
        Ok(groups)
    }

    /// Create a Signal group. Returns the send id (`group.…`).
    #[instrument(skip(self, members))]
    pub async fn create_group(
        &self,
        phone_number: &str,
        name: &str,
        members: Vec<String>,
        description: Option<&str>,
    ) -> Result<Group, SignalError> {
        let encoded_number = encode(phone_number);
        let body = CreateGroupRequest {
            name: name.to_string(),
            members,
            description: description.map(str::to_string),
        };
        let response = self
            .client
            .post(format!("{}/v1/groups/{}", self.base_url, encoded_number))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        let created: CreateGroupResponse = response.json().await?;
        const LIST_RETRIES: u32 = 5;
        const LIST_BACKOFF_MS: u64 = 200;

        let mut group = Group {
            name: name.to_string(),
            id: created.id.clone(),
            internal_id: created.id.clone(),
            members: vec![],
            pending_invites: vec![],
            pending_requests: vec![],
            admins: vec![],
        };

        for attempt in 0..LIST_RETRIES {
            let groups = self.list_groups(phone_number).await?;
            if let Some(found) = groups.into_iter().find(|g| g.id == created.id) {
                group = found;
                if group.internal_id != group.id {
                    break;
                }
            }
            if attempt + 1 < LIST_RETRIES {
                tokio::time::sleep(Duration::from_millis(LIST_BACKOFF_MS)).await;
            }
        }

        self.cache_group_mapping(phone_number, &group).await;
        debug!(
            "Created group {} (internal {}) for {}",
            group.id, group.internal_id, phone_number
        );
        Ok(group)
    }

    /// Add members to an existing group (`group.…` send id).
    #[instrument(skip(self, members))]
    pub async fn add_members(
        &self,
        phone_number: &str,
        group_send_id: &str,
        members: Vec<String>,
    ) -> Result<(), SignalError> {
        self.change_members(phone_number, group_send_id, members, true)
            .await
    }

    /// Remove members from an existing group.
    #[instrument(skip(self, members))]
    pub async fn remove_members(
        &self,
        phone_number: &str,
        group_send_id: &str,
        members: Vec<String>,
    ) -> Result<(), SignalError> {
        self.change_members(phone_number, group_send_id, members, false)
            .await
    }

    /// Update a group's display name (`PUT /v1/groups/{number}/{groupid}`).
    #[instrument(skip(self))]
    pub async fn update_group(
        &self,
        phone_number: &str,
        group_send_id: &str,
        name: &str,
    ) -> Result<(), SignalError> {
        let encoded_number = encode(phone_number);
        let encoded_group = encode(group_send_id);
        let body = UpdateGroupRequest {
            name: Some(name.to_string()),
        };
        let response = self
            .client
            .put(format!(
                "{}/v1/groups/{}/{}",
                self.base_url, encoded_number, encoded_group
            ))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }
        Ok(())
    }

    /// Accept a pending group invite (`POST /v1/groups/{number}/{groupid}/join`).
    #[instrument(skip(self))]
    pub async fn join_group(
        &self,
        phone_number: &str,
        group_send_id: &str,
    ) -> Result<(), SignalError> {
        let encoded_number = encode(phone_number);
        let encoded_group = encode(group_send_id);
        let response = self
            .client
            .post(format!(
                "{}/v1/groups/{}/{}/join",
                self.base_url, encoded_number, encoded_group
            ))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        debug!("Joined group {} as {}", group_send_id, phone_number);
        Ok(())
    }

    async fn change_members(
        &self,
        phone_number: &str,
        group_send_id: &str,
        members: Vec<String>,
        add: bool,
    ) -> Result<(), SignalError> {
        let encoded_number = encode(phone_number);
        let encoded_group = encode(group_send_id);
        let url = format!(
            "{}/v1/groups/{}/{}/members",
            self.base_url, encoded_number, encoded_group
        );
        let body = ChangeGroupMembersRequest { members };
        let request = if add {
            self.client.post(&url).json(&body)
        } else {
            self.client.delete(&url).json(&body)
        };
        let response = request.send().await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }
        Ok(())
    }

    async fn cache_group_mapping(&self, phone_number: &str, group: &Group) {
        let key = (phone_number.to_string(), group.internal_id.clone());
        self.group_cache.write().await.insert(key, group.id.clone());
    }

    /// Resolve the recipient id for `/v2/send` (DM source or `group.*` id).
    #[instrument(skip(self))]
    pub async fn resolve_send_recipient(
        &self,
        message: &BotMessage,
    ) -> Result<String, SignalError> {
        let Some(group_id) = message.group_id.as_deref() else {
            return Ok(message.source.clone());
        };

        if group_id.starts_with("group.") {
            return Ok(group_id.to_string());
        }

        let cache_key = (message.receiving_account.clone(), group_id.to_string());
        if let Some(send_id) = self.group_cache.read().await.get(&cache_key) {
            return Ok(send_id.clone());
        }

        let groups = self.list_groups(&message.receiving_account).await?;
        let send_id = match resolve_group_send_id(group_id, &groups) {
            Some(id) => id,
            None => {
                return Err(SignalError::Api(format!(
                    "Unknown group id {group_id} for account {}",
                    message.receiving_account
                )));
            }
        };

        self.group_cache
            .write()
            .await
            .insert(cache_key, send_id.clone());

        debug!("Resolved group internal_id {} -> {}", group_id, send_id);
        Ok(send_id)
    }

    /// Resolve `internal_id` (or pass-through `group.…`) to send id for an account.
    pub async fn resolve_group_send_id_for_account(
        &self,
        phone_number: &str,
        group_id: &str,
    ) -> Result<String, SignalError> {
        if group_id.starts_with("group.") {
            return Ok(group_id.to_string());
        }

        let cache_key = (phone_number.to_string(), group_id.to_string());
        if let Some(send_id) = self.group_cache.read().await.get(&cache_key) {
            return Ok(send_id.clone());
        }

        let groups = self.list_groups(phone_number).await?;
        let send_id = resolve_group_send_id(group_id, &groups).ok_or_else(|| {
            SignalError::Api(format!(
                "Unknown group id {group_id} for account {phone_number}"
            ))
        })?;

        self.group_cache
            .write()
            .await
            .insert(cache_key, send_id.clone());
        Ok(send_id)
    }

    /// Check if the Signal API is healthy.
    pub async fn health_check(&self) -> bool {
        self.client
            .get(format!("{}/v1/health", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Get account information for a specific phone number.
    #[instrument(skip(self))]
    pub async fn get_account(&self, phone_number: &str) -> Result<Account, SignalError> {
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .get(format!("{}/v1/accounts/{}", self.base_url, encoded_number))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        Ok(response.json().await?)
    }

    /// Receive pending messages for a specific phone number.
    #[instrument(skip(self))]
    pub async fn receive(&self, phone_number: &str) -> Result<Vec<IncomingMessage>, SignalError> {
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .get(format!("{}/v1/receive/{}", self.base_url, encoded_number))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        let messages: Vec<IncomingMessage> = response.json().await?;
        debug!("Received {} messages for {}", messages.len(), phone_number);
        Ok(messages)
    }

    /// Trust a peer identity (`PUT /v1/identities/{number}/trust/{numberToTrust}`).
    ///
    /// Uses `trust_all_known_keys` so paired product bots can exchange group messages after
    /// re-registration (safety-number change).
    #[instrument(skip(self))]
    pub async fn trust_identity(
        &self,
        phone_number: &str,
        number_to_trust: &str,
    ) -> Result<(), SignalError> {
        let encoded_number = encode(phone_number);
        let encoded_peer = encode(number_to_trust);
        let response = self
            .client
            .put(format!(
                "{}/v1/identities/{}/trust/{}",
                self.base_url, encoded_number, encoded_peer
            ))
            .json(&serde_json::json!({ "trust_all_known_keys": true }))
            .send()
            .await?;

        let status = response.status();
        if status.is_success() || status.as_u16() == 204 {
            return Ok(());
        }
        let msg = response.text().await.unwrap_or_default();
        Err(SignalError::Api(format!(
            "Trust identity failed ({status}): {msg}"
        )))
    }

    /// List known identities for an account (`GET /v1/identities/{number}`).
    #[instrument(skip(self))]
    pub async fn list_identities(
        &self,
        phone_number: &str,
    ) -> Result<Vec<IdentityEntry>, SignalError> {
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .get(format!(
                "{}/v1/identities/{}",
                self.base_url, encoded_number
            ))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(msg));
        }

        Ok(response.json().await?)
    }

    /// Download attachment bytes by ID (auto-downloaded during receive).
    #[instrument(skip(self))]
    pub async fn download_attachment(&self, attachment_id: &str) -> Result<Vec<u8>, SignalError> {
        let encoded_id = encode(attachment_id);
        let response = self
            .client
            .get(format!("{}/v1/attachments/{}", self.base_url, encoded_id))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::AttachmentDownloadFailed(msg));
        }

        let bytes = response.bytes().await?.to_vec();
        debug!(
            "Downloaded attachment {} ({} bytes)",
            attachment_id,
            bytes.len()
        );
        Ok(bytes)
    }

    /// Send a message from a specific account to a recipient.
    #[instrument(skip(self, message))]
    pub async fn send(
        &self,
        from_number: &str,
        recipient: &str,
        message: &str,
    ) -> Result<(), SignalError> {
        self.send_with_mode(from_number, recipient, message, TextMode::Normal)
            .await
    }

    /// [`Self::send`] with an explicit [`TextMode`].
    #[instrument(skip(self, message))]
    pub async fn send_with_mode(
        &self,
        from_number: &str,
        recipient: &str,
        message: &str,
        mode: TextMode,
    ) -> Result<(), SignalError> {
        self.send_v2(SendMessageV2Request {
            message: message.to_string(),
            number: from_number.to_string(),
            recipients: vec![recipient.to_string()],
            text_mode: text_mode_field(mode),
            quote_timestamp: None,
            quote_author: None,
            quote_message: None,
        })
        .await
    }

    /// Send a message via `/v2/send` with optional quote-reply fields.
    #[instrument(skip(self, request))]
    pub async fn send_v2(&self, request: SendMessageV2Request) -> Result<(), SignalError> {
        let response = self
            .client
            .post(format!("{}/v2/send", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            warn!("Send failed: {}", msg);
            return Err(SignalError::SendFailed(msg));
        }

        debug!(
            "Sent message from {} to {:?}",
            request.number, request.recipients
        );
        Ok(())
    }

    /// Shows (`true`) or clears (`false`) the bot's typing indicator in the chat that
    /// `original` came from.
    pub async fn set_typing(&self, original: &BotMessage, typing: bool) -> Result<(), SignalError> {
        let recipient = self.resolve_send_recipient(original).await?;
        let url = format!(
            "{}/v1/typing-indicator/{}",
            self.base_url,
            encode(&original.receiving_account)
        );
        let request = if typing {
            self.client.put(url)
        } else {
            self.client.delete(url)
        };
        let response = request
            .json(&serde_json::json!({ "recipient": recipient }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(SignalError::SendFailed(
                response.text().await.unwrap_or_default(),
            ));
        }
        Ok(())
    }

    /// Reply to a message (handles both direct and group messages).
    /// Uses the receiving account to send the reply.
    pub async fn reply(&self, original: &BotMessage, message: &str) -> Result<(), SignalError> {
        self.reply_with_mode(original, message, TextMode::Normal)
            .await
    }

    /// [`Self::reply`] with an explicit [`TextMode`].
    pub async fn reply_with_mode(
        &self,
        original: &BotMessage,
        message: &str,
        mode: TextMode,
    ) -> Result<(), SignalError> {
        let recipient = self.resolve_send_recipient(original).await?;
        self.send_with_mode(&original.receiving_account, &recipient, message, mode)
            .await
    }

    /// Quote-reply to a message (threads bot output to the original).
    #[instrument(skip(self, message))]
    pub async fn reply_quoted(
        &self,
        original: &BotMessage,
        message: &str,
        quote_snippet: Option<&str>,
    ) -> Result<(), SignalError> {
        self.reply_quoted_with_mode(original, message, quote_snippet, TextMode::Normal)
            .await
    }

    /// [`Self::reply_quoted`] with an explicit [`TextMode`].
    #[instrument(skip(self, message))]
    pub async fn reply_quoted_with_mode(
        &self,
        original: &BotMessage,
        message: &str,
        quote_snippet: Option<&str>,
        mode: TextMode,
    ) -> Result<(), SignalError> {
        let snippet = quote_snippet
            .map(str::to_string)
            .or_else(|| quote_snippet_for_message(original));
        let recipient = self.resolve_send_recipient(original).await?;

        self.send_v2(SendMessageV2Request {
            message: message.to_string(),
            number: original.receiving_account.clone(),
            recipients: vec![recipient],
            text_mode: text_mode_field(mode),
            quote_timestamp: Some(original.message_timestamp),
            quote_author: Some(original.quote_author().to_string()),
            quote_message: snippet,
        })
        .await
    }

    /// Quote-reply to a specific message (e.g. the message quoted by the user's command).
    #[instrument(skip(self, message))]
    pub async fn reply_quoted_target(
        &self,
        context: &BotMessage,
        quote_timestamp: i64,
        quote_author: &str,
        quote_snippet: Option<&str>,
        message: &str,
    ) -> Result<(), SignalError> {
        let recipient = self.resolve_send_recipient(context).await?;

        self.send_v2(SendMessageV2Request {
            message: message.to_string(),
            number: context.receiving_account.clone(),
            recipients: vec![recipient],
            text_mode: None,
            quote_timestamp: Some(quote_timestamp),
            quote_author: Some(quote_author.to_string()),
            quote_message: quote_snippet.map(str::to_string),
        })
        .await
    }

    /// Set (or refresh) a Signal username for an account.
    ///
    /// Pass a nickname (e.g. `sigstack`); Signal assigns a discriminator
    /// (`sigstack.54`) and returns a shareable `username_link`.
    #[instrument(skip(self))]
    pub async fn set_username(
        &self,
        phone_number: &str,
        nickname: &str,
    ) -> Result<UsernameInfo, SignalError> {
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .post(format!(
                "{}/v1/accounts/{}/username",
                self.base_url, encoded_number
            ))
            .json(&serde_json::json!({ "username": nickname }))
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(format!("Set username failed: {msg}")));
        }

        Ok(response.json().await?)
    }

    /// Set the account's Signal profile: display name, optional about line, and optional
    /// picture (`avatar` is the bytes of a JPEG or PNG file). Signal keeps the profile, so
    /// this only needs to succeed once per change.
    #[instrument(skip(self, avatar), fields(avatar_bytes = avatar.map(<[u8]>::len)))]
    pub async fn set_profile(
        &self,
        phone_number: &str,
        name: &str,
        about: Option<&str>,
        avatar: Option<&[u8]>,
    ) -> Result<(), SignalError> {
        use base64::Engine as _;

        let mut body = serde_json::json!({ "name": name });
        if let Some(about) = about {
            body["about"] = about.into();
        }
        if let Some(avatar) = avatar {
            body["base64_avatar"] = base64::engine::general_purpose::STANDARD
                .encode(avatar)
                .into();
        }
        let encoded_number = encode(phone_number);
        let response = self
            .client
            .put(format!("{}/v1/profiles/{}", self.base_url, encoded_number))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            return Err(SignalError::Api(format!("Set profile failed: {msg}")));
        }
        Ok(())
    }
}

/// Map incoming `groupInfo.groupId` (`internal_id`) to list-groups `id` for send.
pub fn resolve_group_send_id(incoming_group_id: &str, groups: &[Group]) -> Option<String> {
    if incoming_group_id.starts_with("group.") {
        return Some(incoming_group_id.to_string());
    }

    groups
        .iter()
        .find(|g| g.internal_id == incoming_group_id)
        .map(|g| g.id.clone())
}

/// `text_mode` for a send request: omitted for normal text, as before styled mode existed.
fn text_mode_field(mode: TextMode) -> Option<TextMode> {
    (mode == TextMode::Styled).then_some(mode)
}

fn truncate_snippet(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_len).collect();
        format!("{truncated}…")
    }
}

fn quote_snippet_for_message(original: &BotMessage) -> Option<String> {
    if original.text.is_empty() {
        original
            .primary_audio_attachment()
            .map(|a| format!("[voice note: {}]", a.content_type))
    } else {
        Some(truncate_snippet(&original.text, 120))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_group_send_id_maps_internal_id() {
        let groups = vec![Group {
            name: "test".into(),
            id: "group.TUIzYitaQy85SmtteUpTMEo2ZE9wZ3lib0tOWVZrcDEzNFA3bDU0N1BrOD0=".into(),
            internal_id: "MB3b+ZC/9JkmyJS0J6dOpgyboKNYVkp134P7l547Pk8=".into(),
            members: vec![],
            pending_invites: vec![],
            pending_requests: vec![],
            admins: vec![],
        }];

        assert_eq!(
            resolve_group_send_id("MB3b+ZC/9JkmyJS0J6dOpgyboKNYVkp134P7l547Pk8=", &groups),
            Some("group.TUIzYitaQy85SmtteUpTMEo2ZE9wZ3lib0tOWVZrcDEzNFA3bDU0N1BrOD0=".into())
        );
    }

    #[test]
    fn resolve_group_send_id_passes_through_send_id() {
        let groups = vec![];
        assert_eq!(
            resolve_group_send_id("group.abc123=", &groups),
            Some("group.abc123=".into())
        );
    }
}
