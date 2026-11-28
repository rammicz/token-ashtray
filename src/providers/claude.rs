use super::*;
use serde_json::json;
use std::io::Write;
pub struct Claude;
const D: Descriptor = Descriptor {
    id: "claude",
    name: "Claude",
    link: "https://claude.ai/settings/usage",
    login: "Run `claude`, then `/login`.",
};
const USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE: &str = "https://api.anthropic.com/api/oauth/profile";
const REFRESH: &str = "https://platform.claude.com/v1/oauth/token";

fn discover_at(path: PathBuf, diagnostic: &mut impl Write) -> Result<Credentials> {
    let credentials = Credentials::load(path.clone(), D, vec!["claudeAiOauth".into()])?;
    let _ = writeln!(diagnostic, "Claude credentials: {}", path.display());
    Ok(credentials)
}

fn build_refresh_body(credentials: &Credentials) -> Result<RefreshBody> {
    let refresh = credentials.refresh(D)?;
    let scope = credentials
        .object()
        .get("scopes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "user:profile user:inference".into());
    Ok(RefreshBody::Json(json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        "scope": scope,
    })))
}

#[cfg(test)]
fn refresh_body(credentials: &Credentials) -> Result<Option<RefreshBody>> {
    if !credentials.near_expiry() {
        return Ok(None);
    }
    build_refresh_body(credentials).map(Some)
}

/// The credentials file caches `subscriptionType` at sign-in and never notices
/// a plan change, so prefer the live profile. `plan_display_name` is what the
/// account page shows ("Ultra"); `organization_type` ("claude_max") is the
/// fallback. Returns `None` on any profile failure so usage still renders.
async fn live_plan_at(
    client: &Client,
    credentials: &Credentials,
    profile_endpoint: &str,
) -> Option<String> {
    let access = credentials.access(D).ok()?;
    let Ok(profile) = response(
        client
            .get(profile_endpoint)
            .bearer_auth(access)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await,
        profile_endpoint,
        D,
    )
    .await
    else {
        return None;
    };
    plan_from_profile(&profile)
}

async fn fetch_at(
    client: &Client,
    credentials: &mut Credentials,
    usage_endpoint: &str,
    profile_endpoint: &str,
    refresh_endpoint: &str,
    no_proxy_refresh: bool,
) -> Result<Value> {
    let mut payload = fetch_with_reactive_refresh(
        credentials,
        RefreshTarget {
            descriptor: D,
            endpoint: refresh_endpoint,
            camel_case: true,
            no_proxy: no_proxy_refresh,
        },
        build_refresh_body,
        |token| async move {
            response(
                client
                    .get(usage_endpoint)
                    .bearer_auth(token)
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .send()
                    .await,
                usage_endpoint,
                D,
            )
            .await
        },
    )
    .await?;
    let cached = credentials
        .object()
        .get("subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(plan) = live_plan_at(client, credentials, profile_endpoint)
        .await
        .or(cached)
    {
        payload["_plan"] = json!(plan);
    }
    Ok(payload)
}

fn plan_from_profile(profile: &Value) -> Option<String> {
    let org = &profile["organization"];
    org.get("plan_display_name")
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            org.get("organization_type")
                .and_then(Value::as_str)
                .and_then(|tier| tier.strip_prefix("claude_").map(str::to_owned))
        })
}

#[async_trait]
impl Provider for Claude {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        discover_at(
            home().join(".claude/.credentials.json"),
            &mut std::io::stderr().lock(),
        )
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        fetch_at(client, credentials, USAGE, PROFILE, REFRESH, false).await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let meters = [
            ("five_hour", "5 hours"),
            ("seven_day", "Weekly"),
            ("seven_day_opus", "Opus weekly"),
            ("seven_day_sonnet", "Sonnet weekly"),
            ("seven_day_oauth_apps", "OAuth apps weekly"),
        ]
        .into_iter()
        .filter_map(|(key, label)| quota_meter(label, &payload[key]))
        .collect::<Vec<_>>();
        if meters.is_empty() {
            return Err(D.invalid());
        }
        Ok(Usage {
            meters,
            plan: string_at(payload, &["_plan"]),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    #[test]
    fn discovery_logs_only_matched_path_without_changing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let bytes = br#"{"claudeAiOauth":{"accessToken":"sentinel-access-secret","refreshToken":"sentinel-refresh-secret"},"untouched":{"value":42}}"#;
        std::fs::write(&path, bytes).unwrap();
        let mut diagnostic = Vec::new();
        discover_at(path.clone(), &mut diagnostic).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let emitted = String::from_utf8(diagnostic).unwrap();
        assert_eq!(emitted, format!("Claude credentials: {}\n", path.display()));
        assert!(!emitted.contains("sentinel-access-secret"));
        assert!(!emitted.contains("sentinel-refresh-secret"));
    }

    #[test]
    fn missing_oauth_object_requests_reconnect_without_logging_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let bytes = br#"{"other":{"accessToken":"sentinel-other-secret"}}"#;
        std::fs::write(&path, bytes).unwrap();
        let mut diagnostic = Vec::new();
        let error = match discover_at(path.clone(), &mut diagnostic) {
            Ok(_) => panic!("missing Claude login was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.status, Status::Reconnect);
        assert!(diagnostic.is_empty());
        assert!(!error.message.contains("sentinel-other-secret"));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn expiry_gate_skips_valid_login_and_catches_near_or_past_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let now_ms = || time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
        let mut diagnostic = Vec::new();
        // The gate refreshes within 60 s of expiry. Each case reads the clock
        // when it writes, and the just-outside case leaves slack for a slow machine.
        for (offset_seconds, should_refresh) in [(3600, false), (75, false), (30, true), (-5, true)]
        {
            std::fs::write(
                &path,
                json!({"claudeAiOauth":{"accessToken":"sentinel-access-secret","expiresAt":now_ms() + offset_seconds * 1000}}).to_string(),
            )
            .unwrap();
            let credentials = discover_at(path.clone(), &mut diagnostic).unwrap();
            assert_eq!(
                credentials.near_expiry(),
                should_refresh,
                "offset {offset_seconds}"
            );
            let prepared = refresh_body(&credentials);
            if should_refresh {
                assert_eq!(prepared.err().unwrap().status, Status::Reconnect);
            } else {
                assert!(prepared.unwrap().is_none(), "offset {offset_seconds}");
            }
        }
        std::fs::write(
            &path,
            json!({"claudeAiOauth":{
                "accessToken":"sentinel-access-secret",
                "refreshToken":"sentinel-refresh-secret",
                "expiresAt":now_ms() + 30_000,
                "scopes":["user:profile"]
            }})
            .to_string(),
        )
        .unwrap();
        let credentials = discover_at(path, &mut Vec::new()).unwrap();
        let Some(RefreshBody::Json(body)) = refresh_body(&credentials).unwrap() else {
            panic!("near-expiry Claude login did not prepare a refresh");
        };
        assert_eq!(body["refresh_token"], "sentinel-refresh-secret");
        assert_eq!(body["scope"], "user:profile");
    }

    #[test]
    fn rotated_claude_tokens_preserve_unrelated_root_and_nested_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let bytes = br#"{"other":{"keep":true},"claudeAiOauth":{"accessToken":"sentinel-old-access","refreshToken":"sentinel-old-refresh","scopes":["user:profile"],"custom":{"keep":42}}}"#;
        std::fs::write(&path, bytes).unwrap();
        let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
        credentials
            .apply_refresh(
                &json!({"access_token":"sentinel-new-access","refresh_token":"sentinel-new-refresh","expires_in":3600}),
                true,
            )
            .unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["other"], json!({"keep":true}));
        assert_eq!(saved["claudeAiOauth"]["custom"], json!({"keep":42}));
        assert_eq!(saved["claudeAiOauth"]["scopes"], json!(["user:profile"]));
        assert_eq!(saved["claudeAiOauth"]["accessToken"], "sentinel-new-access");
        assert_eq!(
            saved["claudeAiOauth"]["refreshToken"],
            "sentinel-new-refresh"
        );
        assert!(saved["claudeAiOauth"]["expiresAt"].as_i64().unwrap() > 0);
        assert!(!credentials.near_expiry());
    }

    #[test]
    fn captured_usage_and_missing_meter() {
        let v: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/claude.json")).unwrap();
        let usage = Claude.parse(&v).unwrap();
        assert_eq!(usage.meters.len(), 2);
        assert_eq!(usage.meters[0].used_percent, 18.0);
        assert!(usage.meters[0].resets_at.is_some());
        assert!(
            Claude
                .parse(&json!({"five_hour":{"utilization":null}}))
                .is_err()
        );
    }

    #[test]
    fn profile_plan_prefers_display_name_then_organization_type() {
        assert_eq!(
            plan_from_profile(&json!({
                "organization": {
                    "organization_type": "claude_max",
                    "plan_display_name": "Ultra",
                }
            }))
            .as_deref(),
            Some("Ultra")
        );
        assert_eq!(
            plan_from_profile(&json!({
                "organization": {"organization_type": "claude_max"}
            }))
            .as_deref(),
            Some("max")
        );
        assert_eq!(plan_from_profile(&json!({"organization": {}})), None);
        assert_eq!(plan_from_profile(&json!({})), None);
    }

    const CLAUDE_USAGE_JSON: &str =
        r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"}}"#;
    const CLAUDE_PROFILE_JSON: &str = r#"{"organization":{"plan_display_name":"Max"}}"#;
    const CLAUDE_REFRESH_JSON: &str = r#"{"access_token":"synthetic-new-access","refresh_token":"synthetic-new-refresh","expires_in":3600}"#;

    async fn run_claude_local_fetch(
        client: &Client,
        credentials: &mut Credentials,
        usage_replies: Vec<crate::providers::test_http::ScriptedReply>,
        profile_replies: Vec<crate::providers::test_http::ScriptedReply>,
        refresh_replies: Vec<crate::providers::test_http::ScriptedReply>,
    ) -> (Result<Value>, Vec<String>, Vec<String>, Vec<String>) {
        use crate::providers::test_http::MockServer;
        let usage_srv = MockServer::spawn("/api/oauth/usage", usage_replies);
        let profile_srv = MockServer::spawn("/api/oauth/profile", profile_replies);
        let refresh_srv = MockServer::spawn("/v1/oauth/token", refresh_replies);
        let result = fetch_at(
            client,
            credentials,
            usage_srv.url(),
            profile_srv.url(),
            refresh_srv.url(),
            true,
        )
        .await;
        (
            result,
            usage_srv.finish(),
            profile_srv.finish(),
            refresh_srv.finish(),
        )
    }

    #[tokio::test]
    async fn reactive_refresh_on_401_rotates_disk_and_retries_usage_and_live_plan() {
        use crate::providers::test_http::{ScriptedReply, local_client};
        let client = local_client();
        let future_ms = (time::OffsetDateTime::now_utc().unix_timestamp() + 3600) * 1000;

        // Test both non-expiring `expiresAt` and absent expiry field.
        for oauth_entry in [
            json!({
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": future_ms,
                "scopes": ["user:profile", "user:inference"],
                "custom": {"keep": 7}
            }),
            json!({
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "scopes": ["user:profile", "user:inference"],
                "custom": {"keep": 7}
            }),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials.json");
            std::fs::write(
                &path,
                json!({"other": {"keep": 42}, "claudeAiOauth": oauth_entry}).to_string(),
            )
            .unwrap();
            let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();

            let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
                &client,
                &mut credentials,
                vec![
                    ScriptedReply::status(401, r#"{"error":"revoked"}"#),
                    ScriptedReply::ok(CLAUDE_USAGE_JSON),
                ],
                vec![ScriptedReply::ok(CLAUDE_PROFILE_JSON)],
                vec![ScriptedReply::ok(CLAUDE_REFRESH_JSON)],
            )
            .await;
            let payload = result.expect("reactive refresh on 401 should rotate and succeed");

            assert_eq!(usage_reqs.len(), 2);
            assert!(
                usage_reqs[0]
                    .to_ascii_lowercase()
                    .contains("authorization: bearer synthetic-old-access")
            );
            assert!(
                usage_reqs[1]
                    .to_ascii_lowercase()
                    .contains("authorization: bearer synthetic-new-access")
            );
            assert_eq!(refresh_reqs.len(), 1);
            assert!(refresh_reqs[0].contains(r#""grant_type":"refresh_token""#));
            assert!(refresh_reqs[0].contains(r#""refresh_token":"synthetic-old-refresh""#));
            assert_eq!(profile_reqs.len(), 1);
            assert!(
                profile_reqs[0]
                    .to_ascii_lowercase()
                    .contains("authorization: bearer synthetic-new-access"),
                "live_plan must use rotated token: {}",
                profile_reqs[0]
            );

            let usage = Claude.parse(&payload).unwrap();
            assert_eq!(usage.meters.len(), 1);
            assert_eq!(usage.meters[0].used_percent, 25.0);
            assert_eq!(usage.plan.as_deref(), Some("Max"));

            let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["other"], json!({"keep": 42}));
            assert_eq!(saved["claudeAiOauth"]["custom"], json!({"keep": 7}));
            assert_eq!(
                saved["claudeAiOauth"]["accessToken"],
                "synthetic-new-access"
            );
            assert_eq!(
                saved["claudeAiOauth"]["refreshToken"],
                "synthetic-new-refresh"
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[tokio::test]
    async fn reactive_refresh_stops_without_looping_on_second_401_and_after_proactive_refresh() {
        use crate::providers::test_http::{ScriptedReply, local_client};
        let client = local_client();
        let now_ms = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;

        // 1) Reactive refresh succeeds, then retried usage also returns 401 -> no loop.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            json!({"claudeAiOauth": {
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": now_ms + 3_600_000
            }})
            .to_string(),
        )
        .unwrap();
        let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![
                ScriptedReply::status(401, r#"{"error":"secret-body-401"}"#),
                ScriptedReply::status(401, r#"{"error":"secret-body-retry"}"#),
            ],
            vec![],
            vec![ScriptedReply::ok(CLAUDE_REFRESH_JSON)],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Reconnect);
        assert!(!err.message.contains("synthetic-old-access"));
        assert!(!err.message.contains("synthetic-new-access"));
        assert!(!err.message.contains("secret-body"));
        assert_eq!(usage_reqs.len(), 2);
        assert_eq!(refresh_reqs.len(), 1);
        assert_eq!(profile_reqs.len(), 0);
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved["claudeAiOauth"]["accessToken"],
            "synthetic-new-access"
        );

        // 2) Proactive refresh ran (near_expiry == true), then usage returns 401 -> no 2nd refresh.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            json!({"claudeAiOauth": {
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": now_ms + 30_000
            }})
            .to_string(),
        )
        .unwrap();
        let mut credentials = discover_at(path, &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![ScriptedReply::status(401, r#"{"error":"still-401"}"#)],
            vec![],
            vec![ScriptedReply::ok(CLAUDE_REFRESH_JSON)],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Reconnect);
        assert_eq!(usage_reqs.len(), 1);
        assert!(
            usage_reqs[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-new-access")
        );
        assert_eq!(refresh_reqs.len(), 1);
        assert_eq!(profile_reqs.len(), 0);
    }

    #[tokio::test]
    async fn terminal_reconnect_rate_limit_backoff_and_concurrent_cli_edit() {
        use crate::providers::test_http::{ScriptedReply, local_client};
        let client = local_client();
        let future_ms = (time::OffsetDateTime::now_utc().unix_timestamp() + 3600) * 1000;

        // Missing refreshToken -> terminal Reconnect, 0 refresh calls.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            json!({"claudeAiOauth": {"accessToken": "synthetic-old-access", "expiresAt": future_ms}})
                .to_string(),
        )
        .unwrap();
        let mut credentials = discover_at(path, &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![ScriptedReply::status(401, r#"{"error":"revoked"}"#)],
            vec![],
            vec![],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Reconnect);
        assert!(err.message.contains("HTTP 401"));
        assert_eq!(usage_reqs.len(), 1);
        assert_eq!(refresh_reqs.len(), 0);
        assert_eq!(profile_reqs.len(), 0);

        // Blank accessToken with !near_expiry() and present refreshToken fails before HTTP.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            json!({"claudeAiOauth": {
                "accessToken": "   ",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": future_ms
            }})
            .to_string(),
        )
        .unwrap();
        let mut credentials = discover_at(path, &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) =
            run_claude_local_fetch(&client, &mut credentials, vec![], vec![], vec![]).await;
        assert_eq!(result.unwrap_err().status, Status::Reconnect);
        assert_eq!(usage_reqs.len(), 0);
        assert_eq!(refresh_reqs.len(), 0);
        assert_eq!(profile_reqs.len(), 0);

        // Refresh returns 400/401/403 -> terminal Reconnect, disk untouched, no secrets.
        for status in [400u16, 401, 403] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials.json");
            let original = json!({"claudeAiOauth": {
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": future_ms
            }})
            .to_string();
            std::fs::write(&path, &original).unwrap();
            let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
            let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
                &client,
                &mut credentials,
                vec![ScriptedReply::status(401, r#"{"error":"revoked"}"#)],
                vec![],
                vec![ScriptedReply::status(
                    status,
                    r#"{"error":"invalid_grant","secret":"synthetic-leak"}"#,
                )],
            )
            .await;
            let err = result.unwrap_err();
            assert_eq!(err.status, Status::Reconnect, "status {status}");
            assert!(!err.message.contains("synthetic-old-refresh"));
            assert!(!err.message.contains("synthetic-leak"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(usage_reqs.len(), 1);
            assert_eq!(refresh_reqs.len(), 1);
            assert_eq!(profile_reqs.len(), 0);
        }

        // Usage 429 preserves Retry-After and triggers 0 refresh calls.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            json!({"claudeAiOauth": {
                "accessToken": "synthetic-old-access",
                "refreshToken": "synthetic-old-refresh",
                "expiresAt": future_ms
            }})
            .to_string(),
        )
        .unwrap();
        let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![ScriptedReply::rate_limit("45")],
            vec![],
            vec![],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Error);
        assert_eq!(err.retry_after_secs, Some(45));
        assert_eq!(usage_reqs.len(), 1);
        assert_eq!(refresh_reqs.len(), 0);
        assert_eq!(profile_reqs.len(), 0);

        // Usage 401 -> Refresh 429 preserves Retry-After: 60.
        let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![ScriptedReply::status(401, r#"{"error":"revoked"}"#)],
            vec![],
            vec![ScriptedReply::rate_limit("60")],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Error);
        assert_eq!(err.retry_after_secs, Some(60));
        assert_eq!(usage_reqs.len(), 1);
        assert_eq!(refresh_reqs.len(), 1);
        assert_eq!(profile_reqs.len(), 0);

        // Concurrent CLI edit during reactive refresh returns Error and preserves CLI file.
        let mut credentials = discover_at(path.clone(), &mut Vec::new()).unwrap();
        let cli_updated =
            r#"{"claudeAiOauth":{"accessToken":"synthetic-cli-newer","refreshToken":"cli-r"}}"#;
        let path_for_hook = path.clone();
        let (result, usage_reqs, profile_reqs, refresh_reqs) = run_claude_local_fetch(
            &client,
            &mut credentials,
            vec![ScriptedReply::status(401, r#"{"error":"revoked"}"#)],
            vec![],
            vec![ScriptedReply::ok(CLAUDE_REFRESH_JSON).with_hook(move || {
                std::fs::write(&path_for_hook, cli_updated).unwrap();
            })],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Error);
        assert!(!err.message.contains("synthetic-new-access"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), cli_updated);
        assert_eq!(usage_reqs.len(), 1);
        assert_eq!(refresh_reqs.len(), 1);
        assert_eq!(profile_reqs.len(), 0);
    }
}
