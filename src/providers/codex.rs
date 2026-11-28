use super::*;
use crate::model::{percent100, timestamp};
pub struct Codex;
const D: Descriptor = Descriptor {
    id: "codex",
    name: "Codex",
    link: "https://chatgpt.com/",
    login: "Run `codex login`.",
};
const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

fn discover_at(path: PathBuf) -> Result<Credentials> {
    let mut credentials = Credentials::load(path, D, vec![])?;
    if credentials
        .document
        .get("tokens")
        .is_some_and(Value::is_object)
    {
        credentials.object_path = vec!["tokens".into()];
    }
    credentials.access(D)?;
    Ok(credentials)
}

async fn fetch_at(client: &Client, credentials: &mut Credentials, endpoint: &str) -> Result<Value> {
    let mut request = client.get(endpoint).bearer_auth(credentials.access(D)?);
    if let Some(account) = [credentials.object(), &credentials.document]
        .into_iter()
        .find_map(|node| {
            node.get("account_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        request = request.header("ChatGPT-Account-Id", account);
    }
    let result = request.send().await;
    let status = result.as_ref().ok().map(reqwest::Response::status);
    let payload = response(result, endpoint, D).await?;
    // The shared poll parses after fetch, when the HTTP response is gone.
    // Validate here too so a changed successful shape retains its real status.
    Codex.parse(&payload).map_err(|error| match status {
        Some(status) => {
            ProviderError::error(format!("{endpoint}: HTTP {status}: {}", error.message))
        }
        None => error,
    })?;
    Ok(payload)
}

#[async_trait]
impl Provider for Codex {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        discover_at(home().join(".codex/auth.json"))
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        fetch_at(client, credentials, ENDPOINT).await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let mut meters = windows(&payload["rate_limit"], "");
        meters.extend(windows(&payload["code_review_rate_limit"], "Code review "));
        if let Some(additional) = payload["additional_rate_limits"].as_array() {
            for item in additional {
                let name = additional_name(item);
                meters.extend(windows(&item["rate_limit"], &format!("{name} ")));
            }
        }
        if meters.is_empty() {
            return Err(D.invalid());
        }
        Ok(Usage {
            meters,
            plan: string_at(payload, &["plan_type"]),
            ..Default::default()
        })
    }
}

fn additional_name(item: &Value) -> &str {
    ["limit_name", "metered_feature"]
        .into_iter()
        .filter_map(|field| item[field].as_str())
        .map(str::trim)
        .find(|name| !name.is_empty() && *name != "<redacted>")
        .unwrap_or("Additional")
}

fn windows(value: &Value, prefix: &str) -> Vec<Meter> {
    ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|key| {
            let window = &value[key];
            let used_percent = percent100(&window["used_percent"])?;
            let seconds = window["limit_window_seconds"].as_i64();
            let label = match seconds {
                Some(18000) => "5 hours".into(),
                Some(604800) => "Weekly".into(),
                Some(n) => crate::model::duration_label(n),
                None => key.replace('_', " "),
            };
            Some(Meter {
                label: format!("{prefix}{label}"),
                used_percent,
                resets_at: timestamp(&window["reset_at"]).or_else(|| {
                    window["reset_after_seconds"]
                        .as_i64()
                        .map(|s| OffsetDateTime::now_utc() + time::Duration::seconds(s))
                }),
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::{Duration, Instant},
    };

    fn synthetic_credentials() -> (tempfile::TempDir, Credentials) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            serde_json::json!({"tokens":{
                "access_token":"synthetic-bearer-secret",
                "account_id":"synthetic-account",
                "expires_at":time::OffsetDateTime::now_utc().unix_timestamp() + 3600
            }})
            .to_string(),
        )
        .unwrap();
        let credentials = Credentials::load(path, D, vec!["tokens".into()]).unwrap();
        (dir, credentials)
    }

    fn local_response(
        status: u16,
        body: Option<&'static str>,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/codex-usage", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no local Codex request arrived");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("local Codex accept failed: {error}"),
                }
            };
            // Winsock inherits the listener's nonblocking mode.
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 4096];
            let read = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]).into_owned();
            if let Some(body) = body {
                write!(
                    socket,
                    "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 45\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            request
        });
        (endpoint, server)
    }

    fn expect_error(result: Result<Value>, reason: &str) -> ProviderError {
        match result {
            Ok(_) => panic!("{reason}"),
            Err(error) => error,
        }
    }

    #[test]
    fn local_response_waits_for_request_after_connection() {
        let (endpoint, server) = local_response(200, Some("{}"));
        let address = endpoint
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/codex-usage")
            .unwrap();
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let error = socket.read(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        let request = "GET /codex-usage HTTP/1.1\r\nHost: localhost\r\n\r\n";
        socket.write_all(request.as_bytes()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 Synthetic\r\n"));
        assert!(response.ends_with("\r\n\r\n{}"));
        assert_eq!(server.join().unwrap(), request);
    }

    #[tokio::test]
    async fn unknown_http_200_shape_reports_endpoint_and_status_without_body() {
        let (_dir, mut credentials) = synthetic_credentials();
        let auth_path = _dir.path().join("auth.json");
        let original = fs::read(&auth_path).unwrap();
        let (endpoint, server) =
            local_response(200, Some(r#"{"secret":"synthetic-response-secret"}"#));
        let client = Client::builder().no_proxy().build().unwrap();
        let result = fetch_at(&client, &mut credentials, &endpoint).await;
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /codex-usage HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-bearer-secret")
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("chatgpt-account-id: synthetic-account")
        );
        let error = expect_error(result, "unknown quota shape was accepted");
        assert_eq!(error.status, crate::model::Status::Error);
        assert!(error.message.contains(&endpoint));
        assert!(error.message.contains("HTTP 200"));
        assert!(!error.message.contains("synthetic-response-secret"));
        assert!(!error.message.contains("synthetic-bearer-secret"));
        assert_eq!(fs::read(auth_path).unwrap(), original);
    }

    const CODEX_VALID_USAGE_JSON: &str = r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_at":1788575709}}}"#;

    #[tokio::test]
    async fn discovery_prefers_nested_tokens_falls_back_to_root_and_rejects_missing_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let client = Client::builder().no_proxy().build().unwrap();

        let missing_file = discover_at(path.clone()).err().unwrap();
        assert_eq!(missing_file.status, crate::model::Status::Reconnect);
        assert!(missing_file.message.contains("codex login"));

        // Nested `tokens` object without access_token still reconnects (does not bypass tokens object).
        fs::write(
            &path,
            serde_json::json!({
                "access_token":"synthetic-root-only-secret",
                "tokens":{"expires_at":time::OffsetDateTime::now_utc().unix_timestamp() + 3600}
            })
            .to_string(),
        )
        .unwrap();
        let err = discover_at(path.clone()).err().unwrap();
        assert_eq!(err.status, crate::model::Status::Reconnect);
        assert!(err.message.contains("codex login"));
        assert!(!err.message.contains("synthetic-root-only-secret"));

        // API-key-only files (with or without `"tokens": null`) reconnect without leaking the key.
        for doc in [
            serde_json::json!({"OPENAI_API_KEY": "sk-synthetic-secret"}),
            serde_json::json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-synthetic-secret", "tokens": null}),
        ] {
            fs::write(&path, doc.to_string()).unwrap();
            let err = discover_at(path.clone()).err().unwrap();
            assert_eq!(err.status, crate::model::Status::Reconnect);
            assert!(err.message.contains("codex login"));
            assert!(!err.message.contains("sk-synthetic-secret"));
        }

        // Root-level access_token and account_id work when `tokens` is absent.
        fs::write(
            &path,
            serde_json::json!({
                "access_token": "synthetic-root-token",
                "account_id": "synthetic-root-account"
            })
            .to_string(),
        )
        .unwrap();
        let mut root_creds = discover_at(path.clone()).expect("root-level Codex auth should load");
        let (endpoint, server) = local_response(200, Some(CODEX_VALID_USAGE_JSON));
        let payload = fetch_at(&client, &mut root_creds, &endpoint)
            .await
            .expect("root-level Codex fetch should succeed");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer synthetic-root-token"));
        assert!(request.contains("chatgpt-account-id: synthetic-root-account"));
        assert_eq!(Codex.parse(&payload).unwrap().meters[0].used_percent, 12.0);

        // Nested `tokens` is preferred over root `access_token`, with root `account_id` fallback
        // (including when nested `account_id` is whitespace-only).
        fs::write(
            &path,
            serde_json::json!({
                "access_token": "synthetic-ignored-root-token",
                "account_id": "synthetic-root-fallback-account",
                "tokens": {
                    "access_token": "synthetic-nested-token",
                    "account_id": "   "
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut nested_creds = discover_at(path.clone()).expect("nested Codex auth should load");
        let (endpoint, server) = local_response(200, Some(CODEX_VALID_USAGE_JSON));
        fetch_at(&client, &mut nested_creds, &endpoint)
            .await
            .expect("nested Codex fetch should succeed");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer synthetic-nested-token"));
        assert!(request.contains("chatgpt-account-id: synthetic-root-fallback-account"));

        // Absent account_id omits the ChatGPT-Account-Id header.
        fs::write(
            &path,
            serde_json::json!({"access_token": "synthetic-no-account-token"}).to_string(),
        )
        .unwrap();
        let mut no_account_creds =
            discover_at(path).expect("root Codex auth without account_id should load");
        let (endpoint, server) = local_response(200, Some(CODEX_VALID_USAGE_JSON));
        fetch_at(&client, &mut no_account_creds, &endpoint)
            .await
            .expect("Codex fetch without account_id should succeed");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer synthetic-no-account-token"));
        assert!(!request.contains("chatgpt-account-id"));
    }

    #[tokio::test]
    async fn past_or_near_expiry_reaches_server_and_follows_authoritative_status() {
        let client = Client::builder().no_proxy().build().unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();

        for offset in [-60, 30] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            let original = serde_json::json!({"tokens": {
                "access_token": "synthetic-skew-token",
                "account_id": "synthetic-account",
                "expires_at": now + offset
            }})
            .to_string();
            fs::write(&path, &original).unwrap();

            // Clock skew: server authoritatively accepts the token with 200.
            let mut creds = discover_at(path.clone()).unwrap();
            let (endpoint, server) = local_response(200, Some(CODEX_VALID_USAGE_JSON));
            let payload = fetch_at(&client, &mut creds, &endpoint)
                .await
                .expect("past/near expires_at should reach server and succeed on 200");
            let request = server.join().unwrap().to_ascii_lowercase();
            assert!(request.contains("authorization: bearer synthetic-skew-token"));
            assert_eq!(Codex.parse(&payload).unwrap().meters[0].used_percent, 12.0);
            assert_eq!(fs::read_to_string(&path).unwrap(), original);

            // Truly expired: server authoritatively rejects with 401 -> Status::Reconnect.
            let mut creds = discover_at(path.clone()).unwrap();
            let (endpoint, server) = local_response(401, Some(r#"{"error":"token_expired"}"#));
            let err = expect_error(
                fetch_at(&client, &mut creds, &endpoint).await,
                "server 401 was accepted",
            );
            server.join().unwrap();
            assert_eq!(err.status, crate::model::Status::Reconnect);
            assert!(err.message.contains("HTTP 401"));
            assert!(err.message.contains("codex login"));
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn malformed_http_200_json_reports_endpoint_and_status_without_body() {
        let (_dir, mut credentials) = synthetic_credentials();
        let (endpoint, server) =
            local_response(200, Some(r#"{"secret":"synthetic-response-secret""#));
        let client = Client::builder().no_proxy().build().unwrap();
        let result = fetch_at(&client, &mut credentials, &endpoint).await;
        server.join().unwrap();
        let error = expect_error(result, "malformed JSON was accepted");
        assert_eq!(error.status, crate::model::Status::Error);
        assert!(error.message.contains(&endpoint));
        assert!(error.message.contains("HTTP 200"));
        assert!(!error.message.contains("synthetic-response-secret"));
        assert!(!error.message.contains("synthetic-bearer-secret"));
    }

    #[tokio::test]
    async fn diagnostics_keep_actual_status_and_do_not_invent_one_for_transport_errors() {
        let client = Client::builder().no_proxy().build().unwrap();
        for (status, body) in [
            (201, r#"{"secret":"synthetic-response-secret"}"#),
            (202, r#"{"secret":"synthetic-response-secret""#),
        ] {
            let (_dir, mut credentials) = synthetic_credentials();
            let (endpoint, server) = local_response(status, Some(body));
            let result = fetch_at(&client, &mut credentials, &endpoint).await;
            server.join().unwrap();
            let error = expect_error(result, "changed response was accepted");
            assert_eq!(error.status, crate::model::Status::Error);
            assert!(error.message.contains(&endpoint));
            assert!(error.message.contains(&format!("HTTP {status}")));
            assert!(!error.message.contains("HTTP 200"));
            assert!(!error.message.contains("synthetic-response-secret"));
        }

        for (status, expected) in [
            (401, crate::model::Status::Reconnect),
            (404, crate::model::Status::Error),
            (429, crate::model::Status::Error),
        ] {
            let (_dir, mut credentials) = synthetic_credentials();
            let (endpoint, server) =
                local_response(status, Some(r#"{"secret":"synthetic-response-secret"}"#));
            let result = fetch_at(&client, &mut credentials, &endpoint).await;
            server.join().unwrap();
            let error = expect_error(result, "HTTP error was accepted");
            assert_eq!(error.status, expected);
            assert!(error.message.contains(&endpoint));
            assert!(error.message.contains(&format!("HTTP {status}")));
            assert!(!error.message.contains("synthetic-response-secret"));
            if status == 401 {
                assert!(error.message.contains("codex login"));
            }
            if status == 429 {
                assert_eq!(error.retry_after_secs, Some(45));
            }
        }

        let (_dir, mut credentials) = synthetic_credentials();
        let (endpoint, server) = local_response(200, None);
        let result = fetch_at(&client, &mut credentials, &endpoint).await;
        server.join().unwrap();
        let error = expect_error(result, "closed response was accepted");
        assert_eq!(error.status, crate::model::Status::Error);
        assert!(error.message.contains(&endpoint));
        assert!(!error.message.contains("HTTP 200"));
        assert!(!error.message.contains("synthetic-bearer-secret"));
    }

    #[test]
    fn captured_window_duration_drives_label() {
        let v: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/codex.json")).unwrap();
        let u = Codex.parse(&v).unwrap();
        assert_eq!(u.meters[0].label, "Weekly");
        assert_eq!(u.meters[0].used_percent, 2.0);
        assert!(
            Codex
                .parse(&serde_json::json!({"rate_limit":{"primary_window":{}}}))
                .is_err()
        );
    }

    #[test]
    fn named_additional_quota_keeps_model_and_both_reported_windows() {
        let usage = Codex
            .parse(&serde_json::json!({
                "additional_rate_limits": [{
                    "limit_name": "GPT-5.3-Codex-Spark",
                    "metered_feature": "codex_bengalfox",
                    "rate_limit": {
                        "primary_window": {
                            "used_percent": 20,
                            "limit_window_seconds": 18000,
                            "reset_at": 1788575709
                        },
                        "secondary_window": {
                            "used_percent": 40,
                            "limit_window_seconds": 604800,
                            "reset_at": 1789156978
                        }
                    }
                }]
            }))
            .unwrap();
        assert_eq!(usage.meters.len(), 2);
        assert_eq!(usage.meters[0].label, "GPT-5.3-Codex-Spark 5 hours");
        assert_eq!(usage.meters[1].label, "GPT-5.3-Codex-Spark Weekly");
        assert_eq!(usage.meters[0].used_percent, 20.0);
        assert_eq!(usage.meters[1].used_percent, 40.0);
        assert!(usage.meters.iter().all(|meter| meter.resets_at.is_some()));
    }

    #[test]
    fn additional_quota_falls_back_without_guessing_a_model_name() {
        let feature = serde_json::json!({"metered_feature": "codex_bengalfox"});
        assert_eq!(additional_name(&feature), "codex_bengalfox");
        let redacted_name = serde_json::json!({
            "limit_name": "<redacted>",
            "metered_feature": "codex_bengalfox"
        });
        assert_eq!(additional_name(&redacted_name), "codex_bengalfox");
        let absent = serde_json::json!({"limit_name": "  ", "metered_feature": "<redacted>"});
        assert_eq!(additional_name(&absent), "Additional");
    }
}
