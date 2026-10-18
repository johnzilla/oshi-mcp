//! Helpers shared by the end-to-end test files.
#![allow(dead_code)] // each test file uses a different subset

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use oshi_mcp::{
    client::OshiClient,
    config::{ApiKey, parse_enabled_tools},
    server::OshiServer,
};
use rmcp::{
    ClientHandler, ErrorData, RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
        ElicitationAction, JsonObject, Tool,
    },
    service::{RequestContext, RunningService},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

pub const CAMPAIGN: &str = "72cb8983-f65a-4103-abf7-382c993d0abe";

/// Connects a client with per-call approval off.
pub async fn connect(enable: &str) -> (MockServer, RunningService<RoleClient, ()>) {
    connect_with(enable, false, ()).await
}

pub async fn connect_with<C: ClientHandler>(
    enable: &str,
    confirm_calls: bool,
    client_handler: C,
) -> (MockServer, RunningService<RoleClient, C>) {
    let mock = MockServer::start().await;
    let client = OshiClient::new(&ApiKey::new("test_key").unwrap(), &mock.uri()).unwrap();
    let peer = serve(client, enable, confirm_calls, client_handler).await;
    (mock, peer)
}

/// Serves an `OshiServer` built around `client` and connects `client_handler` to it.
pub async fn serve<C: ClientHandler>(
    client: OshiClient,
    enable: &str,
    confirm_calls: bool,
    client_handler: C,
) -> RunningService<RoleClient, C> {
    let server = OshiServer::new(client, &parse_enabled_tools(enable).unwrap(), confirm_calls);
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await });
    client_handler.serve(client_io).await.unwrap()
}

pub fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

/// Returns (is_error, text) for a tool call result.
pub async fn call<C: ClientHandler>(
    peer: &RunningService<RoleClient, C>,
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

/// Minimal valid arguments built from a tool's input schema, so every tool,
/// including ones added later, is covered without a hand-written list.
pub fn sample_args(tool: &Tool) -> Value {
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

pub const GATED: &str = "grant_reward,void_reward,update_campaign,mint_portal_link";

#[derive(Clone, Copy, Debug)]
pub enum Answer {
    Approve,
    /// Accepts the form but leaves the approval box unticked.
    Unticked,
    Decline,
    Cancel,
    /// Answers the elicitation request with a JSON-RPC error.
    Error,
    /// Accepts without sending any form content.
    NoContent,
    /// Accepts with content that doesn't match the schema.
    Malformed,
}

/// A client that supports elicitation, answers every prompt the same way, and
/// records what it was asked.
#[derive(Clone)]
pub struct Approver {
    answer: Answer,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl Approver {
    pub fn new(answer: Answer) -> Self {
        Self {
            answer,
            prompts: Arc::default(),
        }
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

impl ClientHandler for Approver {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.capabilities = ClientCapabilities::builder().enable_elicitation().build();
        info
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            self.prompts.lock().unwrap().push(message.clone());
        }
        Ok(match self.answer {
            Answer::Approve => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({"approve": true}))
            }
            Answer::Unticked => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({"approve": false}))
            }
            Answer::Decline => ElicitResult::new(ElicitationAction::Decline),
            Answer::Cancel => ElicitResult::new(ElicitationAction::Cancel),
            Answer::Error => return Err(ErrorData::internal_error("client UI crashed", None)),
            Answer::NoContent => ElicitResult::new(ElicitationAction::Accept),
            Answer::Malformed => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({"approve": "yes"}))
            }
        })
    }
}

pub async fn mount_campaign(mock: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/campaigns/{CAMPAIGN}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"campaign": {
                "name": "App Referral", "fixed_reward_sats": 100, "holding_period_days": 30,
            }})),
        )
        .mount(mock)
        .await;
}

pub fn grant_args() -> Value {
    json!({
        "rewards_campaign_id": CAMPAIGN,
        "user_id": "user_123",
        "external_id": "action_456",
        "lightning_address": "satoshi@wallet.example",
    })
}

pub async fn posts(mock: &MockServer) -> usize {
    let requests = mock.received_requests().await.unwrap();
    requests
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .count()
}
