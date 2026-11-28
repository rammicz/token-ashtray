use super::*;
use crate::model::{number, percent, percent100, timestamp};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::json;
pub struct Cursor;
pub struct GrokBot;
const D: Descriptor = Descriptor {
    id: "cursor",
    name: "Cursor",
    link: "https://cursor.com/dashboard/usage",
    login: "Open Cursor and sign in to your account.",
};
const BOT: Descriptor = Descriptor {
    id: "grok-bot",
    name: "Grok Bot",
    link: "https://cursor.com/dashboard/usage",
    login: "Open Cursor and sign in to your account.",
};
const PLAN: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo";
const SAND: [&str; 2] = [
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus",
    "https://api2.cursor.sh/aiserver.v1.SandBoxService/GetSandUsageStatus",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageMethod {
    PostJson,
    Get,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UsageEndpoint<'a> {
    method: UsageMethod,
    url: &'a str,
}

const USAGE_ENDPOINTS: [UsageEndpoint<'static>; 4] = [
    UsageEndpoint {
        method: UsageMethod::PostJson,
        url: "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
    },
    UsageEndpoint {
        method: UsageMethod::PostJson,
        url: "https://cursor.com/api/dashboard/get-current-period-usage",
    },
    UsageEndpoint {
        method: UsageMethod::Get,
        url: "https://cursor.com/api/usage-summary",
    },
    UsageEndpoint {
        method: UsageMethod::Get,
        url: "https://api2.cursor.sh/auth/usage",
    },
];
fn discover() -> Result<Credentials> {
    let paths = candidate_paths(&crate::platform::credential_base_paths());
    let path = paths
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| D.reconnect("Cursor login database not found"))?;
    read_token(&path).map(Credentials::token)
}
pub(super) fn candidate_paths(bases: &crate::platform::CredentialBasePaths) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [&bases.roaming_app_data, &bases.local_app_data]
        .into_iter()
        .flatten()
    {
        paths.push(path.join("Cursor/User/globalStorage/state.vscdb"));
    }
    paths.push(
        bases
            .home
            .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
    );
    let config = bases
        .xdg_config_home
        .as_ref()
        .cloned()
        .unwrap_or_else(|| bases.home.join(".config"));
    paths.push(config.join("Cursor/User/globalStorage/state.vscdb"));
    paths
}
fn read_token(path: &std::path::Path) -> Result<String> {
    let absolute = path
        .canonicalize()
        .map_err(|_| ProviderError::error("Cannot resolve Cursor database path"))?;
    let mut uri = reqwest::Url::from_file_path(absolute)
        .map_err(|_| ProviderError::error("Invalid Cursor database path"))?;
    uri.query_pairs_mut().append_pair("mode", "ro");
    let connection = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ProviderError::error("Cannot open Cursor login database read-only"))?;
    connection
        .busy_timeout(std::time::Duration::from_millis(200))
        .ok();
    let token = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
            [],
            |row| {
                Ok(match row.get_ref(0)? {
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                        String::from_utf8_lossy(bytes).into_owned()
                    }
                    _ => String::new(),
                })
            },
        )
        .map_err(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                D.reconnect("Cursor access token missing")
            } else {
                ProviderError::error("Cannot read Cursor access token row")
            }
        })?;
    if token.trim().is_empty() {
        return Err(D.reconnect("Cursor access token is empty"));
    }
    Ok(token)
}
// Try Cursor's usage routes in order on unrecognised HTTP 200 shapes or
// non-auth/non-429 failures (404/405/5xx, invalid JSON, transport errors).
// Authentication (401) and rate-limit (429) responses stay terminal across
// primary and fallback routes so rejected credentials and backoff are retained.
async fn fetch_usage(
    client: &Client,
    token: &str,
    endpoints: &[UsageEndpoint<'_>],
) -> Result<Value> {
    let mut last = D.invalid();
    for endpoint in endpoints {
        let request = match endpoint.method {
            UsageMethod::PostJson => client.post(endpoint.url).json(&json!({})),
            UsageMethod::Get => client.get(endpoint.url),
        };
        match response(
            request
                .bearer_auth(token)
                .header("Accept", "application/json")
                .send()
                .await,
            endpoint.url,
            D,
        )
        .await
        {
            Ok(v) if !cursor_meters(&v).is_empty() => return Ok(v),
            Ok(_) => last = D.invalid(),
            // A fallback route cannot repair rejected credentials or backoff.
            Err(e) if e.status == Status::Reconnect || e.retry_after_secs.is_some() => {
                return Err(e);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}
#[async_trait]
impl Provider for Cursor {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        discover()
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        let token = credentials.access(D)?;
        let mut payload = fetch_usage(client, token, &USAGE_ENDPOINTS).await?;
        match response(
            client
                .post(PLAN)
                .bearer_auth(token)
                .json(&json!({}))
                .send()
                .await,
            PLAN,
            D,
        )
        .await
        {
            Ok(plan) => payload["_plan_info"] = plan,
            Err(e) if e.retry_after_secs.is_some() => return Err(e),
            Err(_) => {}
        }
        Ok(payload)
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let meters = cursor_meters(payload);
        if meters.is_empty() {
            return Err(D.invalid());
        }
        // The live plan-info answer nests the name under `planInfo` (2026-11-26).
        let plan = string_at(&payload["_plan_info"]["planInfo"], &["planName"])
            .or_else(|| {
                string_at(
                    &payload["_plan_info"],
                    &["planName", "planType", "membershipType", "plan"],
                )
            })
            .or_else(|| string_at(payload, &["membershipType", "plan", "planName"]));
        Ok(Usage {
            meters,
            plan,
            ..Default::default()
        })
    }
}
#[async_trait]
impl Provider for GrokBot {
    fn descriptor(&self) -> Descriptor {
        BOT
    }
    fn discover(&self) -> Result<Credentials> {
        discover()
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        fetch_sand(client, credentials.access(BOT)?, &SAND).await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let meters = sand_meters(payload);
        if meters.is_empty() {
            return Err(BOT.invalid());
        }
        Ok(Usage {
            meters,
            ..Default::default()
        })
    }
}
async fn fetch_sand(client: &Client, token: &str, endpoints: &[&str]) -> Result<Value> {
    let mut last = BOT.invalid();
    for &endpoint in endpoints {
        match response(
            client
                .post(endpoint)
                .bearer_auth(token)
                .json(&json!({}))
                .send()
                .await,
            endpoint,
            BOT,
        )
        .await
        {
            Ok(v) if sand_suppressed(&v) => return Err(BOT.invalid()),
            Ok(v) if !sand_meters(&v).is_empty() => return Ok(v),
            Ok(_) => last = BOT.invalid(),
            // A second route cannot repair rejected credentials or backoff.
            Err(e) if e.status == Status::Reconnect || e.retry_after_secs.is_some() => {
                return Err(e);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}
fn cursor_reset(value: &Value) -> Option<time::OffsetDateTime> {
    [
        &value["billingCycleEnd"],
        &value["billingCycleEndMs"],
        &value["planUsage"]["billingCycleEnd"],
        &value["individualUsage"]["plan"]["billingCycleEnd"],
        &value["_plan_info"]["planInfo"]["billingCycleEnd"],
        &value["_plan_info"]["billingCycleEnd"],
    ]
    .into_iter()
    .find_map(timestamp)
}
fn cursor_meters(value: &Value) -> Vec<Meter> {
    let mut meters = Vec::new();
    let reset = cursor_reset(value);
    // Cursor's reported usage percentage and included-spend budget have
    // different semantics. Keep the fields separate instead of treating
    // totalSpend/limit as an equivalent percentage (vendor support confirms).
    // Auto and API usage are separate percentages in the same response
    // (structure verified live 2026-11-26); each is shown only when reported.
    let plan = &value["planUsage"];
    let percent_source = if plan.is_object() { plan } else { value };
    for (label, key) in [
        ("Reported plan usage", "totalPercentUsed"),
        ("Auto usage", "autoPercentUsed"),
        ("API usage", "apiPercentUsed"),
    ] {
        if let Some(used_percent) = percent100(&percent_source[key]) {
            meters.push(Meter {
                label: label.into(),
                used_percent,
                resets_at: reset,
            });
        }
    }
    if let (Some(used), Some(limit)) = (number(&plan["includedSpend"]), number(&plan["limit"]))
        && used >= 0.0
        && limit > 0.0
    {
        meters.push(Meter {
            label: "Included credit budget".into(),
            used_percent: (used / limit * 100.0).clamp(0.0, 100.0) as f32,
            resets_at: reset,
        });
    }
    for (label, node) in [
        ("Plan budget", &value["individualUsage"]["plan"]),
        ("On-demand budget", &value["individualUsage"]["onDemand"]),
    ] {
        if node["enabled"].as_bool() != Some(false)
            && let Some(mut meter) = quota_meter(label, node)
        {
            meter.resets_at = meter.resets_at.or(reset);
            meters.push(meter);
        }
    }
    for (label, node) in [
        ("Usage", value),
        ("Included", &value["includedUsage"]),
        ("Plan", &value["planUsage"]),
        ("Requests", &value["usage"]),
        ("Premium requests", &value["gpt-4"]),
    ] {
        if let Some(mut m) = quota_meter(label, node) {
            m.resets_at = m.resets_at.or(reset);
            meters.push(m);
            continue;
        }
        if let (Some(used), Some(limit)) = (
            number(&node["numRequests"]),
            number(&node["maxRequestUsage"]),
        ) && used >= 0.0
            && limit > 0.0
        {
            meters.push(Meter {
                label: label.into(),
                used_percent: (used / limit * 100.0).clamp(0.0, 100.0) as f32,
                resets_at: timestamp(&value["endOfMonth"]).or(reset),
            });
        }
    }
    meters
}
const SAND_ENVELOPES: [&str; 4] = ["usageStatus", "sandUsageStatus", "status", "config"];
fn sand_envelopes(value: &Value) -> impl Iterator<Item = &Value> {
    SAND_ENVELOPES
        .iter()
        .filter_map(|key| value.get(key).filter(|nested| nested.is_object()))
}
fn has_sand_suppression_flag(node: &Value) -> bool {
    [
        "usesPooledEnterpriseAllowance",
        "uses_pooled_enterprise_allowance",
        "includedLimitZero",
        "included_limit_zero",
    ]
    .iter()
    .any(|key| node.get(key).and_then(Value::as_bool) == Some(true))
}
fn sand_suppressed(value: &Value) -> bool {
    has_sand_suppression_flag(value) || sand_envelopes(value).any(has_sand_suppression_flag)
}
fn sand_meters(value: &Value) -> Vec<Meter> {
    if sand_suppressed(value) {
        return Vec::new();
    }
    sand_envelopes(value)
        .chain(std::iter::once(value))
        .map(|target| sand_meters_from(target, value))
        .find(|meters| !meters.is_empty())
        .unwrap_or_default()
}
fn sand_meters_from(target: &Value, outer: &Value) -> Vec<Meter> {
    [
        target,
        &target["usage"],
        &target["daily"],
        &target["weekly"],
    ]
    .iter()
    .enumerate()
    .filter_map(|(i, node)| {
        let label = ["Grok Bot", "Usage", "Daily", "Weekly"][i];
        let reset = sand_reset(node).or_else(|| {
            (i == 0 && !std::ptr::eq(*node, outer))
                .then(|| sand_reset(outer))
                .flatten()
        });
        let mut meter = quota_meter(label, node).or_else(|| {
            // The live `usagePercent` / `usage_percent` is already 0–100
            // (2026-11-25), so a value below 1 is a small percentage, not a fraction.
            let used_percent = ["percentage", "percentUsed"]
                .iter()
                .find_map(|key| node.get(key).and_then(percent))
                .or_else(|| {
                    ["usagePercent", "usage_percent"]
                        .iter()
                        .find_map(|key| node.get(key).and_then(percent100))
                })?;
            Some(Meter {
                label: label.into(),
                used_percent,
                resets_at: None,
            })
        })?;
        meter.resets_at = meter.resets_at.or(reset);
        Some(meter)
    })
    .collect()
}
fn utc_timestamp(value: &Value) -> Option<time::OffsetDateTime> {
    timestamp(value).or_else(|| {
        let text = value.as_str()?;
        if text.is_empty() {
            return None;
        }
        timestamp(&Value::String(text.replacen(' ', "T", 1) + "Z"))
    })
}
/// The live response (2026-11-25) reports `nextResetTimestampUtc` (and snake_case
/// `next_reset_timestamp_utc`); its offset form is unverified, so a date-time
/// without an offset is read as the UTC the key names. The other keys are older
/// synthetic aliases with no UTC contract.
fn sand_reset(node: &Value) -> Option<time::OffsetDateTime> {
    ["nextResetTimestampUtc", "next_reset_timestamp_utc"]
        .iter()
        .find_map(|key| node.get(key).and_then(utc_timestamp))
        .or_else(|| {
            ["resetTime", "resetAt", "resets_at"]
                .iter()
                .find_map(|key| node.get(key).and_then(timestamp))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn read_http_request(socket: &mut std::net::TcpStream) -> (String, String, String) {
        use std::{io::Read, time::Duration};
        // Winsock inherits the listener's nonblocking mode.
        // Keep accept cancellable, but wait for each request body.
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut raw = Vec::new();
        let (header_end, content_length) = loop {
            let mut chunk = [0; 4096];
            let size = socket.read(&mut chunk).unwrap();
            assert_ne!(size, 0, "request ended before headers");
            raw.extend_from_slice(&chunk[..size]);
            if let Some(header_end) = raw.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&raw[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if raw.len() >= header_end + 4 + content_length {
                    break (header_end, content_length);
                }
            }
        };
        let headers = String::from_utf8_lossy(&raw[..header_end]).into_owned();
        let line = headers.lines().next().unwrap().to_owned();
        assert!(
            headers.lines().any(|header| {
                header
                    .trim_end_matches('\r')
                    .eq_ignore_ascii_case("authorization: Bearer synthetic-test-token")
            }),
            "missing Bearer auth header: {headers}"
        );
        let body = String::from_utf8_lossy(&raw[header_end + 4..header_end + 4 + content_length])
            .into_owned();
        (line, headers, body)
    }
    struct MockHttpServer {
        origin: String,
        stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
        requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
        handle: std::thread::JoinHandle<()>,
    }
    impl MockHttpServer {
        fn start<F>(handler: F) -> Self
        where
            F: Fn(&str, &str) -> (u16, &'static str) + Send + 'static,
        {
            use std::{
                io::Write,
                net::TcpListener,
                sync::{
                    Arc, Mutex,
                    atomic::{AtomicBool, Ordering},
                },
                time::Duration,
            };
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stopped = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let server_stopped = stopped.clone();
            let server_requests = requests.clone();
            let handle = std::thread::spawn(move || {
                while !server_stopped.load(Ordering::Acquire) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    let (line, headers, body) = read_http_request(&mut socket);
                    let (status, response_body) = handler(&line, &headers);
                    server_requests.lock().unwrap().push((line, body));
                    write!(
                        socket,
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 45\r\nConnection: close\r\n\r\n{response_body}",
                        response_body.len()
                    )
                    .unwrap();
                }
            });
            Self {
                origin,
                stopped,
                requests,
                handle,
            }
        }
        fn finish(self) -> Vec<(String, String)> {
            self.stopped
                .store(true, std::sync::atomic::Ordering::Release);
            self.handle.join().unwrap();
            self.requests.lock().unwrap().clone()
        }
    }
    async fn sand_fixture(
        dashboard: (u16, &'static str),
        sandbox: (u16, &'static str),
    ) -> (Result<Value>, Vec<(String, String)>) {
        let server = MockHttpServer::start(move |line, _| {
            if line.contains("/sandbox ") {
                sandbox
            } else {
                dashboard
            }
        });
        let client = Client::builder().no_proxy().build().unwrap();
        let result = fetch_sand(
            &client,
            "synthetic-test-token",
            &[
                &format!("{}/dashboard", server.origin),
                &format!("{}/sandbox", server.origin),
            ],
        )
        .await;
        (result, server.finish())
    }
    async fn usage_fixture(
        primary: (u16, &'static str),
        fallback_post: (u16, &'static str),
        fallback_get: (u16, &'static str),
    ) -> (Result<Value>, Vec<(String, String)>) {
        let server = MockHttpServer::start(move |line, headers| {
            assert!(
                headers.lines().any(|header| {
                    header
                        .trim_end_matches('\r')
                        .eq_ignore_ascii_case("accept: application/json")
                }),
                "missing Accept: application/json header: {headers}"
            );
            if line.contains("/primary ") {
                primary
            } else if line.contains("/fallback-post ") {
                fallback_post
            } else {
                fallback_get
            }
        });
        let client = Client::builder().no_proxy().build().unwrap();
        let primary_url = format!("{}/primary", server.origin);
        let fallback_post_url = format!("{}/fallback-post", server.origin);
        let fallback_get_url = format!("{}/fallback-get", server.origin);
        let endpoints = [
            UsageEndpoint {
                method: UsageMethod::PostJson,
                url: &primary_url,
            },
            UsageEndpoint {
                method: UsageMethod::PostJson,
                url: &fallback_post_url,
            },
            UsageEndpoint {
                method: UsageMethod::Get,
                url: &fallback_get_url,
            },
        ];
        let result = fetch_usage(&client, "synthetic-test-token", &endpoints).await;
        (result, server.finish())
    }
    #[tokio::test]
    async fn sand_auth_failure_is_terminal_even_if_sandbox_would_error() {
        let (result, requests) = sand_fixture((401, "{}"), (500, "{}")).await;
        let error = result.unwrap_err();
        assert_eq!(error.status, Status::Reconnect);
        assert_eq!(
            requests,
            vec![("POST /dashboard HTTP/1.1".into(), "{}".into())]
        );
    }
    #[tokio::test]
    async fn sand_version_skew_fallback_keeps_post_order_and_json_body() {
        const KNOWN: &str = r#"{"percentage":"0.42","resetTime":{"seconds":1770000000}}"#;
        for dashboard in [(200, "{}"), (404, "{}"), (500, "{}")] {
            let (result, requests) = sand_fixture(dashboard, (200, KNOWN)).await;
            let payload = result.unwrap();
            assert_eq!(
                GrokBot.parse(&payload).unwrap().meters[0].used_percent,
                42.0
            );
            assert_eq!(
                requests,
                vec![
                    ("POST /dashboard HTTP/1.1".into(), "{}".into()),
                    ("POST /sandbox HTTP/1.1".into(), "{}".into())
                ],
                "Dashboard HTTP {} must try the SandBox route second",
                dashboard.0
            );
        }
        let (result, requests) = sand_fixture((200, KNOWN), (500, "{}")).await;
        assert!(result.is_ok());
        assert_eq!(
            requests,
            vec![("POST /dashboard HTTP/1.1".into(), "{}".into())]
        );
        let (result, requests) = sand_fixture((200, "{}"), (200, "{}")).await;
        assert!(
            result
                .unwrap_err()
                .message
                .contains("no recognised quota meters")
        );
        assert_eq!(requests.len(), 2);
    }
    #[tokio::test]
    async fn sand_rate_limit_is_terminal_with_retry_after() {
        let (result, requests) = sand_fixture((429, "{}"), (200, r#"{"percentage":42}"#)).await;
        let error = result.unwrap_err();
        assert_eq!(error.status, Status::Error);
        assert_eq!(error.retry_after_secs, Some(45));
        assert_eq!(
            requests,
            vec![("POST /dashboard HTTP/1.1".into(), "{}".into())]
        );
    }
    #[tokio::test]
    async fn sand_suppressed_response_is_terminal_without_sandbox_fallback() {
        for suppressed in [
            r#"{"usesPooledEnterpriseAllowance":true,"usagePercent":50}"#,
            r#"{"includedLimitZero":true,"usagePercent":0}"#,
            r#"{"usageStatus":{"uses_pooled_enterprise_allowance":true,"usage_percent":50}}"#,
        ] {
            let (result, requests) =
                sand_fixture((200, suppressed), (200, r#"{"usagePercent":42}"#)).await;
            let error = result.unwrap_err();
            assert!(error.message.contains("no recognised quota meters"));
            assert_eq!(
                requests,
                vec![("POST /dashboard HTTP/1.1".into(), "{}".into())],
                "suppressed dashboard response must not fall back to SandBoxService: {suppressed}"
            );
        }
    }
    #[test]
    fn usage_endpoints_use_expected_methods_and_order() {
        assert_eq!(
            USAGE_ENDPOINTS,
            [
                UsageEndpoint {
                    method: UsageMethod::PostJson,
                    url: "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
                },
                UsageEndpoint {
                    method: UsageMethod::PostJson,
                    url: "https://cursor.com/api/dashboard/get-current-period-usage",
                },
                UsageEndpoint {
                    method: UsageMethod::Get,
                    url: "https://cursor.com/api/usage-summary",
                },
                UsageEndpoint {
                    method: UsageMethod::Get,
                    url: "https://api2.cursor.sh/auth/usage",
                },
            ]
        );
    }
    #[tokio::test]
    async fn usage_fallback_continues_on_non_auth_errors_and_unknown_shapes() {
        const KNOWN_POST: &str = r#"{"planUsage":{"totalPercentUsed":10}}"#;
        const KNOWN_GET: &str = r#"{"planUsage":{"totalPercentUsed":20}}"#;

        for primary in [
            (200, "{}"),
            (200, "not-json"),
            (404, "{}"),
            (405, "{}"),
            (500, "{}"),
        ] {
            let (result, requests) = usage_fixture(primary, (200, KNOWN_POST), (500, "{}")).await;
            let payload = result.unwrap();
            assert_eq!(Cursor.parse(&payload).unwrap().meters[0].used_percent, 10.0);
            assert_eq!(
                requests,
                vec![
                    ("POST /primary HTTP/1.1".into(), "{}".into()),
                    ("POST /fallback-post HTTP/1.1".into(), "{}".into()),
                ],
                "primary {:?} must fall back to POST /fallback-post",
                primary
            );
        }

        for fallback_post in [
            (200, "{}"),
            (200, "not-json"),
            (404, "{}"),
            (405, "{}"),
            (500, "{}"),
        ] {
            let (result, requests) =
                usage_fixture((404, "{}"), fallback_post, (200, KNOWN_GET)).await;
            let payload = result.unwrap();
            assert_eq!(Cursor.parse(&payload).unwrap().meters[0].used_percent, 20.0);
            assert_eq!(
                requests,
                vec![
                    ("POST /primary HTTP/1.1".into(), "{}".into()),
                    ("POST /fallback-post HTTP/1.1".into(), "{}".into()),
                    ("GET /fallback-get HTTP/1.1".into(), "".into()),
                ],
                "intermediate fallback {:?} must fall through to GET /fallback-get",
                fallback_post
            );
        }

        // Returning last error when every endpoint fails.
        let (result, requests) = usage_fixture((404, "{}"), (405, "{}"), (503, "{}")).await;
        let error = result.unwrap_err();
        assert_eq!(error.status, Status::Error);
        assert!(error.message.contains("HTTP 503"), "{}", error.message);
        assert_eq!(requests.len(), 3);

        // All endpoints returning unknown HTTP 200 shapes returns invalid-shape error.
        let (result, requests) = usage_fixture((200, "{}"), (200, "{}"), (200, "{}")).await;
        let error = result.unwrap_err();
        assert!(
            error.message.contains("no recognised quota meters"),
            "{}",
            error.message
        );
        assert_eq!(requests.len(), 3);

        // Transport error on primary falls back to working fallback endpoint.
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let (line, _, body) = read_http_request(&mut socket);
            use std::io::Write;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{KNOWN_GET}",
                KNOWN_GET.len()
            )
            .unwrap();
            (line, body)
        });
        let dead_url = format!("http://127.0.0.1:{dead_port}/primary");
        let live_url = format!("{origin}/fallback-get");
        let result = fetch_usage(
            &Client::builder().no_proxy().build().unwrap(),
            "synthetic-test-token",
            &[
                UsageEndpoint {
                    method: UsageMethod::PostJson,
                    url: &dead_url,
                },
                UsageEndpoint {
                    method: UsageMethod::Get,
                    url: &live_url,
                },
            ],
        )
        .await;
        let (line, body) = server.join().unwrap();
        assert!(result.is_ok(), "transport error must fall back: {result:?}");
        assert_eq!(
            (line, body),
            ("GET /fallback-get HTTP/1.1".into(), "".into())
        );
    }
    #[tokio::test]
    async fn usage_auth_and_rate_limit_errors_stay_terminal_on_primary_and_fallback() {
        const KNOWN: &str = r#"{"planUsage":{"totalPercentUsed":10}}"#;

        for status in [401, 429] {
            let cases = [
                (
                    "primary",
                    usage_fixture((status, "{}"), (200, KNOWN), (200, KNOWN)).await,
                    vec![("POST /primary HTTP/1.1".into(), "{}".into())],
                ),
                (
                    "fallback",
                    usage_fixture((404, "{}"), (status, "{}"), (200, KNOWN)).await,
                    vec![
                        ("POST /primary HTTP/1.1".into(), "{}".into()),
                        ("POST /fallback-post HTTP/1.1".into(), "{}".into()),
                    ],
                ),
            ];
            for (stage, (result, requests), expected_requests) in cases {
                let error = result.unwrap_err();
                assert_eq!(
                    requests, expected_requests,
                    "{stage} HTTP {status} must not issue subsequent requests"
                );
                assert!(error.message.contains(&format!("HTTP {status}")));
                assert_eq!(
                    error.status,
                    if status == 401 {
                        Status::Reconnect
                    } else {
                        Status::Error
                    }
                );
                if status == 429 {
                    assert_eq!(error.retry_after_secs, Some(45));
                }
            }
        }
    }
    #[test]
    fn sqlite_text_blob_read_only_and_encoded_path() {
        let dir = tempfile::tempdir().unwrap();
        // Windows forbids '?' in filenames; spaces, '%' and '#' still exercise
        // URI encoding there. Unix also covers the query delimiter.
        let name = if cfg!(windows) {
            "state %#.vscdb"
        } else {
            "state ?%#.vscdb"
        };
        let path = dir.path().join(name);
        let db = Connection::open(&path).unwrap();
        db.execute("CREATE TABLE ItemTable (key TEXT, value)", [])
            .unwrap();
        db.execute(
            "INSERT INTO ItemTable VALUES ('cursorAuth/accessToken',?1)",
            ["text-value"],
        )
        .unwrap();
        assert_eq!(read_token(&path).unwrap(), "text-value");
        db.execute("UPDATE ItemTable SET value=?1", [b"blob-value".as_slice()])
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(read_token(&path).unwrap(), "blob-value");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn unknown_missing_and_timestamp_fraction() {
        assert!(Cursor.parse(&json!({"something_new":123})).is_err());
        let u = GrokBot
            .parse(&json!({"percentage":"0.42","resetTime":{"seconds":1770000000}}))
            .unwrap();
        assert_eq!(u.meters[0].used_percent, 42.0);
        assert_eq!(
            u.meters[0].resets_at,
            Some(time::OffsetDateTime::from_unix_timestamp(1770000000).unwrap())
        );
    }
    #[test]
    fn sand_reset_aliases_use_exact_protobuf_seconds() {
        let usage = GrokBot
            .parse(&json!({
                "used_percent": 12,
                "resetTime": {"seconds": "1770000000"},
                "usage": {"percentage": 0.25, "resetTime": {"seconds": 1770000001}},
                "daily": {"percentUsed": 50, "resetAt": {"seconds": 1770000002}},
                "weekly": {"usagePercent": 75, "resets_at": {"seconds": 1770000003}}
            }))
            .unwrap();
        assert_eq!(usage.meters.len(), 4);
        for (meter, seconds) in usage.meters.iter().zip(1770000000..=1770000003) {
            assert_eq!(
                meter.resets_at,
                Some(time::OffsetDateTime::from_unix_timestamp(seconds).unwrap())
            );
        }
        assert!(GrokBot.parse(&json!({"percentage": "unknown"})).is_err());
    }
    #[test]
    fn grok_bot_card_is_distinct_from_grok_cli() {
        let bot = GrokBot.descriptor().empty();
        let grok = super::super::grok::Grok.descriptor().empty();
        assert_ne!(bot.id, grok.id);
        assert_ne!(bot.short_name, grok.short_name);
        assert_eq!(bot.link, Cursor.descriptor().link);
        assert_eq!(GrokBot.descriptor().login, Cursor.descriptor().login);
        assert_ne!(bot.link, grok.link);
    }
    #[test]
    fn cursor_reported_percentage_is_not_invented_from_total_spend() {
        let meters = cursor_meters(
            &json!({"billingCycleEnd":"1780000000000","planUsage":{"totalPercentUsed":10,"totalSpend":500,"includedSpend":50,"limit":100}}),
        );
        assert_eq!(meters[0].used_percent, 10.0);
        assert_eq!(meters[1].used_percent, 50.0);
        assert!(meters.iter().all(|m| m.resets_at.is_some()));
    }
}
