---
name: oshi-api-rewards
description: Integrate Oshi API Rewards to grant real Bitcoin (sats) to users of any app, game, or service. Covers funding constraints, holding periods, voids, campaigns, and portal links. Use when building features that reward users with Bitcoin via Oshi.
---

# Oshi API Rewards

Reward users of any app, game, or service with real Bitcoin. One API call grants a reward; it pays out over Lightning or lands in a branded rewards portal. Users do not need an Oshi account.

Official docs: https://app.oshi.tech/docs/api  
OpenAPI: https://app.oshi.tech/api-rewards-openapi.yaml

## Core rules (always enforce)

- **Funding**: Rewards can only be paid from Bitcoin-loaded sats in the merchant wallet. Card loads do not fund API rewards. Errors: `bitcoin_funding_exceeded`, `insufficient_merchant_funds`.
- **Holding period**: Every reward starts `pending` for the campaign’s holding period (30–90 days). You can void it while pending. After release/paid, it cannot be reversed.
- **Idempotency**: `external_id` is unique per campaign. Retries with the same `external_id` return the existing event (even if voided). Use a new `external_id` to grant again.
- **Payout modes**:
  - `wallet` (default): sats stay in the user’s Oshi wallet; user withdraws from the portal after the hold.
  - `lightning`: provide `lightning_address` on the grant; paid out once the hold ends.
- **Auth**: Bearer API key in `Authorization` header only. Rate limit: 120 requests/minute per key.

## Typical flow

1. Ensure the merchant wallet is funded with Bitcoin (Dashboard → Rewards Balance).
2. Create or confirm a campaign (amount, holding period, budget, payout mode).
3. Grant: `POST /api/rewards/events` with `rewards_campaign_id`, your stable `user_id`, and a unique `external_id`.
4. Return the `portal_url` (wallet mode) or rely on Lightning payout.
5. Reconcile or void with the event endpoints if needed.

## Tools (when MCP is available)

Prefer the [oshi-mcp](https://github.com/johnzilla/oshi-mcp) tools if connected. Otherwise construct the HTTP calls carefully against the OpenAPI.

- `get_reward` / `lookup_reward` — status and reconciliation
- `list_campaigns` / `get_campaign` — campaign config and usage
- `get_user` — rewarded customer summary
- `grant_reward`, `void_reward`, `update_campaign`, `mint_portal_link` — off by default (they spend sats or mint wallet sign-in links). If one is missing, ask the user to add it to `OSHI_MCP_ENABLE_TOOLS`; don't fall back to raw HTTP calls with their key.

Treat portal URLs as credentials: they sign the holder into the user's wallet. Only hand them to that user.

## Example grant (wallet mode)

```bash
curl -X POST "https://app.oshi.tech/api/rewards/events" \
  -H "Authorization: Bearer $OSHI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "rewards_campaign_id": "72cb8983-f65a-4103-abf7-382c993d0abe",
    "user_id": "user_123",
    "external_id": "workout_2026-10-18_001",
    "metadata": { "source": "fitness_app", "type": "workout" }
  }'
```

Response includes `reward_event_id`, `status` (`pending`), `available_at`, `portal_url`, and fee.

## Notes

- Campaigns are created in the Oshi dashboard; the API can list and edit them.
- Unclaimed rewards eventually return to the merchant balance (per platform rules).
- Shopify purchase verification is optional and only needed for campaigns that enforce it.
- Support: support@oshi.tech
