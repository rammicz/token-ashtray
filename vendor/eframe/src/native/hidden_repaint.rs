/// Dispatch one already-due repaint without changing its deadline or visibility.
pub(super) fn dispatch_due_repaint(
    on_windows: bool,
    visible: Option<bool>,
    request_native_paint: impl FnOnce(),
    update_hidden_window: impl FnOnce(),
) {
    if on_windows && visible == Some(false) {
        // Hidden HWNDs do not receive WM_PAINT. Their pending UI actions and
        // queued close events still need an update, without showing the window,
        // including a window hidden while minimized (0x0 client area).
        update_hidden_window();
    } else {
        request_native_paint();
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch_due_repaint;
    use std::cell::Cell;

    #[test]
    fn hidden_window_repaint_updates_without_waiting_for_wm_paint() {
        let updates = Cell::new(0);
        let native_paints = Cell::new(0);
        dispatch_due_repaint(
            true,
            Some(false),
            // Win32 queues no WM_PAINT for an invisible HWND.
            || native_paints.set(native_paints.get() + 1),
            || updates.set(updates.get() + 1),
        );
        assert_eq!(
            updates.get(),
            1,
            "a hidden repaint must reach the UI update"
        );
        assert_eq!(native_paints.get(), 0, "it must not wait for WM_PAINT");
    }

    #[test]
    fn visible_and_other_platform_windows_keep_native_paint_dispatch() {
        for (windows, visible) in [(true, Some(true)), (true, None), (false, Some(false))] {
            let native_paints = Cell::new(0);
            dispatch_due_repaint(
                windows,
                visible,
                || native_paints.set(native_paints.get() + 1),
                || panic!("visible, unknown and non-Windows windows retain native scheduling"),
            );
            assert_eq!(native_paints.get(), 1);
        }
    }
}
