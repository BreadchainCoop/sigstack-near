//! Auto-accept pending Signal group invites.
//!
//! Unified bot: accept any pending invite/request (`AcceptAll`).
//!
//! Listing groups holds the Signal account in signal-cli, so sends, typing indicators and
//! receives wait behind it. The acceptor therefore scans when the receive loop sees a group
//! change (an invite arrives as one) and otherwise only on a slow fallback poll.

use signal_client::{Group, SignalClient};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

/// Fallback interval for scanning `GET /v1/groups` for pending invites, for invites the
/// receive loop didn't flag as group updates.
pub const DEFAULT_INVITE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Policy for which pending invites to accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvitePolicy {
    /// Accept every group where this account is pending.
    AcceptAll,
}

/// Whether this account should `POST .../join` for `group`.
pub fn should_join(group: &Group, self_identity: &str, _policy: &InvitePolicy) -> bool {
    group.is_pending_for(self_identity)
}

/// One scan: list groups and join those that match policy.
pub async fn accept_pending_invites(
    signal: &SignalClient,
    phone_number: &str,
    policy: &InvitePolicy,
) -> usize {
    let groups = match signal.list_groups(phone_number).await {
        Ok(g) => g,
        Err(e) => {
            warn!(error = %e, "Failed to list groups for invite accept");
            return 0;
        }
    };

    let mut joined = 0;
    for group in groups {
        if !should_join(&group, phone_number, policy) {
            continue;
        }
        match signal.join_group(phone_number, &group.id).await {
            Ok(()) => {
                info!(
                    group_id = %group.id,
                    group_name = %group.name,
                    "Accepted pending group invite"
                );
                joined += 1;
            }
            Err(e) => {
                warn!(
                    error = %e,
                    group_id = %group.id,
                    group_name = %group.name,
                    "Failed to join group from pending invite"
                );
            }
        }
    }
    joined
}

/// Resolve which Signal account to poll (configured phone, else first registered).
pub async fn resolve_account_phone(
    signal: &SignalClient,
    configured: Option<&str>,
) -> Option<String> {
    if let Some(phone) = configured.map(str::trim).filter(|p| !p.is_empty()) {
        return Some(phone.to_string());
    }
    match signal.list_accounts().await {
        Ok(accounts) => accounts.into_iter().next(),
        Err(e) => {
            warn!(error = %e, "Failed to list accounts for invite accept");
            None
        }
    }
}

/// Background loop: accept pending invites per policy, each time `group_updates` is notified
/// and at least every `poll_interval`.
pub async fn run_invite_acceptor(
    signal: Arc<SignalClient>,
    phone_number: Option<String>,
    policy: InvitePolicy,
    poll_interval: Duration,
    group_updates: Arc<Notify>,
) {
    info!(
        ?policy,
        interval_secs = poll_interval.as_secs(),
        "Group invite acceptor started"
    );
    loop {
        if let Some(account) = resolve_account_phone(&signal, phone_number.as_deref()).await {
            let n = accept_pending_invites(&signal, &account, &policy).await;
            if n == 0 {
                debug!("No pending group invites to accept");
            }
        } else {
            warn!("No Signal account available for invite accept; retrying");
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = group_updates.notified() => debug!("Group update received; checking invites"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn group_json(
        id: &str,
        members: &[&str],
        pending: &[&str],
        admins: &[&str],
    ) -> serde_json::Value {
        json!({
            "name": "G",
            "id": id,
            "internal_id": format!("{id}-internal"),
            "members": members,
            "pending_invites": pending,
            "pending_requests": [],
            "admins": admins
        })
    }

    fn sample_group(members: &[&str], pending: &[&str], admins: &[&str]) -> Group {
        serde_json::from_value(group_json("group.abc==", members, pending, admins)).unwrap()
    }

    #[test]
    fn translation_accepts_any_pending() {
        let policy = InvitePolicy::AcceptAll;
        let pending = sample_group(&["+15550001111"], &["+15550002222"], &["+15550001111"]);
        assert!(should_join(&pending, "+15550002222", &policy));
        assert!(!should_join(&pending, "+15550001111", &policy));
    }

    #[tokio::test]
    async fn accept_pending_joins_matching_groups() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15550002222"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                group_json("group.skip==", &["+15550001111"], &[], &["+15550001111"]),
                group_json(
                    "group.join==",
                    &["+15550001111"],
                    &["+15550002222"],
                    &["+15550001111"]
                ),
            ])))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/groups/%2B15550002222/group.join%3D%3D/join"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let signal = SignalClient::new(server.uri()).unwrap();
        let n = accept_pending_invites(&signal, "+15550002222", &InvitePolicy::AcceptAll).await;
        assert_eq!(n, 1);
    }

    async fn list_calls(server: &MockServer) -> usize {
        server.received_requests().await.unwrap_or_default().len()
    }

    async fn wait_for_calls(server: &MockServer, n: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while list_calls(server).await < n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {n} group listings"));
    }

    #[tokio::test]
    async fn acceptor_scans_on_group_update_not_only_on_poll() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15550002222"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;

        let notify = Arc::new(Notify::new());
        let signal = Arc::new(SignalClient::new(server.uri()).unwrap());
        let task = tokio::spawn(run_invite_acceptor(
            signal,
            Some("+15550002222".into()),
            InvitePolicy::AcceptAll,
            Duration::from_secs(3600),
            notify.clone(),
        ));

        wait_for_calls(&server, 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            list_calls(&server).await,
            1,
            "no scan before the hour-long poll"
        );

        notify.notify_one();
        wait_for_calls(&server, 2).await;
        task.abort();
    }

    #[tokio::test]
    async fn acceptor_falls_back_to_polling() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/groups/%2B15550002222"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;

        let signal = Arc::new(SignalClient::new(server.uri()).unwrap());
        let task = tokio::spawn(run_invite_acceptor(
            signal,
            Some("+15550002222".into()),
            InvitePolicy::AcceptAll,
            Duration::from_millis(20),
            Arc::new(Notify::new()),
        ));
        wait_for_calls(&server, 3).await;
        task.abort();
    }
}
