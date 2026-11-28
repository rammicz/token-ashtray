use serde_json::{Value, json};
use time::OffsetDateTime;
use token_ashtray::{config::QuotaDisplayMode, providers};

fn parse(payload: &Value) -> Vec<(String, f32, Option<OffsetDateTime>)> {
    providers::all()
        .into_iter()
        .find(|provider| provider.descriptor().id == "cursor")
        .unwrap()
        .parse(payload)
        .unwrap()
        .meters
        .into_iter()
        .map(|meter| (meter.label, meter.used_percent, meter.resets_at))
        .collect()
}

fn fixture() -> Value {
    std::fs::read_to_string("tests/fixtures/cursor-usage.json")
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn plan_auto_and_api_meters_share_the_billing_cycle_reset() {
    let cycle_end = Some(OffsetDateTime::from_unix_timestamp(1_796_000_000).unwrap());
    assert_eq!(
        parse(&fixture()),
        vec![
            ("Reported plan usage".into(), 50.5, cycle_end),
            ("Auto usage".into(), 100.0, cycle_end),
            ("API usage".into(), 0.0, cycle_end),
        ]
    );
    assert_eq!(QuotaDisplayMode::Used.percent_label(50.5), "51%");
}

#[test]
fn auto_and_api_percentages_are_whole_percent_values() {
    let mut payload = fixture();
    payload["planUsage"]["autoPercentUsed"] = json!(1);
    payload["planUsage"]["apiPercentUsed"] = json!(0.5);
    let meters = parse(&payload);
    assert_eq!((meters[1].1, meters[2].1), (1.0, 0.5));
}

#[test]
fn unreported_or_unreadable_meters_are_omitted() {
    for (auto, api) in [(Value::Null, json!("?")), (json!(101), json!(-1))] {
        let mut payload = fixture();
        let plan = payload["planUsage"].as_object_mut().unwrap();
        plan.insert("autoPercentUsed".into(), auto);
        plan.insert("apiPercentUsed".into(), api);
        let labels: Vec<_> = parse(&payload).into_iter().map(|meter| meter.0).collect();
        assert_eq!(labels, ["Reported plan usage"]);
    }
    let mut payload = fixture();
    let plan = payload["planUsage"].as_object_mut().unwrap();
    plan.remove("autoPercentUsed");
    plan.remove("totalPercentUsed");
    let labels: Vec<_> = parse(&payload).into_iter().map(|meter| meter.0).collect();
    assert_eq!(labels, ["API usage"]);
}

fn plan(payload: &Value) -> Option<String> {
    providers::all()
        .into_iter()
        .find(|provider| provider.descriptor().id == "cursor")
        .unwrap()
        .parse(payload)
        .unwrap()
        .plan
}

#[test]
fn plan_label_comes_from_the_nested_plan_info() {
    assert_eq!(plan(&fixture()).as_deref(), Some("Synthetic plan"));
    let mut older = fixture();
    older["_plan_info"] = json!({"planName": "Older shape"});
    assert_eq!(plan(&older).as_deref(), Some("Older shape"));
}

#[test]
fn billing_cycle_reset_resolves_across_primary_and_plan_info_candidates() {
    let expected = Some(OffsetDateTime::from_unix_timestamp(1_796_000_000).unwrap());

    // Fixture without top-level billingCycleEnd falls back to _plan_info.planInfo.billingCycleEnd.
    let mut from_plan_info = fixture();
    from_plan_info
        .as_object_mut()
        .unwrap()
        .remove("billingCycleEnd");
    assert!(
        parse(&from_plan_info)
            .iter()
            .all(|(_, _, reset)| *reset == expected),
        "expected _plan_info.planInfo.billingCycleEnd fallback"
    );

    for (name, payload) in [
        (
            "billingCycleEndMs",
            json!({
                "billingCycleEndMs": 1_796_000_000_000_i64,
                "planUsage": {"totalPercentUsed": 25}
            }),
        ),
        (
            "planUsage.billingCycleEnd",
            json!({
                "planUsage": {
                    "totalPercentUsed": 25,
                    "billingCycleEnd": "1796000000000"
                }
            }),
        ),
        (
            "individualUsage.plan.billingCycleEnd",
            json!({
                "individualUsage": {
                    "plan": {
                        "used": 25,
                        "limit": 100,
                        "billingCycleEnd": "1796000000000"
                    }
                }
            }),
        ),
        (
            "quota_meter fallback reset",
            json!({
                "billingCycleEndMs": 1_796_000_000_000_i64,
                "usage": {"used": 10, "limit": 50}
            }),
        ),
        (
            "numRequests fallback reset",
            json!({
                "billingCycleEndMs": 1_796_000_000_000_i64,
                "gpt-4": {"numRequests": 10, "maxRequestUsage": 50}
            }),
        ),
    ] {
        let meters = parse(&payload);
        assert_eq!(meters.len(), 1, "{name}: {meters:?}");
        assert_eq!(meters[0].2, expected, "{name}");
    }

    // Absent reset stays None (never invents a date).
    let no_reset = parse(&json!({"planUsage": {"totalPercentUsed": 25}}));
    assert_eq!(no_reset.len(), 1);
    assert_eq!(no_reset[0].2, None);

    // Existing endOfMonth / meter-specific reset is preserved over fallback reset.
    let own_reset = Some(OffsetDateTime::from_unix_timestamp(1_795_000_000).unwrap());
    let num_req_with_eom = parse(&json!({
        "endOfMonth": "1795000000000",
        "billingCycleEndMs": 1_796_000_000_000_i64,
        "gpt-4": {"numRequests": 10, "maxRequestUsage": 50}
    }));
    assert_eq!(num_req_with_eom[0].2, own_reset);

    let quota_with_own = parse(&json!({
        "billingCycleEndMs": 1_796_000_000_000_i64,
        "usage": {"used": 10, "limit": 50, "resets_at": "1795000000000"}
    }));
    assert_eq!(quota_with_own[0].2, own_reset);
}

#[test]
fn flat_top_level_percent_fields_parse_without_plan_usage_and_never_duplicate() {
    let expected = Some(OffsetDateTime::from_unix_timestamp(1_796_000_000).unwrap());

    for flat in [
        json!({
            "totalPercentUsed": 40.5,
            "autoPercentUsed": 20,
            "apiPercentUsed": 5,
            "billingCycleEndMs": 1_796_000_000_000_i64
        }),
        json!({
            "planUsage": null,
            "totalPercentUsed": 40.5,
            "autoPercentUsed": 20,
            "apiPercentUsed": 5,
            "billingCycleEndMs": 1_796_000_000_000_i64
        }),
    ] {
        assert_eq!(
            parse(&flat),
            vec![
                ("Reported plan usage".into(), 40.5, expected),
                ("Auto usage".into(), 20.0, expected),
                ("API usage".into(), 5.0, expected),
            ]
        );
    }

    // When planUsage is an object, top-level *PercentUsed must not duplicate or leak.
    let both = json!({
        "billingCycleEnd": "1796000000000",
        "totalPercentUsed": 99,
        "autoPercentUsed": 88,
        "apiPercentUsed": 77,
        "planUsage": {
            "totalPercentUsed": 12
        }
    });
    assert_eq!(
        parse(&both),
        vec![("Reported plan usage".into(), 12.0, expected)]
    );
}
