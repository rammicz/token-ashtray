# Captured usage fixtures

The authenticated GET captures and synthetic variants are identified below.
Antigravity came from its first-party CLI quota command. Claude and Codex used
existing logins without refresh; the Grok billing capture followed the adapter's
OAuth refresh as described below. No credentials or raw response files are
stored in this repository. Sanitization removes identity/token-named fields and
redacts unapproved strings where noted for each fixture.

- claude.json: HTTP 200 GET https://api.anthropic.com/api/oauth/usage ; Authorization Bearer; anthropic-beta oauth-2025-04-20; User-Agent token-ashtray/0.0.0. Source credential ~/.claude/.credentials.json claudeAiOauth.accessToken, read-only.
- codex.json: HTTP 200 GET https://chatgpt.com/backend-api/wham/usage ; Authorization Bearer; ChatGPT-Account-Id; same User-Agent. Source ~/.codex/auth.json tokens.access_token and tokens.account_id, read-only.
- No Kimi/Cursor/Copilot fixture: expected local credentials absent.

Parser relevant shape remains authentic. Redacted strings are literal <redacted>, not genuine provider values. These fixtures prove usage meter parsing, not optional metadata labels that sanitizer removed.

Grok `grok.json`: captured 2026-09-04 by read-only GET of both documented
billing routes after the production adapter refreshed an expired first-party
OIDC login. Contains only sanitized billing/credit quota fields; identity/token
values were not captured. The observed unified billing model is described as
"Unified credits · weekly", not mapped to an unverified marketing tier or price.
Other parser tests use explicitly synthetic boundary inputs derived from cited
first-party field contracts; they are not described as captured fixtures.

Grok `grok-products.json`: synthetic variant of the same sanitized quota shape,
with the product names observed in the 2026-09-05 read-only credits response:
`GrokBuild`, `GrokChat`, and `GrokImagine`. It keeps the observed weekly shared
usage (72%) and attribution values (70%, 1%, 1%) while containing no account
identity or authentication material. The parser treats these as a breakdown of
the shared credits meter, never as independent quota limits.

Grok `grok-user.json`: captured 2026-09-05 by authenticated read-only GET
`https://cli-chat-proxy.grok.com/v1/user?include=subscription` using the existing
CLI access token, with no refresh. HTTP 200 returned `subscriptionTier`;
only that field was retained. Identity fields and authentication material were
discarded in memory. The query and response contract are documented in the
first-party source linked from DATA-SOURCES §3.

Grok policy cross-check (2026-11-22): the current
[Grok FAQ](https://docs.x.ai/grok/faq#usage-limits),
[Bot FAQ](https://docs.x.ai/grok-bot/faq),
[X Subscription usage policy](https://docs.x.ai/x-subscription/usage-limits) and
[Bot policy](https://docs.x.ai/x-subscription/grok-bot) distinguish paid shared
pools, free product limits, legacy Bot billing and per-seat sharing. These public
docs are policy evidence, not current API captures or a new field schema. None of
the tracked Grok fixtures contains Bot attribution, independent free Chat limits
or verified cross-provider account/seat/workspace/pool identity. The inline
`GrokBot` spelling test and Cursor/Bot quota/reset tests use synthetic inputs;
they do not establish a captured upstream enum or dedicated/shared entitlement.
No provider account request was made for this cross-check.

Antigravity `antigravity.json`: captured 2026-10-22 UTC through an authorized
signed-in first-party `agy` CLI, independently verified as version 1.4.4. One
standalone `agy -p /usage --output-format json --print-timeout 15s` invocation
used the CLI's existing login, an 18-second outer timeout and a 65,536-byte
combined-output cap. It exited 0 after 8.9 seconds, capturing 1,141 bytes of
JSON with `status: SUCCESS`, empty `conversation_id`, `num_turns: 0`, and zero
top-level CLI token-usage counters. Those counters are not provider quota.
The probe made no interactive login request, model prompt, separate
credential-store/keyring read or second quota invocation. The raw response is
private and was not stored in the repository. In the published fixture, only
the two account-specific
`command.data.groups[].buckets[].reset_time` timestamps and their two matching
occurrences in the rendered `response` text were replaced with the explicit
placeholder `2000-01-01T00:00:00Z`. All other fields, group/bucket identities,
windows and values remain as captured; the reviewed fields contained no
credential or account identifier. The scrubbed file's SHA-256 is
`ccb2ea92cbf5ec7f00ad275a58175495137614d8badb4abbe9abab84d127a17c`.
It proves only the observed CLI 1.4.4 nested shape and two 100% weekly buckets,
not parser support or behavior for other values, windows, accounts or versions.

Grok Bot `grok-bot-sand.json`: synthetic values in the structure observed on
2026-11-25 from one owner-authorized, read-only
`DashboardService/GetSandUsageStatus` call. The call went through the adapter's
own fetch path with the existing Cursor desktop sign-in, and no token refresh.
Only key names, nesting and value types were recorded. Every value is synthetic:
strings are placeholders, URLs use `example.invalid`, and the usage percentage
and reset time were chosen for the test. The exact format of the
`nextResetTimestampUtc` string was not captured. See DATA-SOURCES §5.

Cursor `cursor-usage.json`: synthetic values in the structure observed on
2026-11-26 from one owner-authorized, read-only Cursor poll through the
adapter's own fetch path (`GetCurrentPeriodUsage` plus `GetPlanInfo`, stored
under `_plan_info`), using the existing Cursor desktop sign-in. Only key names,
nesting and value types were recorded. Every value is synthetic, including the
plan, Auto and API percentages, the 13-digit millisecond cycle timestamps and
the model names. See DATA-SOURCES §4.

Mistral Vibe `vibe-account.json`: synthetic values in the `account/read`
`AccountView` structure observed on 2026-11-26 from one owner-authorized,
read-only `vibe-app-server` (3.1.5) poll on Linux using an isolated temporary
`VIBE_HOME` with `enable_telemetry = false`, an ephemeral session, and the
CLI's existing OS-keyring login. `apiKey` is omitted (`null`), and the plan,
usage percentage and reset timestamp are synthetic placeholders. See
DATA-SOURCES §10.
