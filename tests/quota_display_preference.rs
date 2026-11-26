use std::fs;
use token_ashtray::config::{ConfigStore, QuotaDisplayMode};

#[test]
fn saved_quota_display_mode_is_read_into_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[board]\nquota_display_mode = 'used'\n").unwrap();
    let store = ConfigStore::load_from(path);
    let board = serde_json::to_value(&store.preferences().board).unwrap();
    assert_eq!(board["quota_display_mode"].as_str(), Some("used"));
}

#[test]
fn older_config_defaults_and_upgrade_reload_preserves_saved_choice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert_eq!(
        ConfigStore::load_from(path.clone())
            .preferences()
            .board
            .quota_display_mode,
        QuotaDisplayMode::Remaining
    );
    fs::write(&path, "future = 9\n[board]\norder = ['codex', 'claude']\nrefresh_secs = 420\nfuture_board = 'kept'\n[[panels]]\nid = 0\npinned = true\nfuture_panel = 'kept'\n").unwrap();
    let mut running_version = ConfigStore::load_from(path.clone());
    assert_eq!(
        running_version.preferences().board.quota_display_mode,
        QuotaDisplayMode::Remaining
    );
    running_version.preferences_mut().board.quota_display_mode = QuotaDisplayMode::Used;
    running_version.save().unwrap();
    drop(running_version);
    // A replacement executable reloads this same durable per-user file. Native
    // executable replacement is checked separately; no process is started here.
    let mut replacement_version = ConfigStore::load_from(path.clone());
    assert_eq!(
        replacement_version.preferences().board.quota_display_mode,
        QuotaDisplayMode::Used
    );
    assert_eq!(
        replacement_version.preferences().board.order,
        ["codex", "claude"]
    );
    assert_eq!(replacement_version.preferences().board.refresh_secs, 420);
    assert!(replacement_version.preferences().window.pinned);
    replacement_version.save().unwrap();
    let document = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(document["future"].as_integer(), Some(9));
    assert_eq!(document["board"]["future_board"].as_str(), Some("kept"));
    assert_eq!(
        document["board"]["quota_display_mode"].as_str(),
        Some("used")
    );
    assert!(document.to_string().contains("future_panel = 'kept'"));
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "atomic save leaves no temporary files"
    );
}

#[test]
fn invalid_mode_defaults_only_that_field_and_cannot_resurrect_on_save() {
    for invalid in [
        "'legacy-typo'",
        "42",
        "true",
        "['used']",
        "{ future = 'mode' }",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, format!("future = 7\n[board]\nquota_display_mode = {invalid}\norder = ['codex', 'claude']\nhidden = ['cursor']\nprimary = 'claude'\nrefresh_secs = 420\nfuture_board = 'kept'\n[[panels]]\nid = 0\nx = 123.0\npinned = true\nfuture_panel = 'kept'\n")).unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        let board = &store.preferences().board;
        assert_eq!(
            board.quota_display_mode,
            QuotaDisplayMode::Remaining,
            "{invalid}"
        );
        assert_eq!(board.order, ["codex", "claude"], "{invalid}");
        assert!(board.hidden.contains("cursor"));
        assert_eq!(board.primary.as_deref(), Some("claude"));
        assert_eq!(board.refresh_secs, 420);
        assert_eq!(store.preferences().window.x, Some(123.0));
        assert!(store.preferences().window.pinned);
        store.save().unwrap();
        let saved = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(
            saved["board"]["quota_display_mode"].as_str(),
            Some("remaining")
        );
        assert_eq!(saved["board"]["future_board"].as_str(), Some("kept"));
        assert_eq!(saved["future"].as_integer(), Some(7));
        assert!(saved.to_string().contains("future_panel = 'kept'"));
        let reloaded = ConfigStore::load_from(path);
        assert_eq!(reloaded.preferences().board, store.preferences().board);
        assert_eq!(reloaded.preferences().window, store.preferences().window);
    }
}

#[test]
fn enum_and_fraction_boundaries_keep_the_selected_meaning() {
    for (used, remaining_text, used_text) in [
        (25.0, "75%", "25%"),
        (0.0, "100%", "0%"),
        (100.0, "0%", "100%"),
        (0.4, "100%", "<1%"),
        (99.6, "<1%", "100%"),
    ] {
        assert_eq!(
            QuotaDisplayMode::Remaining.percent_label(used),
            remaining_text
        );
        assert_eq!(QuotaDisplayMode::Used.percent_label(used), used_text);
    }
    assert_eq!(
        serde_json::to_string(&QuotaDisplayMode::Remaining).unwrap(),
        "\"remaining\""
    );
    assert_eq!(
        serde_json::to_string(&QuotaDisplayMode::Used).unwrap(),
        "\"used\""
    );
}
