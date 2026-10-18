//! Per-call human approval for gated tools, via MCP elicitation.
//!
//! The prompt is built by the server from validated arguments and Oshi's own
//! data, never from text the model wrote, and it's shown to the user by their
//! MCP client rather than to the model. Anything other than an explicit
//! approval counts as a refusal, including clients that can't ask.

use std::time::Duration;

use rmcp::{
    Peer, RoleServer,
    service::{ElicitationError, ElicitationMode},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// How long to wait for the user before treating the request as declined.
const TIMEOUT: Duration = Duration::from_secs(300);
const MAX_SHOWN_CHARS: usize = 80;

pub const UNSUPPORTED: &str = "This tool needs the user's approval for each call, but this MCP \
    client can't show approval prompts (MCP elicitation). Nothing was sent to Oshi. Tell the user; \
    they can switch to a client that supports elicitation, or set OSHI_MCP_CONFIRM=off to skip \
    approval. Don't retry.";

const NOT_APPROVED: &str = "The user did not approve this action. Nothing was sent to Oshi. \
    Don't retry unless the user asks you to.";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Approval {
    /// Tick to approve this action.
    approve: bool,
}

rmcp::elicit_safe!(Approval);

/// Returns the peer if its client can show approval prompts. Checked before
/// building a prompt, so an unsupported client costs no API calls.
pub fn require_support(peer: Option<&Peer<RoleServer>>) -> Result<&Peer<RoleServer>, String> {
    peer.filter(|p| {
        p.supported_elicitation_modes()
            .contains(&ElicitationMode::Form)
    })
    .ok_or_else(|| UNSUPPORTED.into())
}

/// Asks the user to approve `prompt`. `Ok` only on an explicit approval.
pub async fn ask(peer: &Peer<RoleServer>, prompt: String) -> Result<(), String> {
    match peer
        .elicit_with_timeout::<Approval>(prompt, Some(TIMEOUT))
        .await
    {
        Ok(Some(Approval { approve: true })) => Ok(()),
        Err(ElicitationError::CapabilityNotSupported) => Err(UNSUPPORTED.into()),
        Err(ElicitationError::Service(e)) => Err(format!(
            "Couldn't get the user's approval ({e}). Nothing was sent to Oshi."
        )),
        _ => Err(NOT_APPROVED.into()),
    }
}

/// Renders a value for a prompt: JSON-quoted (so newlines and quotes in
/// merchant or user data can't fake prompt structure) and truncated.
pub fn show(v: &Value) -> String {
    match v {
        Value::String(s) if s.chars().count() > MAX_SHOWN_CHARS => {
            let cut: String = s.chars().take(MAX_SHOWN_CHARS).collect();
            format!("{}…", Value::String(cut))
        }
        other => other.to_string(),
    }
}

pub fn show_str(s: &str) -> String {
    show(&Value::String(s.to_owned()))
}

/// Like `show`, for fields Oshi should always return: missing reads "unknown".
pub fn show_known(v: &Value) -> String {
    if v.is_null() {
        "unknown".into()
    } else {
        show(v)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn show_quotes_and_truncates() {
        assert_eq!(show(&json!("a\nb\"c")), r#""a\nb\"c""#);
        assert_eq!(show(&json!(250)), "250");
        assert_eq!(show(&Value::Null), "null");
        assert_eq!(show_known(&Value::Null), "unknown");
        let long = show_str(&"x".repeat(200));
        assert!(long.ends_with('…') && long.chars().count() < 90, "{long}");
    }

    #[test]
    fn no_peer_fails_closed() {
        assert_eq!(require_support(None).unwrap_err(), UNSUPPORTED);
    }
}
