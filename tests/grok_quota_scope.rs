use token_ashtray::providers;

#[test]
fn shared_grok_pool_names_chat_and_build_as_attribution() {
    let provider = providers::all()
        .into_iter()
        .find(|provider| provider.descriptor().id == "grok")
        .unwrap();
    let fixture = std::fs::read_to_string("tests/fixtures/grok-products.json").unwrap();
    let usage = provider.parse(&fixture.parse().unwrap()).unwrap();
    assert_eq!(usage.meters.len(), 1);
    assert_eq!(usage.meters[0].used_percent, 72.0);
    assert_eq!(usage.meters[0].label, "Weekly shared credits");
    assert_eq!(
        usage.meters[0].resets_at.unwrap().unix_timestamp_nanos(),
        1_788_687_890_392_288_000
    );
    let attribution: Vec<_> = usage
        .usage_breakdown
        .iter()
        .map(|product| (product.label.as_str(), product.used_percent))
        .collect();
    assert!(attribution.contains(&("Grok Chat", 1.0)), "{attribution:?}");
    assert!(
        attribution.contains(&("Grok Build", 70.0)),
        "{attribution:?}"
    );
}
