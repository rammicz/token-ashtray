//! Mistral Vibe quota adapter.
//!
//! Resolution order:
//! 1. Direct HTTP with `MISTRAL_API_KEY` from the process environment or
//!    `$VIBE_HOME/.env` / `~/.vibe/.env` (read-only, no subprocess needed).
//! 2. Otherwise, query the installed first-party `vibe-app-server` over stdio
//!    JSON-RPC (`initialize` → ephemeral `session/start` → `account/read` →
//!    `session/stop`) inside an isolated temporary `VIBE_HOME` with telemetry
//!    disabled. The CLI retains ownership of its OS keyring / OIDC login; this
//!    adapter never reads the OS credential store directly.
use super::*;
use crate::model::{number, timestamp};
use serde_json::json;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

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
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(18);
const GRACEFUL_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RPC_BYTES: usize = 512 * 1024;

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
    if let Some(vibe_home) = env::var("VIBE_HOME").ok().filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(vibe_home).join(".env")];
    }
    vec![home().join(".vibe").join(".env")]
}

fn format_plan_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(match trimmed.to_ascii_uppercase().as_str() {
        "FREE" => "Free".into(),
        "INDIVIDUAL" => "Pro".into(),
        "EDU" => "Student".into(),
        "TEAM" => "Team".into(),
        _ => trimmed.to_owned(),
    })
}

fn app_server_executable() -> &'static str {
    if cfg!(windows) {
        "vibe-app-server.exe"
    } else {
        "vibe-app-server"
    }
}

fn cli_executable() -> &'static str {
    if cfg!(windows) { "vibe.exe" } else { "vibe" }
}

fn discover_app_server() -> Option<PathBuf> {
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_owned())
        .unwrap_or_default();
    let path_env = env::var_os("PATH");
    discover_app_server_from(
        &home,
        path_env.as_deref(),
        app_server_executable(),
        cli_executable(),
    )
}

fn discover_app_server_from(
    home: &Path,
    path_env: Option<&std::ffi::OsStr>,
    server_exe: &str,
    cli_exe: &str,
) -> Option<PathBuf> {
    let mut directories = Vec::new();
    if !home.as_os_str().is_empty() {
        directories.push(home.join(".local/bin"));
    }
    if let Some(paths) = path_env {
        directories.extend(
            env::split_paths(paths)
                .filter(|directory| !directory.as_os_str().is_empty())
                .filter(|directory| directory.is_absolute()),
        );
    }

    // 1. Direct `vibe-app-server` binary in ~/.local/bin or PATH.
    for directory in &directories {
        let candidate = directory.join(server_exe);
        if let Some(resolved) = resolve_executable(&candidate) {
            return Some(resolved);
        }
    }

    // 2. Sibling of a resolved `vibe` launcher (e.g. symlink into a uv/venv bin dir).
    for directory in &directories {
        let cli_candidate = directory.join(cli_exe);
        if let Some(resolved_cli) = resolve_executable(&cli_candidate)
            && let Some(parent) = resolved_cli.parent()
            && let Some(sibling) = resolve_executable(&parent.join(server_exe))
        {
            return Some(sibling);
        }
    }

    None
}

fn resolve_executable(path: &Path) -> Option<PathBuf> {
    path.is_absolute()
        .then(|| path.canonicalize().ok())
        .flatten()
        .filter(|path| path.is_file())
}

enum RpcError {
    Io(&'static str),
    TooLarge,
    Protocol(&'static str),
}

async fn write_json_line(
    stdin: &mut tokio::process::ChildStdin,
    message: &Value,
) -> std::result::Result<(), RpcError> {
    let mut encoded = serde_json::to_vec(message)
        .map_err(|_| RpcError::Protocol("Could not serialize Mistral Vibe RPC request"))?;
    encoded.push(b'\n');
    stdin
        .write_all(&encoded)
        .await
        .map_err(|_| RpcError::Io("Could not write to Mistral Vibe app server"))?;
    stdin
        .flush()
        .await
        .map_err(|_| RpcError::Io("Could not flush Mistral Vibe app server request"))
}

async fn read_response_for_id(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    expected_id: u64,
    total_bytes: &mut usize,
) -> std::result::Result<Value, RpcError> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = read_bounded_line(reader, &mut line, total_bytes).await?;
        if read == 0 {
            return Err(RpcError::Io(
                "Mistral Vibe app server closed before answering",
            ));
        }
        let message: Value = serde_json::from_slice(&line)
            .map_err(|_| RpcError::Protocol("Mistral Vibe app server returned invalid JSON"))?;
        // Ignore asynchronous JSON-RPC notifications (no `id`), server-to-client
        // requests (carry `method`), and responses for other IDs.
        if message.get("method").is_none()
            && message.get("id").and_then(Value::as_u64) == Some(expected_id)
        {
            return Ok(message);
        }
    }
}

async fn read_bounded_line(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    out: &mut Vec<u8>,
    total_bytes: &mut usize,
) -> std::result::Result<usize, RpcError> {
    let mut read_here = 0;
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| RpcError::Io("Could not read Mistral Vibe app server output"))?;
        if available.is_empty() {
            return Ok(read_here);
        }
        let (take, found_newline) = match available.iter().position(|&b| b == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if total_bytes.saturating_add(take) > MAX_RPC_BYTES {
            return Err(RpcError::TooLarge);
        }
        out.extend_from_slice(&available[..take]);
        *total_bytes += take;
        read_here += take;
        reader.consume(take);
        if found_newline {
            return Ok(read_here);
        }
    }
}

async fn stop(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn query_app_server(binary: &Path) -> Result<Value> {
    query_app_server_with_timeout(binary, APP_SERVER_TIMEOUT).await
}

fn build_app_server_command(binary: &Path, isolated_home: &Path) -> Command {
    let mut process = Command::new(binary);
    process
        .current_dir(isolated_home)
        .env("VIBE_HOME", isolated_home)
        .env_remove(ENV_KEY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    process
}

async fn query_app_server_with_timeout(binary: &Path, timeout: Duration) -> Result<Value> {
    let isolated = tempfile::Builder::new()
        .prefix("token-ashtray-vibe-")
        .tempdir()
        .map_err(|_| ProviderError::error("Could not isolate Mistral Vibe configuration"))?;
    // Disable telemetry and keep ~/.vibe untouched (no user plugins, hooks, or MCP servers).
    // OS keyring credentials (ai.mistral.vibe / ai.mistral.vibe.oidc) remain owned by Vibe.
    fs::write(
        isolated.path().join("config.toml"),
        "enable_telemetry = false\n",
    )
    .map_err(|_| ProviderError::error("Could not prepare isolated Mistral Vibe config"))?;

    let mut process = build_app_server_command(binary, isolated.path());

    let mut child = process
        .spawn()
        .map_err(|_| D.reconnect("Could not start Mistral Vibe app server"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::error("Could not open Mistral Vibe app server input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::error("Could not capture Mistral Vibe app server output"))?;

    let mut reader = BufReader::new(stdout);
    let mut total_bytes = 0usize;
    let cwd_str = isolated.path().to_string_lossy().into_owned();
    let result = tokio::time::timeout(
        timeout,
        run_account_read_rpc(&mut stdin, &mut reader, &mut total_bytes, &cwd_str),
    )
    .await;

    let outcome = match result {
        Ok(Ok((account, session_id))) => {
            // Best-effort clean shutdown; keep stdout open until session/stop replies or times out.
            let _ = tokio::time::timeout(
                GRACEFUL_EXIT_TIMEOUT,
                stop_session(&mut stdin, &mut reader, &mut total_bytes, &session_id),
            )
            .await;
            drop(stdin);
            drop(reader);
            if tokio::time::timeout(GRACEFUL_EXIT_TIMEOUT, child.wait())
                .await
                .is_err()
            {
                stop(&mut child).await;
            }
            Ok(json!({ "account": account }))
        }
        other => {
            drop(stdin);
            drop(reader);
            stop(&mut child).await;
            let message = match other {
                Ok(Err(RpcError::TooLarge)) => {
                    "Mistral Vibe app server returned more output than the safe limit"
                }
                Ok(Err(RpcError::Io(message) | RpcError::Protocol(message))) => message,
                Err(_) => "Mistral Vibe account check timed out",
                Ok(Ok(_)) => unreachable!(),
            };
            Err(ProviderError::error(message))
        }
    };
    drop(isolated);
    outcome
}

async fn run_account_read_rpc(
    stdin: &mut tokio::process::ChildStdin,
    reader: &mut BufReader<tokio::process::ChildStdout>,
    total_bytes: &mut usize,
    cwd: &str,
) -> std::result::Result<(Value, String), RpcError> {
    write_json_line(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "token-ashtray",
                    "version": env!("CARGO_PKG_VERSION"),
                    "entrypoint": "programmatic",
                }
            }
        }),
    )
    .await?;
    let init = read_response_for_id(reader, 1, total_bytes).await?;
    if init.get("error").is_some() || init.get("result").is_none() {
        return Err(RpcError::Protocol(
            "Mistral Vibe app server rejected initialize handshake",
        ));
    }

    write_json_line(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )
    .await?;

    // Ephemeral session: no new-session telemetry, no persisted session log, zero model turns.
    write_json_line(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/start",
            "params": {
                "kind": "ephemeral",
                "agentConfig": { "cwd": cwd }
            }
        }),
    )
    .await?;
    let started = read_response_for_id(reader, 2, total_bytes).await?;
    if started.get("error").is_some() {
        return Err(RpcError::Protocol(
            "Mistral Vibe app server could not open an ephemeral session",
        ));
    }
    let session_id = started["result"]["state"]["session"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or(RpcError::Protocol(
            "Mistral Vibe app server did not return a session ID",
        ))?
        .to_owned();

    write_json_line(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "account/read",
            "params": { "sessionId": session_id }
        }),
    )
    .await?;
    let account_reply = read_response_for_id(reader, 3, total_bytes).await?;
    if account_reply.get("error").is_some() {
        return Err(RpcError::Protocol(
            "Mistral Vibe app server rejected account/read",
        ));
    }
    let account = account_reply
        .get("result")
        .and_then(|result| result.get("account"))
        .filter(|account| account.is_object())
        .cloned()
        .ok_or(RpcError::Protocol(
            "Mistral Vibe app server returned an unexpected account payload",
        ))?;

    Ok((account, session_id))
}

async fn stop_session(
    stdin: &mut tokio::process::ChildStdin,
    reader: &mut BufReader<tokio::process::ChildStdout>,
    total_bytes: &mut usize,
    session_id: &str,
) -> std::result::Result<(), RpcError> {
    write_json_line(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/stop",
            "params": { "sessionId": session_id }
        }),
    )
    .await?;
    let _ = read_response_for_id(reader, 4, total_bytes).await?;
    Ok(())
}

fn finish_usage(
    mut plan: Option<String>,
    payg: bool,
    used_percent: Option<f64>,
    resets_at: Option<OffsetDateTime>,
) -> Result<Usage> {
    if payg {
        plan = Some(format!(
            "{} · pay-as-you-go",
            plan.unwrap_or_else(|| "Vibe".into())
        ));
    }
    let mut meters = Vec::new();
    // usage_percentage / usagePercentage is a 0–100 percentage and may exceed
    // 100 under pay-as-you-go; the meter clamps at 100.
    if let Some(used) = used_percent.filter(|used| *used >= 0.0) {
        meters.push(Meter {
            label: "Vibe Code allowance".into(),
            used_percent: (used as f32).clamp(0.0, 100.0),
            resets_at,
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

fn parse_account_view(account: &Value) -> Result<Usage> {
    match account.get("status").and_then(Value::as_str) {
        Some("ready") => {}
        Some("missing_key") => {
            return Err(D.reconnect("Mistral Vibe API key not found"));
        }
        Some("unauthorized") => {
            return Err(D.reconnect("Mistral Vibe login was rejected"));
        }
        Some("unavailable") => {
            return Err(ProviderError::error(
                "Mistral Vibe account information is currently unavailable",
            ));
        }
        _ => return Err(D.invalid()),
    }

    let plan_obj = &account["plan"];
    let plan = string_at(plan_obj, &["title"])
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .or_else(|| string_at(plan_obj, &["name"]).and_then(|name| format_plan_name(&name)));

    let budget = &account["budget"];
    let payg = budget.get("paygEnabled").and_then(Value::as_bool) == Some(true);
    let used = budget.get("usagePercentage").and_then(number);
    let resets_at = budget.get("resetAt").and_then(timestamp);
    finish_usage(plan, payg, used, resets_at)
}

fn parse_http_payload(payload: &Value) -> Result<Usage> {
    let subscription = &payload["subscription"];
    let plan = string_at(subscription, &["plan_name"]).and_then(|name| format_plan_name(&name));
    let budget = &payload["budget"]["vibe_budget"];
    let payg = [&budget["payg_enabled"], &subscription["vibe_payg_enabled"]]
        .into_iter()
        .find_map(Value::as_bool)
        == Some(true);
    let has_positive_initial = budget
        .get("initial_budget")
        .and_then(number)
        .is_none_or(|initial| initial > 0.0);
    let used = has_positive_initial
        .then(|| budget.get("usage_percentage").and_then(number))
        .flatten();
    let resets_at = budget.get("reset_at").and_then(timestamp);
    finish_usage(plan, payg, used, resets_at)
}

#[async_trait]
impl Provider for Vibe {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        resolve_from(|key| env::var(key).ok(), dotenv_files)
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
        if let Some(account) = payload.get("account") {
            return parse_account_view(account);
        }
        parse_http_payload(payload)
    }
    async fn poll(&self, client: &Client) -> Result<ProviderCard> {
        let payload = match self.discover() {
            Ok(mut credentials) => self.fetch(client, &mut credentials).await?,
            Err(error) if error.status != Status::Reconnect => return Err(error),
            Err(reconnect) => {
                let binary = discover_app_server().ok_or(reconnect)?;
                query_app_server(&binary).await?
            }
        };
        let usage = self.parse(&payload)?;
        let mut card = D.empty();
        card.plan = usage.plan;
        card.price = usage.price;
        card.meters = usage.meters;
        card.usage_breakdown = usage.usage_breakdown;
        card.status = Status::Live;
        card.detail.clear();
        card.fetched_at = Some(OffsetDateTime::now_utc());
        Ok(card)
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
        assert_eq!(usage.plan.as_deref(), Some("Team · pay-as-you-go"));
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Vibe Code allowance");
        assert_eq!(usage.meters[0].used_percent, 40.0);
        assert!(usage.meters[0].resets_at.is_some());
    }

    #[test]
    fn parse_clamps_payg_overspend_and_survives_missing_or_zero_budget() {
        let over = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "TEAM"},
                "budget": {"vibe_budget": {"usage_percentage": 140.0, "reset_at": null}}
            }))
            .unwrap();
        assert_eq!(over.meters[0].used_percent, 100.0);
        assert_eq!(over.plan.as_deref(), Some("Team"));

        // initial_budget <= 0 means no Vibe Code allowance meter (matches CLI).
        let zero_budget = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "FREE"},
                "budget": {"vibe_budget": {"usage_percentage": 0.0, "initial_budget": 0.0}}
            }))
            .unwrap();
        assert!(zero_budget.meters.is_empty());
        assert_eq!(zero_budget.plan.as_deref(), Some("Free"));

        // No budget served (403/404 read as null): plan alone, no meters.
        let plan_only = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "INDIVIDUAL"},
                "budget": null
            }))
            .unwrap();
        assert!(plan_only.meters.is_empty());
        assert_eq!(plan_only.plan.as_deref(), Some("Pro"));

        // Neither endpoint produced anything recognisable.
        assert!(
            Vibe.parse(&json!({"subscription": {}, "budget": null}))
                .is_err()
        );
    }

    #[test]
    fn parse_app_server_account_view_and_statuses() {
        let fixture: Value =
            serde_json::from_str(&fs::read_to_string("tests/fixtures/vibe-account.json").unwrap())
                .unwrap();
        let from_fixture = Vibe.parse(&fixture).unwrap();
        assert_eq!(from_fixture.plan.as_deref(), Some("Pro"));
        assert_eq!(from_fixture.meters.len(), 1);
        assert_eq!(from_fixture.meters[0].label, "Vibe Code allowance");
        assert_eq!(from_fixture.meters[0].used_percent, 18.5);
        assert!(from_fixture.meters[0].resets_at.is_some());

        let ready = Vibe
            .parse(&json!({
                "account": {
                    "status": "ready",
                    "plan": {"kind": "chat", "name": "INDIVIDUAL", "title": "Pro"},
                    "budget": {
                        "usagePercentage": 12.5,
                        "resetAt": "2026-12-01T00:00:00Z",
                        "paygEnabled": false
                    }
                }
            }))
            .unwrap();
        assert_eq!(ready.plan.as_deref(), Some("Pro"));
        assert_eq!(ready.meters.len(), 1);
        assert_eq!(ready.meters[0].label, "Vibe Code allowance");
        assert_eq!(ready.meters[0].used_percent, 12.5);
        assert!(ready.meters[0].resets_at.is_some());

        let payg_over = Vibe
            .parse(&json!({
                "account": {
                    "status": "ready",
                    "plan": {"kind": "chat", "name": "TEAM", "title": null},
                    "budget": {
                        "usagePercentage": 135.0,
                        "resetAt": null,
                        "paygEnabled": true
                    }
                }
            }))
            .unwrap();
        assert_eq!(payg_over.plan.as_deref(), Some("Team · pay-as-you-go"));
        assert_eq!(payg_over.meters[0].used_percent, 100.0);
        assert!(payg_over.meters[0].resets_at.is_none());

        let missing = Vibe
            .parse(&json!({"account": {"status": "missing_key"}}))
            .err()
            .unwrap();
        assert_eq!(missing.status, Status::Reconnect);

        let unauthorized = Vibe
            .parse(&json!({"account": {"status": "unauthorized"}}))
            .err()
            .unwrap();
        assert_eq!(unauthorized.status, Status::Reconnect);

        let unavailable = Vibe
            .parse(&json!({"account": {"status": "unavailable"}}))
            .err()
            .unwrap();
        assert_eq!(unavailable.status, Status::Error);
    }

    #[test]
    fn discovers_app_server_directly_or_next_to_vibe_launcher() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();

        let direct = bin.join("vibe-app-server");
        fs::write(&direct, "#!/bin/sh\n").unwrap();
        assert_eq!(
            discover_app_server_from(&home, None, "vibe-app-server", "vibe"),
            Some(direct.canonicalize().unwrap())
        );

        #[cfg(unix)]
        {
            fs::remove_file(&direct).unwrap();
            let tool_bin = root.path().join("uv-tool/bin");
            fs::create_dir_all(&tool_bin).unwrap();
            let real_cli = tool_bin.join("vibe");
            let real_server = tool_bin.join("vibe-app-server");
            fs::write(&real_cli, "#!/bin/sh\n").unwrap();
            fs::write(&real_server, "#!/bin/sh\n").unwrap();
            std::os::unix::fs::symlink(&real_cli, bin.join("vibe")).unwrap();
            assert_eq!(
                discover_app_server_from(&home, None, "vibe-app-server", "vibe"),
                Some(real_server.canonicalize().unwrap())
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn app_server_rpc_reads_account_with_isolated_vibe_home_and_ignores_notifications() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let observed_home = directory.path().join("observed_home");
        let script = directory.path().join("fake-vibe-app-server");
        fs::write(
            &script,
            format!(
                r#"#!/usr/bin/env python3
import json, os, sys
vibe_home = os.environ.get("VIBE_HOME", "")
with open("{observed}", "w") as f:
    f.write(vibe_home + "\n")
    f.write(open(os.path.join(vibe_home, "config.toml")).read())
for line in sys.stdin:
    msg = json.loads(line)
    m = msg.get("method")
    if m == "initialize":
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"serverInfo":{{"name":"vibe-app-server","version":"3.1.5"}}}}}}), flush=True)
    elif m == "initialized":
        print(json.dumps({{"jsonrpc":"2.0","method":"server/notice","params":{{}}}}), flush=True)
    elif m == "session/start":
        assert msg["params"]["kind"] == "ephemeral"
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"state":{{"session":{{"id":"sess-1"}}}}}}}}), flush=True)
    elif m == "account/read":
        assert msg["params"]["sessionId"] == "sess-1"
        print(json.dumps({{"jsonrpc":"2.0","method":"runtime/updated","params":{{}}}}), flush=True)
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"account":{{"status":"ready","plan":{{"kind":"chat","name":"INDIVIDUAL","title":"Pro"}},"budget":{{"usagePercentage":25.0,"resetAt":"2026-12-01T00:00:00Z","paygEnabled":False}}}}}}}}), flush=True)
    elif m == "session/stop":
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{}}}}), flush=True)
        break
"#,
                observed = observed_home.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let payload = query_app_server_with_timeout(&script, Duration::from_secs(5))
            .await
            .unwrap();
        let usage = Vibe.parse(&payload).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].used_percent, 25.0);

        let observed = fs::read_to_string(&observed_home).unwrap();
        let mut lines = observed.lines();
        let temp_home = PathBuf::from(lines.next().unwrap());
        assert!(!temp_home.exists(), "temporary VIBE_HOME must be removed");
        assert_eq!(lines.next(), Some("enable_telemetry = false"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn app_server_uses_isolated_vibe_home_without_a_console_window() {
        use std::os::windows::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        let observed = directory.path().join("observed.txt");
        let source = directory.path().join("vibe_console_probe.rs");
        let binary = directory.path().join("vibe_console_probe.exe");
        fs::write(
            &source,
            format!(
                r##"
                    use std::io::{{self, BufRead, Write}};
                    #[link(name = "kernel32")]
                    unsafe extern "system" {{
                        fn GetConsoleWindow() -> *mut core::ffi::c_void;
                    }}
                    fn main() {{
                        let vibe_home = std::env::var("VIBE_HOME").unwrap();
                        let cwd = std::env::current_dir().unwrap().display().to_string();
                        let cfg = std::fs::read_to_string(
                            std::path::Path::new(&vibe_home).join("config.toml")
                        ).unwrap();
                        let console = if unsafe {{ GetConsoleWindow().is_null() }} {{
                            "none"
                        }} else {{
                            "present"
                        }};
                        std::fs::write(
                            r#"{observed}"#,
                            format!("{{console}}\n{{vibe_home}}\n{{cwd}}\n{{cfg}}")
                        ).unwrap();
                        let stdin = io::stdin();
                        let mut stdout = io::stdout();
                        for line in stdin.lock().lines().map_while(Result::ok) {{
                            if line.contains("\"initialize\"") {{
                                writeln!(stdout, "{{}}", r#"{{"jsonrpc":"2.0","id":1,"result":{{"serverInfo":{{"name":"vibe-app-server","version":"3.1.5"}}}}}}"#).unwrap();
                                stdout.flush().unwrap();
                            }} else if line.contains("\"session/start\"") {{
                                writeln!(stdout, "{{}}", r#"{{"jsonrpc":"2.0","id":2,"result":{{"state":{{"session":{{"id":"sess-1"}}}}}}}}"#).unwrap();
                                stdout.flush().unwrap();
                            }} else if line.contains("\"account/read\"") {{
                                writeln!(stdout, "{{}}", r#"{{"jsonrpc":"2.0","id":3,"result":{{"account":{{"status":"ready","plan":{{"kind":"chat","name":"INDIVIDUAL","title":"Pro"}},"budget":{{"usagePercentage":10.0,"resetAt":null,"paygEnabled":false}}}}}}}}"#).unwrap();
                                stdout.flush().unwrap();
                            }} else if line.contains("\"session/stop\"") {{
                                writeln!(stdout, "{{}}", r#"{{"jsonrpc":"2.0","id":4,"result":{{}}}}"#).unwrap();
                                stdout.flush().unwrap();
                                break;
                            }}
                        }}
                    }}
                "##,
                observed = observed.display()
            ),
        )
        .unwrap();
        let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let build = std::process::Command::new(rustc)
            .creation_flags(0x0800_0000)
            .args(["--edition=2024", "--crate-name=vibe_console_probe"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );

        let payload = query_app_server_with_timeout(&binary, Duration::from_secs(5))
            .await
            .unwrap();
        let usage = Vibe.parse(&payload).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert_eq!(usage.meters[0].used_percent, 10.0);

        let text = fs::read_to_string(&observed).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], "none", "console window must not be created");
        let isolated_home = Path::new(lines[1]);
        assert_eq!(isolated_home, Path::new(lines[2]));
        assert_eq!(lines[3], "enable_telemetry = false");
        assert!(
            !isolated_home.exists(),
            "temporary VIBE_HOME was not removed"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn app_server_timeout_and_oversized_output_fail_cleanly() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let hung = directory.path().join("hung-server");
        fs::write(&hung, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&hung, fs::Permissions::from_mode(0o700)).unwrap();
        let err = query_app_server_with_timeout(&hung, Duration::from_millis(150))
            .await
            .unwrap_err();
        assert!(err.message.contains("timed out"), "{}", err.message);

        let noisy = directory.path().join("noisy-server");
        fs::write(
            &noisy,
            "#!/bin/sh\nhead -c 524289 /dev/zero | tr '\\0' 'a'\nprintf '\\n'\nexec sleep 30\n",
        )
        .unwrap();
        fs::set_permissions(&noisy, fs::Permissions::from_mode(0o700)).unwrap();
        let err = query_app_server_with_timeout(&noisy, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(err.message.contains("safe limit"), "{}", err.message);
    }

    #[test]
    fn parse_treats_whitespace_plan_fields_as_absent() {
        let account = Vibe
            .parse(&json!({
                "account": {
                    "status": "ready",
                    "plan": {"kind": "chat", "name": "INDIVIDUAL", "title": "   "},
                    "budget": null
                }
            }))
            .unwrap();
        assert_eq!(account.plan.as_deref(), Some("Pro"));

        assert!(
            Vibe.parse(&json!({
                "subscription": {"plan_name": "   "},
                "budget": null
            }))
            .is_err()
        );

        let payg = Vibe
            .parse(&json!({
                "subscription": {"plan_name": "   ", "vibe_payg_enabled": true},
                "budget": null
            }))
            .unwrap();
        assert_eq!(payg.plan.as_deref(), Some("Vibe · pay-as-you-go"));
    }

    #[test]
    fn app_server_command_isolates_vibe_home_and_clears_mistral_api_key() {
        let isolated = Path::new("/tmp/token-ashtray-vibe-test");
        let command = build_app_server_command(Path::new("/usr/bin/vibe-app-server"), isolated);
        let envs: Vec<_> = command.as_std().get_envs().collect();
        assert!(
            envs.contains(&(
                std::ffi::OsStr::new("VIBE_HOME"),
                Some(isolated.as_os_str())
            )),
            "expected VIBE_HOME override, got {envs:?}"
        );
        assert!(
            envs.contains(&(std::ffi::OsStr::new(ENV_KEY), None)),
            "expected {ENV_KEY} to be removed from app-server env, got {envs:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn app_server_rpc_sends_programmatic_entrypoint_ignores_server_requests_and_awaits_stop()
    {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let observed = directory.path().join("observed.txt");
        let script = directory.path().join("fake-vibe-protocol");
        fs::write(
            &script,
            format!(
                r#"#!/usr/bin/env python3
import json, sys, time
notes = []
for line in sys.stdin:
    msg = json.loads(line)
    m = msg.get("method")
    if m == "initialize":
        notes.append("entrypoint=" + str(msg.get("params", {{}}).get("clientInfo", {{}}).get("entrypoint")))
        # Server-to-client request reusing id 1 must not be mistaken for our initialize response.
        print(json.dumps({{"jsonrpc":"2.0","id":1,"method":"client/ping","params":{{}}}}), flush=True)
        print(json.dumps({{"jsonrpc":"2.0","id":1,"result":{{"serverInfo":{{"name":"vibe-app-server","version":"3.1.5"}}}}}}), flush=True)
    elif m == "session/start":
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"state":{{"session":{{"id":"sess-1"}}}}}}}}), flush=True)
    elif m == "account/read":
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"account":{{"status":"ready","plan":{{"kind":"chat","name":"INDIVIDUAL","title":"Pro"}},"budget":None}}}}}}), flush=True)
    elif m == "session/stop":
        time.sleep(0.05)
        try:
            print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{}}}}), flush=True)
            notes.append("stop=ok")
        except BrokenPipeError:
            notes.append("stop=broken_pipe")
        break
with open("{observed}", "w") as f:
    f.write("\n".join(notes) + "\n")
"#,
                observed = observed.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let payload = query_app_server_with_timeout(&script, Duration::from_secs(5))
            .await
            .unwrap();
        let usage = Vibe.parse(&payload).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro"));

        let text = fs::read_to_string(&observed).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines, vec!["entrypoint=programmatic", "stop=ok"]);
    }
}
