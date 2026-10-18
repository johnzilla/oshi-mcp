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

The Shopify purchase-verification endpoints aren't covered yet.

## Security model

- **Least privilege by default.** Tools you haven't enabled are left out of the tool list entirely, and calls to them are refused. The server tells the agent to ask you rather than work around it.
- **Portal links are credentials.** A portal link signs whoever opens it into the user's wallet. Unless `mint_portal_link` is enabled, the `portal_url` that `grant_reward` returns is redacted.
- **Strict inputs.** Ids are percent-encoded per path segment, so a crafted `user_id` like `x/../../events/<id>/void` can't reach a different endpoint. UUIDs, lengths and ranges are checked locally, and unknown arguments are rejected instead of dropped.
- **The key stays in one place.** It's read from `OSHI_API_KEY`, sent only as a bearer header to `https://app.oshi.tech`, never logged, and redacted from debug output. Redirects aren't followed. The API base URL is compiled in, with no override.
- **Hints for your client.** Each tool carries MCP annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`), so clients can ask before write actions.
- **Treat results as untrusted.** They contain strings that you and your users wrote, such as campaign names and metadata. The server's instructions tell the agent to treat them as data, not instructions.

Oshi's own safeguards still apply: rewards are held 30–90 days and can be voided during the hold, and only bitcoin-loaded sats can fund them.

## Agent skill

[`skills/oshi-api-rewards/SKILL.md`](skills/oshi-api-rewards/SKILL.md) is an [Agent Skill](https://agentskills.io) covering the API's rules and flows, for agents writing integration code. Copy it into your skills directory (for Claude Code, `~/.claude/skills/oshi-api-rewards/`).

## Development

```bash
cargo test      # unit tests plus end-to-end MCP tests against a mocked Oshi API
cargo clippy --all-targets -- -D warnings
```

To release, bump `version` in `Cargo.toml`, then commit, tag and push (`git tag v0.1.1 && git push --tags`). The workflow builds macOS, Linux (static musl) and Windows binaries, then creates a **draft** release with `SHA256SUMS` and attestations. Review it and publish it. Then run `cargo publish` so `cargo binstall` can find the crate.

## License

[MIT](LICENSE)
