use super::*;
use crate::model::{number, percent100, timestamp};
use serde_json::json;
use std::{io::Read, path::Path};
pub struct Grok;
const D: Descriptor = Descriptor {
    id: "grok",
    name: "Grok",
    link: "https://grok.com/",
    login: "Run `grok` and sign in again.",
};
const USER: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";
const BILLING: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const CREDITS: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const REFRESH: &str = "https://auth.x.ai/oauth2/token";
const DEFAULT_CLIENT_VERSION: &str = "1.0.45";
const AUXILIARY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_VERSION_FILE_BYTES: u64 = 64 * 1024;

struct GrokEndpoints<'a> {
    user: &'a str,
    billing: &'a str,
    credits: &'a str,
}

#[async_trait]
impl Provider for Grok {
    fn descriptor(&self) -> Descriptor {
        D
    }
    fn discover(&self) -> Result<Credentials> {
        let mut credentials = Credentials::load(home().join(".grok/auth.json"), D, vec![])?;
        credentials.object_path =
            credentials::find_token_path(&credentials.document, &["key", "access_token"])
                .ok_or_else(|| D.reconnect("Grok OIDC login not found"))?;
        Ok(credentials)
    }
    async fn fetch(&self, client: &Client, credentials: &mut Credentials) -> Result<Value> {
        if credentials.near_expiry() {
            if credentials
                .object()
                .get("oidc_issuer")
                .and_then(Value::as_str)
                .is_some_and(|issuer| issuer.trim_end_matches('/') != "https://auth.x.ai")
            {
                return Err(D.reconnect("Unrecognised Grok OIDC issuer"));
            }
            let mut form = vec![
                ("grant_type".to_owned(), "refresh_token".to_owned()),
                ("refresh_token".into(), credentials.refresh(D)?),
                (
                    "client_id".into(),
                    credentials.object()["oidc_client_id"]
                        .as_str()
                        .unwrap_or("b1a00492-073a-47ea-816f-4c329264a828")
                        .into(),
                ),
            ];
            for key in ["principal_type", "principal_id"] {
                if let Some(value) = credentials.object()[key].as_str() {
                    form.push((key.into(), value.into()));
                }
            }
            refresh_safely(credentials, REFRESH, D, RefreshBody::Form(form), false).await?;
        }
        let token = credentials.access(D)?;
        let user_id = credentials.object().get("user_id").and_then(Value::as_str);
        let client_version = client_version_from(&home().join(".grok/version.json"));
        fetch_endpoints(
            client,
            token,
            user_id,
            &client_version,
            GrokEndpoints {
                user: USER,
                billing: BILLING,
                credits: CREDITS,
            },
            AUXILIARY_TIMEOUT,
        )
        .await
    }
    fn parse(&self, payload: &Value) -> Result<Usage> {
        let mut meters = Vec::new();
        let credits = payload["credits"]
            .get("config")
            .unwrap_or(&payload["credits"]);
        let billing = payload["billing"]
            .get("config")
            .unwrap_or(&payload["billing"]);
        let mut usage_breakdown = product_usage(credits);
        if let Some(meter) = credits_meter(
            &credits_label(credits, !usage_breakdown.is_empty()),
            credits,
            true,
        ) {
            meters.push(meter);
        } else {
            // Attribution belongs to this credit counter, never an unrelated
            // billing counter that happens to survive an invalid credit view.
            usage_breakdown.clear();
        }
        if let Some(meter) = credits_meter("Billing", billing, false) {
            meters.push(meter);
        }
        if meters.is_empty() {
            return Err(D.invalid());
        }
        Ok(Usage {
            meters,
            usage_breakdown,
            plan: resolve_plan(&payload["user"], &payload["billing"], &payload["credits"]),
            ..Default::default()
        })
    }
}

async fn fetch_endpoints<'a>(
    client: &Client,
    token: &str,
    user_id: Option<&str>,
    client_version: &str,
    endpoints: GrokEndpoints<'a>,
    auxiliary_timeout: Duration,
) -> Result<Value> {
    let fetch_one = |endpoint: &'a str, timeout: Option<Duration>| {
        let mut request = client
            .get(endpoint)
            .bearer_auth(token)
            .header("X-XAI-Token-Auth", "xai-grok-cli")
            .header("x-grok-client-version", client_version)
            .header("x-grok-client-mode", "interactive");
        if let Some(user) = user_id {
            request = request.header("x-userid", user);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let send = request.send();
        async move { response(send.await, endpoint, D).await }
    };
    let (user_res, billing_res, credits_res) = tokio::join!(
        fetch_one(endpoints.user, Some(auxiliary_timeout)),
        fetch_one(endpoints.billing, Some(auxiliary_timeout)),
        fetch_one(endpoints.credits, None),
    );
    let mut rate_limit: Option<ProviderError> = None;
    for result in [&credits_res, &user_res, &billing_res] {
        let Err(error) = result else { continue };
        if error.status == Status::Reconnect {
            return Err(error.clone());
        }
        if let Some(secs) = error.retry_after_secs
            && rate_limit
                .as_ref()
                .is_none_or(|prev| secs > prev.retry_after_secs.unwrap_or(0))
        {
            rate_limit = Some(error.clone());
        }
    }
    if let Some(error) = rate_limit {
        return Err(error);
    }
    let credits = credits_res?;
    let user = user_res.unwrap_or(Value::Null);
    let billing = billing_res.unwrap_or(Value::Null);
    Ok(json!({"user": user, "billing": billing, "credits": credits}))
}

fn client_version_from(path: &Path) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return DEFAULT_CLIENT_VERSION.into();
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_VERSION_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_VERSION_FILE_BYTES as usize
    {
        return DEFAULT_CLIENT_VERSION.into();
    }
    let Ok(document) = serde_json::from_slice::<Value>(&bytes) else {
        return DEFAULT_CLIENT_VERSION.into();
    };
    ["version", "stable_version"]
        .iter()
        .find_map(|key| {
            document
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| is_valid_client_version(value))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| DEFAULT_CLIENT_VERSION.into())
}

fn is_valid_client_version(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let (without_build, build) = match value.split_once('+') {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    };
    let (core, pre) = match without_build.split_once('-') {
        Some((head, tail)) => (head, Some(tail)),
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let valid_core = (0..3).all(|_| {
        parts.next().is_some_and(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
    }) && parts.next().is_none();
    if !valid_core {
        return false;
    }
    let valid_dot_ids = |s: &str| {
        !s.is_empty()
            && s.split('.').all(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
    };
    pre.is_none_or(valid_dot_ids) && build.is_none_or(valid_dot_ids)
}

fn credits_label(config: &Value, shared: bool) -> String {
    let cadence = match config["currentPeriod"]["type"].as_str() {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => "Weekly ",
        Some("USAGE_PERIOD_TYPE_DAILY") => "Daily ",
        Some("USAGE_PERIOD_TYPE_MONTHLY") => "Monthly ",
        _ => "",
    };
    match (cadence, shared) {
        ("", false) => "Credits".into(),
        ("", true) => "Shared credits".into(),
        (_, true) => format!("{cadence}shared credits"),
        (_, false) => format!("{cadence}credits"),
    }
}

fn credits_meter(label: &str, config: &Value, allow_unified_zero: bool) -> Option<Meter> {
    let has_credit_usage_percent = config.get("creditUsagePercent").is_some();
    let used_percent = if has_credit_usage_percent {
        percent100(&config["creditUsagePercent"])
    } else {
        legacy_billing_percent(config).or_else(|| {
            // SuperGrok unified billing serialises as proto3 JSON, omitting
            // zero-valued creditUsagePercent right after a period reset.
            (allow_unified_zero && is_unified_billing_period(config)).then_some(0.0)
        })
    };
    if let Some(used_percent) = used_percent {
        Some(Meter {
            label: label.into(),
            used_percent,
            resets_at: timestamp(&config["currentPeriod"]["end"])
                .or_else(|| timestamp(&config["billingPeriodEnd"])),
        })
    } else if has_credit_usage_percent {
        None
    } else {
        quota_meter(label, config)
    }
}

fn legacy_billing_percent(config: &Value) -> Option<f32> {
    let used = number(&config["used"]["val"])?;
    let limit = number(&config["monthlyLimit"]["val"])?;
    (used >= 0.0 && limit > 0.0).then_some((used / limit * 100.0).clamp(0.0, 100.0) as f32)
}

fn is_unified_billing_period(config: &Value) -> bool {
    config["isUnifiedBillingUser"].as_bool() == Some(true)
        && config
            .get("currentPeriod")
            .and_then(Value::as_object)
            .is_some_and(|period| !period.is_empty())
}

fn product_usage(config: &Value) -> Vec<crate::model::ProductUsage> {
    config["productUsage"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let product = entry["product"].as_str()?.trim();
            if product.is_empty() || product.eq_ignore_ascii_case("<redacted>") {
                return None;
            }
            let used_percent = percent100(&entry["usagePercent"])?;
            let label = match product {
                "GrokBuild" => "Grok Build".into(),
                "GrokChat" => "Grok Chat".into(),
                "GrokImagine" => "Grok Imagine".into(),
                "GrokBot" => "Grok Bot".into(),
                _ => product.to_owned(),
            };
            Some(crate::model::ProductUsage {
                label,
                used_percent,
            })
        })
        .collect()
}
/// The current first-party CLI explicitly requests `include=subscription` for
/// a backend tier. Older responses retain the compatible billing-view fallback;
/// credit amounts alone are not a stable marketing tier or price catalogue.
fn resolve_plan(user: &Value, billing: &Value, credits: &Value) -> Option<String> {
    if let Some(tier) = string_at(user, &["subscriptionTier"]) {
        return Some(match tier.as_str() {
            "XPremiumPlus" => "X Premium Plus".into(),
            "XPremium" => "X Premium".into(),
            "XBasic" => "X Basic".into(),
            "SuperGrokPro" => "SuperGrok Pro".into(),
            "SuperGrokPlus" => "SuperGrok Plus".into(),
            "SuperGrokLite" => "SuperGrok Lite".into(),
            "GrokPro" => "Grok Pro".into(),
            _ => tier,
        });
    }
    let a = string_at(billing, &["plan", "plan_name", "tier"]);
    let b = string_at(credits, &["plan", "plan_name", "tier"]);
    match (a, b) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b) => Some(a),
        _ if credits["config"]["isUnifiedBillingUser"].as_bool() == Some(true)
            && number(&billing["config"]["monthlyLimit"]["val"]) == Some(0.0) =>
        {
            // A current unified-credit account has a disabled legacy monthly
            // allowance. This describes its observed billing model, not a
            // guessed marketing tier or price.
            let cadence = match credits["config"]["currentPeriod"]["type"].as_str() {
                Some("USAGE_PERIOD_TYPE_WEEKLY") => " · weekly",
                Some("USAGE_PERIOD_TYPE_DAILY") => " · daily",
                _ => "",
            };
            Some(format!("Unified credits{cadence}"))
        }
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_shape_is_error_not_fabricated_plan() {
        assert!(
            Grok.parse(&json!({"user":{"name":"ignored"},"billing":{},"credits":{}}))
                .is_err()
        );
        assert_eq!(
            resolve_plan(&Value::Null, &json!({"plan":"pro"}), &json!({"plan":"max"})),
            None
        );
    }
    #[test]
    fn captured_unified_billing_uses_current_credit_window() {
        let v: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/grok.json")).unwrap();
        let u = Grok.parse(&v).unwrap();
        assert_eq!(u.meters.len(), 1);
        assert_eq!(u.meters[0].used_percent, 72.0);
        assert_eq!(u.meters[0].label, "Weekly credits");
        assert!(u.usage_breakdown.is_empty());
        assert_eq!(u.plan.as_deref(), Some("Unified credits · weekly"));
    }

    #[test]
    fn product_usage_attributes_shared_credits_without_creating_quota_windows() {
        let v: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/grok-products.json")).unwrap();
        let u = Grok.parse(&v).unwrap();
        assert_eq!(u.meters.len(), 1);
        assert_eq!(u.meters[0].label, "Weekly shared credits");
        assert_eq!(u.meters[0].used_percent, 72.0);
        assert_eq!(
            u.usage_breakdown
                .iter()
                .map(|entry| (entry.label.as_str(), entry.used_percent))
                .collect::<Vec<_>>(),
            vec![
                ("Grok Build", 70.0),
                ("Grok Chat", 1.0),
                ("Grok Imagine", 1.0)
            ]
        );
    }

    #[test]
    fn product_usage_skips_invalid_or_redacted_values_and_preserves_unknown_names() {
        let config = json!({
            "productUsage": [
                {"product": "<redacted>", "usagePercent": 3},
                {"product": "", "usagePercent": 4},
                {"product": "FutureProduct", "usagePercent": 5},
                {"product": "GrokBuild", "usagePercent": -1},
                {"product": "GrokChat", "usagePercent": 101},
                {"product": "GrokImagine", "usagePercent": "not a number"}
            ]
        });
        assert_eq!(
            product_usage(&config),
            vec![crate::model::ProductUsage {
                label: "FutureProduct".into(),
                used_percent: 5.0,
            }]
        );
    }

    #[test]
    fn credit_cadence_and_reset_only_use_reported_values() {
        for (kind, label) in [
            (json!("USAGE_PERIOD_TYPE_DAILY"), "Daily credits"),
            (json!("USAGE_PERIOD_TYPE_MONTHLY"), "Monthly credits"),
            (json!("unknown"), "Credits"),
            (json!(null), "Credits"),
        ] {
            let payload = json!({"credits": {
                "creditUsagePercent": 1,
                "currentPeriod": {"type": kind, "end": "invalid"}
            }});
            let usage = Grok.parse(&payload).unwrap();
            assert_eq!(usage.meters[0].label, label);
            assert_eq!(usage.meters[0].used_percent, 1.0);
            assert_eq!(usage.meters[0].resets_at, None);
            assert!(usage.usage_breakdown.is_empty());
        }
        for (end, expected) in [
            (json!(null), 1_788_687_890_392_288_000),
            (json!("invalid"), 1_788_687_890_392_288_000),
            (json!("2026-10-01T00:00:00Z"), 1_790_812_800_000_000_000),
        ] {
            let usage = Grok
                .parse(&json!({"credits": {
                    "creditUsagePercent": 0,
                    "currentPeriod": {"end": end},
                    "billingPeriodEnd": "2026-09-06T09:44:50.392288+00:00"
                }}))
                .unwrap();
            assert_eq!(
                usage.meters[0].resets_at.unwrap().unix_timestamp_nanos(),
                expected
            );
        }
    }

    #[test]
    fn missing_or_invalid_attribution_does_not_invent_product_limits() {
        for products in [
            json!(null),
            json!({}),
            json!([]),
            json!([
                {"product":"GrokChat"},
                {"product":"GrokBuild", "usagePercent": -1},
                {"product":"GrokBot", "usagePercent": 101}
            ]),
        ] {
            let usage = Grok
                .parse(&json!({"credits": {
                    "creditUsagePercent": 72,
                    "productUsage": products
                }}))
                .unwrap();
            assert_eq!(usage.meters.len(), 1);
            assert_eq!(usage.meters[0].label, "Credits");
            assert!(usage.usage_breakdown.is_empty());
            assert_eq!(usage.meters[0].resets_at, None);
        }
    }

    #[test]
    fn explicit_bot_attribution_is_formatted_without_inventing_scope() {
        // Synthetic spelling support, not a captured Bot enum or account scope.
        let usage = Grok
            .parse(&json!({"credits": {
                "creditUsagePercent": 2,
                "productUsage": [
                    {"product":"GrokBot", "usagePercent":1},
                    {"product":"FutureProduct", "usagePercent":1}
                ]
            }}))
            .unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Shared credits");
        assert_eq!(
            usage
                .usage_breakdown
                .iter()
                .map(|p| (p.label.as_str(), p.used_percent))
                .collect::<Vec<_>>(),
            vec![("Grok Bot", 1.0), ("FutureProduct", 1.0)]
        );
    }

    #[test]
    fn credit_attribution_never_attaches_to_unrelated_billing_meter() {
        let mut payload = json!({
            "credits": {"creditUsagePercent": "invalid", "productUsage": [
                {"product":"GrokChat", "usagePercent":1}
            ]},
            "billing": {"used":{"val":25}, "monthlyLimit":{"val":100},
                "billingPeriodEnd":"2026-10-01T00:00:00Z"}
        });
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Billing");
        assert_eq!(usage.meters[0].used_percent, 25.0);
        assert_eq!(
            usage.meters[0].resets_at,
            timestamp(&json!("2026-10-01T00:00:00Z"))
        );
        assert!(usage.usage_breakdown.is_empty());
        payload["credits"]["creditUsagePercent"] = json!(72);
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(
            usage
                .meters
                .iter()
                .map(|m| (m.label.as_str(), m.used_percent))
                .collect::<Vec<_>>(),
            vec![("Shared credits", 72.0), ("Billing", 25.0)]
        );
        assert_eq!(usage.usage_breakdown.len(), 1);
    }

    #[test]
    fn captured_subscription_metadata_supplies_real_tier_without_guessing_price() {
        let mut payload: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/grok.json")).unwrap();
        payload["user"] =
            serde_json::from_str(include_str!("../../tests/fixtures/grok-user.json")).unwrap();
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("X Premium Plus"));
        assert_eq!(usage.price, None);
        assert_eq!(usage.meters[0].used_percent, 72.0);
        payload["user"] = json!({"subscriptionTier":""});
        assert_eq!(
            Grok.parse(&payload).unwrap().plan.as_deref(),
            Some("Unified credits · weekly")
        );
    }

    #[test]
    fn post_reset_omitted_credit_usage_percent_emits_zero_percent_meter() {
        let post_reset = json!({
            "user": {"subscriptionTier": "XPremiumPlus"},
            "billing": {"config": {"monthlyLimit": {"val": 0}, "used": {"val": 0}}},
            "credits": {
                "config": {
                    "currentPeriod": {
                        "type": "USAGE_PERIOD_TYPE_WEEKLY",
                        "start": "2026-09-06T09:44:50.392288+00:00",
                        "end": "2026-09-13T09:44:50.392288+00:00"
                    },
                    "isUnifiedBillingUser": true,
                    "billingPeriodEnd": "2026-09-13T09:44:50.392288+00:00"
                }
            }
        });
        let usage = Grok.parse(&post_reset).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Weekly credits");
        assert_eq!(usage.meters[0].used_percent, 0.0);
        assert_eq!(
            usage.meters[0].resets_at,
            timestamp(&json!("2026-09-13T09:44:50.392288+00:00"))
        );
        assert!(usage.usage_breakdown.is_empty());
        assert_eq!(usage.plan.as_deref(), Some("X Premium Plus"));

        // Falls back to billingPeriodEnd when currentPeriod.end is absent or invalid,
        // and respects each reported cadence even when auxiliary payloads are null.
        for (period, expected_label, expected_reset) in [
            (
                json!({"type": "USAGE_PERIOD_TYPE_DAILY"}),
                "Daily credits",
                "2026-09-13T09:44:50.392288+00:00",
            ),
            (
                json!({"type": "USAGE_PERIOD_TYPE_MONTHLY", "end": "invalid"}),
                "Monthly credits",
                "2026-09-13T09:44:50.392288+00:00",
            ),
            (
                json!({"end": "2026-10-01T00:00:00Z"}),
                "Credits",
                "2026-10-01T00:00:00Z",
            ),
        ] {
            let payload = json!({
                "user": null,
                "billing": null,
                "credits": {
                    "config": {
                        "currentPeriod": period,
                        "isUnifiedBillingUser": true,
                        "billingPeriodEnd": "2026-09-13T09:44:50.392288+00:00"
                    }
                }
            });
            let usage = Grok.parse(&payload).unwrap();
            assert_eq!(usage.meters.len(), 1);
            assert_eq!(usage.meters[0].label, expected_label);
            assert_eq!(usage.meters[0].used_percent, 0.0);
            assert_eq!(usage.meters[0].resets_at, timestamp(&json!(expected_reset)));
            assert_eq!(usage.plan, None);
        }
    }

    #[test]
    fn post_reset_zero_omission_rejects_invalid_values_and_unknown_shapes() {
        let period = json!({
            "type": "USAGE_PERIOD_TYPE_WEEKLY",
            "end": "2026-09-13T09:44:50.392288+00:00"
        });
        // Present-but-invalid creditUsagePercent must fail even with unified billing + currentPeriod.
        for bad in [
            json!(null),
            json!("invalid"),
            json!(""),
            json!(-1),
            json!(101),
            json!({}),
        ] {
            let err = Grok
                .parse(&json!({
                    "credits": {
                        "config": {
                            "creditUsagePercent": bad,
                            "isUnifiedBillingUser": true,
                            "currentPeriod": period
                        }
                    }
                }))
                .err()
                .expect("invalid creditUsagePercent must fail");
            assert_eq!(err.status, crate::model::Status::Error);
            assert!(err.message.contains("no recognised quota meters"));
        }

        // Missing, null, empty, or non-object currentPeriod must not fabricate 0%.
        for bad_period in [json!(null), json!({}), json!("weekly"), json!([])] {
            let err = Grok
                .parse(&json!({
                    "credits": {
                        "config": {
                            "isUnifiedBillingUser": true,
                            "currentPeriod": bad_period
                        }
                    }
                }))
                .err()
                .expect("non-object or empty currentPeriod must fail");
            assert_eq!(err.status, crate::model::Status::Error);
        }
        assert!(
            Grok.parse(&json!({
                "credits": {"config": {"isUnifiedBillingUser": true}}
            }))
            .is_err()
        );

        // Missing or false isUnifiedBillingUser must not fabricate 0%.
        for unified in [json!(false), json!(null), json!("true")] {
            assert!(
                Grok.parse(&json!({
                    "credits": {
                        "config": {
                            "isUnifiedBillingUser": unified,
                            "currentPeriod": period
                        }
                    }
                }))
                .is_err()
            );
        }
        assert!(
            Grok.parse(&json!({
                "credits": {"config": {"currentPeriod": period}}
            }))
            .is_err()
        );

        // Legacy billing config must never emit a unified 0% meter.
        assert!(
            Grok.parse(&json!({
                "credits": {},
                "billing": {
                    "config": {
                        "isUnifiedBillingUser": true,
                        "currentPeriod": period
                    }
                }
            }))
            .is_err()
        );
        let usage = Grok
            .parse(&json!({
                "credits": {
                    "config": {
                        "creditUsagePercent": 72,
                        "isUnifiedBillingUser": true,
                        "currentPeriod": period
                    }
                },
                "billing": {
                    "config": {
                        "isUnifiedBillingUser": true,
                        "currentPeriod": period
                    }
                }
            }))
            .unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Weekly credits");
    }

    const POST_RESET_CREDITS_JSON: &str = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-09-06T09:44:50.392288+00:00","end":"2026-09-13T09:44:50.392288+00:00"},"isUnifiedBillingUser":true,"billingPeriodEnd":"2026-09-13T09:44:50.392288+00:00"}}"#;
    const LEGACY_ZERO_BILLING_JSON: &str =
        r#"{"config":{"monthlyLimit":{"val":0},"used":{"val":0}}}"#;
    const USER_TIER_JSON: &str = r#"{"subscriptionTier":"XPremiumPlus"}"#;

    enum MockReply {
        Http {
            status: u16,
            body: &'static str,
            retry_after: Option<&'static str>,
        },
        Close,
        Hang(std::sync::mpsc::Receiver<()>),
    }

    impl MockReply {
        fn ok(body: &'static str) -> Self {
            Self::Http {
                status: 200,
                body,
                retry_after: None,
            }
        }
        fn status(status: u16, body: &'static str) -> Self {
            Self::Http {
                status,
                body,
                retry_after: None,
            }
        }
        fn rate_limit(retry_after: &'static str) -> Self {
            Self::Http {
                status: 429,
                body: r#"{"error":"rate limited"}"#,
                retry_after: Some(retry_after),
            }
        }
    }

    fn spawn_local_endpoint(
        path: &'static str,
        reply: MockReply,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::{io::Write, net::TcpListener, thread, time::Instant};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no local Grok request arrived");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("local Grok accept failed: {error}"),
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
            match reply {
                MockReply::Http {
                    status,
                    body,
                    retry_after,
                } => {
                    let retry_header = retry_after
                        .map(|secs| format!("Retry-After: {secs}\r\n"))
                        .unwrap_or_default();
                    write!(
                        socket,
                        "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry_header}Connection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
                MockReply::Close => {}
                MockReply::Hang(stop_rx) => {
                    let _ = stop_rx.recv_timeout(Duration::from_secs(3));
                }
            }
            request
        });
        (url, handle)
    }

    fn local_client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    async fn run_local_fetch(
        client: &Client,
        user_id: Option<&str>,
        client_version: &str,
        replies: (MockReply, MockReply, MockReply),
    ) -> (Result<Value>, [String; 3], [String; 3]) {
        let (user_url, user_srv) = spawn_local_endpoint("/v1/user?include=subscription", replies.0);
        let (billing_url, billing_srv) = spawn_local_endpoint("/v1/billing", replies.1);
        let (credits_url, credits_srv) =
            spawn_local_endpoint("/v1/billing?format=credits", replies.2);
        let result = fetch_endpoints(
            client,
            "synthetic-token",
            user_id,
            client_version,
            GrokEndpoints {
                user: &user_url,
                billing: &billing_url,
                credits: &credits_url,
            },
            Duration::from_secs(2),
        )
        .await;
        let requests = [
            user_srv.join().unwrap(),
            billing_srv.join().unwrap(),
            credits_srv.join().unwrap(),
        ];
        (result, [user_url, billing_url, credits_url], requests)
    }

    #[tokio::test]
    async fn auxiliary_endpoint_failures_degrade_to_null_when_credits_succeeds() {
        let client = local_client();

        // /user returns 500 while /billing and post-reset /credits succeed.
        let (result, _, _) = run_local_fetch(
            &client,
            Some("synthetic-user"),
            DEFAULT_CLIENT_VERSION,
            (
                MockReply::status(500, "internal error"),
                MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                MockReply::ok(POST_RESET_CREDITS_JSON),
            ),
        )
        .await;
        let payload = result.unwrap();
        assert_eq!(payload["user"], Value::Null);
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].label, "Weekly credits");
        assert_eq!(usage.meters[0].used_percent, 0.0);
        assert_eq!(usage.plan.as_deref(), Some("Unified credits · weekly"));

        // /billing returns non-JSON 200 and /user drops connection while /credits succeeds.
        let (result, _, _) = run_local_fetch(
            &client,
            None,
            DEFAULT_CLIENT_VERSION,
            (
                MockReply::Close,
                MockReply::ok("<html>not json</html>"),
                MockReply::ok(POST_RESET_CREDITS_JSON),
            ),
        )
        .await;
        let payload = result.unwrap();
        assert_eq!(payload["user"], Value::Null);
        assert_eq!(payload["billing"], Value::Null);
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].used_percent, 0.0);
        assert_eq!(usage.plan, None);

        // /billing returns 503 while /user and /credits succeed -> plan still resolves from /user.
        let (result, _, _) = run_local_fetch(
            &client,
            Some("synthetic-user"),
            DEFAULT_CLIENT_VERSION,
            (
                MockReply::ok(USER_TIER_JSON),
                MockReply::status(503, "unavailable"),
                MockReply::ok(POST_RESET_CREDITS_JSON),
            ),
        )
        .await;
        let payload = result.unwrap();
        assert_eq!(payload["billing"], Value::Null);
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("X Premium Plus"));
        assert_eq!(usage.meters[0].used_percent, 0.0);
    }

    #[tokio::test]
    async fn hung_auxiliary_endpoints_time_out_and_degrade_before_outer_timeout() {
        let client = local_client();
        let (user_stop_tx, user_stop_rx) = std::sync::mpsc::channel();
        let (billing_stop_tx, billing_stop_rx) = std::sync::mpsc::channel();
        let (user_url, user_srv) = spawn_local_endpoint(
            "/v1/user?include=subscription",
            MockReply::Hang(user_stop_rx),
        );
        let (billing_url, billing_srv) =
            spawn_local_endpoint("/v1/billing", MockReply::Hang(billing_stop_rx));
        let (credits_url, credits_srv) = spawn_local_endpoint(
            "/v1/billing?format=credits",
            MockReply::ok(POST_RESET_CREDITS_JSON),
        );
        let payload = fetch_endpoints(
            &client,
            "synthetic-token",
            None,
            DEFAULT_CLIENT_VERSION,
            GrokEndpoints {
                user: &user_url,
                billing: &billing_url,
                credits: &credits_url,
            },
            Duration::from_millis(120),
        )
        .await
        .unwrap();
        let _ = user_stop_tx.send(());
        let _ = billing_stop_tx.send(());
        user_srv.join().unwrap();
        billing_srv.join().unwrap();
        credits_srv.join().unwrap();
        assert_eq!(payload["user"], Value::Null);
        assert_eq!(payload["billing"], Value::Null);
        let usage = Grok.parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1);
        assert_eq!(usage.meters[0].used_percent, 0.0);
    }

    #[tokio::test]
    async fn credits_endpoint_failure_is_propagated_not_hidden() {
        let client = local_client();
        let (result, urls, _) = run_local_fetch(
            &client,
            None,
            DEFAULT_CLIENT_VERSION,
            (
                MockReply::ok(USER_TIER_JSON),
                MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                MockReply::status(500, "upstream failure"),
            ),
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.status, Status::Error);
        assert!(err.message.contains(&urls[2]));
        assert!(err.message.contains("HTTP 500"));
        assert!(!err.message.contains("no recognised quota meters"));
    }

    #[tokio::test]
    async fn unauthorized_401_from_any_endpoint_propagates_reconnect_over_other_errors() {
        let client = local_client();
        let cases = [
            (
                MockReply::status(401, "unauthorized"),
                MockReply::status(500, "err"),
                MockReply::ok(POST_RESET_CREDITS_JSON),
            ),
            (
                MockReply::ok(USER_TIER_JSON),
                MockReply::status(401, "unauthorized"),
                MockReply::status(500, "err"),
            ),
            (
                MockReply::rate_limit("45"),
                MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                MockReply::status(401, "unauthorized"),
            ),
            (
                MockReply::status(401, "unauthorized"),
                MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                MockReply::rate_limit("45"),
            ),
        ];
        for replies in cases {
            let (result, _, _) =
                run_local_fetch(&client, None, DEFAULT_CLIENT_VERSION, replies).await;
            let err = result.unwrap_err();
            assert_eq!(err.status, Status::Reconnect);
            assert!(err.message.contains("HTTP 401"));
            assert!(err.message.contains("Run `grok`"));
        }
    }

    #[tokio::test]
    async fn rate_limit_429_from_any_endpoint_propagates_max_retry_after() {
        let client = local_client();
        let cases = [
            (
                (
                    MockReply::rate_limit("45"),
                    MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                    MockReply::ok(POST_RESET_CREDITS_JSON),
                ),
                45,
            ),
            (
                (
                    MockReply::ok(USER_TIER_JSON),
                    MockReply::rate_limit("60"),
                    MockReply::status(500, "err"),
                ),
                60,
            ),
            (
                (
                    MockReply::rate_limit("90"),
                    MockReply::rate_limit("30"),
                    MockReply::rate_limit("45"),
                ),
                90,
            ),
        ];
        for (replies, expected_secs) in cases {
            let (result, _, _) =
                run_local_fetch(&client, None, DEFAULT_CLIENT_VERSION, replies).await;
            let err = result.unwrap_err();
            assert_eq!(err.status, Status::Error);
            assert!(err.message.contains("HTTP 429"));
            assert_eq!(err.retry_after_secs, Some(expected_secs));
        }
    }

    #[tokio::test]
    async fn client_version_resolution_and_header_transmission() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-version.json");
        assert_eq!(client_version_from(&missing), DEFAULT_CLIENT_VERSION);
        // Unreadable path (directory) falls back cleanly.
        assert_eq!(client_version_from(dir.path()), DEFAULT_CLIENT_VERSION);

        let version_path = dir.path().join("version.json");
        std::fs::write(&version_path, "{not json").unwrap();
        assert_eq!(client_version_from(&version_path), DEFAULT_CLIENT_VERSION);

        // Prefers valid `version` over `stable_version`.
        std::fs::write(
            &version_path,
            json!({"version": "1.2.3", "stable_version": "1.2.0"}).to_string(),
        )
        .unwrap();
        assert_eq!(client_version_from(&version_path), "1.2.3");

        // Falls back to `stable_version` when `version` is absent or invalid.
        std::fs::write(
            &version_path,
            json!({"stable_version": "1.0.48-beta.1+build.42"}).to_string(),
        )
        .unwrap();
        assert_eq!(client_version_from(&version_path), "1.0.48-beta.1+build.42");

        std::fs::write(
            &version_path,
            json!({"version": "1.0.46\r\nX-Injected: 1", "stable_version": "1.0.47"}).to_string(),
        )
        .unwrap();
        assert_eq!(client_version_from(&version_path), "1.0.47");

        // Rejects malformed, whitespace, CRLF, leading-zero, and oversized values.
        let too_long = format!("1.0.{}", "9".repeat(64));
        for invalid in [
            "",
            "latest",
            "v1.0.45",
            "1.0",
            "1.0.45.1",
            "01.0.45",
            " 1.0.45",
            "1.0.45 ",
            "1.0.45\r\nEvil: 1",
            "1.0.45-",
            "1.0.45+",
            "1.0.45-beta..1",
            "1.0.45+build+extra",
            &too_long,
        ] {
            std::fs::write(
                &version_path,
                json!({"version": invalid, "stable_version": invalid}).to_string(),
            )
            .unwrap();
            assert_eq!(
                client_version_from(&version_path),
                DEFAULT_CLIENT_VERSION,
                "expected fallback for invalid version {invalid:?}"
            );
        }

        // Verify resolved version and fallback floor are actually sent on all 3 requests.
        std::fs::write(&version_path, json!({"version": "1.0.99"}).to_string()).unwrap();
        let client = local_client();
        for (path, expected_version) in [(&version_path, "1.0.99"), (&missing, "1.0.45")] {
            let resolved = client_version_from(path);
            assert_eq!(resolved, expected_version);
            let (result, _, requests) = run_local_fetch(
                &client,
                Some("synthetic-user"),
                &resolved,
                (
                    MockReply::ok(USER_TIER_JSON),
                    MockReply::ok(LEGACY_ZERO_BILLING_JSON),
                    MockReply::ok(POST_RESET_CREDITS_JSON),
                ),
            )
            .await;
            result.unwrap();
            let expected_header = format!("x-grok-client-version: {expected_version}");
            for request in requests {
                let lower = request.to_ascii_lowercase();
                assert!(lower.contains(&expected_header), "{request}");
                assert!(
                    lower.contains("x-xai-token-auth: xai-grok-cli"),
                    "{request}"
                );
                assert!(
                    lower.contains("x-grok-client-mode: interactive"),
                    "{request}"
                );
                assert!(
                    lower.contains("authorization: bearer synthetic-token"),
                    "{request}"
                );
                assert!(lower.contains("x-userid: synthetic-user"), "{request}");
            }
        }
    }
}
