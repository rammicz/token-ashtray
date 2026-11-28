//! Read quota through Antigravity's documented CLI command. The CLI retains
//! ownership of its existing login; this adapter never reads its credentials.
use super::*;
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use time::format_description::well_known::Rfc3339;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub struct Antigravity;

const D: Descriptor = Descriptor {
    id: "antigravity",
    name: "Antigravity",
    link: "https://antigravity.google/docs/cli/commands/usage",
    login: "Run `agy` and complete its login.",
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CliVersion {
    major: u64,
    minor: u64,
    patch: u64,
    // Ordered after major.minor.patch so prereleases (`false`) sort below the
    // matching stable release (`true`), e.g. `1.1.11-rc.1 < 1.1.11`.
    stable: bool,
}

const MIN_VERSION: CliVersion = CliVersion {
    major: 1,
    minor: 1,
    patch: 11,
    stable: true,
};
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const USAGE_TIMEOUT: Duration = Duration::from_secs(18);
const MAX_OUTPUT_BYTES: u64 = 64 * 1024;

#[async_trait]
impl Provider for Antigravity {
    fn descriptor(&self) -> Descriptor {
        D
    }

    // Antigravity's documented usage command performs its own keyring-backed
    // authentication. `poll` below is intentionally the only supported path.
    fn discover(&self) -> Result<Credentials> {
        Err(ProviderError::error(
            "Antigravity must be polled through its documented CLI usage command",
        ))
    }

    async fn fetch(&self, _client: &Client, _credentials: &mut Credentials) -> Result<Value> {
        Err(ProviderError::error(
            "Antigravity must be polled through its documented CLI usage command",
        ))
    }

    fn parse(&self, payload: &Value) -> Result<Usage> {
        // Verified contract (CLI 1.4.4 and 1.5.9, docs/DATA-SOURCES.md §9):
        // `command.data.groups[].buckets[]` with `remaining_fraction` in 0..=1
        // and an RFC 3339 `reset_time`. Buckets outside that contract are
        // skipped instead of guessed; nothing usable is an error, not 0%.
        if payload["status"].as_str() != Some("SUCCESS")
            || payload["command"]["name"].as_str() != Some("usage")
        {
            return Err(ProviderError::error(
                "Antigravity CLI usage response has an unsupported format",
            ));
        }
        let meters: Vec<Meter> = payload["command"]["data"]["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|group| {
                let group_name = group["name"].as_str().map(str::trim).unwrap_or("");
                group["buckets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |bucket| bucket_meter(group_name, bucket))
            })
            .collect();
        if meters.is_empty() {
            return Err(ProviderError::error(
                "Antigravity CLI usage response contains no supported quota",
            ));
        }
        Ok(Usage {
            meters,
            ..Default::default()
        })
    }

    async fn poll(&self, _client: &Client) -> Result<ProviderCard> {
        let binary =
            discover_binary().ok_or_else(|| D.reconnect("Antigravity CLI executable not found"))?;
        let version = version(&binary).await?;
        if version < MIN_VERSION {
            return Err(
                D.reconnect("Antigravity CLI 1.1.11 or newer is required for safe usage polling")
            );
        }

        let output = run(&binary, UsageCommand::Usage).await?;
        if !output.success {
            return Err(if output.is_auth_error() {
                D.reconnect("Antigravity CLI login is unavailable")
            } else {
                ProviderError::error("Antigravity CLI usage command failed")
            });
        }
        let payload = serde_json::from_slice(&output.bytes).map_err(|_| {
            ProviderError::error("Antigravity CLI usage response is not valid JSON")
        })?;
        if output.is_auth_error() {
            return Err(D.reconnect("Antigravity CLI login is unavailable"));
        }
        let usage = self.parse(&payload)?;
        let mut card = D.empty();
        card.plan = usage.plan;
        card.price = usage.price;
        card.meters = usage.meters;
        card.status = Status::Live;
        card.detail.clear();
        card.fetched_at = Some(time::OffsetDateTime::now_utc());
        Ok(card)
    }
}

fn bucket_meter(group: &str, bucket: &Value) -> Option<Meter> {
    // Only plain JSON numbers: a string or out-of-range value has no verified
    // meaning (unavailable, unlimited and similar states are unobserved).
    let remaining = bucket["remaining_fraction"]
        .as_f64()
        .filter(|fraction| (0.0..=1.0).contains(fraction))?;
    let window = match bucket["window"].as_str() {
        Some("weekly") => "Weekly",
        _ => bucket["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())?,
    };
    let label = if group.is_empty() {
        window.to_owned()
    } else {
        format!("{group} · {window}")
    };
    Some(Meter {
        label,
        used_percent: ((1.0 - remaining) * 100.0) as f32,
        resets_at: bucket["reset_time"]
            .as_str()
            .and_then(|reset| time::OffsetDateTime::parse(reset, &Rfc3339).ok()),
    })
}

fn discover_binary() -> Option<PathBuf> {
    let executable = if cfg!(windows) { "agy.exe" } else { "agy" };
    // Executable discovery is separate from credential directory resolution.
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_owned())
        .unwrap_or_default();
    let mut candidates = vec![home.join(".local/bin").join(executable)];
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        // The current Windows installer uses `agy\bin`; older builds used
        // `Antigravity\bin`. PATH stays the fallback.
        for directory in ["agy/bin", "Antigravity/bin"] {
            candidates.push(local.join(directory).join(executable));
        }
    }
    if let Some(paths) = env::var_os("PATH") {
        candidates.extend(
            env::split_paths(&paths)
                .filter(|directory| !directory.as_os_str().is_empty())
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(executable)),
        );
    }
    candidates.into_iter().find_map(|path| {
        path.is_absolute()
            .then(|| path.canonicalize().ok())
            .flatten()
            .filter(|path| path.is_file())
    })
}

async fn version(binary: &Path) -> Result<CliVersion> {
    let output = run(binary, UsageCommand::Version).await?;
    if !output.success {
        return Err(D.reconnect("Could not determine Antigravity CLI version"));
    }
    parse_version(&output.bytes)
        .ok_or_else(|| D.reconnect("Could not determine Antigravity CLI version"))
}

#[derive(Clone, Copy)]
enum UsageCommand {
    Version,
    Usage,
}

async fn run(binary: &Path, command: UsageCommand) -> Result<CommandOutput> {
    let timeout = match command {
        UsageCommand::Version => VERSION_TIMEOUT,
        UsageCommand::Usage => USAGE_TIMEOUT,
    };
    run_with_timeout(binary, command, timeout).await
}

async fn run_with_timeout(
    binary: &Path,
    command: UsageCommand,
    timeout: Duration,
) -> Result<CommandOutput> {
    let args: &[&str] = match command {
        UsageCommand::Version => &["--version"],
        // This exact command is read-only in CLI 1.1.11+; do not add model,
        // conversation, interactive, or shell arguments here.
        UsageCommand::Usage => &[
            "-p",
            "/usage",
            "--output-format",
            "json",
            "--print-timeout",
            "15s",
        ],
    };
    let mut process = Command::new(binary);
    process
        .args(args)
        .current_dir(env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    let _profile = {
        // CREATE_NO_WINDOW only hides this process: MCP servers started by agy
        // can still create consoles. An empty per-process profile excludes the
        // user's MCP servers, plugins and hooks while agy retains ownership of
        // the existing Windows Credential Manager login. Never copy credentials
        // or fall back to the real profile if isolation cannot be created.
        // https://github.com/google-antigravity/antigravity-cli/issues/1261
        let profile = tempfile::Builder::new()
            .prefix("token-ashtray-agy-")
            .tempdir()
            .map_err(|_| ProviderError::error("Could not isolate Antigravity CLI configuration"))?;
        process
            .env("USERPROFILE", profile.path())
            // A fresh profile must not trigger an updater on every poll.
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .current_dir(profile.path())
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW, including startup errors.
        profile
    };
    let mut child = process
        .spawn()
        .map_err(|_| D.reconnect("Could not start Antigravity CLI"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::error("Could not capture Antigravity CLI output"))?;
    let result = tokio::time::timeout(timeout, collect_output(&mut child, &mut stdout)).await;
    match result {
        Ok(Ok((status, bytes))) => Ok(CommandOutput {
            success: status.success(),
            bytes,
        }),
        Ok(Err(CollectError::TooLarge)) => {
            stop(&mut child).await;
            Err(ProviderError::error(
                "Antigravity CLI returned more output than the safe limit",
            ))
        }
        Ok(Err(CollectError::Io)) => {
            stop(&mut child).await;
            Err(ProviderError::error(
                "Could not read Antigravity CLI output",
            ))
        }
        Err(_) => {
            stop(&mut child).await;
            Err(ProviderError::error(
                "Antigravity CLI usage command timed out",
            ))
        }
    }
}

enum CollectError {
    Io,
    TooLarge,
}

async fn collect_output(
    child: &mut tokio::process::Child,
    stdout: &mut tokio::process::ChildStdout,
) -> std::result::Result<(std::process::ExitStatus, Vec<u8>), CollectError> {
    let mut bytes = Vec::new();
    let mut status = None;
    let mut chunk = [0; 4096];
    loop {
        if let Some(status) = status {
            let count = stdout
                .read(&mut chunk)
                .await
                .map_err(|_| CollectError::Io)?;
            if count == 0 {
                return Ok((status, bytes));
            }
            append_chunk(&mut bytes, &chunk[..count])?;
            continue;
        }
        tokio::select! {
            result = child.wait() => status = Some(result.map_err(|_| CollectError::Io)?),
            result = stdout.read(&mut chunk) => {
                let count = result.map_err(|_| CollectError::Io)?;
                if count == 0 {
                    return child.wait().await.map(|status| (status, bytes)).map_err(|_| CollectError::Io);
                }
                append_chunk(&mut bytes, &chunk[..count])?;
            }
        }
    }
}

fn append_chunk(bytes: &mut Vec<u8>, chunk: &[u8]) -> std::result::Result<(), CollectError> {
    bytes.extend_from_slice(chunk);
    (bytes.len() <= MAX_OUTPUT_BYTES as usize)
        .then_some(())
        .ok_or(CollectError::TooLarge)
}

async fn stop(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[derive(Debug)]
struct CommandOutput {
    success: bool,
    bytes: Vec<u8>,
}

impl CommandOutput {
    fn is_auth_error(&self) -> bool {
        serde_json::from_slice::<Value>(&self.bytes)
            .ok()
            .is_some_and(|value| {
                value.get("status").and_then(Value::as_str) == Some("ERROR")
                    && value.get("error").and_then(Value::as_str)
                        == Some("authentication failed or timed out")
            })
    }
}

fn parse_version(bytes: &[u8]) -> Option<CliVersion> {
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    if line
        .chars()
        .any(|ch| (ch.is_control() && ch != '\t') || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    let line = line.trim_matches([' ', '\t']);
    let head = strip_trailing_parenthetical(line)?;
    let mut tokens = head.split([' ', '\t']).filter(|token| !token.is_empty());
    let version_token = match (tokens.next(), tokens.next(), tokens.next()) {
        (Some(version), None, None) => version,
        (Some(cli), Some(version), None) if is_recognized_cli_name(cli) => version,
        _ => return None,
    };

    let semver = version_token.strip_prefix('v').unwrap_or(version_token);
    let (without_build, build) = match semver.split_once('+') {
        Some((core_and_pre, build)) => (core_and_pre, Some(build)),
        None => (semver, None),
    };
    let (core, pre) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };
    if pre.is_some_and(|tag| !is_valid_semver_identifiers(tag))
        || build.is_some_and(|tag| !is_valid_semver_identifiers(tag))
    {
        return None;
    }

    let mut components = core.split('.');
    let major = parse_semver_component(components.next()?)?;
    let minor = parse_semver_component(components.next()?)?;
    let patch = parse_semver_component(components.next()?)?;
    components.next().is_none().then_some(CliVersion {
        major,
        minor,
        patch,
        stable: pre.is_none(),
    })
}

fn strip_trailing_parenthetical(line: &str) -> Option<&str> {
    let Some(before_close) = line.strip_suffix(')') else {
        return Some(line);
    };
    let (prefix, body) = before_close.split_once('(')?;
    if !prefix.ends_with([' ', '\t']) || body.contains(['(', ')']) || body.trim().is_empty() {
        return None;
    }
    Some(prefix.trim_end_matches([' ', '\t']))
}

fn is_recognized_cli_name(token: &str) -> bool {
    [
        "agy",
        "agy.exe",
        "antigravity",
        "antigravity.exe",
        "antigravity-cli",
        "antigravity-cli.exe",
    ]
    .iter()
    .any(|name| token.eq_ignore_ascii_case(name))
}

fn parse_semver_component(part: &str) -> Option<u64> {
    if part.is_empty()
        || !part.bytes().all(|b| b.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return None;
    }
    part.parse().ok()
}

fn is_valid_semver_identifiers(tag: &str) -> bool {
    tag.split('.')
        .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    static CLI_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(windows)]
    #[tokio::test]
    async fn console_cli_uses_an_isolated_profile_without_a_console_window() {
        use std::os::windows::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("console_probe.rs");
        let binary = directory.path().join("console_probe.exe");
        std::fs::write(
            &source,
            r#"
                    #[link(name = "kernel32")]
                    unsafe extern "system" {
                        fn GetConsoleWindow() -> *mut core::ffi::c_void;
                    }
                    fn main() {
                        println!("{}", if unsafe { GetConsoleWindow().is_null() } {
                            "none"
                        } else {
                            "present"
                        });
                        println!("{}", std::env::var("USERPROFILE").unwrap());
                        println!("{}", std::env::current_dir().unwrap().display());
                        println!("{}", std::path::Path::new(&std::env::var("USERPROFILE").unwrap())
                            .join(".gemini/config/mcp_config.json").exists());
                        println!("{}", std::env::var("AGY_CLI_DISABLE_AUTO_UPDATE").unwrap());
                    }
            "#,
        )
        .unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let build = std::process::Command::new(rustc)
            .creation_flags(0x0800_0000)
            .args(["--edition=2024", "--crate-name=console_probe"])
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
        for command in [UsageCommand::Version, UsageCommand::Usage] {
            let original_profile = std::env::var_os("USERPROFILE").unwrap();
            let result = run_with_timeout(&binary, command, Duration::from_secs(5))
                .await
                .unwrap();
            assert!(result.success);
            let output = std::str::from_utf8(&result.bytes).unwrap();
            let lines = output.lines().collect::<Vec<_>>();
            assert_eq!(lines.len(), 5, "{output}");
            assert_eq!(lines[0], "none");
            let isolated_profile = Path::new(lines[1]);
            assert_ne!(isolated_profile, Path::new(&original_profile));
            assert_eq!(isolated_profile, Path::new(lines[2]));
            assert_eq!(lines[3], "false", "user MCP configuration was inherited");
            assert_eq!(lines[4], "true", "CLI auto-update was not disabled");
            assert!(
                !isolated_profile.exists(),
                "temporary profile was not removed"
            );
            assert_eq!(std::env::var_os("USERPROFILE"), Some(original_profile));
        }
    }

    #[test]
    fn only_exact_semver_versions_enable_usage_command() {
        assert_eq!(parse_version(b"1.1.11\n"), Some(MIN_VERSION));
        assert!(parse_version(b"agy 1.1.27\n").is_some_and(|version| version >= MIN_VERSION));
        assert!(parse_version(b"1.1\n").is_none());
        assert!(parse_version(b"1.1.27 extra\n").is_none());
    }

    #[test]
    fn version_parser_accepts_cli_prefixes_prerelease_build_and_parenthetical_tags() {
        let stable_1_6_0 = Some(CliVersion {
            major: 1,
            minor: 6,
            patch: 0,
            stable: true,
        });
        let prerelease_1_6_0 = Some(CliVersion {
            major: 1,
            minor: 6,
            patch: 0,
            stable: false,
        });

        for (input, expected) in [
            (b"1.6.0".as_slice(), stable_1_6_0),
            (b"1.6.0\n", stable_1_6_0),
            (b"1.6.0\r\n", stable_1_6_0),
            (b"  1.6.0  \n", stable_1_6_0),
            (b"v1.6.0\n", stable_1_6_0),
            (b"agy 1.6.0\n", stable_1_6_0),
            (b"agy\t1.6.0\n", stable_1_6_0),
            (b"AGY v1.6.0\n", stable_1_6_0),
            (b"agy.exe 1.6.0\n", stable_1_6_0),
            (b"AGY.EXE v1.6.0\n", stable_1_6_0),
            (b"antigravity 1.6.0\n", stable_1_6_0),
            (b"Antigravity.Exe 1.6.0\n", stable_1_6_0),
            (b"antigravity-cli 1.6.0\n", stable_1_6_0),
            (b"Antigravity-CLI.exe v1.6.0\n", stable_1_6_0),
            (b"1.6.0-beta.1\n", prerelease_1_6_0),
            (b"1.6.0-alpha-1.0\n", prerelease_1_6_0),
            (b"1.6.0+meta\n", stable_1_6_0),
            (b"1.6.0+build-123\n", stable_1_6_0),
            (b"v1.6.0-rc.1+build.42\n", prerelease_1_6_0),
            (b"1.6.0 (build 123)\n", stable_1_6_0),
            (b"agy v1.6.0 (abc1234)\n", stable_1_6_0),
            (b"agy 1.6.0-rc.1 (abc1234)\n", prerelease_1_6_0),
            (b"antigravity-cli v1.6.0+meta (2026-11-28)\n", stable_1_6_0),
        ] {
            let parsed = parse_version(input);
            assert_eq!(
                parsed,
                expected,
                "unexpected parse result for {:?}",
                String::from_utf8_lossy(input)
            );
            assert!(parsed.unwrap() >= MIN_VERSION);
        }
    }

    #[test]
    fn version_parser_enforces_min_version_including_prerelease_precedence() {
        for below in [
            b"0.0.0\n".as_slice(),
            b"0.9.9\n",
            b"1.1.10\n",
            b"1.1.11-rc.1\n",
            b"1.1.11-rc.1+build.1\n",
            b"agy v1.1.11-beta.2 (abc1234)\n",
        ] {
            let parsed = parse_version(below).unwrap_or_else(|| {
                panic!("expected {:?} to parse", String::from_utf8_lossy(below))
            });
            assert!(
                parsed < MIN_VERSION,
                "expected {:?} ({parsed:?}) < MIN_VERSION ({MIN_VERSION:?})",
                String::from_utf8_lossy(below)
            );
        }

        for at_or_above in [
            b"1.1.11\n".as_slice(),
            b"v1.1.11\n",
            b"1.1.11+meta\n",
            b"1.1.11+build-1\n",
            b"agy 1.1.11+meta (build 1)\n",
            b"1.1.12-rc.1\n",
            b"1.6.0-beta.1\n",
        ] {
            let parsed = parse_version(at_or_above).unwrap_or_else(|| {
                panic!(
                    "expected {:?} to parse",
                    String::from_utf8_lossy(at_or_above)
                )
            });
            assert!(
                parsed >= MIN_VERSION,
                "expected {:?} ({parsed:?}) >= MIN_VERSION ({MIN_VERSION:?})",
                String::from_utf8_lossy(at_or_above)
            );
        }
    }

    #[test]
    fn version_parser_rejects_malformed_multiline_and_unrelated_output() {
        for rejected in [
            b"".as_slice(),
            b"   \n",
            b"\xff1.6.0\n",
            b"agy\n",
            b"agy v\n",
            b"agy (build 123)\n",
            b"(build 123)\n",
            b"1.1\n",
            b"1.6.0.1\n",
            b"1.+1.11\n",
            b"+1.1.11\n",
            b"01.1.11\n",
            b"1.01.11\n",
            b"1.1.011\n",
            b"vv1.6.0\n",
            b"V1.6.0\n",
            b"1.6.0-\n",
            b"1.6.0+\n",
            b"1.6.0-+meta\n",
            b"1.6.0-beta..1\n",
            b"1.6.0-.beta\n",
            b"1.6.0-beta.\n",
            b"1.6.0+meta..1\n",
            b"1.6.0+a!b\n",
            b"1.6.0+a+b\n",
            b"1.6.0-()\n",
            b"1.6.0 ()\n",
            b"1.6.0 (   )\n",
            "1.6.0 (\u{00A0})\n".as_bytes(),
            b"1.6.0 (a (b))\n",
            b"1.6.0 (a (b)\n",
            b"1.6.0 (a) (b)\n",
            b"1.6.0 (a))\n",
            b"1.6.0(build 123)\n",
            b"1.6.0 (unclosed\n",
            b"1.6.0 (a) extra\n",
            b"1.1.27 extra\n",
            b"agy 1.1.27 extra\n",
            b"agy cli 1.6.0\n",
            b"Error: requires node 18.0.0\n",
            b"other-tool 2.0.0\n",
            b"agy 1.6.0\nextra line\n",
            b"\n1.6.0\n",
            b"1.6.0\n\n",
            b"1.6.0\r1.6.0\n",
            b"1.6.0\x1b[0m\n",
            "1.6.0 (a\u{2028}b)\n".as_bytes(),
        ] {
            assert!(
                parse_version(rejected).is_none(),
                "expected {:?} to be rejected",
                String::from_utf8_lossy(rejected)
            );
        }
    }

    #[test]
    fn only_error_status_is_treated_as_login_failure() {
        let error = CommandOutput {
            success: false,
            bytes: br#"{"status":"ERROR","error":"authentication failed or timed out"}"#.to_vec(),
        };
        assert!(error.is_auth_error());
        let generic_error = CommandOutput {
            success: false,
            bytes: br#"{"status":"ERROR","error":"permission denied"}"#.to_vec(),
        };
        assert!(!generic_error.is_auth_error());
        let malformed = CommandOutput {
            success: false,
            bytes: b"not json".to_vec(),
        };
        assert!(!malformed.is_auth_error());
    }

    #[test]
    fn unknown_success_shape_never_fabricates_a_meter() {
        assert!(
            Antigravity
                .parse(&serde_json::json!({"groups": []}))
                .is_err()
        );
    }

    #[test]
    fn captured_usage_response_becomes_one_meter_per_group() {
        let fixture: Value = serde_json::from_str(
            &std::fs::read_to_string("tests/fixtures/antigravity.json").unwrap(),
        )
        .unwrap();
        let usage = Antigravity.parse(&fixture).unwrap();
        let labels = usage
            .meters
            .iter()
            .map(|meter| meter.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["Gemini Models · Weekly", "Claude and GPT models · Weekly"]
        );
        let reset = time::OffsetDateTime::parse("2000-01-01T00:00:00Z", &Rfc3339).unwrap();
        for meter in &usage.meters {
            assert_eq!(meter.used_percent, 0.0);
            assert_eq!(meter.resets_at, Some(reset));
        }
        assert_eq!(usage.plan, None, "the response has no plan or tier field");
    }

    /// Synthetic boundary input in the captured envelope; not a capture.
    fn synthetic(buckets: Value) -> Value {
        serde_json::json!({
            "status": "SUCCESS",
            "command": {"name": "usage", "data": {"groups": [
                {"name": "Gemini Models", "buckets": buckets}
            ]}}
        })
    }

    #[test]
    fn remaining_fraction_is_reported_as_used_percent() {
        let usage = Antigravity
            .parse(&synthetic(serde_json::json!([
                {"window": "weekly", "remaining_fraction": 0.25},
                {"window": "weekly", "remaining_fraction": 0}
            ])))
            .unwrap();
        let used = usage
            .meters
            .iter()
            .map(|meter| meter.used_percent)
            .collect::<Vec<_>>();
        assert_eq!(used, [75.0, 100.0]);
        assert_eq!(usage.meters[0].resets_at, None);
    }

    #[test]
    fn unverified_bucket_states_are_skipped_not_guessed() {
        for bucket in [
            serde_json::json!({"window": "weekly"}),
            serde_json::json!({"window": "weekly", "remaining_fraction": null}),
            serde_json::json!({"window": "weekly", "remaining_fraction": "0.5"}),
            serde_json::json!({"window": "weekly", "remaining_fraction": -0.1}),
            serde_json::json!({"window": "weekly", "remaining_fraction": 1.5}),
            serde_json::json!({"remaining_fraction": 0.5}),
        ] {
            assert!(
                Antigravity
                    .parse(&synthetic(serde_json::json!([bucket])))
                    .is_err(),
                "{bucket}"
            );
        }
        let other_window = Antigravity
            .parse(&synthetic(serde_json::json!([
                {"window": "daily", "name": "Daily Limit Remaining", "remaining_fraction": 0.5}
            ])))
            .unwrap();
        assert_eq!(
            other_window.meters[0].label,
            "Gemini Models · Daily Limit Remaining"
        );
    }

    #[test]
    fn non_success_or_other_command_is_rejected() {
        let mut payload = synthetic(serde_json::json!([
            {"window": "weekly", "remaining_fraction": 0.5}
        ]));
        payload["status"] = "ERROR".into();
        assert!(Antigravity.parse(&payload).is_err());
        payload["status"] = "SUCCESS".into();
        payload["command"]["name"] = "stats".into();
        assert!(Antigravity.parse(&payload).is_err());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timed_out_usage_command_is_reaped() {
        let _lock = CLI_TEST_LOCK.lock().await;
        // A short timeout can fire before a loaded shell publishes its pid,
        // so lengthen it until a run in which the command started.
        for timeout in [50, 200, 1000, 5000].map(Duration::from_millis) {
            let directory = tempfile::tempdir().unwrap();
            let (binary, pid) = fixture(directory.path(), "exec sleep 30");
            let error = run_with_timeout(&binary, UsageCommand::Usage, timeout)
                .await
                .unwrap_err();
            assert!(error.message.contains("timed out"));
            if pid.is_file() {
                assert_reaped(&pid).await;
                return;
            }
        }
        panic!("fixture command never started");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn oversized_usage_output_is_killed_without_waiting_for_timeout() {
        let _lock = CLI_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let (binary, pid) = fixture(directory.path(), "head -c 65537 /dev/zero; exec sleep 30");

        let error = run_with_timeout(&binary, UsageCommand::Usage, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.message.contains("safe limit"), "{}", error.message);
        assert_reaped(&pid).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelling_usage_poll_kills_the_child() {
        use std::future::Future;

        let _lock = CLI_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let (binary, pid) = fixture(directory.path(), "exec sleep 30");
        // The command must outlast the wait for its pid; the task is aborted.
        let task = tokio::spawn(async move {
            run_with_timeout(&binary, UsageCommand::Usage, SETTLE * 3).await
        });
        settle(|| pid.is_file(), "fixture command did not start").await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_killed(&pid).await;
        // Tokio kills a dropped child, then reaps the orphan when its process
        // driver runs. A second child held on stdin makes its wait observably
        // pending before release, so awaiting its exit drives that process
        // driver after the first child has exited.
        let reaper_tick = directory.path().join("reaper-tick");
        std::fs::write(&reaper_tick, "#!/bin/sh\nread ignored\nexit 0\n").unwrap();
        std::fs::set_permissions(
            &reaper_tick,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let mut tick = Command::new(&reaper_tick)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let tick_stdin = tick.stdin.take().unwrap();
        let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            let mut wait = Box::pin(tick.wait());
            let mut pending_tx = Some(pending_tx);
            std::future::poll_fn(|cx| {
                let status = wait.as_mut().poll(cx);
                if status.is_pending()
                    && let Some(tx) = pending_tx.take()
                {
                    let _ = tx.send(());
                }
                status
            })
            .await
        });
        pending_rx
            .await
            .expect("synthetic child exited before its wait was pending");
        drop(tick_stdin);
        assert!(waiter.await.unwrap().unwrap().success());
        assert_reaped(&pid).await;
    }

    #[cfg(target_os = "linux")]
    fn fixture(directory: &Path, body: &str) -> (PathBuf, PathBuf) {
        use std::{fs, os::unix::fs::PermissionsExt};

        let binary = directory.join("agy-fixture");
        let pid = directory.join("pid");
        // Publish the pid by rename, so a reader never sees a partial file.
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\n[ \"$1\" = -p ] && [ \"$2\" = /usage ] || exit 3\nprintf '%s' \"$$\" > '{0}.new' && mv '{0}.new' '{0}'\n{body}\n",
                pid.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        (binary, pid)
    }

    /// How long a loaded machine may take to start, kill or reap a child.
    #[cfg(target_os = "linux")]
    const SETTLE: Duration = Duration::from_secs(10);

    #[cfg(target_os = "linux")]
    async fn settle(mut done: impl FnMut() -> bool, failure: &str) {
        let deadline = tokio::time::Instant::now() + SETTLE;
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "{failure}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(target_os = "linux")]
    async fn assert_killed(pid: &Path) {
        let child_pid = std::fs::read_to_string(pid).unwrap();
        let process = PathBuf::from(format!("/proc/{}/stat", child_pid.trim()));
        settle(
            || stat_shows_exit(std::fs::read_to_string(&process)),
            "CLI child is still running after cancellation",
        )
        .await;
    }

    #[cfg(target_os = "linux")]
    const ESRCH: i32 = 3;

    /// Whether a `/proc/<pid>/stat` read shows that the child has exited.
    /// A child that exits during the read fails it with ENOENT or ESRCH.
    #[cfg(target_os = "linux")]
    fn stat_shows_exit(read: std::io::Result<String>) -> bool {
        match read {
            Err(error) => {
                let gone = error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(ESRCH);
                assert!(gone, "Could not inspect CLI child: {error}");
                true
            }
            Ok(stat) => {
                stat.rsplit_once(") ")
                    .and_then(|(_, fields)| fields.split_whitespace().next())
                    == Some("Z")
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stat_read_racing_exit_counts_as_exited() {
        use std::io::{Error, ErrorKind};
        assert!(stat_shows_exit(Err(Error::from(ErrorKind::NotFound))));
        assert!(stat_shows_exit(Err(Error::from_raw_os_error(ESRCH))));
        assert!(stat_shows_exit(Ok("42 (sleep) Z 1 42".into())));
        assert!(!stat_shows_exit(Ok("42 (sleep) S 1 42".into())));
        // A command name may itself contain ") ".
        assert!(!stat_shows_exit(Ok("42 (a) b) S 1 42".into())));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[should_panic(expected = "Could not inspect CLI child")]
    fn other_stat_read_errors_still_fail() {
        stat_shows_exit(Err(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
    }

    #[cfg(target_os = "linux")]
    async fn assert_reaped(pid: &Path) {
        let child_pid = std::fs::read_to_string(pid).unwrap();
        let process = PathBuf::from(format!("/proc/{}", child_pid.trim()));
        settle(|| !process.exists(), "CLI child was not reaped").await;
    }
}
