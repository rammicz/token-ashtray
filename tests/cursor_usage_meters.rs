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
