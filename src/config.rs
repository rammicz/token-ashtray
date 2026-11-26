//! Durable, deliberately boring user preferences. Provider credentials never
//! belong here; this file only describes how the local board is presented.

use std::{collections::BTreeSet, fs, io, path::PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;

const QUALIFIER: &str = "cz";
const ORGANIZATION: &str = "rammi";
const APPLICATION: &str = "token-ashtray";
const LOAD_FAILURE: &str = "Settings could not be loaded, so changes are not being saved. Repair or move aside the settings file, then restart Token Ashtray.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDisplayMode {
    #[default]
    Remaining,
    Used,
}

impl QuotaDisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Remaining => "Remaining",
            Self::Used => "Used",
        }
    }

    pub fn percent(self, used: f32) -> f32 {
        match self {
            Self::Remaining => 100.0 - used,
            Self::Used => used,
        }
        .clamp(0.0, 100.0)
    }

    /// The whole-percent label for this mode. Ties round half up on the used
    /// share and Remaining is its complement, so both modes add up to 100.
    pub fn percent_label(self, used: f32) -> String {
        let used = used.clamp(0.0, 100.0);
        match self {
            Self::Remaining => {
                crate::model::whole_percent_label(100.0 - used, 100.0 - used.round())
            }
            Self::Used => crate::model::percentage_label(used),
        }
    }
}

/// A single appearance preference shared by every UI window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Manual card-column choice for the board grid; `Auto` keeps the
/// width-based default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnLayout {
    #[default]
    Auto,
    One,
    Two,
    Three,
}

impl ColumnLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::One => "1 column",
            Self::Two => "2 columns",
            Self::Three => "3 columns",
        }
    }
}

// A malformed field from a future version must not discard the other settings.
fn column_layout<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ColumnLayout, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn theme_preference<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ThemePreference, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn tolerant_bool<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(false))
}

// A malformed field from a future version must not discard the other settings.
fn quota_display_mode<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<QuotaDisplayMode, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardPreferences {
    pub order: Vec<String>,
    pub hidden: BTreeSet<String>,
    pub primary: Option<String>,
    pub refresh_secs: u64,
    #[serde(deserialize_with = "quota_display_mode")]
    pub quota_display_mode: QuotaDisplayMode,
    #[serde(deserialize_with = "column_layout")]
    pub columns: ColumnLayout,
    #[serde(deserialize_with = "theme_preference")]
    pub theme_preference: ThemePreference,
    #[serde(deserialize_with = "tolerant_bool")]
    pub author_youtube_visited: bool,
    /// Opt-in for automatic update checks; off by default. The update trust
    /// floors never live here, because the host replaces these preferences
    /// with the UI child's copy.
    #[serde(deserialize_with = "tolerant_bool")]
    pub updates_enabled: bool,
}

impl Default for BoardPreferences {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            hidden: BTreeSet::new(),
            primary: None,
            refresh_secs: 300,
            quota_display_mode: QuotaDisplayMode::Remaining,
            columns: ColumnLayout::Auto,
            theme_preference: ThemePreference::System,
            author_youtube_visited: false,
            updates_enabled: false,
        }
    }
}

impl BoardPreferences {
    /// Only scheduling inputs notify the poller; presentation edits use cached data.
    pub fn replace(&mut self, next: Self, update_polling: impl FnOnce(&Self)) -> bool {
        let changed = *self != next;
        if self.refresh_secs != next.refresh_secs || self.hidden != next.hidden {
            update_polling(&next);
        }
        *self = next;
        changed
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPreferences {
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// Native display scale of `x` and `y`: their physical virtual-desktop
    /// position is `x * position_scale`. Earlier releases did not record it
    /// and are read at the board's current scale, as they were then.
    pub position_scale: Option<f32>,
    pub width: f32,
    pub height: f32,
    /// Client size chosen before a work-area fit reduced `width` and `height`.
    /// Later fits restore it when the area allows; a user resize replaces it.
    pub preferred_size: Option<[f32; 2]>,
    pub pinned: bool,
    pub compact: bool,
}

impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            position_scale: None,
            width: 440.0,
            height: 520.0,
            preferred_size: None,
            pinned: false,
            compact: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub board: BoardPreferences,
    pub window: WindowPreferences,
}

/// The app's per-user directories.
pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Keeps the parsed document as well as its typed representation, so a newer
/// release's unknown keys survive a save by an older binary.
pub struct ConfigStore {
    path: PathBuf,
    document: DocumentMut,
    preferences: Preferences,
    save_blocked: bool,
}

impl ConfigStore {
    pub fn load() -> Self {
        let path = project_dirs()
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("token-ashtray.toml"));
        Self::load_from(path)
    }

    pub fn load_from(path: PathBuf) -> Self {
        let loaded = (|| {
            let source = match fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                Err(_) => return None,
            };
            let mut document = source.parse::<DocumentMut>().ok()?;
            migrate_legacy_panels(&mut document);
            let preferences: Preferences = toml_edit::de::from_document(document.clone()).ok()?;
            Some((document, preferences))
        })();
        let save_blocked = loaded.is_none();
        let (document, preferences) = loaded.unwrap_or_default();
        if save_blocked {
            // Parser and I/O errors can contain user paths, source lines or values.
            eprintln!("{LOAD_FAILURE}");
        }
        Self {
            path,
            document,
            preferences,
            save_blocked,
        }
    }

    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }
    pub fn preferences_mut(&mut self) -> &mut Preferences {
        &mut self.preferences
    }

    pub fn save(&mut self) -> io::Result<()> {
        // Keep the guard even if the file is repaired while old defaults are in memory.
        if self.save_blocked {
            return Err(io::Error::new(io::ErrorKind::InvalidData, LOAD_FAILURE));
        }
        let mut fresh = toml_edit::ser::to_document(&self.preferences)
            .map_err(|error| io::Error::other(error.to_string()))?;
        merge_unknown(self.document.as_table(), fresh.as_table_mut());
        self.document = fresh;
        crate::fsutil::replace_atomically(
            &self.path,
            self.document.to_string().as_bytes(),
            None,
            || Ok(()),
        )
    }
}

// Legacy quota windows are read once. Only ID 0 can supply the main board.
// Unknown detached fields remain opaque under `panels`, associated with their
// original ID; no runtime preference or geometry is read from those entries.
fn migrate_legacy_panels(document: &mut DocumentMut) {
    let Some(legacy) = document
        .get("panels")
        .and_then(|item| item.clone().into_value().ok())
    else {
        return;
    };
    let Some(panels) = legacy.as_array() else {
        return;
    };
    let mut opaque = toml_edit::Array::new();
    for panel in panels.iter().filter_map(toml_edit::Value::as_inline_table) {
        let id = panel.get("id").and_then(toml_edit::Value::as_integer);
        if id == Some(0) && !document.contains_key("window") {
            let mut window = panel.clone();
            window.remove("id");
            window.remove("hidden");
            document.insert("window", toml_edit::value(window));
        } else if id == Some(0) {
            if let Some(mut current) = document
                .get("window")
                .and_then(|item| item.clone().into_value().ok())
                && let Some(window) = current.as_inline_table_mut()
            {
                merge_extra_fields(
                    panel,
                    window,
                    &[
                        "id", "x", "y", "width", "height", "pinned", "compact", "hidden",
                    ],
                );
                document.insert("window", toml_edit::value(current));
            }
        } else {
            let mut extra = panel.clone();
            for key in ["x", "y", "width", "height", "pinned", "compact", "hidden"] {
                extra.remove(key);
            }
            if extra.iter().any(|(key, _)| key != "id") {
                opaque.push(extra);
            }
        }
    }
    document.remove("panels");
    if !opaque.is_empty() {
        document.insert("panels", toml_edit::value(opaque));
    }
}

fn merge_unknown(old: &toml_edit::Table, new: &mut toml_edit::Table) {
    // Optional known fields (primary, x, y) disappear from TOML when cleared.
    // Their absence is intentional; only fields outside the schema survive.
    for (key, value) in old.iter() {
        if !matches!(key, "board" | "window") && !new.contains_key(key) {
            new.insert(key, value.clone());
        }
    }
    if let (Some(old_board), Some(new_board)) = (old.get("board"), new.get_mut("board"))
        && let (Ok(old_value), Some(new_table)) = (
            old_board.clone().into_value(),
            new_board
                .as_value_mut()
                .and_then(toml_edit::Value::as_inline_table_mut),
        )
        && let Some(old_table) = old_value.as_inline_table()
    {
        merge_extra_fields(
            old_table,
            new_table,
            &[
                "order",
                "hidden",
                "primary",
                "refresh_secs",
                "quota_display_mode",
                "theme_preference",
                "author_youtube_visited",
                "updates_enabled",
            ],
        );
    }
    if let (Some(old_window), Some(new_window)) = (old.get("window"), new.get_mut("window"))
        && let (Ok(old_value), Some(new_table)) = (
            old_window.clone().into_value(),
            new_window
                .as_value_mut()
                .and_then(toml_edit::Value::as_inline_table_mut),
        )
        && let Some(old_table) = old_value.as_inline_table()
    {
        merge_extra_fields(
            old_table,
            new_table,
            &[
                "x",
                "y",
                "position_scale",
                "width",
                "height",
                "preferred_size",
                "pinned",
                "compact",
            ],
        );
    }
}

fn merge_extra_fields(
    old: &toml_edit::InlineTable,
    new: &mut toml_edit::InlineTable,
    known: &[&str],
) {
    for (key, value) in old.iter() {
        if !known.contains(&key) && !new.contains_key(key) {
            new.insert(key, value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_columns_default_to_auto_and_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        // A config written before the field existed loads as Auto.
        fs::write(&path, "[board]\nrefresh_secs = 720\n").unwrap();
        let store = ConfigStore::load_from(path.clone());
        assert_eq!(store.preferences().board.columns, ColumnLayout::Auto);
        assert_eq!(store.preferences().board.refresh_secs, 720);
        // A manual choice persists; an unknown future value falls back to Auto.
        let mut store = ConfigStore::load_from(path.clone());
        store.preferences_mut().board.columns = ColumnLayout::Three;
        store.save().unwrap();
        assert_eq!(
            ConfigStore::load_from(path.clone())
                .preferences()
                .board
                .columns,
            ColumnLayout::Three
        );
        fs::write(&path, "[board]\ncolumns = 'seventeen'\n").unwrap();
        assert_eq!(
            ConfigStore::load_from(path).preferences().board.columns,
            ColumnLayout::Auto
        );
    }

    #[test]
    fn legacy_config_loads_only_main_board() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "future = 7\n[board]\nhidden = ['grok']\nquota_display_mode = 'used'\norder = ['codex', 'claude']\nprimary = 'claude'\nrefresh_secs = 720\nfuture_board = 'kept'\n[[panels]]\nid = 8\npinned = false\ncustom = 'eight'\n[[panels]]\nid = 0\nx = 80.0\ny = 40.0\nwidth = 620.0\nheight = 700.0\npinned = true\ncompact = true\nhidden = ['codex']\nfuture_window = 'kept'\n[[panels]]\nid = 9\ncustom = 'nine'\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        let window = &store.preferences().window;
        assert_eq!((window.x, window.y), (Some(80.0), Some(40.0)));
        assert_eq!((window.width, window.height), (620.0, 700.0));
        assert!(window.pinned && window.compact);
        assert!(store.preferences().board.hidden.contains("grok"));
        assert!(!store.preferences().board.hidden.contains("codex"));
        store.preferences_mut().window.x = None;
        store.preferences_mut().window.y = None;
        for _ in 0..2 {
            store.save().unwrap();
            store = ConfigStore::load_from(path.clone());
            let prefs = store.preferences();
            assert_eq!((prefs.window.x, prefs.window.y), (None, None));
            assert_eq!((prefs.window.width, prefs.window.height), (620.0, 700.0));
            assert!(prefs.window.pinned && prefs.window.compact);
            assert_eq!(prefs.board.order, ["codex", "claude"]);
            assert_eq!(prefs.board.primary.as_deref(), Some("claude"));
            assert_eq!(prefs.board.refresh_secs, 720);
            assert_eq!(prefs.board.quota_display_mode, QuotaDisplayMode::Used);
            let saved = fs::read_to_string(&path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(saved["future"].as_integer(), Some(7));
            assert_eq!(saved["board"]["future_board"].as_str(), Some("kept"));
            assert_eq!(saved["window"]["future_window"].as_str(), Some("kept"));
            let opaque = saved["panels"].as_array().unwrap();
            assert_eq!(opaque.len(), 2);
            for (entry, (id, custom)) in opaque.iter().zip([(8, "eight"), (9, "nine")]) {
                let entry = entry.as_inline_table().unwrap();
                assert_eq!(entry.len(), 2, "only identity and unknown data survive");
                assert_eq!(entry.get("id").unwrap().as_integer(), Some(id));
                assert_eq!(entry.get("custom").unwrap().as_str(), Some(custom));
            }
        }
    }

    #[test]
    fn window_position_scale_and_preferred_size_round_trip_and_clear() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[window]\nx = 1720.5\ny = 28.5\nwidth = 592.0\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        assert_eq!(store.preferences().window.position_scale, None);
        assert_eq!(store.preferences().window.preferred_size, None);
        store.preferences_mut().window.position_scale = Some(2.0);
        store.preferences_mut().window.preferred_size = Some([1100.0, 1500.0]);
        store.save().unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        assert_eq!(store.preferences().window.position_scale, Some(2.0));
        assert_eq!(
            store.preferences().window.preferred_size,
            Some([1100.0, 1500.0])
        );
        store.preferences_mut().window.preferred_size = None;
        store.save().unwrap();
        let restored = ConfigStore::load_from(path);
        assert_eq!(restored.preferences().window.preferred_size, None);
        assert_eq!(restored.preferences().window.position_scale, Some(2.0));
    }

    #[test]
    fn detached_only_legacy_config_uses_default_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        for source in [
            "panels = [{ id = 2, x = 900.0, pinned = true, width = 900.0 }]\n",
            "panels = [{ x = 900.0, pinned = true, width = 900.0 }]\n",
        ] {
            fs::write(&path, source).unwrap();
            let mut store = ConfigStore::load_from(path.clone());
            store.save().unwrap();
            let restored = ConfigStore::load_from(path.clone());
            assert_eq!(restored.preferences().window, WindowPreferences::default());
        }
    }

    #[test]
    fn explicit_window_precedes_legacy_geometry_and_keeps_unknown_main_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "window = { width = 800.0, compact = true }\npanels = [{ id = 0, x = 99.0, y = 88.0, width = 900.0, future = 'keep' }]\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        for _ in 0..2 {
            assert_eq!(
                (store.preferences().window.x, store.preferences().window.y),
                (None, None)
            );
            assert_eq!(store.preferences().window.width, 800.0);
            assert!(store.preferences().window.compact);
            store.save().unwrap();
            let saved = fs::read_to_string(&path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(saved["window"]["future"].as_str(), Some("keep"));
            store = ConfigStore::load_from(path.clone());
        }
    }

    #[test]
    fn corrupt_or_empty_config_is_safe() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[board\n").unwrap();
        let store = ConfigStore::load_from(path);
        assert_eq!(store.preferences().board.refresh_secs, 300);
    }

    #[test]
    fn invalid_existing_config_rejects_repeated_saves_without_exposing_contents() {
        for source in [
            b"private_marker = 'SYNTHETIC_PRIVATE_VALUE'\n[board\n".as_slice(),
            b"private_marker = 'SYNTHETIC_PRIVATE_VALUE'\nwindow = { width = 610.0, pinned = true }\n[board]\nrefresh_secs = 'SYNTHETIC_PRIVATE_VALUE'\norder = ['codex', 'claude']\nquota_display_mode = 'used'\n".as_slice(),
            b"\xffSYNTHETIC_PRIVATE_VALUE".as_slice(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("private-user-path.toml");
            fs::write(&path, source).unwrap();
            let mut store = ConfigStore::load_from(path.clone());
            assert_eq!(fs::read(&path).unwrap(), source);
            for theme in [ThemePreference::Dark, ThemePreference::Light] {
                store.preferences_mut().board.theme_preference = theme;
                let error = store.save().expect_err("failed load must block ordinary saves");
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                let message = error.to_string();
                assert!(message.contains("Repair") && message.contains("restart"));
                assert!(!message.contains("SYNTHETIC_PRIVATE_VALUE"));
                assert!(!message.contains("private-user-path"));
                assert!(!message.contains(directory.path().to_str().unwrap()));
                assert_eq!(fs::read(&path).unwrap(), source);
            }
        }
    }

    #[test]
    fn missing_and_empty_config_remain_writable() {
        for existing_empty in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            if existing_empty {
                fs::write(&path, "").unwrap();
            }
            let mut store = ConfigStore::load_from(path.clone());
            assert_eq!(path.exists(), existing_empty, "load must not create a file");
            for theme in [ThemePreference::Dark, ThemePreference::Light] {
                store.preferences_mut().board.theme_preference = theme;
                store.save().unwrap();
                let reopened = ConfigStore::load_from(path.clone());
                assert_eq!(reopened.preferences().board.theme_preference, theme);
            }
        }
    }

    #[test]
    fn read_error_stays_guarded_until_deliberate_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::create_dir(&path).unwrap();
        let canary = path.join("canary");
        fs::write(&canary, b"SYNTHETIC_PRIVATE_VALUE").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        for _ in 0..2 {
            store.preferences_mut().board.theme_preference = ThemePreference::Light;
            assert_eq!(store.save().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read(&canary).unwrap(), b"SYNTHETIC_PRIVATE_VALUE");
        }
        fs::remove_file(canary).unwrap();
        fs::remove_dir(&path).unwrap();
        let repaired = "[board]\nrefresh_secs = 720\n";
        fs::write(&path, repaired).unwrap();
        assert!(
            store.save().is_err(),
            "repair must not silently unlock old defaults"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), repaired);
        let mut reopened = ConfigStore::load_from(path.clone());
        assert_eq!(reopened.preferences().board.refresh_secs, 720);
        reopened.preferences_mut().board.theme_preference = ThemePreference::Dark;
        reopened.save().unwrap();
        assert_eq!(
            ConfigStore::load_from(path)
                .preferences()
                .board
                .refresh_secs,
            720
        );
    }

    #[test]
    fn tolerant_enum_defaults_do_not_block_other_valid_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "future = 7\n[board]\nquota_display_mode = 42\ntheme_preference = 'future_theme'\nrefresh_secs = 720\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        assert_eq!(store.preferences().board.refresh_secs, 720);
        assert_eq!(
            store.preferences().board.quota_display_mode,
            QuotaDisplayMode::Remaining
        );
        assert_eq!(
            store.preferences().board.theme_preference,
            ThemePreference::System
        );
        store.save().unwrap();
        let restored = ConfigStore::load_from(path.clone());
        assert_eq!(restored.preferences().board.refresh_secs, 720);
        assert_eq!(
            fs::read_to_string(path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap()["future"]
                .as_integer(),
            Some(7)
        );
    }

    #[test]
    fn rewriting_keeps_unknown_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "future = 7\n[board]\nfuture_board = 'kept'\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        store.preferences_mut().board.primary = Some("claude".into());
        store.save().unwrap();
        let saved = fs::read_to_string(path).unwrap();
        assert!(saved.contains("future = 7"));
        assert!(saved.contains("future_board = 'kept'"));
    }

    #[test]
    fn rewriting_preserves_future_fields_in_array_of_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[[panels]]\nid = 0\nfuture_panel = 'kept'\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        store.preferences_mut().window.pinned = true;
        store.save().unwrap();
        let saved = fs::read_to_string(path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let panel = saved["window"]
            .as_value()
            .unwrap()
            .as_inline_table()
            .unwrap();
        assert_eq!(panel.get("future_panel").unwrap().as_str(), Some("kept"));
        assert_eq!(panel.get("pinned").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn clearing_known_optional_fields_does_not_resurrect_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[board]\nprimary = 'cursor'\nfuture_board = 'kept'\n[[panels]]\nid = 0\nx = 80.0\ny = 40.0\nfuture_panel = 'kept'\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        store.preferences_mut().board.primary = None;
        store.preferences_mut().window.x = None;
        store.preferences_mut().window.y = None;
        store.save().unwrap();
        let restored = ConfigStore::load_from(path.clone());
        assert_eq!(restored.preferences().board.primary, None);
        assert_eq!(restored.preferences().window.x, None);
        assert_eq!(restored.preferences().window.y, None);
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("future_board"));
        assert!(text.contains("future_panel"));
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn appearance_defaults_and_exact_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[board]\nrefresh_secs = 600\n").unwrap();
        assert_eq!(
            ConfigStore::load_from(path.clone())
                .preferences()
                .board
                .theme_preference,
            ThemePreference::System
        );
        for (theme, encoded) in [
            (ThemePreference::System, "system"),
            (ThemePreference::Light, "light"),
            (ThemePreference::Dark, "dark"),
        ] {
            let mut store = ConfigStore::load_from(path.clone());
            store.preferences_mut().board.theme_preference = theme;
            store.save().unwrap();
            let source = fs::read_to_string(&path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(source["board"]["theme_preference"].as_str(), Some(encoded));
            let reopened = ConfigStore::load_from(path.clone());
            assert_eq!(reopened.preferences().board.theme_preference, theme);
            assert_eq!(reopened.preferences().board.refresh_secs, 600);
        }
    }

    #[test]
    fn invalid_appearance_is_field_local_and_cannot_resurrect_on_save() {
        for invalid in ["'garbage'", "42", "['light']", "{ future = true }"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            fs::write(&path, format!("future = 9\n[board]\ntheme_preference = {invalid}\nrefresh_secs = 720\nquota_display_mode = 'used'\norder = ['codex', 'claude']\nhidden = ['grok']\nprimary = 'codex'\nfuture_board = 'kept'\n[[panels]]\nid = 0\npinned = true\nfuture_panel = 'kept'\n")).unwrap();
            let mut store = ConfigStore::load_from(path.clone());
            let board = store.preferences().board.clone();
            assert_eq!(board.theme_preference, ThemePreference::System);
            assert_eq!(board.refresh_secs, 720);
            assert_eq!(board.quota_display_mode, QuotaDisplayMode::Used);
            assert_eq!(board.order, ["codex", "claude"]);
            assert!(board.hidden.contains("grok"));
            assert_eq!(board.primary.as_deref(), Some("codex"));
            assert!(store.preferences().window.pinned);
            store.save().unwrap();
            let source = fs::read_to_string(&path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(source["board"]["theme_preference"].as_str(), Some("system"));
            assert_eq!(source["future"].as_integer(), Some(9));
            assert_eq!(source["board"]["future_board"].as_str(), Some("kept"));
            assert_eq!(
                source["window"]
                    .as_value()
                    .unwrap()
                    .as_inline_table()
                    .unwrap()
                    .get("future_panel")
                    .unwrap()
                    .as_str(),
                Some("kept")
            );
            assert_eq!(ConfigStore::load_from(path).preferences().board, board);
        }
    }
}

/// A scanner, indexer or editor can hold `config.toml` open while the board
/// saves (#67). Windows refuses a plain replace of an open file.
#[cfg(all(test, windows))]
mod open_target_tests {
    use super::*;
    use crate::fsutil::file_names;
    use std::{io::Read, os::windows::fs::OpenOptionsExt, path::Path, time::Duration};

    const OLD: &str = "[board]\nrefresh_secs = 720\n";
    /// FILE_SHARE_READ alone: the holder refuses write and delete access.
    const SHARE_READ: u32 = 0x1;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, OLD).unwrap();
        (directory, path)
    }

    fn save_refresh(path: &Path, refresh_secs: u64) -> io::Result<()> {
        let mut store = ConfigStore::load_from(path.to_owned());
        store.preferences_mut().board.refresh_secs = refresh_secs;
        store.save()
    }

    fn exclusive_reader(path: &Path) -> fs::File {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ)
            .open(path)
            .unwrap()
    }

    #[test]
    fn save_replaces_config_while_a_reader_holds_it_open() {
        let (directory, path) = fixture();
        // std readers, Defender and the indexer share read, write and delete.
        let mut reader = fs::File::open(&path).unwrap();
        save_refresh(&path, 900).unwrap();
        let saved = ConfigStore::load_from(path.clone());
        assert_eq!(saved.preferences().board.refresh_secs, 900);
        let mut held = String::new();
        reader.read_to_string(&mut held).unwrap();
        assert_eq!(held, OLD, "the open handle keeps reading the old file");
        assert_eq!(file_names(directory.path()), ["config.toml"]);
    }

    #[test]
    fn save_waits_out_a_brief_exclusive_holder() {
        let (directory, path) = fixture();
        let holder = exclusive_reader(&path);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(holder);
        });
        save_refresh(&path, 900).unwrap();
        release.join().unwrap();
        let saved = ConfigStore::load_from(path.clone());
        assert_eq!(saved.preferences().board.refresh_secs, 900);
        assert_eq!(file_names(directory.path()), ["config.toml"]);
    }

    #[test]
    fn save_failure_keeps_old_config_and_leaves_no_temp() {
        let (directory, path) = fixture();
        let _holder = exclusive_reader(&path);
        let error = save_refresh(&path, 900).unwrap_err();
        assert!(
            crate::fsutil::held_by_another_handle(&error),
            "unexpected error: {error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), OLD);
        assert_eq!(file_names(directory.path()), ["config.toml"]);
    }
}

#[cfg(test)]
mod author_visit_tests {
    use super::*;
    #[test]
    fn update_opt_in_defaults_off_tolerates_bad_values_and_round_trips() {
        assert!(!BoardPreferences::default().updates_enabled);
        for (field, enabled) in [
            ("", false),
            ("updates_enabled = 'true'", false),
            ("updates_enabled = 1", false),
            ("updates_enabled = false", false),
            ("updates_enabled = true", true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            fs::write(&path, format!("[board]\n{field}\nrefresh_secs=720\n")).unwrap();
            let mut store = ConfigStore::load_from(path.clone());
            assert_eq!(
                store.preferences().board.updates_enabled,
                enabled,
                "{field}"
            );
            assert_eq!(store.preferences().board.refresh_secs, 720, "{field}");
            store.preferences_mut().board.updates_enabled = !enabled;
            store.save().unwrap();
            let reloaded = ConfigStore::load_from(path);
            assert_eq!(reloaded.preferences().board.updates_enabled, !enabled);
        }
    }

    #[test]
    fn author_visit_missing_invalid_and_round_trip_preserve_other_and_unknown_fields() {
        for field in [
            "",
            "author_youtube_visited = 'true'",
            "author_youtube_visited = 42",
            "author_youtube_visited = []",
            "author_youtube_visited = false",
            "author_youtube_visited = true",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            fs::write(&path,format!("future = 8\n[board]\n{field}\ntheme_preference='light'\nquota_display_mode='used'\nrefresh_secs=720\norder=['codex']\nhidden=['grok']\nprimary='codex'\nfuture_board='keep'\n[[panels]]\nid=0\npinned=true\nfuture_panel='keep'\n")).unwrap();
            let mut store = ConfigStore::load_from(path.clone());
            let p = &store.preferences().board;
            assert_eq!(
                p.author_youtube_visited,
                field == "author_youtube_visited = true"
            );
            assert_eq!(p.theme_preference, ThemePreference::Light);
            assert_eq!(p.refresh_secs, 720);
            assert_eq!(p.quota_display_mode, QuotaDisplayMode::Used);
            assert_eq!(p.primary.as_deref(), Some("codex"));
            assert!(p.hidden.contains("grok"));
            assert_eq!(p.order, ["codex"]);
            store.preferences_mut().board.author_youtube_visited = true;
            store.save().unwrap();
            assert!(
                ConfigStore::load_from(path.clone())
                    .preferences()
                    .board
                    .author_youtube_visited
            );
            let d = fs::read_to_string(path)
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(d["board"]["author_youtube_visited"].as_bool(), Some(true));
            assert_eq!(d["future"].as_integer(), Some(8));
            assert_eq!(d["board"]["future_board"].as_str(), Some("keep"));
            assert_eq!(
                d["window"]
                    .as_value()
                    .unwrap()
                    .as_inline_table()
                    .unwrap()
                    .get("future_panel")
                    .unwrap()
                    .as_str(),
                Some("keep")
            );
        }
    }
}
