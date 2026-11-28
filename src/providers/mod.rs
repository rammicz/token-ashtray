//! Clean-room provider adapters. Credentials deliberately have no Debug impl.
mod antigravity;
mod claude;
mod codex;
mod copilot;
pub mod credentials;
mod cursor;
mod grok;
mod kimi;
mod vibe;

use crate::model::{Meter, ProductUsage, ProviderCard, Status};
use async_trait::async_trait;
pub use credentials::Credentials;
use reqwest::{Client, Response};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use time::OffsetDateTime;

#[derive(Clone, Copy)]
pub struct Descriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub link: &'static str,
    pub login: &'static str,
}
impl Descriptor {
    pub fn empty(self) -> ProviderCard {
        ProviderCard {
            id: self.id.into(),
            short_name: self.name.into(),
            plan: None,
            price: None,
            meters: vec![],
            usage_breakdown: vec![],
            status: Status::Reconnect,
            detail: format!("Checking existing login… {}", self.login),
            link: self.link.into(),
            fetched_at: None,
        }
    }
    pub fn reconnect(self, reason: &str) -> ProviderError {
        ProviderError {
            status: Status::Reconnect,
            message: format!("{reason}. {}", self.login),
            retry_after_secs: None,
        }
    }
    pub fn invalid(self) -> ProviderError {
        ProviderError::error(format!(
            "{} usage response has no recognised quota meters",
            self.name
        ))
    }
}

#[derive(Clone, Debug)]
pub struct ProviderError {
    pub status: Status,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}
impl ProviderError {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: Status::Error,
            message: message.into(),
            retry_after_secs: None,
        }
    }
}
pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Default)]
pub struct Usage {
    pub plan: Option<String>,
    pub price: Option<String>,
    pub meters: Vec<Meter>,
    pub usage_breakdown: Vec<ProductUsage>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn descriptor(&self) -> Descriptor;
    fn discover(&self) -> Result<Credentials>;
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value>;
    fn parse(&self, payload: &Value) -> Result<Usage>;
    async fn poll(&self, client: &Client) -> Result<ProviderCard> {
        let mut credentials = self.discover()?;
        let payload = self.fetch(client, &mut credentials).await?;
        let usage = self.parse(&payload)?;
        let mut card = self.descriptor().empty();
        card.plan = usage.plan;
        card.price = usage.price;
        card.meters = usage.meters;
        card.usage_breakdown = usage.usage_breakdown;
        card.status = Status::Live;
        card.detail = String::new();
        card.fetched_at = Some(OffsetDateTime::now_utc());
        Ok(card)
    }
}

pub fn all() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(antigravity::Antigravity),
        Arc::new(claude::Claude),
        Arc::new(codex::Codex),
        Arc::new(cursor::Cursor),
        Arc::new(cursor::GrokBot),
        Arc::new(grok::Grok),
        Arc::new(copilot::Copilot),
        Arc::new(kimi::Kimi),
        Arc::new(vibe::Vibe),
    ]
}

pub fn client() -> Result<Client> {
    build_client(false)
}

fn build_client(no_proxy: bool) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(concat!("TokenAshtray/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        // Credentials may only be sent to the documented provider origin.
        .redirect(reqwest::redirect::Policy::none());
    if no_proxy {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| ProviderError::error("Could not initialize TLS client"))
}

pub fn home() -> PathBuf {
    crate::platform::credential_base_paths().home
}

#[cfg(test)]
mod credential_path_tests {
    use super::*;

    #[test]
    fn provider_candidates_consume_each_injected_platform_base() {
        let bases = crate::platform::CredentialBasePaths {
            home: PathBuf::from("/fixture/home"),
            xdg_config_home: Some(PathBuf::from("/fixture/xdg")),
            local_app_data: Some(PathBuf::from("/fixture/local")),
            roaming_app_data: Some(PathBuf::from("/fixture/roaming")),
        };
        let cursor = cursor::candidate_paths(&bases);
        assert_eq!(
            cursor,
            [
                PathBuf::from("/fixture/roaming/Cursor/User/globalStorage/state.vscdb"),
                PathBuf::from("/fixture/local/Cursor/User/globalStorage/state.vscdb"),
                PathBuf::from(
                    "/fixture/home/Library/Application Support/Cursor/User/globalStorage/state.vscdb"
                ),
                PathBuf::from("/fixture/xdg/Cursor/User/globalStorage/state.vscdb"),
            ]
        );

        let copilot = copilot::candidate_directories_from(&bases);
        assert_eq!(
            copilot,
            [
                PathBuf::from("/fixture/home/.config/github-copilot"),
                PathBuf::from("/fixture/home/Library/Application Support/github-copilot"),
                PathBuf::from("/fixture/home/AppData/Local/github-copilot"),
                PathBuf::from("/fixture/local/github-copilot"),
                PathBuf::from("/fixture/roaming/GitHub Copilot"),
            ]
        );

        let home_only = crate::platform::CredentialBasePaths {
            home: PathBuf::from("/fixture/home"),
            xdg_config_home: None,
            local_app_data: None,
            roaming_app_data: None,
        };
        assert_eq!(
            cursor::candidate_paths(&home_only),
            [
                PathBuf::from(
                    "/fixture/home/Library/Application Support/Cursor/User/globalStorage/state.vscdb"
                ),
                PathBuf::from("/fixture/home/.config/Cursor/User/globalStorage/state.vscdb"),
            ]
        );
        assert_eq!(copilot::candidate_directories_from(&home_only).len(), 3);
    }
}

pub async fn response(
    response: std::result::Result<Response, reqwest::Error>,
    endpoint: &str,
    descriptor: Descriptor,
) -> Result<Value> {
    let response = response.map_err(|e| {
        ProviderError::error(format!(
            "{endpoint}: {}",
            if e.is_timeout() {
                "request timed out"
            } else {
                "connection failed"
            }
        ))
    })?;
    let status = response.status();
    if status.as_u16() == 401 {
        return Err(descriptor.reconnect(&format!("{endpoint}: HTTP {status}")));
    }
    if !status.is_success() {
        let mut error = ProviderError::error(format!("{endpoint}: HTTP {status}"));
        if status.as_u16() == 429 {
            error.retry_after_secs = Some(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(retry_after_seconds)
                    .unwrap_or(300),
            );
        }
        return Err(error);
    }
    // Never include the response body in diagnostics: OAuth responses contain secrets.
    response.json().await.map_err(|_| {
        ProviderError::error(format!(
            "{endpoint}: HTTP {status}: response is not valid JSON"
        ))
    })
}

pub async fn oauth_response(
    result: std::result::Result<Response, reqwest::Error>,
    endpoint: &str,
    descriptor: Descriptor,
) -> Result<Value> {
    if let Ok(response) = &result
        && matches!(response.status().as_u16(), 400 | 401 | 403)
    {
        return Err(descriptor.reconnect(&format!(
            "OAuth refresh failed (HTTP {})",
            response.status()
        )));
    }
    response(result, endpoint, descriptor).await
}

pub fn string_at(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

pub fn quota_meter(label: &str, value: &Value) -> Option<Meter> {
    use crate::model::{number, percent100, timestamp};
    let used_percent = [
        "used_percent",
        "usedPercent",
        "utilization",
        "percentage_used",
    ]
    .iter()
    .find_map(|key| value.get(key).and_then(percent100))
    .or_else(|| {
        value
            .get("percent_remaining")
            .and_then(percent100)
            .map(|n| 100.0 - n)
    })
    .or_else(|| {
        let used = ["used", "usage", "consumed"]
            .iter()
            .find_map(|key| value.get(key).and_then(number))?;
        let limit = ["limit", "total", "entitlement"]
            .iter()
            .find_map(|key| value.get(key).and_then(number))?;
        (used >= 0.0 && limit > 0.0).then_some((used / limit * 100.0).clamp(0.0, 100.0) as f32)
    })?;
    let resets_at = [
        "resets_at",
        "reset_at",
        "resetTime",
        "reset_date",
        "resetDate",
    ]
    .iter()
    .find_map(|key| value.get(key).and_then(timestamp));
    Some(Meter {
        label: label.into(),
        used_percent,
        resets_at,
    })
}

pub enum RefreshBody {
    Json(Value),
    Form(Vec<(String, String)>),
}

pub async fn refresh_safely(
    credentials: &mut Credentials,
    endpoint: impl Into<String>,
    descriptor: Descriptor,
    body: RefreshBody,
    camel_case: bool,
) -> Result<()> {
    refresh_safely_inner(
        credentials,
        endpoint.into(),
        descriptor,
        body,
        camel_case,
        false,
    )
    .await
}

async fn refresh_safely_inner(
    credentials: &mut Credentials,
    endpoint: String,
    descriptor: Descriptor,
    body: RefreshBody,
    camel_case: bool,
    no_proxy: bool,
) -> Result<()> {
    credentials::finish_refresh(credentials, move |mut owned| {
        // Own the I/O driver too: shutting down the polling runtime must not
        // interrupt a refresh after the provider invalidates its previous grant.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ProviderError::error("Could not start OAuth refresh worker"))?;
        runtime.block_on(async {
            let client = build_client(no_proxy)?;
            let request = client.post(&endpoint);
            let request = match body {
                RefreshBody::Json(body) => request.json(&body),
                RefreshBody::Form(body) => request.form(&body),
            };
            let payload = oauth_response(request.send().await, &endpoint, descriptor).await?;
            owned.apply_refresh(&payload, camel_case)?;
            Ok(owned)
        })
    })
    .await
}

pub async fn fetch_with_reactive_refresh<F, Fut>(
    credentials: &mut Credentials,
    descriptor: Descriptor,
    refresh_endpoint: &str,
    camel_case: bool,
    no_proxy_refresh: bool,
    build_refresh: impl Fn(&Credentials) -> Result<RefreshBody>,
    mut fetch_once: F,
) -> Result<Value>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let mut refreshed = false;
    if credentials.near_expiry() {
        let body = build_refresh(credentials)?;
        refresh_safely_inner(
            credentials,
            refresh_endpoint.to_owned(),
            descriptor,
            body,
            camel_case,
            no_proxy_refresh,
        )
        .await?;
        refreshed = true;
    }
    // Validate access_token before issuing usage requests so any Status::Reconnect
    // returned by `fetch_once` strictly represents an HTTP 401 from the server.
    let token = credentials.access(descriptor)?.to_owned();
    match fetch_once(token).await {
        Ok(value) => Ok(value),
        Err(error)
            if error.status == Status::Reconnect
                && !refreshed
                && credentials.refresh(descriptor).is_ok() =>
        {
            let body = build_refresh(credentials)?;
            refresh_safely_inner(
                credentials,
                refresh_endpoint.to_owned(),
                descriptor,
                body,
                camel_case,
                no_proxy_refresh,
            )
            .await?;
            let rotated = credentials.access(descriptor)?.to_owned();
            fetch_once(rotated).await
        }
        Err(error) => Err(error),
    }
}

fn retry_after_seconds(value: &str) -> Option<u64> {
    value.parse().ok().or_else(|| {
        let deadline =
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822)
                .ok()?;
        Some(
            (deadline - time::OffsetDateTime::now_utc())
                .whole_seconds()
                .max(0) as u64,
        )
    })
}

#[cfg(test)]
pub(crate) mod test_http {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Instant,
    };

    pub struct ScriptedReply {
        pub status: u16,
        pub body: String,
        pub retry_after: Option<String>,
        pub on_request: Option<Box<dyn FnOnce() + Send>>,
    }

    impl ScriptedReply {
        pub fn ok(body: impl Into<String>) -> Self {
            Self {
                status: 200,
                body: body.into(),
                retry_after: None,
                on_request: None,
            }
        }

        pub fn status(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                retry_after: None,
                on_request: None,
            }
        }

        pub fn rate_limit(retry_after: impl Into<String>) -> Self {
            Self {
                status: 429,
                body: r#"{"error":"rate limited"}"#.into(),
                retry_after: Some(retry_after.into()),
                on_request: None,
            }
        }

        pub fn with_hook(mut self, hook: impl FnOnce() + Send + 'static) -> Self {
            self.on_request = Some(Box::new(hook));
            self
        }
    }

    pub struct MockServer {
        url: String,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<Vec<String>>>,
    }

    impl MockServer {
        pub fn spawn(path: &str, replies: Vec<ScriptedReply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}{path}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_flag = Arc::clone(&stop);
            let mut queue = VecDeque::from(replies);
            let handle = thread::spawn(move || {
                let mut requests = Vec::new();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !stop_flag.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut socket, _)) => {
                            // Winsock inherits the listener's nonblocking mode.
                            socket.set_nonblocking(false).unwrap();
                            socket
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let request = read_http_request(&mut socket);
                            requests.push(request);
                            let reply = queue.pop_front().unwrap_or_else(|| {
                                ScriptedReply::status(500, r#"{"error":"unexpected request"}"#)
                            });
                            if let Some(hook) = reply.on_request {
                                hook();
                            }
                            let retry_header = reply
                                .retry_after
                                .as_deref()
                                .map(|secs| format!("Retry-After: {secs}\r\n"))
                                .unwrap_or_default();
                            let _ = write!(
                                socket,
                                "HTTP/1.1 {} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry_header}Connection: close\r\n\r\n{}",
                                reply.status,
                                reply.body.len(),
                                reply.body
                            );
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                break;
                            }
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("local mock server accept failed: {error}"),
                    }
                }
                requests
            });
            Self {
                url,
                stop,
                handle: Some(handle),
            }
        }

        pub fn url(&self) -> &str {
            &self.url
        }

        pub fn finish(mut self) -> Vec<String> {
            self.stop.store(true, Ordering::Release);
            self.handle.take().unwrap().join().unwrap()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_http_request(socket: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut header_end = None;
        let mut content_length = 0usize;
        loop {
            match socket.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if header_end.is_none()
                        && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let end = pos + 4;
                        header_end = Some(end);
                        let headers = String::from_utf8_lossy(&buf[..end]);
                        content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                    }
                    if let Some(end) = header_end
                        && buf.len() >= end + content_length
                    {
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("local mock server read failed: {error}"),
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn local_client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }
}

#[cfg(test)]
mod retry_header_tests {
    #[test]
    fn seconds_and_http_dates_both_preserve_backoff() {
        assert_eq!(super::retry_after_seconds("3600"), Some(3600));
        let future = (time::OffsetDateTime::now_utc() + time::Duration::hours(2))
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap();
        assert!(super::retry_after_seconds(&future).unwrap() >= 7198);
        assert_eq!(super::retry_after_seconds("invalid"), None);
    }
}
