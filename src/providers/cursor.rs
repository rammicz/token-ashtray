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
const USAGE: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const PLAN: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo";
const SAND: [&str; 2] = [
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus",
    "https://api2.cursor.sh/aiserver.v1.SandBoxService/GetSandUsageStatus",
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
// Only a successful response with an unrecognised quota shape permits a
// version-skew fallback. Authentication, HTTP and transport errors retain their
// original status and must not trigger more credential-bearing requests.
async fn fetch_usage(
    client: &Client,
    token: &str,
    primary_endpoint: &str,
    fallback_endpoints: &[&str],
) -> Result<Value> {
    let primary = response(
        client
            .post(primary_endpoint)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .json(&json!({}))
            .send()
            .await,
        primary_endpoint,
        D,
    )
    .await?;
    if !cursor_meters(&primary).is_empty() {
        return Ok(primary);
    }
    for endpoint in fallback_endpoints {
        let payload = response(
            client
                .get(*endpoint)
                .bearer_auth(token)
                .header("Accept", "application/json")
                .send()
                .await,
            endpoint,
            D,
        )
        .await?;
        if !cursor_meters(&payload).is_empty() {
            return Ok(payload);
        }
    }
    Err(D.invalid())
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
        let mut payload = fetch_usage(
            client,
            token,
            USAGE,
            &[
                "https://cursor.com/api/dashboard/get-current-period-usage",
                "https://cursor.com/api/usage-summary",
                "https://api2.cursor.sh/auth/usage",
            ],
        )
        .await?;
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
fn cursor_meters(value: &Value) -> Vec<Meter> {
    let mut meters = Vec::new();
    let reset = timestamp(&value["billingCycleEnd"]);
    // Cursor's reported usage percentage and included-spend budget have
    // different semantics. Keep the fields separate instead of treating
    // totalSpend/limit as an equivalent percentage (vendor support confirms).
    // Auto and API usage are separate percentages in the same response
    // (structure verified live 2026-11-26); each is shown only when reported.
    let plan = &value["planUsage"];
    for (label, key) in [
        ("Reported plan usage", "totalPercentUsed"),
        ("Auto usage", "autoPercentUsed"),
        ("API usage", "apiPercentUsed"),
    ] {
        if let Some(used_percent) = percent100(&plan[key]) {
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
        if let Some(m) = quota_meter(label, node) {
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
                resets_at: timestamp(&value["endOfMonth"]),
            });
        }
    }
    meters
}
fn sand_meters(value: &Value) -> Vec<Meter> {
    [value, &value["usage"], &value["daily"], &value["weekly"]]
        .iter()
        .enumerate()
        .filter_map(|(i, node)| {
            let label = ["Grok Bot", "Usage", "Daily", "Weekly"][i];
            let reset = sand_reset(node);
            let mut meter = quota_meter(label, node).or_else(|| {
                // The live `usagePercent` is already 0–100 (2026-11-25), so a
                // value below 1 is a small percentage, not a fraction.
                let used_percent = ["percentage", "percentUsed"]
                    .iter()
                    .find_map(|key| node.get(key).and_then(percent))
                    .or_else(|| node.get("usagePercent").and_then(percent100))?;
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
/// The live response (2026-11-25) reports `nextResetTimestampUtc`; its offset
/// form is unverified, so a date-time without an offset is read as the UTC the
/// key names. The other keys are older synthetic aliases with no UTC contract.
fn sand_reset(node: &Value) -> Option<time::OffsetDateTime> {
    let utc = &node["nextResetTimestampUtc"];
    timestamp(utc)
        .or_else(|| timestamp(&Value::String(utc.as_str()?.replacen(' ', "T", 1) + "Z")))
        .or_else(|| {
            ["resetTime", "resetAt", "resets_at"]
                .iter()
                .find_map(|key| node.get(key).and_then(timestamp))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn sand_fixture(
        dashboard: (u16, &'static str),
        sandbox: (u16, &'static str),
    ) -> (Result<Value>, Vec<(String, String)>) {
        use std::{
            io::{Read, Write},
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
        let server = std::thread::spawn(move || {
            while !server_stopped.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
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
                    if let Some(header_end) = raw.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    {
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
                let line = String::from_utf8_lossy(&raw[..header_end])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                assert!(
                    String::from_utf8_lossy(&raw[..header_end])
                        .lines()
                        .any(|header| header
                            .trim_end_matches('\r')
                            .eq_ignore_ascii_case("authorization: Bearer synthetic-test-token"))
                );
                let body =
                    String::from_utf8_lossy(&raw[header_end + 4..header_end + 4 + content_length])
                        .into_owned();
                let (status, response_body) = if line.contains("/sandbox ") {
                    sandbox
                } else {
                    dashboard
                };
                server_requests.lock().unwrap().push((line, body));
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 45\r\nConnection: close\r\n\r\n{response_body}", response_body.len()).unwrap();
            }
        });
        let client = Client::builder().no_proxy().build().unwrap();
        let result = fetch_sand(
            &client,
            "synthetic-test-token",
            &[&format!("{origin}/dashboard"), &format!("{origin}/sandbox")],
        )
        .await;
        stopped.store(true, Ordering::Release);
        server.join().unwrap();
        let captured = requests.lock().unwrap().clone();
        (result, captured)
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
    async fn fallback_is_only_used_for_successful_unknown_shapes() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{
                Arc, Mutex,
                atomic::{AtomicBool, Ordering},
            },
            time::Duration,
        };
        for status in [200, 401, 429, 500] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stopped = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let server_stopped = stopped.clone();
            let server_requests = requests.clone();
            let server = std::thread::spawn(move || {
                while !server_stopped.load(Ordering::Acquire) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    // Winsock inherits the listener's nonblocking mode.
                    // Keep accept cancellable, but wait for each request body.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let read = socket.read(&mut buffer).unwrap();
                    let line = String::from_utf8_lossy(&buffer[..read])
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned();
                    let fallback = line.contains("/fallback ");
                    server_requests.lock().unwrap().push(line);
                    let code = if fallback { 200 } else { status };
                    let body = if fallback {
                        r#"{"planUsage":{"totalPercentUsed":10}}"#
                    } else {
                        "{}"
                    };
                    write!(socket, "HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 45\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let result = fetch_usage(
                &client().unwrap(),
                "synthetic-test-token",
                &format!("{origin}/primary"),
                &[&format!("{origin}/fallback")],
            )
            .await;
            stopped.store(true, Ordering::Release);
            server.join().unwrap();
            let requests = requests.lock().unwrap();
            if status == 200 {
                assert!(result.is_ok());
                assert_eq!(requests.len(), 2);
                assert!(requests[0].starts_with("POST /primary "));
                assert!(requests[1].starts_with("GET /fallback "));
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    requests.len(),
                    1,
                    "HTTP {status} must not issue fallback requests"
                );
                assert!(error.message.contains(&format!("HTTP {status}")));
                assert_eq!(
                    error.status,
                    if status == 401 {
                        crate::model::Status::Reconnect
                    } else {
                        crate::model::Status::Error
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
