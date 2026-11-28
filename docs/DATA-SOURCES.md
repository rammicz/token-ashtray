# Token Ashtray — Data Sources

Where every number on the board comes from. The point of this document is that
none of this has to be rediscovered. When a provider breaks, start here.

> **These are undocumented, internal endpoints.** They are not public APIs, they
> carry no compatibility promise, and they will change without notice. Breakage
> is expected, not exceptional. Every provider must fail soft.

---

## First run and provider failures

Sign in using the service that owns the login; Token Ashtray has no login form
or API-key setup. Claude uses `claude` then `/login`, Codex uses `codex login`,
Grok uses its CLI sign-in, and Kimi uses `kimi login`. Cursor and Grok Bot share
Cursor desktop sign-in and read its SQLite storage without modifying it.
Copilot uses `copilot login` or an editor sign-in; already-set
`GITHUB_COPILOT_API_TOKEN` and then `GITHUB_TOKEN` take priority over its files.
Mistral Vibe uses `MISTRAL_API_KEY` from the environment or `~/.vibe/.env` when
present; otherwise it queries the installed first-party `vibe-app-server` over
stdio JSON-RPC using an ephemeral session inside an isolated temporary
`VIBE_HOME`, so the CLI retains ownership of its OS-keyring / OIDC login
(`vibe --setup`) without Token Ashtray ever reading the OS credential store; see
[§10](#10-mistral-vibe--live-linux-via-vibe-app-server).
An app opened from a desktop launcher may inherit different environment values
from a terminal. See the provider sections below for exact paths and precedence.

Antigravity requires CLI 1.1.11+ and a completed sign-in (`agy` interactively if
needed). Its bounded read-only `/usage` command owns authentication; the app
never reads its OS credential store. It shows one observed weekly meter per
model group, without inventing a plan or absolute limit. Historical successful
Linux and native Windows samples are documented in [§9](#9-antigravity--live-q1).
Those samples do not establish acceptance of every later app/CLI build.

If a provider shows **Reconnect**, use its displayed sign-in command in the
owning CLI/editor, then refresh. For **Error**, check the shown connection/HTTP
message and try again when the service is reachable. A 429 response imposes
backoff; repeated manual refresh does not bypass it. One provider's failure
does not stop the others. Non-auth failures can retain previous readings with
their original age/error state; a reconnect state clears meters so an expired
login cannot appear live. Missing/unsupported quota values stay unknown.

Diagnostics omit access/refresh tokens and raw response bodies. File errors may
name local credential paths: redact local usernames/account identifiers before
sharing an error, along with app version, provider name and HTTP/status text.
Do not attach login files, databases or raw OAuth/provider responses. Provider
requests go directly to that service; required OAuth refreshes save rotated
credentials atomically while preserving unrelated fields.

Existing external provider/author links open in the system browser on explicit
action. The main-board latest-video action first fetches the public YouTube
feed with a 5-second deadline, 256 KiB body limit and no redirects, then opens
the video or author's videos page. Startup, opening About and hover do not
fetch the feed. It sends the application/version User-Agent, no provider
credentials/auth/cookies; the service sees IP and request time. Browser pages
follow the browser's own session policy. See [G7](REQUIREMENTS.md#g7-network-scope-and-implementation-status).
Automatic updates are off by default; [G7](REQUIREMENTS.md#g7-network-scope-and-implementation-status)
lists their requests. No release is signed yet, so current builds send no
update request; use the install guides' manual update steps.

Live Linux account evidence exists for Claude, Codex, Grok and Mistral Vibe; Antigravity has
historical Linux/native Windows samples. Cursor and Grok Bot have one read-only
native Windows web comparison on a single account ([§5](#5-grok-bot-via-cursor)).
Copilot still needs live-account validation for release claims, and Kimi's captured-account
evidence remains deferred. Parser/fixture tests are not live account evidence.

## 0. Rules that apply to every provider

1. **Read-only.** Credential files and databases are opened read-only. SQLite is
   opened with `file:<path>?mode=ro` via URI so the app can never take a write
   lock on a database another app is using.
2. **Never log a token.** Not at debug level, not truncated, not hashed-for-
   convenience. Log the *source* of a credential (which path matched), never its
   contents. Same for response bodies that embed tokens.
3. **Refresh only when the provider's own protocol requires it.** Some CLIs store
   a short-lived access token plus a refresh token; the access token expires and
   the only way forward is an OAuth refresh, which invalidates the old one. Where
   this is unavoidable the file is rewritten in place, atomically, preserving
   every field the app did not touch. Marked per-provider below. Everywhere else:
   never write.
4. **Timeouts.** Every request gets a hard timeout (20 s is the working figure).
   A hung provider must not stall the poll cycle.
5. **Independent failure.** One provider erroring produces one `Error` /
   `Reconnect` card. It never blanks the board.
6. **Identify honestly.** Send a real `User-Agent` naming the app and version.
7. **Respect the interval.** Default poll is 5 minutes. Do not poll on board
   activation, focus, or repaint — the board displays the cached provider
   snapshot.

### Status semantics

| Status | Meaning |
|---|---|
| `Live` | Fetched successfully this cycle. |
| `Stale` | Last fetch succeeded but is older than the interval (e.g. offline). Show the age. |
| `Reconnect` | Credentials missing or expired and unrefreshable. The user must sign in with the owning CLI or editor again. Show the appropriate instruction. |
| `Error` | Credentials present, request failed or response unparseable. Show the HTTP status. |

`Reconnect` and `Error` are different and must not be collapsed. The first is the
user's to fix, the second is ours.

---

## 1. Claude

| | |
|---|---|
| Credentials | `~/.claude/.credentials.json`, object under key `claudeAiOauth` |
| Written by | Claude Code — `claude`, then `/login` |
| Usage endpoint | `GET https://api.anthropic.com/api/oauth/usage` |
| Token refresh | `POST https://platform.claude.com/v1/oauth/token` |
| Auth | `Authorization: Bearer <access token>` |
| Refresh writes file | **Yes** — refreshed tokens must be written back to `.credentials.json`, otherwise the next run refreshes again and the CLI's own session is invalidated |
| User-facing page | `https://claude.ai/settings/usage` |
| Reconnect hint | "Run `claude` and complete `/login`." |

Notes:

- The credential object carries an expiry. Refresh when expired or near expiry;
  do not refresh eagerly on every poll.
- Absent `claudeAiOauth` inside an otherwise present file means the user is
  logged in by another method — treat as `Reconnect`, not `Error`.
- Supported top-level quota windows: `five_hour` ("5 hours"), `seven_day`
  ("Weekly"), `seven_day_opus` ("Opus weekly"), `seven_day_sonnet` ("Sonnet
  weekly"), `seven_day_oauth_apps` ("OAuth apps weekly"), and
  `seven_day_cowork` ("Cowork weekly").
- Dynamic `limits[]` entries are parsed using explicit `0–100` percentage
  scaling (`percent` or `utilization`) and `resets_at` / `reset_at`, deriving
  neutral labels from `kind`, `scope` (`scope.model.display_name`,
  `scope.display_name`, `scope.model.id`, `scope.surface`), and `name`. Entries
  with `is_active: false` are retained (in observed payloads, `is_active`
  identifies the tightest window rather than enabled state). Entries are
  deduplicated case-insensitively against already-parsed top-level windows and
  earlier `limits[]` entries; entries with missing/invalid percentages or with
  all-missing, blank, or `"<redacted>"` label fields are skipped rather than
  defaulting to `0%` or synthetic labels.
- Billing and unrecognised codename fields (`extra_usage`, `spend`, e.g.
  `nimbus_quill`) are ignored and never emitted as quota meters.

---

## 2. Codex (ChatGPT)

| | |
|---|---|
| Credentials | `~/.codex/auth.json` |
| Written by | Codex CLI login |
| Usage endpoint | `GET https://chatgpt.com/backend-api/wham/usage` |
| Auth | Bearer token from `auth.json` |
| Refresh writes file | No — expired means `Reconnect` |
| User-facing page | `https://chatgpt.com/` |

Notes:

- `wham` is the ChatGPT backend's internal usage surface. This is the single most
  volatile endpoint in the set; expect it to move.

---

## 3. Grok

| | |
|---|---|
| Credentials | `~/.grok/auth.json` — OIDC; look for `key` **or** `access_token`, plus `refresh_token` |
| Written by | Grok CLI login |
| Endpoints | `GET https://cli-chat-proxy.grok.com/v1/user?include=subscription` — plan / identity<br>`GET https://cli-chat-proxy.grok.com/v1/billing` — limits<br>`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` — credit view |
| Token refresh | OIDC refresh using `refresh_token` |
| Refresh writes file | **Yes** — updates `~/.grok/auth.json` in place |
| User-facing page | `https://grok.com/` |

Notes:

- Two credential field names exist in the wild (`key`, `access_token`) depending
  on CLI version. Accept both.
- `/v1/billing` and `/v1/billing?format=credits` return different quota shapes.
  The current first-party CLI also requests `include=subscription` on `/v1/user`;
  its `subscriptionTier` gives the actual tier. This supersedes the earlier
  assumption that marketing tier must be inferred from billing amounts.
  The [typed response](https://github.com/xai-org/grok-build/blob/72a61251fcffb464bcc687aeb5a998e5a98ec0c9/crates/codegen/xai-grok-shell/src/auth/model.rs#L252-L289)
  documents the query requirement. A read-only authenticated check returned
  `XPremiumPlus`; the scrubbed fixture retains that field alone. The adapter
  formats it as “X Premium Plus” without inventing a price. When explicit tier
  metadata is absent, compatible billing evidence may describe the billing
  model/cadence; credit amounts alone never establish a marketing tier.

- The credit view supplies `creditUsagePercent`, `currentPeriod.type` and
  `currentPeriod.end` (or `billingPeriodEnd`), plus optional `productUsage`
  percentages. Because unified billing uses proto3 JSON serialization, zero-valued
  scalars are omitted immediately after a period reset: when `isUnifiedBillingUser`
  is `true`, `currentPeriod` is a non-empty object, and the `creditUsagePercent`
  key is omitted from `/v1/billing?format=credits`, the adapter emits a `0%` meter
  for the reported cadence with `currentPeriod.end` (falling back to
  `billingPeriodEnd`) as the reset time. Present-but-invalid `creditUsagePercent`
  values, missing/empty/non-object `currentPeriod`, legacy `/v1/billing` payloads,
  and unknown shapes never fabricate `0%`. Recognized cadence is shown only when
  reported. Valid product attribution makes this a shared credits meter: observed
  `GrokBuild`, `GrokChat` and `GrokImagine` are displayed as Grok Build, Grok Chat
  and Grok Imagine. Attribution stays Used in either display mode and creates no
  extra allowance or reset. Missing/invalid attribution is omitted; unrelated
  recognized billing meters survive. No free Chat counter or reset is inferred
  from an absent field.
- `/v1/billing?format=credits` is the primary usage request; `/v1/user?include=subscription`
  and legacy `/v1/billing` run concurrently with a shorter 10-second per-request
  timeout and fail soft (degrading to `null`) when `/v1/billing?format=credits`
  succeeds. HTTP `401` (`Reconnect`, taking precedence over non-auth failures) and
  HTTP `429` (`Retry-After` backoff, keeping the maximum backoff across concurrent
  failures) still propagate from any of the three endpoints.
- Requests keep sending `x-grok-client-version` (required by the proxy's
  version-gate check per `xai-org/grok-build`), resolved read-only from
  `~/.grok/version.json` (`version`, falling back to `stable_version`) after
  strict semver validation, with a fallback floor of `1.0.45` when the file is
  absent, unreadable, or invalid.
- [Grok's usage FAQ](https://docs.x.ai/grok/faq#usage-limits) describes paid
  SuperGrok's weekly pool and product breakdown, while free Chat/Voice limits
  have their own schedules. This policy does not document new response fields.
  The September captures below establish the existing credit shape, not current
  X Subscription seat/pool identity. An older `XPremiumPlus` tier or
  `isUnifiedBillingUser` alone cannot establish that identity.

---

## 4. Cursor

| | |
|---|---|
| Credentials | SQLite database `Cursor/User/globalStorage/state.vscdb` |
| Query | `SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'` |
| Written by | Cursor desktop, signed in |
| Auth | `Authorization: Bearer <token>`, `Accept: application/json` |
| Refresh writes file | **No — never write to this database.** Cursor is usually running and holds it open. |
| User-facing page | `https://cursor.com/dashboard/usage` |

Search order for the database (first hit wins):

```
%APPDATA%\Cursor\User\globalStorage\state.vscdb                        Windows
%LOCALAPPDATA%\Cursor\User\globalStorage\state.vscdb                   Windows, alt
~/Library/Application Support/Cursor/User/globalStorage/state.vscdb    macOS
~/.config/Cursor/User/globalStorage/state.vscdb                        Linux
```

Endpoints:

```
POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage
POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo
POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus
POST https://api2.cursor.sh/aiserver.v1.SandBoxService/GetSandUsageStatus
GET  https://api2.cursor.sh/auth/usage
POST https://cursor.com/api/dashboard/get-current-period-usage
GET  https://cursor.com/api/usage-summary
```

Notes:

- `api2.cursor.sh/aiserver.v1.*` and `cursor.com/api/dashboard/get-current-period-usage`
  expect `POST` with a JSON body (`{}` where no arguments are needed);
  `cursor.com/api/usage-summary` and `api2.cursor.sh/auth/usage` use `GET`.
- The `cursor.com/api/*` pair and the `api2` pair overlap. Prefer `api2`; fall
  back through `POST cursor.com/api/dashboard/get-current-period-usage`,
  `GET cursor.com/api/usage-summary`, and `GET api2.cursor.sh/auth/usage` on
  unrecognised HTTP 200 shapes or non-auth/non-429 errors (such as `404`, `405`,
  `5xx`, invalid JSON, or transport errors). HTTP `401` (`Reconnect`) and `429`
  (`Retry-After`) remain terminal on both primary and fallback routes.
- The value read from SQLite may come back as `TEXT` or as `BLOB`. Handle both;
  decode bytes as UTF-8 lossily.
- Two `GetSandUsageStatus` routes exist under different services. Version skew —
  try `DashboardService` first, fall back to `SandBoxService`.

**Usage response structure (live, 2026-11-26).** One owner-authorized,
read-only poll through the adapter's own fetch path made two requests:
`GetCurrentPeriodUsage` and `GetPlanInfo`. The plan-info answer is stored under
`_plan_info`. Only key names, nesting and value types were recorded; no values
were captured. The account was on Cursor's Free plan.

| Key | Type | Use |
|---|---|---|
| `planUsage.totalPercentUsed` | number (float), 0–100 | "Reported plan usage" meter |
| `planUsage.autoPercentUsed` | number (int), 0–100 | "Auto usage" meter |
| `planUsage.apiPercentUsed` | number (int), 0–100 | "API usage" meter |
| `billingCycleEnd` | string, 13-digit epoch milliseconds | reset for all three meters |
| `billingCycleStart` | string, 13-digit epoch milliseconds | not read |
| `planUsage.totalSpend`, `bonusSpend` | number (int) | not read |
| `planUsage.remainingBonus` / `bonusTooltip` | bool / string | not read |
| `spendLimitUsage` `{individualLimit, overallLimit, overallRemaining}` | number (int) | not read |
| `spendLimitUsage` `{limitType}` / `{pooledLimit, pooledRemaining}` | string / numeric string | not read |
| `autoBucketModels[]` | array of strings | not read |
| `displayMessage`, `autoModelSelectedDisplayMessage`, `namedModelSelectedDisplayMessage` | string | not read |
| `displayThreshold` | number (int) | not read |
| `_plan_info.planInfo.planName` | string | card plan label |
| `_plan_info.planInfo.billingCycleEnd` | string, 13-digit epoch milliseconds | fallback reset when primary payload has none |
| `_plan_info.planInfo.price` | string | not read |
| `_plan_info.nextUpgrade` `{description, includedAmountCents, includedUsagePeriod, name, price, tier}` | string / number (int) | not read |

Each of the three meters appears only when its percentage is present and
within 0–100, so a missing or unreadable value never becomes 0%. Flat responses
without a `planUsage` object also accept top-level `totalPercentUsed`,
`autoPercentUsed`, and `apiPercentUsed` (never duplicating when `planUsage` is
an object). Reset resolution checks `billingCycleEnd`, `billingCycleEndMs`,
`planUsage.billingCycleEnd`, `individualUsage.plan.billingCycleEnd`, and
`_plan_info.planInfo.billingCycleEnd` (leaving reset unknown when all are
absent), and also supplies missing resets on legacy `quota_meter` /
`numRequests` meters. This response had no `includedSpend`/`limit` pair, so the
"Included credit budget" meter does not appear for it. The plan label reads
`_plan_info.planInfo.planName` first and keeps the older `_plan_info.planName`
keys as fallbacks. The synthetic fixture `tests/fixtures/cursor-usage.json`
mirrors this structure.

---

## 5. Grok Bot (via Cursor)

The current adapter reads Cursor's `GetSandUsageStatus` using Cursor desktop
credentials, without a separate Bot login. Its reported quota and reset appear
on a separate card with neutral labels.

**Legacy Bot allowance (live, 2026-11-25).** An owner-authorized, read-only
comparison on one SuperGrok account with linked X Premium+ and Cursor
"Free (with X Premium+)" showed that the legacy Grok Bot has its own weekly
allowance, separate from the SuperGrok weekly pool. grok.com showed a separate
"Weekly Grok Bot limit", Cursor's Spending view described it as "Separate from
existing Grok usage", and both showed the same weekly renewal time. That
renewal time differed from the SuperGrok pool's. This account was not on The X
Subscription shared pool, which Cursor offered as an upgrade. Same-seat X
Subscription pooling therefore remains unverified live.

One authenticated `DashboardService/GetSandUsageStatus` call through the
adapter's own fetch path returned this structure. Only key names and value
types were recorded; no values were captured:

| Key | Type | Use |
|---|---|---|
| `usagePercent` | number (float) | Bot meter, used percent on a 0–100 scale; a value below 1 is a small percentage, not a fraction |
| `nextResetTimestampUtc` | string, date-time | Bot meter reset |
| `currentPeriodStart` | string, date-time | not read |
| `billingBrand`, `cursorPlanName`, `grokPlanLabel`, `includedUsageSuperGrokPlan` | string | not read |
| `hasAvailableUsage`, `hasNonZeroIncludedLimit` | bool | not read |
| `onDemandSettings` `{dashboardUrl, eligible, visible}` | object | not read |
| `unifiedPlanMigration` `{href}`, `upgradeOption` `{reason}` | object | not read |
| `upgradeRecommendation`, `upgradeRecommendations[]` `{cta{label, url{url}}, kind, supportingText}` | object, array | not read |

The exact string format of `nextResetTimestampUtc` was not captured. The parser
reads `nextResetTimestampUtc` and snake_case `next_reset_timestamp_utc` as RFC
3339 and, because the keys name UTC, reads a date-time without an offset as UTC.
`resetTime`, `resetAt` and `resets_at` remain fallbacks and still need an
offset. The parser also accepts snake_case `usage_percent` on the same 0–100
scale as `usagePercent`, and unwraps nested JSON-object envelopes
(`usageStatus`, `sandUsageStatus`, `status`, `config`) while ignoring scalar
fields such as `"status": "ok"`. When `usesPooledEnterpriseAllowance` or
`includedLimitZero` (or their snake_case equivalents) is `true` on the root or
unwrapped object, meters are omitted (without triggering a redundant
`SandBoxService` fallback). The synthetic fixture
`tests/fixtures/grok-bot-sand.json` mirrors this structure.

The [Bot FAQ](https://docs.x.ai/grok-bot/faq) distinguishes legacy Cursor billing
from The X Subscription. A legacy SuperGrok link does not merge the Grok.com and
Bot pools. [X Subscription Bot policy](https://docs.x.ai/x-subscription/grok-bot)
describes Premium/Super/Ultra participation and the switch to Grok's usage view.
[Its usage policy](https://docs.x.ai/x-subscription/usage-limits) defines a weekly
pool per paid seat; separate seats and Cursor third-party/Additional Usage remain
separate. The response above has no field that identifies a seat or pool across
providers. The adapter does not deduplicate Grok, Bot or Cursor by plan name, matching
percentages or shared credentials, nor infer missing product limits. If a credit
response explicitly reports a `GrokBot` contribution, the adapter can format it as
Grok Bot; that spelling support is synthetic and does not create a contribution
when absent.

The parser accepts the following reset timestamp forms:

| Shape | Example | Interpretation |
|---|---|---|
| protobuf `Timestamp` object | `{"seconds": 1770000000}` | epoch seconds |
| bare number, `< 1e12` | `1770000000` | epoch seconds |
| bare number, `>= 1e12` | `1770000000000` | epoch milliseconds |
| string | `"2026-02-01T00:00:00Z"` | ISO 8601 |
| string without offset (Bot `nextResetTimestampUtc` / `next_reset_timestamp_utc` only) | `"2026-02-01T00:00:00"`, `"2026-02-01 00:00:00"` | UTC |
| `null` / `""` | | no reset known — render the meter without a reset line |

Same normalisation applies to percentages: accept a number, a numeric string, and
a `0.0–1.0` fraction as well as `0–100` (except `usagePercent` / `usage_percent`,
which are always `0–100`); anything unparseable is a missing meter,
not a zero. **A missing value must never render as 0%** — that would invent a
known quota value.

---

## 6. GitHub Copilot

| | |
|---|---|
| Credentials | environment first, then `apps.json` / `hosts.json` |
| Endpoint | `GET https://api.github.com/copilot_internal/user` |
| Refresh writes file | No |
| User-facing page | `https://github.com/settings/copilot` |

Resolution order:

```
1. $GITHUB_COPILOT_API_TOKEN
2. $GITHUB_TOKEN
3. first existing apps.json / hosts.json under:
     ~/.config/github-copilot/
     ~/Library/Application Support/github-copilot/
     ~/AppData/Local/github-copilot/
     %LOCALAPPDATA%\github-copilot\
     %APPDATA%\GitHub Copilot\
```

Notes:

- `apps.json` and `hosts.json` have **different shapes across editor versions**,
  and the token is nested under a host-keyed object. Probe defensively: walk the
  object and take the first plausible token field rather than assuming a path.
- Environment variables win over files, so a user can override without touching
  their editor install.

---

## 7. Kimi

| | |
|---|---|
| Credentials | `~/.kimi-code/credentials/kimi-code.json` |
| Root override | Nonempty `$KIMI_CODE_HOME` replaces `~/.kimi-code` and is authoritative |
| Current-CLI fallback | If no explicit `KIMI_CODE_HOME` and no legacy file exists, use nonempty `$KIMI_SHARE_DIR` or `~/.kimi`, then `credentials/kimi-code.json` |
| Usage endpoint | `GET https://api.kimi.com/coding/v1/usages` |
| Token refresh | `POST https://auth.kimi.com/api/oauth/token` |
| Refresh writes file | **Yes** |
| User-facing page | `https://www.kimi.com/code` |

---

## 8. Gemini CLI — deprecated

| | |
|---|---|
| Credentials | `~/.gemini/oauth_creds.json`, settings in `~/.gemini/settings.json` |
| Dir override | `$GEMINI_DIR` |
| User-facing page | `https://gemini.google.com/usage` |

**Status: not planned for v1.** Gemini CLI is superseded by Antigravity. Recorded
here only so the path is known if it ever needs reviving.

The browser-cookie route to Gemini usage (reading Chrome/Firefox cookie stores,
or a bookmarklet posting to a local port) is **permanently out of scope** — see
REQUIREMENTS §3. Do not reintroduce it.

---

## 9. Antigravity — live (Q1)

Replacement for Gemini. The selected route is the installed first-party CLI's
quota command using its existing login. Two authenticated successful samples
(CLI 1.4.4 on Linux, 1.5.9 on native Windows 11) established the quota shape,
and the app parses it into one meter per group. States not yet observed stay
unsupported; see [Parsed contract](#parsed-contract-2026-11-18).

### Current protocol decision

Require Antigravity CLI 1.1.11 or newer, then invoke it as a standalone command:

```sh
agy -p /usage --output-format json --print-timeout 15s
```

The first-party [1.1.11 release notes](https://github.com/google-antigravity/antigravity-cli/releases/tag/1.1.11)
document print-mode `/usage` with structured JSON, no agent turn and no quota
spend. The [headless documentation](https://antigravity.google/docs/cli/headless)
requires CLI-handled slash commands to run separately and documents the JSON
output mode, existing-login authentication and `--print-timeout`. The
[usage documentation](https://antigravity.google/docs/cli/commands/usage)
describes model quotas, but does not publish the nested machine JSON schema.
Older CLI versions must not receive this command because they may treat the
slash command as a model prompt.

The CLI owns authentication. The app may use that existing login through this
read-only command under [REQUIREMENTS §3](REQUIREMENTS.md#3-non-goals); it does
not read the CLI's credential store or implement a private HTTP endpoint. The
adapter currently discovers the CLI, enforces the minimum version, and bounds
execution and captured output (15-second CLI print timeout, 18-second process
timeout, 64 KiB output limit). It supplies no interactive login or model turn.
It reports authentication failure and parses successful payloads as described
under [Parsed contract](#parsed-contract-2026-11-18).

On Windows, each CLI invocation gets a temporary, empty `USERPROFILE` and uses
that directory as its working directory. This excludes the user's MCP servers,
plugins and hooks without editing shared configuration. The CLI continues to
own authentication through the Windows user's existing Credential Manager login,
as documented by the [CLI maintainer](https://github.com/google-antigravity/antigravity-cli/issues/1261).
If isolation cannot be created, polling fails instead of using the normal
profile. `CREATE_NO_WINDOW` remains enabled for the direct child; that flag alone
does not prevent MCP descendants from opening their own console windows.
The child also receives `AGY_CLI_DISABLE_AUTO_UPDATE=true` so a fresh profile
does not start the CLI's self-updater on every quota poll; see the
[CLI troubleshooting documentation](https://antigravity.google/docs/cli/troubleshooting/).
File-based logins that are unavailable in the isolated profile report a login
failure; the adapter never copies credentials or starts interactive login.

The earlier bounded attempt in the Linux desktop's existing DBus session
returned exit 1, status `ERROR`, `authentication failed or timed out`, and zero
turns. It was a failure, not a live quota result. No keyring contents,
credentials, account identifiers or raw diagnostic output were captured here.
Static binary metadata and synthetic fixtures did not establish a quota schema;
the successful CLI capture below is the evidence for the observed nested shape.

### Authenticated successful sample (2026-10-22 UTC)

The installed CLI was independently identified as version 1.4.4. One bounded,
standalone invocation of the command above through its existing login exited 0
in 8.9 seconds (18-second outer timeout, 64 KiB combined-output cap). Its JSON
had `status: SUCCESS`, `command.name: usage`, an empty `conversation_id`,
`num_turns: 0`, and zero in all five top-level `usage` token counters. Those
`usage` counters measure the CLI run, **not provider quota**. The probe made no
interactive login request, model prompt, separate credential-store read or
second quota invocation.

The [captured, timestamp-redacted fixture](../tests/fixtures/antigravity.json)
(SHA-256 `ccb2ea92cbf5ec7f00ad275a58175495137614d8badb4abbe9abab84d127a17c`)
preserves the observed envelope and `command.data.groups[].buckets[]` shape.
Its two `reset_time` values and the matching timestamps in `response` were
replaced with the explicit placeholder `2000-01-01T00:00:00Z`; those are **not**
the account's reset instants. No credential or account identifier appeared in
the reviewed fields. The raw response remains private outside the repository;
fixture provenance and redactions are recorded in
[PROVENANCE.md](../tests/fixtures/PROVENANCE.md).

In this sample, `command.data` contains a description and two groups:

| Group (`name`) | Described models | Bucket `id` | `window` | `remaining_fraction` |
|---|---|---|---|---|
| `Gemini Models` | Gemini Flash, Gemini Pro | `gemini-weekly` | `weekly` | `1` |
| `Claude and GPT models` | Claude Opus, Claude Sonnet, GPT-OSS | `3p-weekly` | `weekly` | `1` |

Each observed group has one bucket named `Weekly Limit Remaining`, with keys
`id`, `name`, `window`, `remaining_fraction` and `reset_time`. The actual reset
values were equal RFC 3339 UTC timestamps approximately seven days after capture.
The CLI's `response` rendered each `remaining_fraction: 1` as `100%`. The
description says models within a group share a weekly limit and consumption is
weighted by token cost; it says the limit depends on the user's tier but gives
no tier name or amount. These nested fields are supported by this capture, not
by the public CLI documentation, which does not publish the nested JSON schema.

### Native Windows sample (2026-11-17 UTC)

CLI 1.5.9, installed at `%LOCALAPPDATA%\agy\bin\agy.exe`, ran the same command
with the app's isolation (empty temporary `USERPROFILE`, auto-update disabled,
`CREATE_NO_WINDOW`, null stdin). It exited 0 in 3.3 seconds with 1146 bytes of
JSON and empty stderr; the login came from Windows Credential Manager. The
envelope, groups, bucket keys, `window: weekly` and `remaining_fraction: 1` match
the 1.4.4 capture; only the described model names differ. The verbatim payload
contains no credential or account identifier.
A poll through the production adapter on 2026-11-18 returned a live card with
both meters, and the bucket `reset_time` had moved from 2026-11-24T21:03:33Z to
2026-11-25T14:08:34Z. With an unused quota, the reset therefore appears to be
rolling rather than a fixed weekly boundary; that is an observation, not a
documented contract.

### Parsed contract (2026-11-18)

The parser accepts only `status: SUCCESS` with `command.name: usage`. Each
bucket under `command.data.groups[].buckets[]` becomes one meter when:

- `remaining_fraction` is a JSON number from 0 to 1. Used percent is
  `(1 − remaining_fraction) × 100`. The field name and the CLI's own `100%`
  rendering of `1` establish the direction; the
  [status-line documentation](https://antigravity.google/docs/cli/statusline)
  also shows fractional values below one for a weekly quota.
- The label is `<group name> · Weekly` for `window: weekly`; any other window
  uses the bucket's own `name`, and a bucket with neither is skipped.
- `reset_time` is used when it is an RFC 3339 timestamp; otherwise the meter
  has no reset.

Missing, null, string, negative or greater-than-one fractions are skipped, not
mapped to 0% or 100%. A response without any usable bucket is an error. The
response has no plan, tier, used amount or absolute limit, so the card shows
none. Fractions below one, zero, unavailable/unlimited markers and other windows
have not been observed live; parser tests cover them with explicitly synthetic
inputs only.

### Superseded investigation record

The following Windows 11 observations and earlier research proposal are kept
for provenance. They do not direct current credential, keyring, log or private
endpoint discovery.

Observed on a Windows 11 machine with Antigravity CLI and IDE installed:

```
~/.gemini/antigravity-cli/
    settings.json          only: enableTelemetry, model, trustedWorkspaces — no creds
    installation_id        36 bytes, UUID-shaped
    jetski_state.pbtxt     518 B, protobuf text
    conversation_summaries.db
    history.jsonl
    cli.log
    bin/ brain/ cache/ conversations/ crashes/ log/ mcp/ presence/ updater/

~/.gemini/antigravity/
    antigravity_state.pbtxt      822 B, protobuf text
    user_settings.pb             41 B
    agyhub_summaries_proto.pb
    browserAllowlist.txt
    bin/ brain/ context_state/ conversations/ knowledge/ mcp/ playground/

~/.gemini/antigravity-browser-profile/
~/.gemini/antigravity-ide/
%LOCALAPPDATA%\Antigravity\
%LOCALAPPDATA%\antigravity-updater\
%APPDATA%\Antigravity\
```

Historical findings:

- No plaintext JSON credential file was observed in those paths, unlike the
  known Claude, Kimi and Gemini CLI files. Observed state included protobuf
  (`.pbtxt` text format and `.pb` binary).
- `~/.config/opencode/antigravity-accounts.json` exists on the same machine, but
  that is **OpenCode's** file, not Antigravity's. Reading it would only work for
  users who also run OpenCode; it is not the selected source.

The superseded proposal was to:

1. Read `antigravity_state.pbtxt` and `jetski_state.pbtxt` for protobuf field names.
2. Look in Windows Credential Manager, macOS Keychain or `cache/` for a token.
3. Watch CLI traffic for a quota endpoint and plan/limit response.
4. Search `cli.log` for endpoint hostnames.

The original keyring concern applied to the app directly reading credentials;
it does not prohibit the accepted CLI-owned existing authentication route.

### Earlier first-party and local evidence (2026-09-05)

The official [CLI installation/authentication documentation](https://antigravity.google/docs/cli/install/)
and [first-party README](https://github.com/google-antigravity/antigravity-cli/blob/3bc5795ff561c9d71bf1ce272f185aec6013e5e4/README.md)
place signed-in credentials in the operating-system keyring: Keychain, Linux
Secret Service, or Windows Credential Manager. No keyring access was attempted.

The official [usage command documentation](https://antigravity.google/docs/cli/commands/usage)
documents interactive `/usage` and `/quota`, but publishes neither a quota HTTP
endpoint nor its response schema. The public first-party repository contains
documentation/examples/changelog rather than the implementation needed to
establish that protocol. A changelog reference to an internal file-storage
logout fallback does not identify a supported readable path or schema.

The earlier local-installation check is superseded: the user installed
`~/.local/bin/agy` (1.1.27) and resumed this integration on 2026-09-05.
The new CLI state contains onboarding/migration fields; its cache contains
onboarding flags and a project identifier, not readable login credentials.

---

## 10. Mistral Vibe — live Linux via `vibe-app-server`

### Protocol (from the first-party CLI source, 2026-11-24 / 2026-11-26)

The installed `mistral-vibe` Python package (verified on 3.1.5) is the protocol reference:

- **Credential & authentication boundary.** `vibe/setup/auth/api_key_persistence.py`
  stores `MISTRAL_API_KEY` in the OS keyring (`ai.mistral.vibe`, legacy `vibe`)
  whenever a keyring backend is available, and removes any plaintext copy from
  `~/.vibe/.env` (falling back to `$VIBE_HOME/.env` / `~/.vibe/.env` only on
  `KeyringError`). Browser sign-in uses OIDC tokens in the OS keyring
  (`ai.mistral.vibe.oidc`). Token Ashtray **never reads the OS keyring
  directly** ([REQUIREMENTS §3](REQUIREMENTS.md#3-non-goals)):
  1. **Direct HTTP path** (when `MISTRAL_API_KEY` is set in the environment or
     readable from `$VIBE_HOME/.env` / `~/.vibe/.env`, read-only, no refresh,
     no writes): sends `Authorization: Bearer <key>` to
     `GET https://api.mistral.ai/v1/billing/subscription` and
     `GET https://api.mistral.ai/v1/billing/budget` (per `vibe/app_server/_billing.py`).
  2. **CLI-owned-auth path** (default for desktop installs whose login lives in
     the OS keyring or OIDC): discovers the installed first-party
     `vibe-app-server` (`~/.local/bin`, `PATH`, or sibling of the resolved `vibe`
     launcher) and runs a bounded, read-only newline-delimited JSON-RPC 2.0
     sequence over stdio:
     `initialize` → `initialized` → `session/start` (`kind: "ephemeral"`) →
     `account/read` → `session/stop`.
     The child runs with `VIBE_HOME` and `cwd` set to a fresh temporary
     directory containing only `config.toml` (`enable_telemetry = false`),
     excluding user MCP servers, plugins, hooks and `~/.vibe` files while
     leaving `~/.vibe` untouched and allowing `vibe-app-server` to authenticate
     through its own OS-keyring login with zero model turns. Execution is
     bounded (18-second timeout, 512 KiB stdout cap, `kill_on_drop`,
     `CREATE_NO_WINDOW` on Windows, clean reap before temp-dir cleanup).
- **Direct HTTP endpoints** (`vibe/app_server/_billing.py`):
  - `billing/subscription` → `{"plan_name": string, "vibe_payg_enabled": bool|null}`
  - `billing/budget` → `{"vibe_budget": {"usage_percentage": number, "initial_budget":
    number, "reset_at": string|null, "payg_enabled": bool|null}} | null`
- **`account/read` response shape** (`vibe/app_server/models.py::AccountView`):
  - `status`: `"ready"` | `"missing_key"` (`Reconnect`) | `"unauthorized"` (`Reconnect`) | `"unavailable"` (`Error`)
  - `plan`: `{"kind": "chat"|"api"|"mistral_code", "name": string, "title": string|null}`
  - `budget`: `{"usagePercentage": number, "resetAt": string|null, "paygEnabled": bool} | null`
- **Semantics.** `usage_percentage` / `usagePercentage` is a 0–100 percentage
  and may exceed 100 under pay-as-you-go ("usage past 100% is billed instead of
  paused", `app_server/models.py::AccountBudgetView`); the meter clamps at 100.
  `reset_at` / `resetAt` is an RFC3339 string. `initial_budget` is the monthly
  allowance amount; `<= 0` suppresses the meter (matching `vibe/app_server/_account.py`)
  and its currency is unstated so no price is shown.
  A **403** on `billing/budget` (missing `see_org_inference_usage`) or **404**
  (platform without billing) means "no budget", never an error; **401** means
  `Reconnect`. When no budget is served, the plan alone is shown.
- **Live Linux check (2026-11-26).** One owner-authorized, read-only poll
  through `provider_probe vibe` on Linux with `vibe` 3.1.5 signed in via the
  OS keyring (no `MISTRAL_API_KEY` env var and no `~/.vibe/.env`) discovered
  `vibe-app-server`, completed the isolated `account/read` handshake in ~2.8 s
  with exit 0, and returned a `Live` card with plan `Pro`, meter
  `Vibe Code allowance` and an RFC 3339 UTC `resetAt`. The synthetic structural
  fixture `tests/fixtures/vibe-account.json` mirrors the observed `AccountView`
  shape without retaining account values. Native Windows validation of
  `vibe-app-server.exe` remains deferred to a Windows machine.

---

## 11. How to add a new provider

The pattern every provider above follows:

1. **Find the login on disk.** The CLI had to persist it somewhere. Look in
   `~/.<tool>/`, `~/.config/<tool>/`, `~/Library/Application Support/<tool>/`,
   `%APPDATA%`, `%LOCALAPPDATA%`. Prefer JSON; SQLite next; protobuf last.
2. **Find the endpoint.** Watch the CLI's traffic while it displays quota, or
   grep its logs for hostnames. The provider's own web dashboard is a second
   source — the XHR it makes for the usage page is usually the same API.
3. **Record the response shape**, specifically: which field is *used*, which is
   *limit*, and which is *reset*. Note whether percent arrives as `0–1` or
   `0–100`.
4. **Decide the refresh story.** Access token only, or access + refresh? If a
   refresh is needed, does writing the file back break the CLI?
5. **Write it down here first**, then implement.

Never require the user to paste a key. If a provider cannot be read from an
existing login, it is not supported — see REQUIREMENTS §3.

## 12. Implementation verification (2026-09-05)

Live Linux account checks succeeded for Claude, Codex and Grok. Scrubbed quota
payloads are in `tests/fixtures/`; provenance distinguishes captured payloads
from synthetic boundary examples. Cursor, Grok Bot, Copilot and Kimi credentials
were unavailable here, so their live endpoint behavior remains unverified.

- Claude uses the installed first-party CLI OAuth client, JSON refresh and the
  OAuth beta header. Refresh saves preserve unrelated JSON fields.
- Codex reads the existing access token and account ID, sends
  `ChatGPT-Account-Id`, and never refreshes the CLI login. Additional quota names
  are displayed neutrally instead of assigning undocumented model meanings.
- Grok stores credentials under an issuer/client key, including `key`,
  `refresh_token` and an RFC3339 `expires_at`. Requests include CLI token-auth,
  user ID, client-version and mode headers. Both legacy billing and credits
  views are read. A real refresh and usage request succeeded. Unified credit
  flags and a zero legacy monthly allowance do not imply a marketing price.
  The supported subscription query now supplies the actual backend tier;
  `grok-user.json` captures its single whitelisted metadata field.
  Protocol reference: [xAI's Grok CLI source](https://github.com/xai-org/grok-build).
- Kimi honors the documented `KIMI_CODE_HOME` override and legacy directory;
  when neither exists it also discovers the current CLI's `KIMI_SHARE_DIR` or
  `~/.kimi` directory. Refresh follows the first-party
  [OAuth implementation](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/oauth.py).
- Cursor SQLite is opened read-only with escaped file URIs and accepts TEXT or
  BLOB credential values. `totalPercentUsed` is an explicit 0–100 percentage,
  separate from spend/limit amounts. A 429 retains its backoff through fallback
  endpoints instead of triggering another request.
- Copilot environment priority and recursive apps/hosts discovery remain as
  documented. Quota snapshots interpret percent remaining and unlimited
  separately; zero-entitlement windows do not manufacture limits.
- **Historical, superseded by [§9](#9-antigravity--live-q1):** Antigravity's
  expected local state/log files were absent on the development
  machine. No supported token and quota endpoint could be established. No
  keychain, browser profile, OpenCode login or speculative endpoint was used;
  implementation awaits actual protocol evidence.

OAuth refresh runs to its bounded completion even if a polling task is
cancelled, then atomically saves rotated tokens. Concurrent CLI modifications
are checked after temporary-file fsync immediately before replacement. The
portable filesystem API has no compare-and-swap rename against an uncooperative
CLI, so a write precisely between that final check and rename remains a race. No access/refresh tokens or raw OAuth
responses are included in diagnostics.

### Kimi fixture evidence

The first-party [usage implementation](https://github.com/MoonshotAI/kimi-cli/blob/86f136422a0aae6b217ea49e7ea1d2e8a1defcd2/src/kimi_cli/ui/shell/usage.py#L75-L145)
confirms the coding usages endpoint, Bearer authentication, remaining/used
amounts and reset aliases. Its [usage tests](https://github.com/MoonshotAI/kimi-cli/blob/86f136422a0aae6b217ea49e7ea1d2e8a1defcd2/tests/ui/test_usage.py)
construct synthetic rows; the inspected upstream tree contains no captured
quota JSON or HTTP replay with provenance. Local Kimi logins/overrides are
absent. The captured-fixture gate for Kimi remains unverified; existing parser
examples are explicitly synthetic rather than mislabeled account captures.

### Deferred evidence, user decision (2026-09-05)

The user deferred Kimi captured-account evidence and initially deferred Antigravity
research. Antigravity research resumed after the user installed its CLI. The
Kimi parser and login discovery remain implemented with their current stated
evidence limits; no synthetic fixture is relabeled as captured. Antigravity
became live on 2026-11-18; see §9. Native Windows credential-path validation is likewise
deferred to the user's Windows machine. These decisions do not convert missing
evidence into successful verification.
