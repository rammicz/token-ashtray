//! Preserve position events when entering a window at its previous client position.

pub(crate) fn should_emit_cursor_moved<T: PartialEq>(
    entered: bool,
    previous: Option<&T>,
    current: &T,
) -> bool {
    // Consumers clear their pointer on CursorLeft. Reentry must restore it even
    // when the new client position equals this window's previous position.
    entered || previous != Some(current)
}

#[cfg(test)]
mod tests {
    use super::should_emit_cursor_moved;

    #[test]
    fn identical_position_on_reentry_restores_pointer_before_button() {
        let position = [420, 32];
        let last_native_position = Some(position);
        // CursorLeft cleared the consumer's pointer, but winit kept its position.
        let mut pointer = None;
        let mut events = Vec::new();
        if should_emit_cursor_moved(true, last_native_position.as_ref(), &position) {
            pointer = Some(position);
            events.push("PointerMoved");
        }
        // egui-winit ignores button input until it has a current pointer position.
        if pointer.is_some() {
            events.push("PointerButton");
        }
        assert_eq!(events, ["PointerMoved", "PointerButton"]);
    }

    #[test]
    fn stationary_moves_inside_the_window_remain_suppressed() {
        let position = [420, 32];
        assert!(!should_emit_cursor_moved(false, Some(&position), &position));
    }

    #[test]
    fn fresh_windows_and_distinct_positions_emit_moves() {
        let position = [420, 32];
        assert!(should_emit_cursor_moved(false, None, &position));
        assert!(should_emit_cursor_moved(false, Some(&[200, 50]), &position));
    }
}
