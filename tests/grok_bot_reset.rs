use serde_json::{Value, json};
use time::OffsetDateTime;
use token_ashtray::providers;

fn grok_bot() -> std::sync::Arc<dyn providers::Provider> {
    providers::all()
        .into_iter()
        .find(|provider| provider.descriptor().id == "grok-bot")
        .unwrap()
}

fn fixture() -> Value {
    std::fs::read_to_string("tests/fixtures/grok-bot-sand.json")
        .unwrap()
        .parse()
        .unwrap()
}

// 2026-11-29T15:51:00Z, the synthetic weekly reset in the fixture.
fn weekly_reset() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_795_967_460).unwrap()
}

fn reset_of(payload: &Value) -> Option<OffsetDateTime> {
    let usage = grok_bot().parse(payload).unwrap();
    assert_eq!(usage.meters.len(), 1, "{:?}", usage.meters);
    usage.meters[0].resets_at
}

#[test]
fn legacy_bot_allowance_keeps_the_reported_weekly_reset() {
    let usage = grok_bot().parse(&fixture()).unwrap();
    assert_eq!(usage.meters.len(), 1, "{:?}", usage.meters);
    assert_eq!(usage.meters[0].label, "Grok Bot");
    assert_eq!(usage.meters[0].used_percent, 47.25);
    assert_eq!(usage.meters[0].resets_at, Some(weekly_reset()));
}

#[test]
fn usage_percent_is_already_a_percentage() {
    for (reported, expected) in [(0.6, 0.6), (1.0, 1.0), (47.3, 47.3)] {
        let mut payload = fixture();
        payload["usagePercent"] = json!(reported);
        let usage = grok_bot().parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1, "{:?}", usage.meters);
        assert_eq!(usage.meters[0].used_percent, expected, "{reported}");
    }
}

#[test]
fn utc_reset_string_without_offset_is_read_as_utc() {
    let fraction = weekly_reset().replace_millisecond(250).unwrap();
    for (reset, expected) in [
        ("2026-11-29T15:51:00", weekly_reset()),
        ("2026-11-29 15:51:00", weekly_reset()),
        ("2026-11-29T15:51:00.250", fraction),
        ("2026-11-29T16:51:00+01:00", weekly_reset()),
    ] {
        let mut payload = fixture();
        payload["nextResetTimestampUtc"] = json!(reset);
        assert_eq!(reset_of(&payload), Some(expected), "{reset}");
    }
}

#[test]
fn reported_reset_also_fills_a_used_and_limit_meter() {
    let payload = json!({
        "used": 47,
        "limit": 100,
        "nextResetTimestampUtc": "2026-11-29T15:51:00Z"
    });
    assert_eq!(reset_of(&payload), Some(weekly_reset()));
}

#[test]
fn missing_or_unreadable_reset_falls_back_or_stays_unknown() {
    let mut payload = fixture();
    payload["nextResetTimestampUtc"] = json!("not a time");
    payload["resetTime"] = json!({"seconds": 1_770_000_000});
    assert_eq!(
        reset_of(&payload),
        Some(OffsetDateTime::from_unix_timestamp(1_770_000_000).unwrap())
    );
    for absent in [json!(null), json!(""), json!("not a time")] {
        let mut payload = fixture();
        payload["nextResetTimestampUtc"] = absent;
        assert_eq!(reset_of(&payload), None);
    }
    // Older alias keys name no time zone, so an offset-less time stays unknown.
    let mut payload = fixture();
    payload
        .as_object_mut()
        .unwrap()
        .remove("nextResetTimestampUtc");
    payload["resetTime"] = json!("2026-11-29T15:51:00");
    assert_eq!(reset_of(&payload), None);
}

#[test]
fn unwraps_nested_object_envelopes_and_ignores_scalar_status() {
    for key in ["usageStatus", "sandUsageStatus", "status", "config"] {
        let payload = json!({
            key: {
                "usagePercent": 42.5,
                "nextResetTimestampUtc": "2026-11-29T15:51:00Z"
            }
        });
        let usage = grok_bot().parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1, "{key}");
        assert_eq!(usage.meters[0].label, "Grok Bot", "{key}");
        assert_eq!(usage.meters[0].used_percent, 42.5, "{key}");
        assert_eq!(usage.meters[0].resets_at, Some(weekly_reset()), "{key}");
    }

    // Scalar "status": "ok" must not swallow top-level fields.
    let scalar_status = json!({
        "status": "ok",
        "usagePercent": 42.5,
        "nextResetTimestampUtc": "2026-11-29T15:51:00Z"
    });
    let usage = grok_bot().parse(&scalar_status).unwrap();
    assert_eq!(usage.meters.len(), 1);
    assert_eq!(usage.meters[0].used_percent, 42.5);
    assert_eq!(usage.meters[0].resets_at, Some(weekly_reset()));

    // Outer reset still fills an envelope that only reports usagePercent.
    let split_reset = json!({
        "usageStatus": {"usagePercent": 42.5},
        "nextResetTimestampUtc": "2026-11-29T15:51:00Z"
    });
    assert_eq!(reset_of(&split_reset), Some(weekly_reset()));

    // An unrelated nested object under config/status does not hide top-level usagePercent.
    let unrelated_config = json!({
        "config": {"featureEnabled": true},
        "usagePercent": 42.5,
        "nextResetTimestampUtc": "2026-11-29T15:51:00Z"
    });
    let usage = grok_bot().parse(&unrelated_config).unwrap();
    assert_eq!(usage.meters.len(), 1);
    assert_eq!(usage.meters[0].used_percent, 42.5);
    assert_eq!(usage.meters[0].resets_at, Some(weekly_reset()));
}

#[test]
fn snake_case_percent_and_utc_reset_preserve_scale_and_offset_rules() {
    for (reported, expected) in [(0.6, 0.6), (1.0, 1.0), (47.3, 47.3)] {
        let payload = json!({
            "usage_percent": reported,
            "next_reset_timestamp_utc": "2026-11-29 15:51:00"
        });
        let usage = grok_bot().parse(&payload).unwrap();
        assert_eq!(usage.meters.len(), 1, "{reported}");
        assert_eq!(usage.meters[0].used_percent, expected, "{reported}");
        assert_eq!(
            usage.meters[0].resets_at,
            Some(weekly_reset()),
            "{reported}"
        );
    }

    let wrapped_snake = json!({
        "sandUsageStatus": {
            "usage_percent": 0.6,
            "next_reset_timestamp_utc": "2026-11-29T15:51:00"
        }
    });
    let usage = grok_bot().parse(&wrapped_snake).unwrap();
    assert_eq!(usage.meters[0].used_percent, 0.6);
    assert_eq!(usage.meters[0].resets_at, Some(weekly_reset()));
}

#[test]
fn pooled_enterprise_or_zero_included_limit_omits_meters() {
    for payload in [
        json!({"usesPooledEnterpriseAllowance": true, "usagePercent": 47.25}),
        json!({"uses_pooled_enterprise_allowance": true, "usage_percent": 47.25}),
        json!({"includedLimitZero": true, "usagePercent": 0}),
        json!({"included_limit_zero": true, "usage_percent": 0}),
        json!({"usageStatus": {"usesPooledEnterpriseAllowance": true, "usagePercent": 47.25}}),
        json!({"sandUsageStatus": {"included_limit_zero": true, "usage_percent": 15}}),
        json!({"usesPooledEnterpriseAllowance": true, "usageStatus": {"usagePercent": 47.25}}),
    ] {
        let err = grok_bot().parse(&payload).err().unwrap_or_else(|| {
            panic!("expected suppressed payload to omit meters: {payload}");
        });
        assert!(
            err.message.contains("no recognised quota meters"),
            "{payload}: {}",
            err.message
        );
    }

    // Explicit false suppression flags and hasNonZeroIncludedLimit: false do NOT suppress.
    let not_suppressed = json!({
        "usesPooledEnterpriseAllowance": false,
        "includedLimitZero": false,
        "hasNonZeroIncludedLimit": false,
        "usagePercent": 47.25
    });
    let usage = grok_bot().parse(&not_suppressed).unwrap();
    assert_eq!(usage.meters[0].used_percent, 47.25);
}
