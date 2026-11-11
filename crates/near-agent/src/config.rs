//! `AGENT__*` settings. Registration is off unless `AGENT__CONTRACT_ID` is set.

use crate::{collateral, AgentError};
use serde::Deserialize;
use std::fmt;

const YOCTO_PER_NEAR: u128 = 1_000_000_000_000_000_000_000_000;

#[derive(Clone, Deserialize)]
pub struct AgentConfig {
    /// Agent contract account, e.g. `sigstack-agent.testnet`.
    #[serde(default)]
    pub contract_id: String,

    /// NEAR JSON-RPC endpoint.
    #[serde(default = "default_rpc_url")]
    pub rpc_url: String,

    /// Account that funds each boot's new agent account.
    #[serde(default)]
    pub sponsor_account_id: String,

    /// Sponsor's full-access key, `ed25519:<base58>`. An encrypted env secret.
    #[serde(default)]
    pub sponsor_private_key: String,

    /// Top up the agent account when its balance drops below this (NEAR).
    #[serde(default = "default_min_balance")]
    pub min_balance: String,

    /// ...to this balance (NEAR). Covers storage, the registration deposit and gas.
    #[serde(default = "default_top_up_balance")]
    pub top_up_balance: String,

    /// Comma-separated PCCS base URLs for quote collateral, tried in order.
    #[serde(default = "default_pccs_urls")]
    pub pccs_urls: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            contract_id: String::new(),
            rpc_url: default_rpc_url(),
            sponsor_account_id: String::new(),
            sponsor_private_key: String::new(),
            min_balance: default_min_balance(),
            top_up_balance: default_top_up_balance(),
            pccs_urls: default_pccs_urls(),
        }
    }
}

impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConfig")
            .field("contract_id", &self.contract_id)
            .field("rpc_url", &self.rpc_url)
            .field("sponsor_account_id", &self.sponsor_account_id)
            .field("sponsor_private_key", &"<redacted>")
            .field("min_balance", &self.min_balance)
            .field("top_up_balance", &self.top_up_balance)
            .field("pccs_urls", &self.pccs_urls)
            .finish()
    }
}

impl AgentConfig {
    pub fn enabled(&self) -> bool {
        !self.contract_id.trim().is_empty()
    }

    pub fn pccs_endpoints(&self) -> Vec<String> {
        self.pccs_urls
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// `(min_balance, top_up_balance)` in yoctoNEAR.
    pub fn balances(&self) -> Result<(u128, u128), AgentError> {
        let min = parse_near_amount(&self.min_balance)?;
        let top_up = parse_near_amount(&self.top_up_balance)?;
        if top_up <= min {
            return Err(AgentError::Config(
                "AGENT__TOP_UP_BALANCE must be above AGENT__MIN_BALANCE".into(),
            ));
        }
        Ok((min, top_up))
    }

    pub fn validate(&self) -> Result<(), AgentError> {
        for (name, value) in [
            ("AGENT__CONTRACT_ID", &self.contract_id),
            ("AGENT__RPC_URL", &self.rpc_url),
            ("AGENT__SPONSOR_ACCOUNT_ID", &self.sponsor_account_id),
            ("AGENT__SPONSOR_PRIVATE_KEY", &self.sponsor_private_key),
        ] {
            if value.trim().is_empty() {
                return Err(AgentError::Config(format!("{name} is required")));
            }
        }
        if self.pccs_endpoints().is_empty() {
            return Err(AgentError::Config("AGENT__PCCS_URLS is empty".into()));
        }
        self.balances()?;
        Ok(())
    }
}

/// `"0.05"` → 5×10²² yoctoNEAR.
pub fn parse_near_amount(amount: &str) -> Result<u128, AgentError> {
    let invalid = || AgentError::Config(format!("invalid NEAR amount `{amount}`"));
    let amount = amount.trim();
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || fraction.len() > 24
    {
        return Err(invalid());
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| invalid())?
    };
    let fraction: u128 = format!("{fraction:0<24}").parse().map_err(|_| invalid())?;
    whole
        .checked_mul(YOCTO_PER_NEAR)
        .and_then(|w| w.checked_add(fraction))
        .ok_or_else(invalid)
}

fn default_rpc_url() -> String {
    "https://test.rpc.fastnear.com".into()
}

fn default_min_balance() -> String {
    "0.05".into()
}

fn default_top_up_balance() -> String {
    "0.1".into()
}

fn default_pccs_urls() -> String {
    format!("{},{}", collateral::PHALA_PCCS_URL, collateral::INTEL_PCS_URL)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn config() -> AgentConfig {
        AgentConfig {
            contract_id: "sigstack-agent.testnet".into(),
            sponsor_account_id: "sponsor.testnet".into(),
            sponsor_private_key: "ed25519:secret".into(),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn parses_near_amounts() {
        assert_eq!(parse_near_amount("1").unwrap(), YOCTO_PER_NEAR);
        assert_eq!(parse_near_amount(" 0.05 ").unwrap(), YOCTO_PER_NEAR / 20);
        assert_eq!(parse_near_amount(".1").unwrap(), YOCTO_PER_NEAR / 10);
        assert_eq!(parse_near_amount("2.").unwrap(), 2 * YOCTO_PER_NEAR);
        assert_eq!(parse_near_amount("0.000000000000000000000001").unwrap(), 1);
        for bad in [
            "",
            ".",
            "-1",
            "1.2.3",
            "abc",
            "1e3",
            "0.0000000000000000000000001",
            "999999999999999999999",
        ] {
            assert!(parse_near_amount(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn defaults_and_endpoints() {
        let cfg = AgentConfig::default();
        assert!(!cfg.enabled());
        assert_eq!(
            cfg.pccs_endpoints(),
            vec![
                "https://pccs.phala.network".to_string(),
                "https://api.trustedservices.intel.com".to_string()
            ]
        );
        assert_eq!(
            cfg.balances().unwrap(),
            (YOCTO_PER_NEAR / 20, YOCTO_PER_NEAR / 10)
        );
        assert!(config().enabled());
    }

    #[test]
    fn validation() {
        assert!(config().validate().is_ok());
        assert!(AgentConfig::default()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("AGENT__CONTRACT_ID"));

        let mut cfg = config();
        cfg.sponsor_private_key = " ".into();
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("AGENT__SPONSOR_PRIVATE_KEY"));

        let mut cfg = config();
        cfg.pccs_urls = " , ".into();
        assert!(cfg.validate().unwrap_err().to_string().contains("PCCS"));

        let mut cfg = config();
        cfg.top_up_balance = "0.05".into();
        assert!(cfg.validate().unwrap_err().to_string().contains("TOP_UP"));

        let mut cfg = config();
        cfg.min_balance = "lots".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn debug_redacts_the_sponsor_key() {
        let debug = format!("{:?}", config());
        assert!(debug.contains("sponsor.testnet"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("ed25519:secret"));
    }
}
