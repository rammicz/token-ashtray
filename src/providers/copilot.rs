use super::*;
use crate::model::{percent100, timestamp};
pub struct Copilot;
const D: Descriptor = Descriptor {
    id: "copilot",
    name: "Copilot",
    link: "https://github.com/settings/copilot",
    login: "Run `copilot login` or sign in to GitHub Copilot in your editor.",
};
const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";

fn resolve_from(
    mut environment: impl FnMut(&str) -> Option<String>,
    directories: impl FnOnce() -> Vec<PathBuf>,
) -> Result<Credentials> {
    for key in ["GITHUB_COPILOT_API_TOKEN", "GITHUB_TOKEN"] {
        if let Some(token) = environment(key)
            && !token.trim().is_empty()
        {
            return Ok(Credentials::token(token));
        }
    }
    for directory in directories() {
        for filename in ["apps.json", "hosts.json"] {
            let path = directory.join(filename);
            if !path.exists() {
                continue;
            }
            let mut credentials = match Credentials::load(path, D, vec![]) {
                Ok(credentials) => credentials,
                Err(error) if error.status == Status::Reconnect => continue,
                Err(error) => return Err(error),
            };
            if let Some(path) = credentials::find_token_path(
                &credentials.document,
                &["oauth_token", "access_token", "token", "accessToken"],
            ) {
                credentials.object_path = path;
                return Ok(credentials);
            }
        }
    }
    Err(D.reconnect("GitHub Copilot login not found"))
}

#[async_trait]
impl Provider for Copilot {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        resolve_from(|key| std::env::var(key).ok(), candidate_directories)
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        response(
            client
                .get(ENDPOINT)
                .bearer_auth(credentials.access(D)?)
                .header("Accept", "application/json")
                .send()
                .await,
            ENDPOINT,
            D,
        )
        .await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let fallback = [
            "quota_reset_date_utc",
            "quota_reset_date",
            "limited_user_reset_date",
        ]
        .iter()
        .find_map(|k| payload.get(k).and_then(timestamp));
        let mut meters = Vec::new();
        let mut unlimited = false;
        for (key, label) in [
            ("premium_interactions", "Premium requests"),
            ("chat", "Chat"),
            ("completions", "Completions"),
        ] {
            let quota = &payload["quota_snapshots"][key];
            if quota["unlimited"].as_bool() == Some(true) {
                unlimited = true;
                continue;
            }
            if quota.get("entitlement").and_then(crate::model::number) == Some(0.0) {
                continue;
            }
            if let Some(remaining) = percent100(&quota["percent_remaining"]) {
                meters.push(Meter {
                    label: label.into(),
                    used_percent: 100.0 - remaining,
                    resets_at: timestamp(&quota["quota_reset_at"]).or(fallback),
                });
            }
        }
        if meters.is_empty() && !unlimited {
            return Err(D.invalid());
        }
        let mut plan = string_at(payload, &["copilot_plan", "plan"]);
        if unlimited && meters.is_empty() {
            plan = Some(format!(
                "{} · unlimited quotas",
                plan.unwrap_or_else(|| "Copilot".into())
            ));
        }
        Ok(Usage {
            meters,
            plan,
            ..Default::default()
        })
    }
}
fn candidate_directories() -> Vec<PathBuf> {
    candidate_directories_from(&crate::platform::credential_base_paths())
}
pub(super) fn candidate_directories_from(
    bases: &crate::platform::CredentialBasePaths,
) -> Vec<PathBuf> {
    let home = &bases.home;
    let mut paths = vec![
        home.join(".config/github-copilot"),
        home.join("Library/Application Support/github-copilot"),
        home.join("AppData/Local/github-copilot"),
    ];
    if let Some(local) = &bases.local_app_data {
        paths.push(local.join("github-copilot"));
    }
    if let Some(roaming) = &bases.roaming_app_data {
        paths.push(roaming.join("GitHub Copilot"));
    }
    paths
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn blank_apps_token_does_not_hide_later_nested_hosts_token() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github-copilot");
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("apps.json"),
            json!({"github.com":{"oauth_token":" \t "}}).to_string(),
        )
        .unwrap();
        fs::write(
            directory.join("hosts.json"),
            json!({"github.com":{"editor":{"access_token":"synthetic-host-token"}}}).to_string(),
        )
        .unwrap();

        let credentials = resolve_from(|_| None, || vec![directory]).unwrap();
        assert_eq!(credentials.access(D).ok(), Some("synthetic-host-token"));
    }

    #[test]
    fn valid_nonobject_apps_does_not_hide_later_nested_hosts_token() {
        for apps in ["null", "[]", r#""scalar""#] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("github-copilot");
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("apps.json"), apps).unwrap();
            fs::write(
                directory.join("hosts.json"),
                json!({"github.com":{"editor":{"access_token":"synthetic-host-token"}}})
                    .to_string(),
            )
            .unwrap();

            let credentials = resolve_from(|_| None, || vec![directory]).unwrap();
            assert_eq!(credentials.access(D).unwrap(), "synthetic-host-token");
        }
    }

    #[test]
    fn environment_priority_then_apps_before_hosts() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github-copilot");
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("apps.json"),
            json!({"github.com":{"token":"synthetic-app-token"}}).to_string(),
        )
        .unwrap();
        fs::write(
            directory.join("hosts.json"),
            json!({"github.com":{"editor":{"oauth_token":"synthetic-host-token"}}}).to_string(),
        )
        .unwrap();

        let mut queried = Vec::new();
        let first = resolve_from(
            |key| {
                queried.push(key.to_owned());
                Some("synthetic-first-env-token".into())
            },
            || panic!("file candidates were inspected despite environment override"),
        )
        .unwrap();
        assert_eq!(first.access(D).unwrap(), "synthetic-first-env-token");
        assert_eq!(queried, ["GITHUB_COPILOT_API_TOKEN"]);

        queried.clear();
        let second = resolve_from(
            |key| {
                queried.push(key.to_owned());
                Some(if key == "GITHUB_COPILOT_API_TOKEN" {
                    " \t ".into()
                } else {
                    "synthetic-second-env-token".into()
                })
            },
            || panic!("file candidates were inspected despite second environment override"),
        )
        .unwrap();
        assert_eq!(second.access(D).unwrap(), "synthetic-second-env-token");
        assert_eq!(queried, ["GITHUB_COPILOT_API_TOKEN", "GITHUB_TOKEN"]);

        let apps = resolve_from(|_| Some(" \t ".into()), || vec![directory]).unwrap();
        assert_eq!(apps.access(D).unwrap(), "synthetic-app-token");
    }

    #[test]
    fn each_of_five_supplied_directories_can_supply_first_usable_token() {
        for target in 0..5 {
            let root = tempfile::tempdir().unwrap();
            let bases = crate::platform::CredentialBasePaths {
                home: root.path().join("home"),
                xdg_config_home: None,
                local_app_data: Some(root.path().join("local")),
                roaming_app_data: Some(root.path().join("roaming")),
            };
            let directories = candidate_directories_from(&bases);
            assert_eq!(
                directories,
                [
                    bases.home.join(".config/github-copilot"),
                    bases
                        .home
                        .join("Library/Application Support/github-copilot"),
                    bases.home.join("AppData/Local/github-copilot"),
                    bases
                        .local_app_data
                        .as_ref()
                        .unwrap()
                        .join("github-copilot"),
                    bases
                        .roaming_app_data
                        .as_ref()
                        .unwrap()
                        .join("GitHub Copilot"),
                ]
            );
            for (index, directory) in directories.iter().enumerate() {
                fs::create_dir_all(directory).unwrap();
                if index < target {
                    let earlier = if index % 2 == 0 {
                        json!({"github.com":{"oauth_token":" \t "}})
                    } else {
                        json!({"github.com":{"other":"no token"}})
                    };
                    fs::write(directory.join("apps.json"), earlier.to_string()).unwrap();
                } else if index == target {
                    fs::write(
                        directory.join("hosts.json"),
                        json!({"github.com":{"editor":{"accessToken":format!("target-{target}")}}})
                            .to_string(),
                    )
                    .unwrap();
                } else {
                    fs::write(
                        directory.join("apps.json"),
                        json!({"github.com":{"token":"later-token"}}).to_string(),
                    )
                    .unwrap();
                }
            }
            let credentials = resolve_from(|_| None, || directories).unwrap();
            assert_eq!(credentials.access(D).unwrap(), format!("target-{target}"));
        }
    }

    #[test]
    fn malformed_earlier_file_still_errors_before_later_credential() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("apps.json"), b"{invalid-json").unwrap();
        fs::write(
            second.join("hosts.json"),
            json!({"github.com":{"oauth_token":"synthetic-later-token"}}).to_string(),
        )
        .unwrap();
        let error = match resolve_from(|_| None, || vec![first.clone(), second]) {
            Ok(_) => panic!("malformed earlier file was skipped"),
            Err(error) => error,
        };
        assert_eq!(error.status, crate::model::Status::Error);
        assert!(
            error
                .message
                .contains(&first.join("apps.json").display().to_string())
        );
        assert!(!error.message.contains("synthetic-later-token"));
    }

    #[test]
    fn unreadable_earlier_candidate_still_errors_before_later_credential() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github-copilot");
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join("apps.json")).unwrap();
        fs::write(
            directory.join("hosts.json"),
            json!({"github.com":{"oauth_token":"synthetic-later-token"}}).to_string(),
        )
        .unwrap();

        let error = match resolve_from(|_| None, || vec![directory.clone()]) {
            Ok(_) => panic!("unreadable earlier candidate was skipped"),
            Err(error) => error,
        };
        assert_eq!(error.status, Status::Error);
        assert!(
            error
                .message
                .contains(&directory.join("apps.json").display().to_string())
        );
    }

    #[test]
    fn schema_percent_remaining_and_unlimited() {
        let usage=Copilot.parse(&json!({"quota_snapshots":{"premium_interactions":{"percent_remaining":75},"chat":{"unlimited":true}}})).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].used_percent, 25.0);
        assert!(
            Copilot
                .parse(&json!({"quota_snapshots":{"chat":{}}}))
                .is_err()
        );
    }
    #[test]
    fn host_nested_credentials() {
        let v = json!({"github.com":{"editor":{"oauth_token":"test-token"}}});
        assert_eq!(
            credentials::find_token_path(&v, &["oauth_token"]).unwrap(),
            vec!["github.com", "editor"]
        );
    }
}
