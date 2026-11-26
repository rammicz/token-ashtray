use super::{Descriptor, ProviderError, Result};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use time::OffsetDateTime;

pub struct Credentials {
    pub document: Value,
    pub object_path: Vec<String>,
    path: Option<PathBuf>,
    original: Vec<u8>,
}

impl Credentials {
    pub fn token(token: String) -> Self {
        Self {
            document: json!({"access_token":token}),
            object_path: vec![],
            path: None,
            original: vec![],
        }
    }
    pub fn load(path: PathBuf, descriptor: Descriptor, object_path: Vec<String>) -> Result<Self> {
        let original = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                descriptor.reconnect("Login file not found")
            } else {
                ProviderError::error(format!("Cannot read credential file {}", path.display()))
            }
        })?;
        let document: Value = serde_json::from_slice(&original).map_err(|_| {
            ProviderError::error(format!(
                "Credential file {} is not valid JSON",
                path.display()
            ))
        })?;
        let result = Self {
            document,
            object_path,
            path: Some(path),
            original,
        };
        if !result.object().is_object() {
            return Err(descriptor.reconnect("Subscription login not present"));
        }
        Ok(result)
    }
    pub fn object(&self) -> &Value {
        self.object_path
            .iter()
            .fold(&self.document, |node, key| &node[key])
    }
    pub fn object_mut(&mut self) -> &mut Value {
        self.object_path
            .iter()
            .fold(&mut self.document, |node, key| &mut node[key])
    }
    pub fn access(&self, descriptor: Descriptor) -> Result<&str> {
        ["access_token", "accessToken", "key", "oauth_token", "token"]
            .iter()
            .find_map(|key| {
                self.object()
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
            .ok_or_else(|| descriptor.reconnect("Access token not present"))
    }
    pub fn near_expiry(&self) -> bool {
        ["expiresAt", "expires_at", "expiry"]
            .iter()
            .find_map(|key| self.object().get(key).and_then(crate::model::timestamp))
            .is_some_and(|expiry| (expiry - OffsetDateTime::now_utc()).whole_seconds() < 60)
    }
    pub fn refresh(&self, descriptor: Descriptor) -> Result<String> {
        ["refresh_token", "refreshToken"]
            .iter()
            .find_map(|key| self.object().get(key).and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| descriptor.reconnect("Refresh token not present"))
    }
    pub fn apply_refresh(&mut self, payload: &Value, camel_case: bool) -> Result<()> {
        let access = payload
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ProviderError::error("OAuth refresh response did not include an access token")
            })?;
        let object = self.object_mut();
        let access_key = if camel_case {
            "accessToken"
        } else if object.get("access_token").is_some() {
            "access_token"
        } else if object.get("key").is_some() {
            "key"
        } else {
            "access_token"
        };
        object[access_key] = json!(access);
        if let Some(refresh) = payload
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            object[if camel_case {
                "refreshToken"
            } else {
                "refresh_token"
            }] = json!(refresh);
        }
        if let Some(seconds) = payload.get("expires_in").and_then(crate::model::number) {
            let expiry = OffsetDateTime::now_utc().unix_timestamp() + seconds as i64;
            let expiry_key = if camel_case {
                "expiresAt"
            } else {
                "expires_at"
            };
            // Preserve the CLI's date-string convention where present.
            object[expiry_key] = if camel_case {
                json!(expiry * 1000)
            } else if object[expiry_key].is_string() {
                json!(
                    OffsetDateTime::from_unix_timestamp(expiry)
                        .ok()
                        .and_then(|t| t
                            .format(&time::format_description::well_known::Rfc3339)
                            .ok())
                        .unwrap_or_default()
                )
            } else {
                json!(expiry)
            };
        }
        self.persist()
    }
    fn persist(&mut self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| ProviderError::error("Refresh has no credential file"))?;
        if fs::read(path).ok().as_deref() != Some(self.original.as_slice()) {
            return Err(ProviderError::error(
                "CLI login changed while refreshing; refusing to overwrite it. Refresh again.",
            ));
        }
        let encoded = serde_json::to_vec_pretty(&self.document)
            .map_err(|_| ProviderError::error("Could not encode refreshed login"))?;
        atomic_write_checked(path, &encoded, &self.original).map_err(|_| {
            ProviderError::error(
                "CLI login changed or could not be saved atomically; reconnect with the CLI",
            )
        })?;
        self.original = encoded;
        Ok(())
    }
}

fn atomic_write_checked(path: &Path, bytes: &[u8], expected: &[u8]) -> std::io::Result<()> {
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    // Recheck after every potentially slow operation, including each wait for
    // a holder of the file. There is no portable compare-and-swap rename
    // against a non-cooperating CLI; the final check and rename are adjacent
    // so serialization, fsync or a retry cannot hide a CLI update.
    crate::fsutil::replace_atomically(path, bytes, permissions, || {
        if fs::read(path)?.as_slice() != expected {
            return Err(std::io::Error::other(
                "CLI login changed before replacement",
            ));
        }
        Ok(())
    })
}

pub fn find_token_path(value: &Value, keys: &[&str]) -> Option<Vec<String>> {
    if let Some(object) = value.as_object() {
        if keys.iter().any(|key| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
        }) {
            return Some(vec![]);
        }
        for (key, nested) in object {
            if let Some(mut path) = find_token_path(nested, keys) {
                path.insert(0, key.clone());
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const DESC: Descriptor = Descriptor {
        id: "test",
        name: "Test",
        link: "",
        login: "Log in",
    };
    #[test]
    fn recursive_token_finder_ignores_whitespace_only_fields() {
        let document = json!({"github.com":{"oauth_token":" \t\n "}});
        assert_eq!(find_token_path(&document, &["oauth_token"]), None);

        // Grok uses the same finder with a different pair of token keys.
        let grok = json!({"first":{"key":" \t "},"second":{"access_token":"synthetic-grok-token"}});
        assert_eq!(
            find_token_path(&grok, &["key", "access_token"]),
            Some(vec!["second".into()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn refresh_keeps_the_login_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        for mode in [0o600, 0o640] {
            fs::write(&path, br#"{"access_token":"old"}"#).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let mut c = Credentials::load(path.clone(), DESC, vec![]).unwrap();
            c.apply_refresh(&json!({"access_token":"new"}), false)
                .unwrap();
            let saved = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(saved, mode);
        }
    }

    #[test]
    fn refresh_preserves_fields_and_detects_concurrent_cli_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(&path, br#"{"other":{"keep":true},"oauth":{"accessToken":"old","refreshToken":"keep","scope":["x"]}}"#).unwrap();
        let mut c = Credentials::load(path.clone(), DESC, vec!["oauth".into()]).unwrap();
        c.apply_refresh(&json!({"access_token":"new","expires_in":3600}), true)
            .unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["other"]["keep"], true);
        assert_eq!(v["oauth"]["refreshToken"], "keep");
        fs::write(&path, b"{}").unwrap();
        assert!(
            c.apply_refresh(&json!({"access_token":"next"}), true)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"{}");
    }
}

/// The CLI, a scanner or an editor can hold the login file open (#67).
#[cfg(all(test, windows))]
mod open_target_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn refresh_replaces_login_while_a_reader_holds_it_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let old = br#"{"access_token":"old","refresh_token":"keep"}"#;
        fs::write(&path, old).unwrap();
        let descriptor = Descriptor {
            id: "test",
            name: "Test",
            link: "",
            login: "Log in",
        };
        let mut credentials = Credentials::load(path.clone(), descriptor, vec![]).unwrap();
        let mut reader = fs::File::open(&path).unwrap();
        credentials
            .apply_refresh(&json!({"access_token":"rotated"}), false)
            .unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["access_token"], "rotated");
        assert_eq!(saved["refresh_token"], "keep");
        let mut held = Vec::new();
        reader.read_to_end(&mut held).unwrap();
        assert_eq!(held, old);
        assert_eq!(crate::fsutil::file_names(dir.path()), ["auth.json"]);
    }
}

/// A rotating OAuth grant is a small transaction: once submitted it must finish
/// saving its result even if a panel closes, settings change, or polling times
/// out. Tokio blocking jobs are not cancelled with their async waiter. The
/// caller's runtime waits for these bounded jobs during graceful shutdown.
pub async fn finish_refresh(
    credentials: &mut Credentials,
    job: impl FnOnce(Credentials) -> Result<Credentials> + Send + 'static,
) -> Result<()> {
    let owned = std::mem::replace(credentials, Credentials::token(String::new()));
    let refreshed = tokio::task::spawn_blocking(move || job(owned))
        .await
        .map_err(|_| ProviderError::error("OAuth refresh worker failed"))??;
    *credentials = refreshed;
    Ok(())
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn cancelling_refresh_waiter_still_saves_rotated_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            br#"{"access_token":"old","refresh_token":"previous","untouched":42}"#,
        )
        .unwrap();
        let d = Descriptor {
            id: "test",
            name: "Test",
            link: "",
            login: "login",
        };
        let mut credentials = Credentials::load(path.clone(), d, vec![]).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (saved_tx, saved_rx) = std::sync::mpsc::channel();
        let task = runtime.spawn(async move {
            finish_refresh(&mut credentials, move |mut owned| {
                started_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(30));
                let result = owned.apply_refresh(
                    &json!({"access_token":"rotated","refresh_token":"next","expires_in":3600}),
                    false,
                );
                // The waiter is gone; report the save to the test directly.
                let _ = saved_tx.send(result.as_ref().map(|_| ()).map_err(|e| e.message.clone()));
                result.map(|()| owned)
            })
            .await
        });
        // Generous bounds: they only fail a job that never runs or never ends.
        let bound = std::time::Duration::from_secs(10);
        started_rx.recv_timeout(bound).unwrap();
        task.abort();
        runtime.shutdown_timeout(bound);
        // Blocking jobs outlive their waiter but not on shutdown's schedule,
        // so wait for the job's own result. If cancellation ever did kill the
        // job, the sender drops and the receive fails.
        saved_rx
            .recv_timeout(bound)
            .expect("the refresh job ended without reporting its save")
            .expect("the rotated login could not be saved");
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["access_token"], "rotated");
        assert_eq!(saved["untouched"], 42);
    }
}
