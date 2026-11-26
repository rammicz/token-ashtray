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

fn refresh_body(credentials: &Credentials) -> Result<Option<RefreshBody>> {
    if !credentials.near_expiry() {
        return Ok(None);
    }
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
    Ok(Some(RefreshBody::Json(json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        "scope": scope,
    }))))
}

/// The credentials file caches `subscriptionType` at sign-in and never notices
/// a plan change, so prefer the live profile. `plan_display_name` is what the
/// account page shows ("Ultra"); `organization_type` ("claude_max") is the
/// fallback. Returns `None` on any profile failure so usage still renders.
async fn live_plan(client: &Client, credentials: &mut Credentials) -> Option<String> {
    let access = credentials.access(D).ok()?;
    let Ok(profile) = response(
        client
            .get(PROFILE)
            .bearer_auth(access)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await,
        PROFILE,
        D,
    )
    .await
    else {
        return None;
    };
    plan_from_profile(&profile)
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
        if let Some(body) = refresh_body(credentials)? {
            refresh_safely(credentials, REFRESH, D, body, true).await?;
        }
        let mut payload = response(
            client
                .get(USAGE)
                .bearer_auth(credentials.access(D)?)
                .header("anthropic-beta", "oauth-2025-04-20")
                .send()
                .await,
            USAGE,
            D,
        )
        .await?;
        let cached = credentials
            .object()
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(plan) = live_plan(client, credentials).await.or(cached) {
            payload["_plan"] = json!(plan);
        }
        Ok(payload)
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
}
