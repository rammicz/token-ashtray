/// First native exposure of one Windows Glow viewport generation.
pub(super) struct FirstShow {
    pending: bool,
    painted: bool,
    requested_visible: bool,
    focus_requested: bool,
    repaint_requested: bool,
}

impl FirstShow {
    pub(super) fn new(visible: Option<bool>) -> Self {
        Self {
            pending: true,
            painted: false,
            requested_visible: visible.unwrap_or(true),
            focus_requested: false,
            repaint_requested: false,
        }
    }

    pub(super) fn creation_builder(
        &self,
        builder: &egui::ViewportBuilder,
    ) -> egui::ViewportBuilder {
        builder.clone().with_visible(false)
    }

    pub(super) fn recreate(&mut self) {
        *self = Self::new(Some(self.requested_visible));
    }

    /// Called only after this native window has been added to the ID maps.
    pub(super) fn request_initial_repaint(&mut self, request: impl FnOnce()) {
        if !self.repaint_requested {
            self.repaint_requested = true;
            request();
        }
    }

    pub(super) fn painted(&mut self) {
        self.painted = true;
    }

    pub(super) fn apply_commands(
        &mut self,
        mut commands: Vec<egui::ViewportCommand>,
        apply: impl FnOnce(Vec<egui::ViewportCommand>),
    ) {
        commands.retain(|command| match command {
            egui::ViewportCommand::Visible(visible) => {
                self.requested_visible = *visible;
                !self.pending
            }
            egui::ViewportCommand::Focus if self.pending => {
                self.focus_requested = true;
                false
            }
            _ => true,
        });
        if self.pending && self.painted {
            self.pending = false;
            // All initial style/size/position commands must run before show,
            // even when the application's Visible command appeared earlier.
            if self.requested_visible {
                commands.push(egui::ViewportCommand::Visible(true));
                if self.focus_requested {
                    commands.push(egui::ViewportCommand::Focus);
                }
            }
            self.focus_requested = false;
        }
        apply(commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::ViewportCommand;

    #[test]
    fn first_painted_root_applies_all_geometry_before_exposure() {
        let mut gate = FirstShow::new(Some(true));
        let mut geometry_applied = false;
        gate.painted();
        gate.apply_commands(
            vec![
                ViewportCommand::Visible(true),
                ViewportCommand::InnerSize(egui::vec2(1600.0, 993.0)),
                ViewportCommand::OuterPosition(egui::pos2(32.0, 0.0)),
            ],
            |commands| {
                for command in commands {
                    match command {
                        ViewportCommand::OuterPosition(_) => geometry_applied = true,
                        ViewportCommand::Visible(true) => assert!(
                            geometry_applied,
                            "first native show must follow fitted geometry"
                        ),
                        _ => {}
                    }
                }
            },
        );
    }

    #[test]
    fn deferred_creation_keeps_requested_visibility_but_creates_hidden() {
        let gate = FirstShow::new(None);
        let desired = egui::ViewportBuilder::default();
        let native = gate.creation_builder(&desired);
        assert_eq!(
            native.visible,
            Some(false),
            "a deferred HWND must wait for its own first callback"
        );
        assert_eq!(desired.visible, None);
    }

    #[test]
    fn other_viewports_output_cannot_expose_a_child_before_its_own_paint() {
        let mut child = FirstShow::new(None);
        let mut applied = Vec::new();
        child.request_initial_repaint(|| applied.push("repaint"));
        child.request_initial_repaint(|| panic!("initial repaint must be queued once"));
        child.apply_commands(
            vec![ViewportCommand::Visible(true), ViewportCommand::Focus],
            |commands| {
                assert!(
                    commands.is_empty(),
                    "parent output must wait for the child's callback"
                );
            },
        );
        assert_eq!(applied, ["repaint"]);
        child.painted();
        child.apply_commands(vec![], |commands| {
            assert_eq!(
                commands,
                [ViewportCommand::Visible(true), ViewportCommand::Focus]
            );
        });
        child.painted();
        child.apply_commands(vec![], |commands| {
            assert!(
                commands.is_empty(),
                "later paint must not show, resize or reposition again"
            )
        });
    }

    #[test]
    fn explicit_hidden_startup_and_last_visibility_request_are_honored() {
        for initial in [Some(false), Some(true), None] {
            let mut gate = FirstShow::new(initial);
            gate.painted();
            gate.apply_commands(
                vec![
                    ViewportCommand::Visible(true),
                    ViewportCommand::Focus,
                    ViewportCommand::Visible(false),
                ],
                |commands| {
                    assert!(
                        commands.is_empty(),
                        "an explicitly hidden first frame must never show or focus"
                    );
                },
            );
            let reopen = vec![
                ViewportCommand::OuterPosition(egui::pos2(20.0, 20.0)),
                ViewportCommand::Visible(true),
                ViewportCommand::Focus,
            ];
            gate.apply_commands(reopen.clone(), |commands| {
                assert_eq!(
                    commands, reopen,
                    "later show retains ordinary command order"
                )
            });
        }
        let mut hidden = FirstShow::new(Some(false));
        hidden.painted();
        hidden.apply_commands(vec![], |commands| assert!(commands.is_empty()));
    }

    #[test]
    fn recreation_resets_only_its_native_generation_and_retains_desired_visibility() {
        let mut root = FirstShow::new(Some(true));
        let mut child = FirstShow::new(None);
        for gate in [&mut root, &mut child] {
            gate.painted();
            gate.apply_commands(vec![], |_| {});
        }
        child.apply_commands(vec![ViewportCommand::Visible(false)], |_| {});
        child.recreate();
        child.request_initial_repaint(|| {});
        assert_eq!(
            child
                .creation_builder(&egui::ViewportBuilder::default())
                .visible,
            Some(false)
        );
        child.apply_commands(vec![], |commands| assert!(commands.is_empty()));
        root.apply_commands(vec![], |commands| {
            assert!(
                commands.is_empty(),
                "another viewport's recreation must not show root again"
            )
        });
        child.painted();
        child.apply_commands(vec![], |commands| {
            assert!(commands.is_empty(), "recreated hidden child stays hidden")
        });
    }
}
