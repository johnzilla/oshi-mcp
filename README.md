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

### Audit log

Every call to those tools writes one JSON line to stderr, whatever happens: approved, declined, rejected, failed, or unknown (for example a timeout). Most MCP clients save a server's stderr to a log file. Here's a record, wrapped for reading:

```json
{"approval":"approved","audit":"oshi-mcp","campaign_id":"72cb8983-…","campaign_name":"App Referral",
 "external_id":"action_456","issuance_fee_sats":2,"outcome":"ok","reward_event_id":"7f0a1b2c-…",
 "sats":100,"status":"pending","tool":"grant_reward","ts":"2026-10-18T14:03:12.500Z","user_id":"user_123"}
```

`outcome` is one of `ok`, `invalid_input`, `not_approved`, `failed` or `outcome_unknown`. `approval` is `approved`, `not_required` (when `OSHI_MCP_CONFIRM=off`), or `not_given`. Records contain identifiers, amounts, statuses and Oshi's error messages. They never include the API key, metadata or portal links. Read-only tools aren't logged.

To pull the audit records out of a client's log, filter on `"audit":"oshi-mcp"`.

The Shopify purchase-verification endpoints aren't covered yet.

## Security model

- **Least privilege by default.** Tools you haven't enabled are left out of the tool list entirely, and calls to them are refused. The server tells the agent to ask you rather than work around it.
- **A human approves every write.** Enabled tools that spend sats, change campaigns or mint portal links pause for your approval on each call, and refuse to run if they can't ask. Values from Oshi in the prompt (like campaign names) are quoted and escaped, so they can't pass themselves off as part of the prompt.
- **Portal links are credentials.** A portal link signs whoever opens it into the user's wallet. Unless `mint_portal_link` is enabled, every tool result and API error is scanned at any depth: portal-link fields and anything shaped like a portal link are redacted, wherever they appear.
- **Strict inputs.** Ids are percent-encoded per path segment, so a crafted `user_id` like `x/../../events/<id>/void` can't reach a different endpoint. UUIDs, lengths and ranges are checked locally, and unknown arguments are rejected instead of dropped.
- **The key stays in one place.** It's read from `OSHI_API_KEY`, sent only as a bearer header to `https://app.oshi.tech`, never logged, and redacted from debug output. Redirects aren't followed. The API base URL is compiled in, with no override.
- **Hints for your client.** Each tool carries MCP annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, and `openWorldHint` since every tool calls an outside service), so clients that have their own permission prompts can use them too.
- **An audit trail.** Every write attempt leaves a stderr record of the tool, identifiers, amount, approval and outcome, for review afterwards.
- **Honest errors.** Oshi's error message and field errors reach the agent as they are; HTML error pages are summarized rather than dumped into its context. When a request may have taken effect but the result is unknown (a timeout, a 5xx, an unreadable reply), the error says so, and a failed grant tells the agent to check or retry with the *same* `external_id`, never a new one.
- **Treat results as untrusted.** They contain strings that you and your users wrote, such as campaign names and metadata. The server's instructions tell the agent to treat them as data, not instructions.

Oshi's own safeguards still apply: rewards are held 30–90 days and can be voided during the hold, and only bitcoin-loaded sats can fund them.

## Agent skill

[`skills/oshi-api-rewards/SKILL.md`](skills/oshi-api-rewards/SKILL.md) is an [Agent Skill](https://agentskills.io) covering the API's rules and flows, for agents writing integration code. Copy it into your skills directory (for Claude Code, `~/.claude/skills/oshi-api-rewards/`).

## Development

```bash
cargo test      # unit tests plus end-to-end MCP tests against a mocked Oshi API
cargo clippy --all-targets -- -D warnings
cargo audit     # RustSec advisories (cargo install cargo-audit)
```

CI runs lint, tests on Linux, macOS and Windows, `cargo audit`, and the tests on the minimum supported Rust (`rust-version` in `Cargo.toml`, currently 1.88). Releases rerun all of it. `cargo audit` also runs daily, since new advisories don't need a code change.

`tests/errors.rs` replays error responses recorded from the live API (including ones the spec doesn't list, like its HTML 404 page and a 308 redirect), plus network failures and approval-step failures.

To release, bump `version` in `Cargo.toml`, commit to `main`, then tag and push (`git tag -a v0.1.1 -m v0.1.1 && git push origin v0.1.1`). The workflow refuses tags that aren't on `main` or don't match the version, and reruns the full CI suite before building. It builds macOS, Linux (static musl) and Windows binaries, then creates a **draft** release with `SHA256SUMS` and attestations. Publish that draft rather than creating a release by hand. Then run `cargo publish --locked`.

## License

[MIT](LICENSE)
