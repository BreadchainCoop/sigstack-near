//! Signal product bot — hub, voice, in-chat, and language threads.

use anyhow::Context;
use dstack_client::DstackClient;
use near_agent::Registrar;
use signal_bot::bot_identity::BotIdentity;
use signal_bot::config::Config;
use signal_bot::dispatch::dispatch_message;
use signal_bot::ensure_username::ensure_signal_username;
use signal_bot::entitlement_gate::EntitlementGate;
use signal_bot::error::AppResult;
use signal_bot::group_invite_acceptor::{
    run_invite_acceptor, InvitePolicy, DEFAULT_INVITE_POLL_INTERVAL,
};
use signal_bot::handlers_setup::build_handlers;
use signal_client::{MessageReceiver, SignalClient};
use std::sync::Arc;
use tokio::signal;
use tokio_stream::StreamExt;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> AppResult<()> {
    let config = Config::load().context("Failed to load configuration")?;

    init_logging(&config.bot.log_level);

    info!("Starting sigstack Signal bot");

    let dstack = Arc::new(DstackClient::new(&config.dstack.socket_path));

    let signal = Arc::new(
        SignalClient::new(&config.signal.service_url).context("Failed to create Signal client")?,
    );

    let in_tee = dstack.is_in_tee().await;
    if in_tee {
        if let Ok(info) = dstack.get_app_info().await {
            info!(
                "Running in TEE - App ID: {}",
                info.app_id.as_deref().unwrap_or("unknown")
            );
        }
    } else {
        warn!("Not running in TEE environment - attestation unavailable");
    }

    if !signal.health_check().await {
        error!("Signal API not reachable at {}", config.signal.service_url);
        return Err(anyhow::anyhow!("Signal API not reachable").into());
    }
    info!("Signal API healthy");

    // Each boot registers a fresh random agent key with the agent contract and keeps
    // it registered. Failures are logged and retried; they never stop the bot. This
    // runs after the Signal check, so a restart loop while signal-api starts doesn't
    // generate and fund a new agent account on every attempt.
    let mut agent_status = None;
    if config.agent.enabled() {
        if !in_tee {
            warn!("AGENT__CONTRACT_ID set without a TEE - skipping agent registration");
        } else {
            match Registrar::new(&config.agent, dstack.clone()) {
                Ok(registrar) => {
                    info!(
                        "Agent account {} registering with {}",
                        registrar.account_id(),
                        config.agent.contract_id
                    );
                    agent_status = Some(registrar.status());
                    tokio::spawn(registrar.run());
                }
                Err(e) => error!("Agent registration disabled: {e}"),
            }
        }
    }

    let nickname = config
        .bot
        .signal_username
        .as_deref()
        .unwrap_or("sigstack")
        .to_string();
    let claimed_username = if let Some(phone) = config
        .signal
        .phone_number
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        ensure_signal_username(&signal, phone, &nickname).await
    } else {
        warn!(
            "SIGNAL__PHONE_NUMBER unset — skipping Signal username ensure (needed for site Message link)"
        );
        None
    };
    let bot_username = claimed_username.unwrap_or(nickname);

    let bot_identity = BotIdentity::new();

    let built = build_handlers(
        &config,
        signal.clone(),
        dstack.clone(),
        bot_identity.clone(),
        bot_username,
        agent_status,
    )
    .await?;

    let handlers = Arc::new(built.handlers);
    let entitlements = EntitlementGate {
        store: built.entitlements.clone(),
        enforce: config.entitlements.enforce,
    };
    if entitlements.enforce {
        info!("Entitlement enforcement enabled (link + enable-sigstack gate)");
    }
    {
        let store = built.entitlements.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(signal_bot::entitlements_store::ENTITLEMENTS_RELOAD_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match store.reload_if_stale().await {
                    Ok(true) => info!("Reloaded entitlements from disk (peer write)"),
                    Ok(false) => {}
                    Err(e) => warn!("Periodic entitlements reload failed: {e}"),
                }
            }
        });
    }
    info!("Registered {} command handlers", handlers.len());

    {
        let policy = InvitePolicy::AcceptAll;
        let signal_invites = signal.clone();
        let phone = config.signal.phone_number.clone();
        tokio::spawn(async move {
            run_invite_acceptor(signal_invites, phone, policy, DEFAULT_INVITE_POLL_INTERVAL).await;
        });
    }

    info!("Listening for messages...");

    let receiver = MessageReceiver::new((*signal).clone(), config.signal.poll_interval);
    let mut stream = Box::pin(receiver.stream());

    loop {
        tokio::select! {
            Some(message) = stream.next() => {
                let handlers = handlers.clone();
                let signal = signal.clone();
                let bot_identity = bot_identity.clone();
                let entitlements = entitlements.clone();
                tokio::spawn(async move {
                    let _ = dispatch_message(
                        handlers.as_slice(),
                        &signal,
                        &bot_identity,
                        &message,
                        &entitlements,
                    )
                    .await;
                });
            }
            _ = signal::ctrl_c() => {
                info!("Shutdown signal received");
                break;
            }
        }
    }

    info!("Shutting down...");
    Ok(())
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
