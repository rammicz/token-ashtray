use super::*;
use crate::model::{number, percent100, timestamp};
use std::ffi::OsString;
pub struct Copilot;
const D: Descriptor = Descriptor {
    id: "copilot",
    name: "Copilot",
    link: "https://github.com/settings/copilot",
    login: "Run `copilot login` or sign in to GitHub Copilot in your editor.",
};
const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";
const ENV_TOKENS: [&str; 4] = [
    "GITHUB_COPILOT_GITHUB_TOKEN",
    "COPILOT_GITHUB_TOKEN",
    "GITHUB_COPILOT_API_TOKEN",
    "GITHUB_TOKEN",
];

fn resolve_from(
    mut environment: impl FnMut(&str) -> Option<String>,
    directories: impl FnOnce() -> Vec<PathBuf>,
    gh_hosts_files: impl FnOnce() -> Vec<PathBuf>,
) -> Result<Credentials> {
    for key in ENV_TOKENS {
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
    for path in gh_hosts_files() {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if let Some(token) = parse_gh_hosts_yml(&content) {
                    return Ok(Credentials::token(token));
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
    Err(D.reconnect("GitHub Copilot login not found"))
}

#[async_trait]
impl Provider for Copilot {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        resolve_from(
            |key| std::env::var(key).ok(),
            candidate_directories,
            gh_hosts_candidates,
        )
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
            ("ai_credits", "AI credits"),
            ("credits", "Credits"),
        ] {
            let quota = &payload["quota_snapshots"][key];
            if quota["unlimited"].as_bool() == Some(true) {
                unlimited = true;
                continue;
            }
            let entitlement = quota.get("entitlement").and_then(number);
            if entitlement.is_some_and(|e| e <= 0.0) {
                continue;
            }
            let used_percent = match quota.get("percent_remaining") {
                Some(value) if !value.is_null() => percent100(value).map(|r| 100.0 - r),
                _ => {
                    let remaining = quota
                        .get("remaining")
                        .filter(|v| !v.is_null())
                        .or_else(|| quota.get("quota_remaining"))
                        .and_then(number);
                    match (entitlement, remaining) {
                        (Some(entitlement), Some(remaining)) if entitlement > 0.0 => Some(
                            ((entitlement - remaining) / entitlement * 100.0).clamp(0.0, 100.0)
                                as f32,
                        ),
                        _ => None,
                    }
                }
            };
            if let Some(used_percent) = used_percent {
                meters.push(Meter {
                    label: label.into(),
                    used_percent,
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
fn gh_hosts_candidates() -> Vec<PathBuf> {
    gh_hosts_candidates_from(
        &crate::platform::credential_base_paths(),
        std::env::var_os("GH_CONFIG_DIR"),
    )
}
pub(super) fn gh_hosts_candidates_from(
    bases: &crate::platform::CredentialBasePaths,
    gh_config_dir: Option<OsString>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut push_unique = |candidate: PathBuf| {
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    };
    if let Some(dir) = gh_config_dir.filter(|dir| !dir.is_empty()) {
        push_unique(PathBuf::from(dir).join("hosts.yml"));
    }
    if let Some(xdg) = bases
        .xdg_config_home
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        push_unique(xdg.join("gh/hosts.yml"));
    }
    push_unique(bases.home.join(".config/gh/hosts.yml"));
    if let Some(roaming) = bases
        .roaming_app_data
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        push_unique(roaming.join("GitHub CLI/hosts.yml"));
    }
    push_unique(
        bases
            .home
            .join("Library/Application Support/GitHub CLI/hosts.yml"),
    );
    paths
}

/// Minimal zero-dependency extractor for GitHub CLI `hosts.yml`.
/// Scoped strictly to the indent-0 `github.com:` host block so enterprise
/// tokens are never extracted; supports both flat `oauth_token:` and `gh` 2.40+
/// `users: -> <user>: -> oauth_token:` layouts.
fn parse_gh_hosts_yml(content: &str) -> Option<String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut in_github_com = false;
    let mut host_child_indent: Option<usize> = None;
    let mut direct_token: Option<String> = None;
    let mut active_user: Option<String> = None;
    let mut users_indent: Option<usize> = None;
    let mut user_entry_indent: Option<usize> = None;
    let mut current_user: Option<String> = None;
    let mut user_prop_indent: Option<usize> = None;
    let mut user_tokens: Vec<(String, String)> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if indent == 0 {
            host_child_indent = None;
            users_indent = None;
            user_entry_indent = None;
            current_user = None;
            user_prop_indent = None;
            if trimmed == "---" || trimmed == "..." {
                in_github_com = false;
                continue;
            }
            in_github_com = split_yaml_mapping(trimmed).is_some_and(|(key, _)| key == "github.com");
            continue;
        }
        if !in_github_com {
            continue;
        }
        let host_indent = *host_child_indent.get_or_insert(indent);
        if indent < host_indent {
            users_indent = None;
            user_entry_indent = None;
            current_user = None;
            user_prop_indent = None;
            continue;
        }
        if indent == host_indent {
            users_indent = None;
            user_entry_indent = None;
            current_user = None;
            user_prop_indent = None;
            let Some((key, value)) = split_yaml_mapping(trimmed) else {
                continue;
            };
            match key.as_str() {
                "oauth_token" => {
                    if direct_token.is_none() {
                        direct_token = parse_yaml_scalar(value);
                    }
                }
                "user" => {
                    if active_user.is_none() {
                        active_user = parse_yaml_scalar(value);
                    }
                }
                "users" => {
                    users_indent = Some(indent);
                }
                _ => {}
            }
        } else if let Some(u_indent) = users_indent
            && indent > u_indent
        {
            let entry_indent = *user_entry_indent.get_or_insert(indent);
            if indent == entry_indent {
                current_user = split_yaml_mapping(trimmed).map(|(key, _)| key);
                user_prop_indent = None;
            } else if indent > entry_indent
                && let Some(user) = current_user.as_deref()
            {
                let prop_indent = *user_prop_indent.get_or_insert(indent);
                if indent == prop_indent
                    && let Some((key, value)) = split_yaml_mapping(trimmed)
                    && key == "oauth_token"
                    && let Some(token) = parse_yaml_scalar(value)
                {
                    user_tokens.push((user.to_owned(), token));
                }
            }
        }
    }

    if let Some(token) = direct_token {
        return Some(token);
    }
    if let Some(active) = active_user
        && let Some((_, token)) = user_tokens.iter().find(|(user, _)| user == &active)
    {
        return Some(token.clone());
    }
    user_tokens.into_iter().next().map(|(_, token)| token)
}

fn split_yaml_mapping(trimmed: &str) -> Option<(String, &str)> {
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let (key, after) = parse_yaml_quoted(trimmed)?;
        let rest = after.trim_start_matches([' ', '\t']).strip_prefix(':')?;
        if !key.is_empty() && (rest.is_empty() || rest.starts_with([' ', '\t'])) {
            return Some((key, rest));
        }
        return None;
    }
    let bytes = trimmed.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'#' && i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
            break;
        }
        if byte == b':' && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\t')) {
            let key = trimmed[..i].trim();
            if !key.is_empty() {
                return Some((key.to_owned(), &trimmed[i + 1..]));
            }
            return None;
        }
    }
    None
}

fn parse_yaml_scalar(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let (value, after) = parse_yaml_quoted(trimmed)?;
        let after = after.trim();
        if !after.is_empty() && !after.starts_with('#') {
            return None;
        }
        return (!value.trim().is_empty()).then_some(value);
    }
    let bytes = trimmed.as_bytes();
    let end = bytes
        .iter()
        .enumerate()
        .find_map(|(i, &byte)| {
            (byte == b'#' && i > 0 && matches!(bytes[i - 1], b' ' | b'\t')).then_some(i)
        })
        .unwrap_or(trimmed.len());
    let value = trimmed[..end].trim_end();
    if value.is_empty() || value == "~" || value.eq_ignore_ascii_case("null") {
        return None;
    }
    Some(value.to_owned())
}

fn parse_yaml_quoted(input: &str) -> Option<(String, &str)> {
    if let Some(rest) = input.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for (idx, ch) in rest.char_indices() {
            if escaped {
                out.push(match ch {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '\\' => '\\',
                    '"' => '"',
                    other => other,
                });
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return Some((out, &rest[idx + 1..]));
            } else {
                out.push(ch);
            }
        }
        None
    } else if let Some(rest) = input.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if ch == '\'' {
                if matches!(chars.peek(), Some((_, '\''))) {
                    chars.next();
                    out.push('\'');
                } else {
                    return Some((out, &rest[idx + 1..]));
                }
            } else {
                out.push(ch);
            }
        }
        None
    } else {
        None
    }
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

        let credentials = resolve_from(|_| None, || vec![directory], Vec::new).unwrap();
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

            let credentials = resolve_from(|_| None, || vec![directory], Vec::new).unwrap();
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
        let gh_hosts = root.path().join("hosts.yml");
        fs::write(
            &gh_hosts,
            "github.com:\n  oauth_token: synthetic-gh-token\n",
        )
        .unwrap();

        let mut queried = Vec::new();
        let first = resolve_from(
            |key| {
                queried.push(key.to_owned());
                Some("synthetic-first-env-token".into())
            },
            || panic!("file candidates were inspected despite environment override"),
            || panic!("gh hosts.yml was inspected despite environment override"),
        )
        .unwrap();
        assert_eq!(first.access(D).unwrap(), "synthetic-first-env-token");
        assert_eq!(queried, ["GITHUB_COPILOT_GITHUB_TOKEN"]);

        queried.clear();
        let second = resolve_from(
            |key| {
                queried.push(key.to_owned());
                Some(if key == "GITHUB_COPILOT_GITHUB_TOKEN" {
                    " \t ".into()
                } else {
                    "synthetic-second-env-token".into()
                })
            },
            || panic!("file candidates were inspected despite second environment override"),
            || panic!("gh hosts.yml was inspected despite second environment override"),
        )
        .unwrap();
        assert_eq!(second.access(D).unwrap(), "synthetic-second-env-token");
        assert_eq!(
            queried,
            ["GITHUB_COPILOT_GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN"]
        );

        queried.clear();
        let fourth = resolve_from(
            |key| {
                queried.push(key.to_owned());
                Some(if key == "GITHUB_TOKEN" {
                    "synthetic-fourth-env-token".into()
                } else {
                    " \t ".into()
                })
            },
            || panic!("file candidates were inspected despite fourth environment override"),
            || panic!("gh hosts.yml was inspected despite fourth environment override"),
        )
        .unwrap();
        assert_eq!(fourth.access(D).unwrap(), "synthetic-fourth-env-token");
        assert_eq!(
            queried,
            [
                "GITHUB_COPILOT_GITHUB_TOKEN",
                "COPILOT_GITHUB_TOKEN",
                "GITHUB_COPILOT_API_TOKEN",
                "GITHUB_TOKEN",
            ]
        );

        let apps = resolve_from(
            |_| Some(" \t ".into()),
            || vec![directory],
            || panic!("gh hosts.yml was inspected despite apps.json token"),
        )
        .unwrap();
        assert_eq!(apps.access(D).unwrap(), "synthetic-app-token");

        let from_gh = resolve_from(|_| None, Vec::new, || vec![gh_hosts]).unwrap();
        assert_eq!(from_gh.access(D).unwrap(), "synthetic-gh-token");
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
            let credentials = resolve_from(|_| None, || directories, Vec::new).unwrap();
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
        let error = match resolve_from(|_| None, || vec![first.clone(), second], Vec::new) {
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

        let error = match resolve_from(|_| None, || vec![directory.clone()], Vec::new) {
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
    fn gh_hosts_yml_parser_handles_shapes_quotes_comments_crlf_and_host_scoping() {
        // Classic flat layout with CRLF and inline comment.
        assert_eq!(
            parse_gh_hosts_yml(
                "\u{feff}github.com:\r\n    oauth_token: gho_synthetic_flat # inline comment\r\n    user: octocat\r\n"
            ),
            Some("gho_synthetic_flat".into())
        );

        // Quoted host key and quoted values containing '#' must not truncate inside quotes.
        assert_eq!(
            parse_gh_hosts_yml(
                "\"github.com\":\n  oauth_token: \"gho_double#hash\" # trailing comment\n"
            ),
            Some("gho_double#hash".into())
        );
        assert_eq!(
            parse_gh_hosts_yml(
                "'github.com':\n  oauth_token: 'gho_single#hash''quoted' # comment\n"
            ),
            Some("gho_single#hash'quoted".into())
        );

        // gh 2.40+ users: nesting prefers active `user:` over an earlier user entry.
        let multi_account = "\
github.com:
    git_protocol: https
    users:
        earlier_user:
            oauth_token: gho_earlier_token
        active_user:
            nested:
                oauth_token: gho_nested_ignored
            oauth_token: gho_active_token
    user: active_user
";
        assert_eq!(
            parse_gh_hosts_yml(multi_account),
            Some("gho_active_token".into())
        );

        // Fallback to first available user token when active `user:` is absent or keyring-backed.
        let fallback_user = "\
github.com:
    users:
        keyring_user:
        file_user:
            oauth_token: gho_file_user_token
    user: keyring_user
";
        assert_eq!(
            parse_gh_hosts_yml(fallback_user),
            Some("gho_file_user_token".into())
        );

        // Enterprise hosts above or below github.com must never leak their tokens.
        let enterprise_before_and_after = "\
ghe.company.internal:
    oauth_token: gho_enterprise_above
    users:
        corp:
            oauth_token: gho_enterprise_nested
github.com:
    user: octocat
    users:
        octocat:
            oauth_token: gho_public_github
ghe.example.com:8443:
    oauth_token: gho_enterprise_below
";
        assert_eq!(
            parse_gh_hosts_yml(enterprise_before_and_after),
            Some("gho_public_github".into())
        );

        // Enterprise-only, keyring-only, null/tilde, or unrelated nested block -> None.
        for absent in [
            "ghe.company.internal:\n  oauth_token: gho_enterprise_only\n",
            "github.com:\n  git_protocol: https\n  users:\n    octocat:\n  user: octocat\n",
            "github.com:\n  oauth_token: null\n",
            "github.com:\n  oauth_token: ~ # empty\n",
            "github.com:\n  oauth_token: \"   \"\n",
            "github.com:\n  other_section:\n    oauth_token: gho_wrong_section\n",
        ] {
            assert_eq!(parse_gh_hosts_yml(absent), None, "input:\n{absent}");
        }
    }

    #[test]
    fn gh_hosts_discovery_handles_keyring_enterprise_and_unreadable_files() {
        let root = tempfile::tempdir().unwrap();
        let keyring_only = root.path().join("keyring-hosts.yml");
        fs::write(
            &keyring_only,
            "github.com:\n  git_protocol: https\n  users:\n    octocat:\n  user: octocat\n",
        )
        .unwrap();
        let enterprise_only = root.path().join("enterprise-hosts.yml");
        fs::write(
            &enterprise_only,
            "ghe.company.internal:\n  oauth_token: gho_enterprise_secret\n",
        )
        .unwrap();

        // Keyring-backed or enterprise-only hosts.yml returns Reconnect, not Error.
        for path in [&keyring_only, &enterprise_only] {
            let err = match resolve_from(|_| None, Vec::new, || vec![path.clone()]) {
                Ok(_) => panic!("expected Reconnect for {}", path.display()),
                Err(err) => err,
            };
            assert_eq!(err.status, Status::Reconnect);
            assert!(!err.message.contains("gho_enterprise_secret"));
        }

        // Keyring-backed earlier hosts.yml falls through to a later readable hosts.yml with a token.
        let later = root.path().join("later-hosts.yml");
        fs::write(
            &later,
            "github.com:\n  users:\n    octocat:\n      oauth_token: gho_later_token\n  user: octocat\n",
        )
        .unwrap();
        let creds = resolve_from(|_| None, Vec::new, || vec![keyring_only, later.clone()]).unwrap();
        assert_eq!(creds.access(D).unwrap(), "gho_later_token");

        // Unreadable hosts.yml (a directory) returns Status::Error without leaking tokens.
        let unreadable = root.path().join("unreadable-hosts.yml");
        fs::create_dir(&unreadable).unwrap();
        let err = match resolve_from(|_| None, Vec::new, || vec![unreadable.clone(), later]) {
            Ok(_) => panic!("unreadable hosts.yml was skipped"),
            Err(err) => err,
        };
        assert_eq!(err.status, Status::Error);
        assert!(err.message.contains(&unreadable.display().to_string()));
        assert!(!err.message.contains("gho_later_token"));
    }

    #[test]
    fn gh_hosts_candidates_respect_precedence_dedup_and_ignore_empty_env() {
        let bases = crate::platform::CredentialBasePaths {
            home: PathBuf::from("/fixture/home"),
            xdg_config_home: Some(PathBuf::from("/fixture/xdg")),
            local_app_data: Some(PathBuf::from("/fixture/local")),
            roaming_app_data: Some(PathBuf::from("/fixture/roaming")),
        };
        assert_eq!(
            gh_hosts_candidates_from(&bases, Some(OsString::from("/fixture/gh"))),
            [
                PathBuf::from("/fixture/gh/hosts.yml"),
                PathBuf::from("/fixture/xdg/gh/hosts.yml"),
                PathBuf::from("/fixture/home/.config/gh/hosts.yml"),
                PathBuf::from("/fixture/roaming/GitHub CLI/hosts.yml"),
                PathBuf::from("/fixture/home/Library/Application Support/GitHub CLI/hosts.yml"),
            ]
        );

        let empty_overrides = crate::platform::CredentialBasePaths {
            home: PathBuf::from("/fixture/home"),
            xdg_config_home: Some(PathBuf::new()),
            local_app_data: None,
            roaming_app_data: Some(PathBuf::new()),
        };
        assert_eq!(
            gh_hosts_candidates_from(&empty_overrides, Some(OsString::new())),
            [
                PathBuf::from("/fixture/home/.config/gh/hosts.yml"),
                PathBuf::from("/fixture/home/Library/Application Support/GitHub CLI/hosts.yml"),
            ]
        );

        // XDG matching ~/.config deduplicates rather than probing the same file twice.
        let colliding = crate::platform::CredentialBasePaths {
            home: PathBuf::from("/fixture/home"),
            xdg_config_home: Some(PathBuf::from("/fixture/home/.config")),
            local_app_data: None,
            roaming_app_data: None,
        };
        assert_eq!(
            gh_hosts_candidates_from(&colliding, Some(OsString::from("/fixture/home/.config/gh"))),
            [
                PathBuf::from("/fixture/home/.config/gh/hosts.yml"),
                PathBuf::from("/fixture/home/Library/Application Support/GitHub CLI/hosts.yml"),
            ]
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
    fn schema_ai_credits_credits_and_entitlement_remaining_fallback() {
        let usage = Copilot
            .parse(&json!({
                "copilot_plan": "pro",
                "quota_reset_date_utc": "2026-12-01T00:00:00Z",
                "quota_snapshots": {
                    "ai_credits": {"entitlement": 200, "remaining": 50},
                    "credits": {"percent_remaining": null, "entitlement": 100, "quota_remaining": -10},
                    "chat": {"entitlement": 0, "percent_remaining": 50},
                    "completions": {"entitlement": -5, "remaining": 0},
                    "premium_interactions": {"percent_remaining": 150, "entitlement": 100, "remaining": 50}
                }
            }))
            .unwrap();
        assert_eq!(usage.meters.len(), 2);
        assert_eq!(usage.meters[0].label, "AI credits");
        assert_eq!(usage.meters[0].used_percent, 75.0);
        assert!(usage.meters[0].resets_at.is_some());
        assert_eq!(usage.meters[1].label, "Credits");
        assert_eq!(usage.meters[1].used_percent, 100.0);

        // Missing remaining without percent_remaining must never invent 0%.
        assert!(
            Copilot
                .parse(&json!({"quota_snapshots": {"ai_credits": {"entitlement": 100}}}))
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
