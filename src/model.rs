use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meter {
    pub label: String,
    pub used_percent: f32,
    pub resets_at: Option<OffsetDateTime>,
}

/// Attribution of used shared credit to a reported product. This is not an
/// independent quota window: it has no reset time and must not affect the
/// provider's remaining quota calculation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductUsage {
    pub label: String,
    pub used_percent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Live,
    Stale,
    Reconnect,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderCard {
    pub id: String,
    pub short_name: String,
    pub plan: Option<String>,
    pub price: Option<String>,
    pub meters: Vec<Meter>,
    /// Optional product-level attribution for a shared meter. Older IPC
    /// snapshots do not include it, so they remain readable.
    #[serde(default)]
    pub usage_breakdown: Vec<ProductUsage>,
    pub status: Status,
    pub detail: String,
    pub link: String,
    pub fetched_at: Option<OffsetDateTime>,
}

impl ProviderCard {
    pub fn effective_status(&self, interval_secs: u64) -> Status {
        if self.status == Status::Live
            && self.fetched_at.is_some_and(|at| {
                (OffsetDateTime::now_utc() - at).whole_seconds() > interval_secs as i64
            })
        {
            Status::Stale
        } else {
            self.status
        }
    }

    /// The tightest known window: the meter with the highest used share.
    pub fn tightest_meter(&self) -> Option<&Meter> {
        self.meters
            .iter()
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    }

    /// The used share of the tightest known window, exactly as its meter shows it.
    pub fn used(&self) -> Option<f32> {
        (self.status == Status::Live)
            .then(|| self.tightest_meter().map(|meter| meter.used_percent))
            .flatten()
    }

    pub fn remaining(&self) -> Option<f32> {
        // The tightest known window determines how much usable quota remains.
        self.used().map(|used| 100.0 - used)
    }
}

pub fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

/// For schemas explicitly expressed in percent (e.g. Claude utilization).
pub fn percent100(value: &Value) -> Option<f32> {
    number(value)
        .filter(|n| (0.0..=100.0).contains(n))
        .map(|n| n as f32)
}

/// For legacy schemas accepting either a fraction or percent. Explicit percent
/// fields must use percent100: the value 1 in those fields means one percent.
pub fn percent(value: &Value) -> Option<f32> {
    let n = number(value)?;
    percent100(&Value::from(if (0.0..=1.0).contains(&n) {
        n * 100.0
    } else {
        n
    }))
}

pub fn timestamp(value: &Value) -> Option<OffsetDateTime> {
    if value.is_null() || value.as_str() == Some("") {
        return None;
    }
    if let Some(seconds) = value.get("seconds") {
        return number(seconds).and_then(|n| OffsetDateTime::from_unix_timestamp(n as i64).ok());
    }
    if let Some(n) = number(value) {
        let seconds = if n.abs() >= 1e12 { n / 1000.0 } else { n };
        return OffsetDateTime::from_unix_timestamp(seconds as i64).ok();
    }
    OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok()
}

/// Whole-percent label for a used share. Ties round half up (50.5 shows as
/// 51%). A positive share that rounds to 0 shows "<1%".
pub fn percentage_label(percent: f32) -> String {
    whole_percent_label(percent, percent.round())
}

/// `whole` is the rounded value to show for the `exact` share.
pub(crate) fn whole_percent_label(exact: f32, whole: f32) -> String {
    let whole = whole.clamp(0.0, 100.0);
    if exact > 0.0 && whole == 0.0 {
        "<1%".into()
    } else {
        format!("{whole:.0}%")
    }
}

pub fn duration_label(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 86400 {
        format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600)
    } else if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_is_never_zero_and_known_scales_are_unambiguous() {
        for v in [json!(null), json!("?"), json!(-1), json!(101)] {
            assert_eq!(percent(&v), None);
        }
        assert_eq!(percent(&json!("0.42")), Some(42.0));
        assert_eq!(percent(&json!(42)), Some(42.0));
        assert_eq!(percent100(&json!(1)), Some(1.0));
        assert_eq!(percent(&json!(0)), Some(0.0));
    }

    #[test]
    fn percentage_label_rounds_half_up_like_provider_web_views() {
        for (percent, expected) in [
            (50.5, "51%"),
            (49.5, "50%"),
            (0.5, "1%"),
            (99.5, "100%"),
            (100.0, "100%"),
            (50.4, "50%"),
            (0.4, "<1%"),
            (0.0, "0%"),
        ] {
            assert_eq!(percentage_label(percent), expected, "{percent}");
        }
    }

    #[test]
    fn timestamp_shapes_agree() {
        let expected = timestamp(&json!(1770000000));
        for v in [
            json!({"seconds":"1770000000"}),
            json!(1770000000000_i64),
            json!("2026-02-02T02:40:00Z"),
        ] {
            assert_eq!(timestamp(&v), expected);
        }
        assert_eq!(timestamp(&json!("")), None);
        assert_eq!(timestamp(&json!({"seconds":"bad"})), None);
    }

    #[test]
    fn old_ipc_snapshot_defaults_missing_product_breakdown() {
        let card: ProviderCard = serde_json::from_value(json!({
            "id": "grok",
            "short_name": "Grok",
            "plan": null,
            "price": null,
            "meters": [],
            "status": "Live",
            "detail": "",
            "link": "https://grok.com/",
            "fetched_at": null
        }))
        .unwrap();
        assert!(card.usage_breakdown.is_empty());
    }

    #[test]
    fn product_attribution_does_not_change_shared_quota_remaining() {
        let card = ProviderCard {
            id: "grok".into(),
            short_name: "Grok".into(),
            plan: None,
            price: None,
            meters: vec![Meter {
                label: "Weekly".into(),
                used_percent: 72.0,
                resets_at: None,
            }],
            usage_breakdown: vec![
                ProductUsage {
                    label: "Build".into(),
                    used_percent: 70.0,
                },
                ProductUsage {
                    label: "GrokChat".into(),
                    used_percent: 1.0,
                },
            ],
            status: Status::Live,
            detail: String::new(),
            link: String::new(),
            fetched_at: None,
        };
        assert_eq!(card.remaining(), Some(28.0));
    }
}
