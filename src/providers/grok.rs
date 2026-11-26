use super::*;
use crate::model::{number, percent100, timestamp};
use serde_json::json;
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
        let request = |endpoint| {
            let mut request = client
                .get(endpoint)
                .bearer_auth(token)
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-client-version", "1.0.13")
                .header("x-grok-client-mode", "interactive");
            if let Some(user) = credentials.object().get("user_id").and_then(Value::as_str) {
                request = request.header("x-userid", user);
            }
            request
        };
        let (user, billing, credits) = tokio::try_join!(
            async { response(request(USER).send().await, USER, D).await },
            async { response(request(BILLING).send().await, BILLING, D).await },
            async { response(request(CREDITS).send().await, CREDITS, D).await }
        )?;
        Ok(json!({"user":user,"billing":billing,"credits":credits}))
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
        ) {
            meters.push(meter);
        } else {
            // Attribution belongs to this credit counter, never an unrelated
            // billing counter that happens to survive an invalid credit view.
            usage_breakdown.clear();
        }
        if let Some(meter) = credits_meter("Billing", billing) {
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

fn credits_meter(label: &str, config: &Value) -> Option<Meter> {
    let used_percent = percent100(&config["creditUsagePercent"]).or_else(|| {
        let used = number(&config["used"]["val"])?;
        let limit = number(&config["monthlyLimit"]["val"])?;
        (used >= 0.0 && limit > 0.0).then_some((used / limit * 100.0).clamp(0.0, 100.0) as f32)
    });
    if let Some(used_percent) = used_percent {
        Some(Meter {
            label: label.into(),
            used_percent,
            resets_at: timestamp(&config["currentPeriod"]["end"])
                .or_else(|| timestamp(&config["billingPeriodEnd"])),
        })
    } else {
        quota_meter(label, config)
    }
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
}
