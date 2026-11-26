//! Read the app theme after a Windows appearance change.

/// The `WM_SETTINGCHANGE` area name Windows sends when apps switch light/dark mode.
const IMMERSIVE_COLOR_SET: &str = "ImmersiveColorSet";

/// Whether a `WM_SETTINGCHANGE` area names the immersive color set.
pub(crate) fn is_immersive_color_set(area: Option<&[u16]>) -> bool {
    area.is_some_and(|area| {
        String::from_utf16(area).is_ok_and(|area| area.eq_ignore_ascii_case(IMMERSIVE_COLOR_SET))
    })
}

/// Query the theme after a settings change. uxtheme keeps answering with the
/// previous appearance until its immersive color policy is refreshed, so an
/// immersive color change refreshes it first.
pub(crate) fn theme_after_setting_change<T>(
    area: Option<&[u16]>,
    refresh: impl FnOnce(),
    query: impl FnOnce() -> T,
) -> T {
    if is_immersive_color_set(area) {
        refresh();
    }
    query()
}

#[cfg(test)]
mod tests {
    use super::{is_immersive_color_set, theme_after_setting_change};
    use std::cell::Cell;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// uxtheme's per-process answer: it keeps the previous value until refreshed.
    struct CachedPolicy {
        system_dark: Cell<bool>,
        cached_dark: Cell<bool>,
    }

    impl CachedPolicy {
        fn new(dark: bool) -> Self {
            Self {
                system_dark: Cell::new(dark),
                cached_dark: Cell::new(dark),
            }
        }

        fn refresh(&self) {
            self.cached_dark.set(self.system_dark.get());
        }

        fn query(&self) -> bool {
            self.cached_dark.get()
        }
    }

    #[test]
    fn each_appearance_change_is_applied_on_the_same_message() {
        let area = wide("ImmersiveColorSet");
        for start_dark in [false, true] {
            let policy = CachedPolicy::new(start_dark);
            for flip in 1..=4 {
                let dark = start_dark ^ (flip % 2 == 1);
                policy.system_dark.set(dark);
                let seen =
                    theme_after_setting_change(Some(&area), || policy.refresh(), || policy.query());
                assert_eq!(seen, dark, "start dark {start_dark}, flip {flip}");
            }
        }
    }

    /// Source code without comment lines, so a comment cannot satisfy a check.
    fn code(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn settings_change_handler_refreshes_through_uxtheme_before_reading_the_theme() {
        let event_loop = code(include_str!("event_loop.rs"));
        let start = event_loop
            .find("WM_SETTINGCHANGE => {")
            .expect("settings change arm");
        let arm = &event_loop[start..];
        let arm = &arm[..arm
            .find("result = ProcResult::DefWindowProc")
            .expect("arm end")];
        let call = arm
            .find("theme_refresh::theme_after_setting_change(")
            .expect("refresh order");
        let refresh = arm
            .find("dark_mode::refresh_immersive_color_policy,")
            .expect("refresh");
        let query = arm
            .find("|| try_theme(window, preferred_theme)")
            .expect("theme query");
        assert!(
            call < refresh && refresh < query,
            "refresh must be passed before the query"
        );
        assert_eq!(
            arm.matches("try_theme(").count(),
            1,
            "the theme is read only after refreshing"
        );
        let dark_mode = code(include_str!("dark_mode.rs"));
        let function = &dark_mode[dark_mode
            .find("pub fn refresh_immersive_color_policy()")
            .expect("uxtheme refresh")..];
        assert!(function.contains("UXTHEME_REFRESHIMMERSIVECOLORPOLICYSTATE_ORDINAL: PCSTR = 104"));
    }

    #[test]
    fn refresh_runs_before_the_query_only_for_immersive_color_changes() {
        for (area, refreshes) in [
            (Some(wide("ImmersiveColorSet")), true),
            (Some(wide("immersivecolorset")), true),
            (Some(wide("ImmersiveColorSetX")), false),
            (Some(wide("Policy")), false),
            (Some(wide("")), false),
            (None, false),
        ] {
            let calls = Cell::new(Vec::new());
            let record = |name: &'static str| {
                let mut seen = calls.take();
                seen.push(name);
                calls.set(seen);
            };
            theme_after_setting_change(area.as_deref(), || record("refresh"), || record("query"));
            let expected: &[&str] = if refreshes {
                &["refresh", "query"]
            } else {
                &["query"]
            };
            assert_eq!(calls.take(), expected, "area {area:?}");
            assert_eq!(
                is_immersive_color_set(area.as_deref()),
                refreshes,
                "area {area:?}"
            );
        }
    }
}
