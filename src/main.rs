//! stdio entry point. Configuration comes only from the environment:
//! `OSHI_API_KEY` (required) and `OSHI_MCP_ENABLE_TOOLS` (optional).

use anyhow::Result;
use oshi_mcp::{
    client::{DEFAULT_BASE_URL, OshiClient},
    config::{Config, GATED_TOOLS},
    server::OshiServer,
};
use rmcp::{ServiceExt, transport::stdio};

const VERSION: &str = concat!("oshi-mcp ", env!("CARGO_PKG_VERSION"));

fn usage() -> String {
    format!(
        "{VERSION}\n\
         MCP server for Oshi API Rewards, spoken over stdio.\n\n\
         Environment:\n  \
           OSHI_API_KEY           Oshi API key (required)\n  \
           OSHI_MCP_ENABLE_TOOLS  Comma-separated tools that spend sats or mint wallet links.\n                         \
                                  Off unless listed: {}\n  \
           OSHI_MCP_CONFIRM       \"on\" (default): those tools ask the user to approve each\n                         \
                                  call via MCP elicitation, and refuse if the client can't.\n                         \
                                  \"off\": skip approval.\n",
        GATED_TOOLS.join(", ")
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("{VERSION}");
            return Ok(());
        }
        Some("--help" | "-h") => {
            print!("{}", usage());
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown argument {other:?}\n\n{}", usage()),
        None => {}
    }

    let config = Config::from_env()?;
    let client = OshiClient::new(&config.api_key, DEFAULT_BASE_URL)?;
    let server = OshiServer::new(client, &config.enabled_tools, config.confirm_calls);
    // stdout carries the protocol, so diagnostics go to stderr.
    eprintln!(
        "{VERSION}: tools enabled: {}; per-call approval: {}",
        server.tool_names().join(", "),
        if config.confirm_calls { "on" } else { "off" }
    );

    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}
