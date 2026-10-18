//! Error paths, end to end: what the agent sees when Oshi, the network or the
//! approval step fails. Specs rarely list every failure, so the first group
//! replays responses recorded from the live API.

mod common;

use std::time::Duration;

use common::*;
use oshi_mcp::{client::OshiClient, config::ApiKey};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{any, method, path},
};

/// A response recorded from `https://app.oshi.tech/api/rewards` on 2026-10-18.
/// Request validation runs before the API key check, so these were all
/// reachable with a missing or invalid key.
struct Recorded {
    request: &'static str,
    status: u16,
    content_type: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
}

const NO_KEY: Recorded = Recorded {
    request: "GET /campaigns without Authorization",
    status: 401,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"API key is required."}"#,
};
const INVALID_KEY: Recorded = Recorded {
    request: "GET /campaigns with an unknown key",
    status: 401,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"Invalid API key."}"#,
};
const BAD_EVENT_UUID: Recorded = Recorded {
    request: "GET /events/not-a-uuid",
    status: 400,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"A reward event uuid is required."}"#,
};
const MISSING_EXTERNAL_ID: Recorded = Recorded {
    request: "GET /events",
    status: 400,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"An external_id query parameter is required."}"#,
};
const BAD_JSON: Recorded = Recorded {
    request: r#"POST /events with body {"oops""#,
    status: 400,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"Request body must be valid JSON."}"#,
};
const INVALID_PAYLOAD: Recorded = Recorded {
    request: "POST /events with body {}",
    status: 422,
    content_type: "application/json",
    headers: &[],
    body: r#"{"message":"Invalid reward event payload.","errors":{"rewards_campaign_id":["Required"],"user_id":["Required"],"external_id":["Required"]}}"#,
};
const METHOD_NOT_ALLOWED: Recorded = Recorded {
    request: "DELETE /campaigns",
    status: 405,
    content_type: "",
    headers: &[],
    body: "",
};
/// Unknown routes fall through to the web app's 404 page (trimmed here).
const UNKNOWN_ROUTE: Recorded = Recorded {
    request: "GET /no-such-endpoint",
    status: 404,
    content_type: "text/html; charset=utf-8",
    headers: &[],
    body: r#"<!DOCTYPE html><html lang="en" data-sentry-component="RootLayout" data-sentry-source-file="layout.tsx"><head><meta charSet="utf-8"/><meta name="viewport" content="width=device-width, initial-scale=1"/></head><body><h1>404</h1></body></html>"#,
};
/// A trailing slash redirects. The server never adds one, but must not follow it.
const TRAILING_SLASH: Recorded = Recorded {
    request: "GET /campaigns/",
    status: 308,
    content_type: "text/plain",
    headers: &[("location", "/api/rewards/campaigns")],
    body: "Redirecting...",
};

impl Recorded {
    fn template(&self) -> ResponseTemplate {
        let mut t = ResponseTemplate::new(self.status);
        if !self.body.is_empty() {
            t = t.set_body_raw(self.body, self.content_type);
        }
        for (name, value) in self.headers {
            t = t.insert_header(*name, *value);
        }
        t
    }
}

const CAMPAIGN_ARGS: &str = r#"{"campaign_id": "72cb8983-f65a-4103-abf7-382c993d0abe"}"#;
const EVENT_ARGS: &str = r#"{"reward_event_id": "7f0a1b2c-3d4e-5f60-7182-93a4b5c6d7e8"}"#;

fn grant() -> Value {
    json!({"rewards_campaign_id": CAMPAIGN, "user_id": "user_123", "external_id": "action_456"})
}

/// Replays `recorded` for every request and calls `tool`; returns the error text.
async fn replay(recorded: &Recorded, tool: &'static str, arguments: Value) -> (MockServer, String) {
    let (mock, peer) = connect_with("grant_reward,void_reward", false, ()).await;
    Mock::given(any())
        .respond_with(recorded.template())
        .mount(&mock)
        .await;
    let (is_error, text) = call(&peer, tool, arguments).await;
    assert!(
        is_error,
        "{}: should be a tool error, got: {text}",
        recorded.request
    );
    peer.cancel().await.unwrap();
    (mock, text)
}

fn assert_contains(recorded: &Recorded, text: &str, expected: &[&str]) {
    for e in expected {
        assert!(
            text.contains(e),
            "{}: missing {e:?} in: {text}",
            recorded.request
        );
    }
}

#[tokio::test]
async fn recorded_auth_errors_point_at_the_key() {
    for r in [&NO_KEY, &INVALID_KEY] {
        let (_, text) = replay(r, "list_campaigns", json!({})).await;
        let message: Value = serde_json::from_str(r.body).unwrap();
        let message = message["message"].as_str().unwrap();
        assert_contains(r, &text, &["401 Unauthorized", "OSHI_API_KEY", message]);
    }
}

#[tokio::test]
async fn recorded_validation_errors_pass_through_with_details() {
    let cases: [(&Recorded, &'static str, Value, &[&str]); 4] = [
        (
            &BAD_EVENT_UUID,
            "get_reward",
            serde_json::from_str(EVENT_ARGS).unwrap(),
            &["400 Bad Request", "A reward event uuid is required."],
        ),
        (
            &MISSING_EXTERNAL_ID,
            "lookup_reward",
            json!({"external_id": "action_456"}),
            &[
                "400 Bad Request",
                "An external_id query parameter is required.",
            ],
        ),
        (
            &BAD_JSON,
            "grant_reward",
            grant(),
            &["400 Bad Request", "Request body must be valid JSON."],
        ),
        (
            &INVALID_PAYLOAD,
            "grant_reward",
            grant(),
            &[
                "422 Unprocessable Entity",
                "Invalid reward event payload.",
                "rewards_campaign_id",
                "Required",
            ],
        ),
    ];
    for (r, tool, arguments, expected) in cases {
        let (_, text) = replay(r, tool, arguments).await;
        assert_contains(r, &text, expected);
        // A definite rejection: no "may or may not" and no retry advice.
        assert!(!text.contains("may or may not"), "{}: {text}", r.request);
    }
}

#[tokio::test]
async fn recorded_empty_405_reads_cleanly() {
    let r = &METHOD_NOT_ALLOWED;
    let (_, text) = replay(r, "void_reward", serde_json::from_str(EVENT_ARGS).unwrap()).await;
    assert_eq!(text, "Oshi API returned 405 Method Not Allowed");
}

#[tokio::test]
async fn recorded_html_404_page_is_not_shown_to_the_model() {
    let r = &UNKNOWN_ROUTE;
    let (_, text) = replay(
        r,
        "get_campaign",
        serde_json::from_str(CAMPAIGN_ARGS).unwrap(),
    )
    .await;
    assert_contains(r, &text, &["404 Not Found", "HTML page instead of JSON"]);
    for leaked in ["<", "sentry", "layout.tsx"] {
        assert!(!text.contains(leaked), "HTML leaked ({leaked}): {text}");
    }
}

#[tokio::test]
async fn recorded_redirect_is_not_followed() {
    let r = &TRAILING_SLASH;
    let (mock, text) = replay(r, "list_campaigns", json!({})).await;
    assert_contains(r, &text, &["308 Permanent Redirect", "not followed"]);
    assert_eq!(mock.received_requests().await.unwrap().len(), 1);
}

// ---- Documented errors that need a real account to record ---------------------

#[tokio::test]
async fn void_conflict_explains_the_current_status() {
    let r = Recorded {
        request: "POST /events/{id}/void on a released reward (from the docs)",
        status: 409,
        content_type: "application/json",
        headers: &[],
        body: r#"{"message":"Only pending (held) rewards can be voided.","reward_event_id":"7f0a1b2c-3d4e-5f60-7182-93a4b5c6d7e8","status":"released"}"#,
    };
    let (_, text) = replay(&r, "void_reward", serde_json::from_str(EVENT_ARGS).unwrap()).await;
    assert_contains(
        &r,
        &text,
        &["409 Conflict", "Only pending (held) rewards", "released"],
    );
}

#[tokio::test]
async fn backend_unreachable_grant_gets_safe_retry_advice() {
    let r = Recorded {
        request: "POST /events while the rewards backend is down (from the docs)",
        status: 502,
        content_type: "application/json",
        headers: &[],
        body: r#"{"message":"Rewards backend unavailable."}"#,
    };
    let (_, text) = replay(&r, "grant_reward", grant()).await;
    assert_contains(
        &r,
        &text,
        &[
            "502 Bad Gateway",
            "may or may not have taken effect",
            "same external_id",
            "Don't use a new external_id",
        ],
    );
}

// ---- Network failures and unreadable success responses ------------------------

async fn grant_with_client(client: OshiClient) -> String {
    let peer = serve(client, "grant_reward", false, ()).await;
    let (is_error, text) = call(&peer, "grant_reward", grant()).await;
    assert!(is_error, "{text}");
    peer.cancel().await.unwrap();
    text
}

#[tokio::test]
async fn grant_timeout_says_it_may_have_happened() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(202).set_delay(Duration::from_secs(2)))
        .mount(&mock)
        .await;
    let key = ApiKey::new("test_key").unwrap();
    let client = OshiClient::with_timeouts(
        &key,
        &mock.uri(),
        Duration::from_secs(2),
        Duration::from_millis(200),
    )
    .unwrap();
    let text = grant_with_client(client).await;
    for expected in [
        "didn't respond within",
        "may or may not have taken effect",
        "lookup_reward",
        "same external_id",
    ] {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
}

#[tokio::test]
async fn grant_connection_refused_says_nothing_was_sent() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let key = ApiKey::new("test_key").unwrap();
    let client = OshiClient::new(&key, &format!("http://127.0.0.1:{port}")).unwrap();
    let text = grant_with_client(client).await;
    assert!(text.starts_with("couldn't connect to Oshi"), "{text}");
    assert!(!text.contains("may or may not"), "{text}");
}

#[tokio::test]
async fn unreadable_grant_success_is_not_reported_as_success() {
    for (what, template) in [
        ("an empty body", ResponseTemplate::new(202)),
        (
            "invalid JSON",
            ResponseTemplate::new(202).set_body_raw("{\"status\":", "application/json"),
        ),
        (
            "an HTML page",
            ResponseTemplate::new(200).set_body_raw("<html><body>ok</body></html>", "text/html"),
        ),
    ] {
        let (mock, peer) = connect_with("grant_reward", false, ()).await;
        Mock::given(any()).respond_with(template).mount(&mock).await;
        let (is_error, text) = call(&peer, "grant_reward", grant()).await;
        assert!(is_error, "{what}: {text}");
        assert!(
            text.contains(what) && text.contains("same external_id"),
            "{what}: {text}"
        );
        peer.cancel().await.unwrap();
    }
}

// ---- Approval-path failures ----------------------------------------------------

#[tokio::test]
async fn approval_prompt_that_cant_be_built_blocks_the_call() {
    let approver = Approver::new(Answer::Approve);
    let (mock, peer) = connect_with("grant_reward", true, approver.clone()).await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"message": "Campaign not found."})),
        )
        .mount(&mock)
        .await;
    let (is_error, text) = call(&peer, "grant_reward", grant()).await;
    assert!(is_error && text.contains("Campaign not found."), "{text}");
    assert!(
        approver.prompts().is_empty(),
        "no prompt without the campaign's details"
    );
    assert_eq!(posts(&mock).await, 0);
    peer.cancel().await.unwrap();
}

#[tokio::test]
async fn broken_or_odd_approval_answers_count_as_no() {
    for (answer, expected) in [
        (Answer::Error, "Couldn't get the user's approval"),
        (Answer::NoContent, "did not approve"),
        (Answer::Malformed, "did not approve"),
    ] {
        let approver = Approver::new(answer);
        let (mock, peer) = connect_with("grant_reward", true, approver.clone()).await;
        mount_campaign(&mock).await;
        let (is_error, text) = call(&peer, "grant_reward", grant()).await;
        assert!(is_error && text.contains(expected), "{answer:?}: {text}");
        assert!(
            text.contains("Nothing was sent to Oshi"),
            "{answer:?}: {text}"
        );
        assert_eq!(approver.prompts().len(), 1, "{answer:?}");
        assert_eq!(posts(&mock).await, 0, "{answer:?} must not reach Oshi");
        peer.cancel().await.unwrap();
    }
}
