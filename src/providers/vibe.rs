use super::*;
use crate::model::{number, timestamp};
use serde_json::json;
use std::fs;

pub struct Vibe;
const D: Descriptor = Descriptor {
    id: "vibe",
    name: "Vibe",
    link: "https://chat.mistral.ai",
    login: "Run `vibe --setup` or set MISTRAL_API_KEY in ~/.vibe/.env.",
};
const SUBSCRIPTION_ENDPOINT: &str = "https://api.mistral.ai/v1/billing/subscription";
const BUDGET_ENDPOINT: &str = "https://api.mistral.ai/v1/billing/budget";
const ENV_KEY: &str = "MISTRAL_API_KEY";

/// Parse one dotenv file's MISTRAL_API_KEY without loading a dotenv crate:
/// `KEY=VALUE` lines, `#` comments, optional `export ` prefix, optional quotes.
fn dotenv_api_key(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != ENV_KEY {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

fn resolve_from(
    mut environment: impl FnMut(&str) -> Option<String>,
    mut dotenv_files: impl FnMut() -> Vec<PathBuf>,
) -> Result<Credentials> {
    if let Some(key) = environment(ENV_KEY).filter(|key| !key.trim().is_empty()) {
        return Ok(Credentials::token(key));
    }
    for path in dotenv_files() {
        match fs::read_to_string(&path) {
            Ok(content) => {
                if let Some(key) = dotenv_api_key(&content) {
                    return Ok(Credentials::token(key));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ProviderError::error(format!(
                    "Cannot read credential file {}",
                    path.display()
                )));
            }
        }
    }
    Err(D.reconnect("Mistral Vibe API key not found"))
}

fn dotenv_files() -> Vec<PathBuf> {
    if let Some(vibe_home) = std::env::var("VIBE_HOME").ok().filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(vibe_home).join(".env")];
    }
    vec![home().join(".vibe").join(".env")]
}

#[async_trait]
impl Provider for Vibe {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        resolve_from(|key| std::env::var(key).ok(), dotenv_files)
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        let token = credentials.access(D)?;
        let subscription = response(
            client
                .get(SUBSCRIPTION_ENDPOINT)
                .bearer_auth(token)
                .header("Accept", "application/json")
                .send()
                .await,
            SUBSCRIPTION_ENDPOINT,
            D,
        )
        .await?;
        // 403 (member without see_org_inference_usage) and 404 (platform without
        // billing) mean "no Vibe Code budget here", not a failure.
        let budget = match client
            .get(BUDGET_ENDPOINT)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(response) if matches!(response.status().as_u16(), 403 | 404) => Value::Null,
            other => response(other, BUDGET_ENDPOINT, D).await?,
        };
        Ok(json!({ "subscription": subscription, "budget": budget }))
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let subscription = &payload["subscription"];
        let mut plan = string_at(subscription, &["plan_name"]);
        let budget = &payload["budget"]["vibe_budget"];
        let payg = [&budget["payg_enabled"], &subscription["vibe_payg_enabled"]]
            .into_iter()
            .find_map(Value::as_bool);
        if payg == Some(true) {
            plan = Some(format!(
                "{} · pay-as-you-go",
                plan.unwrap_or_else(|| "Vibe".into())
            ));
        }
        let mut meters = Vec::new();
        // usage_percentage is a 0–100 percentage and may exceed 100 under
        // pay-as-you-go; the meter clamps at 100.
        if let Some(used) = budget.get("usage_percentage").and_then(number) {
            meters.push(Meter {
                label: "Vibe Code allowance".into(),
                used_percent: (used as f32).clamp(0.0, 100.0),
                resets_at: budget.get("reset_at").and_then(timestamp),
            });
        }
        if meters.is_empty() && plan.is_none() {
            return Err(D.invalid());
        }
        Ok(Usage {
            plan,
            meters,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotenv_parser_handles_comments_export_quotes_and_crlf() {
        let content = "# comment\r\nOTHER=1\nexport MISTRAL_API_KEY=\"synthetic-key\"\nMISTRAL_API_KEY='ignored-second'\n";
        assert_eq!(dotenv_api_key(content), Some("synthetic-key".into()));
        assert_eq!(dotenv_api_key("MISTRAL_API_KEY=\n"), None);
        assert_eq!(dotenv_api_key("MISTRAL_API_KEY=   \n"), None);
        assert_eq!(dotenv_api_key(""), None);
        assert_eq!(dotenv_api_key("# MISTRAL_API_KEY=commented\n"), None);
    }

    #[test]
    fn environment_wins_over_dotenv_file() {
        let root = tempfile::tempdir().unwrap();
        let env_file = root.path().join(".env");
        fs::write(&env_file, "MISTRAL_API_KEY=synthetic-file-key\n").unwrap();
        let credentials = resolve_from(
            |_| Some("synthetic-env-key".into()),
            || vec![env_file.clone()],
        )
        .unwrap();
        assert_eq!(credentials.access(D).unwrap(), "synthetic-env-key");

        let from_file = resolve_from(|_| None, || vec![env_file.clone()]).unwrap();
        assert_eq!(from_file.access(D).unwrap(), "synthetic-file-key");

        let error = match resolve_from(|_| Some("  ".into()), || vec![root.path().join("missing")])
        {
            Ok(_) => panic!("blank environment key and no file must not resolve"),
            Err(error) => error,
        };
        assert_eq!(error.status, Status::Reconnect);
        assert!(error.message.contains("MISTRAL_API_KEY"));
    }

    #[test]
    fn unreadable_dotenv_is_an_error_not_a_silent_skip() {
        let root = tempfile::tempdir().unwrap();
        let env_file = root.path().join(".env");
        fs::create_dir(&env_file).unwrap(); // a directory: read fails with non-NotFound
        let error = match resolve_from(|_| None, || vec![env_file.clone()]) {
            Ok(_) => panic!("unreadable .env must not resolve"),
            Err(error) => error,
        };
        assert_eq!(error.status, Status::Error);
        assert!(error.message.contains(&env_file.display().to_string()));
    }

    #[test]
    fn parse_full_budget_payload() {
        let usage = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "TEAM", "vibe_payg_enabled": true},
                "budget": {"vibe_budget": {
                    "usage_percentage": 40.0,
                    "initial_budget": 200.0,
                    "reset_at": "2026-12-01T00:00:00Z",
                    "payg_enabled": true
                }}
            }))
            .unwrap();
        assert_eq!(usage.plan.as_deref(), Some("TEAM · pay-as-you-go"));
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Vibe Code allowance");
        assert_eq!(usage.meters[0].used_percent, 40.0);
        assert!(usage.meters[0].resets_at.is_some());
    }

    #[test]
    fn parse_clamps_payg_overspend_and_survives_missing_budget() {
        let over = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "TEAM"},
                "budget": {"vibe_budget": {"usage_percentage": 140.0, "reset_at": null}}
            }))
            .unwrap();
        assert_eq!(over.meters[0].used_percent, 100.0);
        assert_eq!(over.plan.as_deref(), Some("TEAM"));

        // No budget served (403/404 read as null): plan alone, no meters.
        let plan_only = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "INDIVIDUAL"},
                "budget": null
            }))
            .unwrap();
        assert!(plan_only.meters.is_empty());
        assert_eq!(plan_only.plan.as_deref(), Some("INDIVIDUAL"));

        // Neither endpoint produced anything recognisable.
        assert!(
            Vibe.parse(&json!({"subscription": {}, "budget": null}))
                .is_err()
        );
    }
}
