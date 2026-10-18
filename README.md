# oshi-mcp

An [MCP](https://modelcontextprotocol.io) server for [Oshi API Rewards](https://app.oshi.tech/docs/api), which pays your app's users in real bitcoin. It lets an AI agent look up campaigns, rewards and users, and grant or void rewards once you allow it to.

It's a single static binary with no runtime dependencies. It's **read-only by default**: tools that spend sats stay hidden until you turn them on.

> Independent open-source project, not affiliated with or endorsed by Oshi.

## Install

Pick one:

```bash
# Prebuilt binary, checked against the release's SHA256SUMS (macOS, Linux)
curl -fsSL https://raw.githubusercontent.com/johnzilla/oshi-mcp/main/install.sh | sh

# Prebuilt binary via cargo-binstall
cargo binstall oshi-mcp

# Build from source (needs Rust 1.88+)
cargo install --locked --git https://github.com/johnzilla/oshi-mcp
```

On Windows, download `oshi-mcp-x86_64-pc-windows-msvc.exe` from [Releases](https://github.com/johnzilla/oshi-mcp/releases). Every release binary has a build-provenance attestation, which you can check with `gh attestation verify <file> --repo johnzilla/oshi-mcp`.

Run `oshi-mcp --version` to confirm the install.

## Configure your agent

You need an Oshi merchant account with API Rewards enabled, a wallet funded with bitcoin, and a campaign. The API key is on the **API → Campaigns** page of the dashboard.

**Claude Code**

```bash
claude mcp add --transport stdio --env OSHI_API_KEY=your_key oshi -- oshi-mcp
```

**Cursor, Claude Desktop, Windsurf and others** (`mcpServers` JSON):

```json
{
  "mcpServers": {
    "oshi": {
      "command": "/absolute/path/to/oshi-mcp",
      "env": { "OSHI_API_KEY": "your_key" }
    }
  }
}
```

**Codex** (`~/.codex/config.toml`):

```toml
[mcp_servers.oshi]
command = "oshi-mcp"
env = { OSHI_API_KEY = "your_key" }
```

Desktop apps often don't inherit your shell's `PATH`, so give them the absolute path, e.g. `~/.local/bin/oshi-mcp` expanded.

## Tools

| Tool | Endpoint | Default |
|---|---|---|
| `list_campaigns` | `GET /campaigns` | on |
| `get_campaign` | `GET /campaigns/{id}` | on |
| `get_reward` | `GET /events/{id}` | on |
| `lookup_reward` | `GET /events?external_id=…` | on |
| `get_user` | `GET /users/{id}` | on |
| `grant_reward` | `POST /events` | **off** (spends sats) |
| `void_reward` | `POST /events/{id}/void` | **off** (irreversible) |
| `update_campaign` | `PATCH /campaigns/{id}` | **off** (changes payouts and budgets) |
| `mint_portal_link` | `POST /users/{id}/portal-link` | **off** (mints a wallet sign-in link) |

To turn on any of the tools marked off, list them in `OSHI_MCP_ENABLE_TOOLS`:

```json
"env": {
  "OSHI_API_KEY": "your_key",
  "OSHI_MCP_ENABLE_TOOLS": "grant_reward,void_reward"
}
```

Unknown names stop the server from starting, so a typo can't silently change what's exposed. There's no `all` shortcut.

### Approving each call

Enabled tools that are off by default also ask you to approve **every call**, in your MCP client's own UI (MCP elicitation). The server writes the prompt from validated arguments and Oshi's data, not from anything the agent wrote:

```
Approve paying 100 sats from campaign "App Referral" to user "user_123" for action "action_456"?
Held for 30 days, and can be voided until then. Reusing an action id never pays twice.
```

Nothing is sent to Oshi unless you tick **approve** and accept. Declining, cancelling and waiting 5 minutes all count as no. If your client can't show these prompts, those tools refuse to run, and the error tells the agent why.

For unattended use, or a client without elicitation support, turn approval off on purpose:

```json
"OSHI_MCP_CONFIRM": "off"
```

Any value other than `on` or `off` stops the server from starting.

The Shopify purchase-verification endpoints aren't covered yet.

## Security model

- **Least privilege by default.** Tools you haven't enabled are left out of the tool list entirely, and calls to them are refused. The server tells the agent to ask you rather than work around it.
- **A human approves every write.** Enabled tools that spend sats, change campaigns or mint portal links pause for your approval on each call, and refuse to run if they can't ask. Values from Oshi in the prompt (like campaign names) are quoted and escaped, so they can't pass themselves off as part of the prompt.
- **Portal links are credentials.** A portal link signs whoever opens it into the user's wallet. Unless `mint_portal_link` is enabled, every tool result and API error is scanned at any depth: portal-link fields and anything shaped like a portal link are redacted, wherever they appear.
- **Strict inputs.** Ids are percent-encoded per path segment, so a crafted `user_id` like `x/../../events/<id>/void` can't reach a different endpoint. UUIDs, lengths and ranges are checked locally, and unknown arguments are rejected instead of dropped.
- **The key stays in one place.** It's read from `OSHI_API_KEY`, sent only as a bearer header to `https://app.oshi.tech`, never logged, and redacted from debug output. Redirects aren't followed. The API base URL is compiled in, with no override.
- **Hints for your client.** Each tool carries MCP annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`), so clients that have their own permission prompts can use them too.
- **Treat results as untrusted.** They contain strings that you and your users wrote, such as campaign names and metadata. The server's instructions tell the agent to treat them as data, not instructions.

Oshi's own safeguards still apply: rewards are held 30–90 days and can be voided during the hold, and only bitcoin-loaded sats can fund them.

## Agent skill

[`skills/oshi-api-rewards/SKILL.md`](skills/oshi-api-rewards/SKILL.md) is an [Agent Skill](https://agentskills.io) covering the API's rules and flows, for agents writing integration code. Copy it into your skills directory (for Claude Code, `~/.claude/skills/oshi-api-rewards/`).

## Development

```bash
cargo test      # unit tests plus end-to-end MCP tests against a mocked Oshi API
cargo clippy --all-targets -- -D warnings
```

To release, bump `version` in `Cargo.toml`, commit to `main`, then tag and push (`git tag -a v0.1.1 -m v0.1.1 && git push origin v0.1.1`). The workflow refuses tags that aren't on `main` or don't match the version, and reruns the full CI suite before building. It builds macOS, Linux (static musl) and Windows binaries, then creates a **draft** release with `SHA256SUMS` and attestations. Publish that draft rather than creating a release by hand. Then run `cargo publish --locked`.

## License

[MIT](LICENSE)
