use super::*;
use crate::model::{number, timestamp};
use std::{ffi::OsString, path::Path};
pub struct Kimi;
const D: Descriptor = Descriptor {
    id: "kimi",
    name: "Kimi",
    link: "https://www.kimi.com/code",
    login: "Run `kimi login`.",
};
const ENDPOINT: &str = "https://api.kimi.com/coding/v1/usages";
const REFRESH: &str = "https://auth.kimi.com/api/oauth/token";

fn credential_path(
    home: &Path,
    code_home: Option<OsString>,
    share_dir: impl FnOnce() -> Option<OsString>,
    exists: impl FnOnce(&Path) -> bool,
) -> PathBuf {
    let code_home = code_home.filter(|value| !value.is_empty());
    let root = code_home
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".kimi-code"));
    let documented = root.join("credentials/kimi-code.json");
    if exists(&documented) || code_home.is_some() {
        documented
    } else {
        share_dir()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".kimi"))
            .join("credentials/kimi-code.json")
    }
}

#[async_trait]
impl Provider for Kimi {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        let path = credential_path(
            &home(),
            std::env::var_os("KIMI_CODE_HOME"),
            || std::env::var_os("KIMI_SHARE_DIR"),
            Path::exists,
        );
        Credentials::load(path, D, vec![])
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        if credentials.near_expiry() {
            let refresh = credentials.refresh(D)?;
            let form = vec![
                (
                    "client_id".into(),
                    "17e5f671-d194-4dfb-9706-5516cb48c098".into(),
                ),
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), refresh),
            ];
            refresh_safely(credentials, REFRESH, D, RefreshBody::Form(form), false).await?;
        }
        response(
            client
                .get(ENDPOINT)
                .bearer_auth(credentials.access(D)?)
                .send()
                .await,
            ENDPOINT,
            D,
        )
        .await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let mut meters = Vec::new();
        if let Some(meter) = kimi_meter("Total", &payload["usage"]) {
            meters.push(meter);
        }
        if let Some(limits) = payload["limits"].as_array() {
            for (i, item) in limits.iter().enumerate() {
                let label = match (
                    item["window"]["duration"].as_i64(),
                    item["window"]["timeUnit"].as_str(),
                ) {
                    (Some(n), Some(unit)) => format!("{n} {}", unit.to_lowercase()),
                    _ => format!("Window {}", i + 1),
                };
                if let Some(m) = kimi_meter(&label, item.get("detail").unwrap_or(item)) {
                    meters.push(m);
                }
            }
        }
        if meters.is_empty() {
            return Err(D.invalid());
        }
        Ok(Usage {
            meters,
            plan: string_at(payload, &["plan", "plan_name"]),
            ..Default::default()
        })
    }
}
fn kimi_meter(label: &str, value: &Value) -> Option<Meter> {
    let limit = number(&value["limit"])?;
    if limit <= 0.0 {
        return None;
    }
    let used = number(&value["used"])
        .or_else(|| number(&value["remaining"]).map(|remaining| limit - remaining))?;
    if used < 0.0 {
        return None;
    }
    let resets_at = ["reset_at", "resetAt", "reset_time", "resetTime"]
        .iter()
        .find_map(|k| value.get(k).and_then(timestamp))
        .or_else(|| {
            ["reset_in", "resetIn", "ttl"]
                .iter()
                .find_map(|k| value.get(k).and_then(number))
                .map(|s| OffsetDateTime::now_utc() + time::Duration::seconds(s as i64))
        });
    Some(Meter {
        label: label.into(),
        used_percent: (used / limit * 100.0).clamp(0.0, 100.0) as f32,
        resets_at,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn empty_code_home_uses_documented_default_instead_of_relative_path() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let documented = home.join(".kimi-code/credentials/kimi-code.json");
        fs::create_dir_all(documented.parent().unwrap()).unwrap();
        fs::write(&documented, b"{}").unwrap();

        let selected = credential_path(
            &home,
            Some(OsString::new()),
            || panic!("share fallback was checked despite documented login"),
            |path| path.is_absolute() && path.exists(),
        );
        assert_eq!(selected, documented);

        fs::remove_file(&documented).unwrap();
        let fallback = home.join(".kimi/credentials/kimi-code.json");
        fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        fs::write(&fallback, b"{}").unwrap();
        let selected = credential_path(
            &home,
            Some(OsString::new()),
            || None,
            |path| path.is_absolute() && path.exists(),
        );
        assert_eq!(selected, fallback);
    }

    #[test]
    fn empty_share_dir_uses_home_fallback_instead_of_relative_path() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let fallback = home.join(".kimi/credentials/kimi-code.json");
        fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        fs::write(&fallback, b"{}").unwrap();

        let selected = credential_path(
            &home,
            None,
            || Some(OsString::new()),
            |path| path.is_absolute() && path.exists(),
        );
        assert_eq!(selected, fallback);
    }

    #[test]
    fn nonempty_code_home_is_authoritative_and_preserves_path_values() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let default = home.join(".kimi-code/credentials/kimi-code.json");
        let override_root = root.path().join("code home with spaces");
        let override_file = override_root.join("credentials/kimi-code.json");
        fs::create_dir_all(default.parent().unwrap()).unwrap();
        fs::create_dir_all(override_file.parent().unwrap()).unwrap();
        fs::write(&default, br#"{"access_token":"synthetic-default"}"#).unwrap();
        let bytes = br#"{"access_token":"synthetic-override","other":{"keep":true}}"#;
        fs::write(&override_file, bytes).unwrap();

        let selected = credential_path(
            &home,
            Some(override_root.into_os_string()),
            || panic!("share fallback was checked for explicit code home"),
            Path::exists,
        );
        assert_eq!(selected, override_file);
        let credentials = Credentials::load(selected, D, vec![]).unwrap();
        assert_eq!(credentials.access(D).unwrap(), "synthetic-override");
        assert_eq!(fs::read(&override_file).unwrap(), bytes);

        let missing = root.path().join("missing explicit root");
        let selected = credential_path(
            &home,
            Some(missing.clone().into_os_string()),
            || panic!("share fallback was checked for missing explicit code home"),
            |path| path.is_absolute() && path.exists(),
        );
        assert_eq!(selected, missing.join("credentials/kimi-code.json"));
        let error = match Credentials::load(selected, D, vec![]) {
            Ok(_) => panic!("missing explicit login fell through to default"),
            Err(error) => error,
        };
        assert_eq!(error.status, crate::model::Status::Reconnect);

        let relative = OsString::from("relative explicit root");
        let selected = credential_path(&home, Some(relative.clone()), || None, |_| false);
        assert_eq!(
            selected,
            PathBuf::from(relative).join("credentials/kimi-code.json")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_code_home_is_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let root = OsString::from_vec(b"non-utf8-\xff".to_vec());
        let selected = credential_path(
            Path::new("/synthetic-home"),
            Some(root.clone()),
            || None,
            |_| false,
        );
        assert_eq!(
            selected,
            PathBuf::from(root).join("credentials/kimi-code.json")
        );
    }

    #[test]
    fn absent_override_keeps_documented_then_current_cli_fallback_order() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let documented = home.join(".kimi-code/credentials/kimi-code.json");
        let share_root = root.path().join("share path with spaces");
        let shared = share_root.join("credentials/kimi-code.json");
        fs::create_dir_all(documented.parent().unwrap()).unwrap();
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(&documented, b"{}").unwrap();
        fs::write(&shared, b"{}").unwrap();

        let selected = credential_path(
            &home,
            None,
            || panic!("share was checked while documented login existed"),
            Path::exists,
        );
        assert_eq!(selected, documented);

        fs::remove_file(&documented).unwrap();
        let selected = credential_path(
            &home,
            None,
            || Some(share_root.clone().into_os_string()),
            Path::exists,
        );
        assert_eq!(selected, shared);
        let selected = credential_path(&home, None, || None, Path::exists);
        assert_eq!(selected, home.join(".kimi/credentials/kimi-code.json"));
    }

    #[test]
    fn root_login_rotation_saves_fields_and_rejects_prior_cli_edit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("kimi-code.json");
        let original = br#"{"access_token":"synthetic-old-access","refresh_token":"synthetic-old-refresh","expires_at":"2026-01-01T00:00:00Z","other":{"keep":42}}"#;
        fs::write(&path, original).unwrap();
        let mut credentials = Credentials::load(path.clone(), D, vec![]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        credentials
            .apply_refresh(
                &json!({"access_token":"synthetic-new-access","refresh_token":"synthetic-new-refresh","expires_in":3600}),
                false,
            )
            .unwrap();

        let saved_bytes = fs::read(&path).unwrap();
        let saved: Value = serde_json::from_slice(&saved_bytes).unwrap();
        assert_ne!(saved_bytes, original);
        assert_eq!(saved["access_token"], "synthetic-new-access");
        assert_eq!(saved["refresh_token"], "synthetic-new-refresh");
        assert_eq!(saved["other"], json!({"keep":42}));
        assert!(saved["expires_at"].is_string());
        assert!(!credentials.near_expiry());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);

        let external = br#"{"access_token":"synthetic-cli-edit","other":{"keep":99}}"#;
        fs::write(&path, external).unwrap();
        let error = credentials
            .apply_refresh(&json!({"access_token":"synthetic-next-access"}), false)
            .err()
            .unwrap();
        assert_eq!(error.status, crate::model::Status::Error);
        assert!(!error.message.contains("synthetic-next-access"));
        assert_eq!(fs::read(&path).unwrap(), external);
    }

    #[test]
    fn documented_remaining_shape_and_missing_are_distinct() {
        let u=Kimi.parse(&json!({"usage":{"limit":100,"remaining":75},"limits":[{"detail":{"limit":10,"used":2},"window":{"duration":5,"timeUnit":"HOUR"}}]})).unwrap();
        assert_eq!(u.meters[0].used_percent, 25.0);
        assert_eq!(u.meters[1].used_percent, 20.0);
        assert!(Kimi.parse(&json!({"usage":{"limit":100}})).is_err());
    }
}
