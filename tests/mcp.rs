//! End-to-end: a real MCP client talks to the server over an in-memory pipe,
//! and the server talks to a mocked Oshi API.

use oshi_mcp::{
    client::OshiClient,
    config::{ApiKey, parse_enabled_tools},
    server::OshiServer,
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, JsonObject},
    service::RunningService,
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
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
    name: &'static str,
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
