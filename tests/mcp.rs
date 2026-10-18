//! End-to-end: a real MCP client talks to the server over an in-memory pipe,
//! and the server talks to a mocked Oshi API.

use oshi_mcp::{
    client::OshiClient,
    config::{ApiKey, parse_enabled_tools},
    server::OshiServer,
};
use std::borrow::Cow;

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, JsonObject, Tool},
    service::RunningService,
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{any, header, method, path},
};

const CAMPAIGN: &str = "72cb8983-f65a-4103-abf7-382c993d0abe";

async fn connect(enable: &str) -> (MockServer, RunningService<rmcp::RoleClient, ()>) {
    let mock = MockServer::start().await;
    let client = OshiClient::new(&ApiKey::new("test_key").unwrap(), &mock.uri()).unwrap();
    let server = OshiServer::new(client, &parse_enabled_tools(enable).unwrap());
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await });
    (mock, ().serve(client_io).await.unwrap())
}

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

/// Returns (is_error, text) for a tool call result.
async fn call(
    peer: &RunningService<rmcp::RoleClient, ()>,
    name: impl Into<Cow<'static, str>>,
    arguments: Value,
) -> (bool, String) {
    let result = peer
        .call_tool(CallToolRequestParams::new(name).with_arguments(args(arguments)))
        .await
        .unwrap();
    let v = serde_json::to_value(&result).unwrap();
    let text = v["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (v["isError"] == json!(true), text)
}

#[tokio::test]
async fn lists_only_read_tools_by_default() {
    let (_mock, peer) = connect("").await;
    let info = peer.peer_info().unwrap();
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("OSHI_MCP_ENABLE_TOOLS")
    );

    let mut names: Vec<_> = peer
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "get_campaign",
            "get_reward",
            "get_user",
            "list_campaigns",
            "lookup_reward"
        ]
    );

    // A hidden tool can't be called by name either.
    let err = peer
        .call_tool(
            CallToolRequestParams::new("grant_reward").with_arguments(args(json!({
                "rewards_campaign_id": CAMPAIGN, "user_id": "u", "external_id": "e",
            }))),
        )
        .await;
    match err {
        Err(_) => {}
        Ok(r) => assert_eq!(r.is_error, Some(true), "hidden tool must not run: {r:?}"),
    }
    peer.cancel().await.unwrap();
}

#[tokio::test]
async fn read_tool_round_trip() {
    let (mock, peer) = connect("").await;
    Mock::given(method("GET"))
        .and(path(format!("/campaigns/{CAMPAIGN}")))
        .and(header("authorization", "Bearer test_key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"campaign": {"name": "App Referral"}})),
        )
        .expect(1)
        .mount(&mock)
        .await;
    let (is_error, text) = call(&peer, "get_campaign", json!({"campaign_id": CAMPAIGN})).await;
    assert!(!is_error, "{text}");
    assert!(text.contains("App Referral"));
    peer.cancel().await.unwrap();
}

#[tokio::test]
async fn bad_arguments_and_api_errors_are_tool_errors() {
    let (mock, peer) = connect("grant_reward").await;
    Mock::given(path("/events"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Bitcoin funding exceeded.", "error": "bitcoin_funding_exceeded",
        })))
        .mount(&mock)
        .await;

    let (is_error, text) = call(&peer, "get_campaign", json!({"campaign_id": "nope"})).await;
    assert!(is_error, "{text}");

    let (is_error, text) = call(
        &peer,
        "grant_reward",
        json!({"rewards_campaign_id": CAMPAIGN, "user_id": "u", "external_id": "e", "amount": 1_000_000}),
    )
    .await;
    assert!(
        is_error && text.contains("amount"),
        "unknown fields must be rejected: {text}"
    );

    let (is_error, text) = call(
        &peer,
        "grant_reward",
        json!({"rewards_campaign_id": CAMPAIGN, "user_id": "u", "external_id": "e"}),
    )
    .await;
    assert!(
        is_error && text.contains("422") && text.contains("bitcoin_funding_exceeded"),
        "{text}"
    );
    peer.cancel().await.unwrap();
}

// ---- Portal-link redaction regression ----------------------------------------

const PORTAL_TOKEN: &str = "Qm9ndXNUb2tlbkV4YW1wbGVPbmx5RG9Ob3RVc2U";

/// An API payload with portal links in every shape we can think of: the
/// documented field, nested fields, other field names, and inside free text.
fn payload_with_portal_links() -> Value {
    let link = format!("https://portal.bitcoinrewards.app/a/{PORTAL_TOKEN}");
    json!({
        "portal_url": link,
        "event": {"portal_url": link, "status": "pending"},
        "events": [{"portal_link": link}],
        "user": {"withdrawals": [{"note": format!("Withdraw at {link}.")}]},
        "next": link,
    })
}

/// Minimal valid arguments built from a tool's input schema, so every tool,
/// including ones added later, is covered without a hand-written list.
fn sample_args(tool: &Tool) -> Value {
    let schema = serde_json::to_value(&*tool.input_schema).unwrap();
    let mut args = serde_json::Map::new();
    for name in schema["required"].as_array().into_iter().flatten() {
        let name = name.as_str().unwrap();
        let value = if schema["properties"][name]["format"] == "uuid" {
            json!(CAMPAIGN)
        } else {
            json!("user_123")
        };
        args.insert(name.into(), value);
    }
    // update_campaign needs at least one field to change.
    if schema["properties"].get("active").is_some() {
        args.insert("active".into(), json!(true));
    }
    Value::Object(args)
}

/// Calls every listed tool against an API that answers every request with
/// `response`, and returns each tool's (name, is_error, text).
async fn call_every_tool(
    enable: &str,
    response: ResponseTemplate,
) -> (MockServer, Vec<(String, bool, String)>) {
    let (mock, peer) = connect(enable).await;
    Mock::given(any()).respond_with(response).mount(&mock).await;
    let mut results = Vec::new();
    for tool in peer.list_all_tools().await.unwrap() {
        let name = tool.name.to_string();
        let (is_error, text) = call(&peer, name.clone(), sample_args(&tool)).await;
        results.push((name, is_error, text));
    }
    peer.cancel().await.unwrap();
    (mock, results)
}

const ALL_BUT_PORTAL_LINKS: &str = "grant_reward,void_reward,update_campaign";

#[tokio::test]
async fn no_tool_leaks_a_portal_link_from_a_success_response() {
    let (mock, results) = call_every_tool(
        ALL_BUT_PORTAL_LINKS,
        ResponseTemplate::new(200).set_body_json(payload_with_portal_links()),
    )
    .await;
    assert_eq!(results.len(), 8, "every tool except mint_portal_link");
    for (name, is_error, text) in &results {
        assert!(!is_error, "{name} never reached the API: {text}");
        assert!(
            !text.contains(PORTAL_TOKEN),
            "{name} leaked a portal link:\n{text}"
        );
        assert!(text.contains("redacted portal link"), "{name}: {text}");
    }
    let requests = mock.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        results.len(),
        "each tool should call the API once"
    );
}

#[tokio::test]
async fn no_tool_leaks_a_portal_link_from_an_error_response() {
    let (_mock, results) = call_every_tool(
        ALL_BUT_PORTAL_LINKS,
        ResponseTemplate::new(409).set_body_json(payload_with_portal_links()),
    )
    .await;
    for (name, is_error, text) in &results {
        assert!(*is_error && text.contains("409"), "{name}: {text}");
        assert!(
            !text.contains(PORTAL_TOKEN),
            "{name} leaked a portal link:\n{text}"
        );
    }

    // Non-JSON error bodies are scanned too.
    let link = format!("https://portal.bitcoinrewards.app/a/{PORTAL_TOKEN}");
    let (_mock, results) = call_every_tool(
        ALL_BUT_PORTAL_LINKS,
        ResponseTemplate::new(500).set_body_string(format!("<p>try {link}</p>")),
    )
    .await;
    for (name, _, text) in &results {
        assert!(
            !text.contains(PORTAL_TOKEN),
            "{name} leaked a portal link:\n{text}"
        );
    }
}

#[tokio::test]
async fn portal_links_pass_through_once_the_operator_opts_in() {
    let (_mock, results) = call_every_tool(
        "mint_portal_link",
        ResponseTemplate::new(201).set_body_json(payload_with_portal_links()),
    )
    .await;
    for (name, is_error, text) in &results {
        assert!(!*is_error && text.contains(PORTAL_TOKEN), "{name}: {text}");
    }
}
