//! Startup configuration, read from environment variables.

use std::{collections::BTreeSet, env, fmt};

use anyhow::{Context, Result, bail};

/// Tools that spend sats, change campaigns, or mint a wallet sign-in link.
/// They are hidden unless named in `OSHI_MCP_ENABLE_TOOLS`.
pub const GATED_TOOLS: &[&str] = &[
    "grant_reward",
    "void_reward",
    "update_campaign",
    "mint_portal_link",
];

/// An Oshi API key. `Debug` never prints the value.
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(raw: &str) -> Result<Self> {
        let key = raw.trim();
        if key.is_empty() {
            bail!("OSHI_API_KEY is empty");
        }
        if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            bail!("OSHI_API_KEY contains whitespace or control characters");
        }
        Ok(Self(key.to_owned()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug)]
pub struct Config {
    pub api_key: ApiKey,
    /// Gated tools the operator opted into.
    pub enabled_tools: BTreeSet<String>,
    /// Whether gated tools ask the user to approve each call (on by default).
    pub confirm_calls: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let raw_key =
            env::var("OSHI_API_KEY").context("OSHI_API_KEY environment variable is required")?;
        let api_key = ApiKey::new(&raw_key)?;
        let enabled_tools = match env::var("OSHI_MCP_ENABLE_TOOLS") {
            Ok(raw) => parse_enabled_tools(&raw)?,
            Err(env::VarError::NotPresent) => BTreeSet::new(),
            Err(e) => bail!("OSHI_MCP_ENABLE_TOOLS: {e}"),
        };
        let confirm_calls = match env::var("OSHI_MCP_CONFIRM") {
            Ok(raw) => parse_confirm(&raw)?,
            Err(env::VarError::NotPresent) => true,
            Err(e) => bail!("OSHI_MCP_CONFIRM: {e}"),
        };
        Ok(Self {
            api_key,
            enabled_tools,
            confirm_calls,
        })
    }
}

/// Parses `OSHI_MCP_CONFIRM`. Only an explicit `off` turns confirmation off;
/// anything unrecognized is an error rather than a silent default.
pub fn parse_confirm(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "on" => Ok(true),
        "off" => Ok(false),
        other => bail!("OSHI_MCP_CONFIRM must be \"on\" or \"off\", not {other:?}"),
    }
}

/// Parses a comma-separated list of gated tool names. Unknown names are an
/// error rather than ignored, so a typo can't silently leave a tool off (or on).
pub fn parse_enabled_tools(raw: &str) -> Result<BTreeSet<String>> {
    let mut enabled = BTreeSet::new();
    for name in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !GATED_TOOLS.contains(&name) {
            bail!(
                "unknown tool {name:?} in OSHI_MCP_ENABLE_TOOLS; expected a comma-separated list of: {}",
                GATED_TOOLS.join(", ")
            );
        }
        enabled.insert(name.to_owned());
    }
    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_tools_parse_and_trim() {
        let set = parse_enabled_tools(" grant_reward, void_reward ,,").unwrap();
        assert_eq!(
            set.into_iter().collect::<Vec<_>>(),
            ["grant_reward", "void_reward"]
        );
        assert!(parse_enabled_tools("").unwrap().is_empty());
    }

    #[test]
    fn enabled_tools_reject_unknown_names() {
        let err = parse_enabled_tools("grant_reward,grant_rewards").unwrap_err();
        assert!(err.to_string().contains("grant_rewards"));
        assert!(parse_enabled_tools("all").is_err());
        assert!(
            parse_enabled_tools("get_user").is_err(),
            "read tools are always on, not gated"
        );
    }

    #[test]
    fn confirm_is_on_unless_explicitly_off() {
        assert!(parse_confirm("").unwrap());
        assert!(parse_confirm("on").unwrap());
        assert!(!parse_confirm(" OFF ").unwrap());
        for bad in ["false", "0", "no", "of"] {
            assert!(parse_confirm(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn api_key_is_validated_and_redacted() {
        assert!(ApiKey::new("  ").is_err());
        assert!(ApiKey::new("abc def").is_err());
        assert!(
            ApiKey::new("abc\n").is_ok(),
            "surrounding whitespace is trimmed"
        );
        let key = ApiKey::new("sk_live_secret").unwrap();
        assert_eq!(key.expose(), "sk_live_secret");
        assert!(!format!("{key:?}").contains("secret"));
    }
}
