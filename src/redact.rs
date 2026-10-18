//! Strips rewards-portal sign-in links from API data.
//!
//! A portal link signs whoever opens it into a user's wallet, so it is a bearer
//! credential. Oshi returns them as `portal_url`, but any endpoint could start
//! embedding them, so this checks field names *and* string values at every
//! depth. It errs on the side of over-redacting.

use std::borrow::Cow;

use serde_json::Value;
use url::Url;

pub const REDACTED: &str =
    "<redacted portal link: enable the mint_portal_link tool to receive portal links>";

/// Recursively redacts portal links in a JSON value.
pub fn value(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let v = if is_portal_key(&k) && !v.is_null() {
                        Value::String(REDACTED.into())
                    } else {
                        value(v)
                    };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(value).collect()),
        Value::String(s) => match text(&s) {
            Cow::Borrowed(_) => Value::String(s),
            Cow::Owned(redacted) => Value::String(redacted),
        },
        other => other,
    }
}

/// Redacts portal-link-shaped URLs anywhere in free text.
pub fn text(s: &str) -> Cow<'_, str> {
    // ASCII lowercasing keeps byte offsets identical to `s`.
    let lower = s.to_ascii_lowercase();
    let mut out = String::new();
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = next_url_start(&lower[search..]) {
        let start = search + found;
        let len = s[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '\\'))
            .unwrap_or(s.len() - start);
        let candidate =
            s[start..start + len].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
        let end = start + candidate.len();
        if looks_like_portal_link(candidate) {
            out.push_str(&s[copied..start]);
            out.push_str(REDACTED);
            copied = end;
        }
        search = end.max(start + 1);
    }
    if copied == 0 {
        return Cow::Borrowed(s);
    }
    out.push_str(&s[copied..]);
    Cow::Owned(out)
}

fn next_url_start(lower: &str) -> Option<usize> {
    match (lower.find("https://"), lower.find("http://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn is_portal_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.ends_with("expires_at") {
        return false;
    }
    (k.contains("portal") && (k.contains("url") || k.contains("link")))
        || k.ends_with("magic_link")
        || k.ends_with("login_url")
        || k.ends_with("sign_in_url")
}

/// Portal links look like `https://portal.bitcoinrewards.app/a/<token>`. The
/// portal can be branded per merchant, so any host serving `/a/<long token>`
/// counts too.
fn looks_like_portal_link(candidate: &str) -> bool {
    let Ok(url) = Url::parse(candidate) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.starts_with("portal.")
        || host == "bitcoinrewards.app"
        || host.ends_with(".bitcoinrewards.app")
    {
        return true;
    }
    let mut segments = url.path_segments().into_iter().flatten();
    segments.next() == Some("a") && segments.next().is_some_and(|token| token.len() >= 16)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const LINK: &str = "https://portal.bitcoinrewards.app/a/Qm9ndXNUb2tlbkV4YW1wbGU";

    #[test]
    fn redacts_portal_fields_at_any_depth() {
        let out = value(json!({
            "portal_url": LINK,
            "portal_url_expires_at": "2026-07-07T12:00:00.000Z",
            "user": {"rewards_history": [{"portal_url": LINK, "id": "987"}]},
            "PortalLink": {"href": "x"},
            "magic_link": "opaque-token-without-a-url",
        }));
        assert_eq!(out["portal_url"], REDACTED);
        assert_eq!(out["portal_url_expires_at"], "2026-07-07T12:00:00.000Z");
        assert_eq!(out["user"]["rewards_history"][0]["portal_url"], REDACTED);
        assert_eq!(out["user"]["rewards_history"][0]["id"], "987");
        assert_eq!(out["PortalLink"], REDACTED);
        assert_eq!(out["magic_link"], REDACTED);
    }

    #[test]
    fn null_portal_fields_stay_null() {
        assert_eq!(
            value(json!({"portal_url": null})),
            json!({"portal_url": null})
        );
    }

    #[test]
    fn redacts_portal_shaped_values_under_any_key() {
        let out = value(json!({
            "next": LINK,
            "message": format!("Sign in at {LINK}. It expires soon."),
            "items": [format!("({LINK})")],
            "loud": "HTTPS://PORTAL.BITCOINREWARDS.APP/a/TOKEN",
            "branded": "https://rewards.merchant.example/a/abcdefghijklmnopqrstuvwxyz",
        }));
        let s = out.to_string();
        assert!(
            !s.to_ascii_lowercase().contains("bitcoinrewards.app"),
            "{s}"
        );
        assert!(!s.contains("abcdefghijklmnop"), "{s}");
        assert_eq!(
            out["message"],
            format!("Sign in at {REDACTED}. It expires soon.")
        );
        assert_eq!(out["items"][0], format!("({REDACTED})"));
    }

    #[test]
    fn leaves_other_urls_and_text_alone() {
        let input = json!({
            "endpoint_url": "https://app.oshi.tech/api/rewards/events",
            "description": "See https://example.com/a/b and http://x",
            "sats_amount": 100,
            "name": "App Referral",
        });
        assert_eq!(value(input.clone()), input);
        assert!(matches!(text("no links here"), Cow::Borrowed(_)));
    }
}
