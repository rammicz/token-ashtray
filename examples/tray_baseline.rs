//! Diagnostic: real polling and SNI without constructing a graphics window.
#[cfg(target_os = "linux")]
use token_ashtray::platform;

#[cfg(target_os = "linux")]
fn main() {
    use std::{
        sync::{Arc, RwLock},
        time::{Duration, Instant},
    };
    use token_ashtray::poll::{PollSettings, Poller, initial_board};
    let board = initial_board();
    let snapshot = Arc::new(RwLock::new(platform::TraySnapshot::default()));
    let context = eframe::egui::Context::default();
    let tray =
        platform::Tray::new(context, Arc::clone(&snapshot)).expect("SNI host required for probe");
    let _poller =
        Poller::start(Arc::clone(&board), PollSettings::default(), Arc::new(|| {})).unwrap();
    if std::env::args().any(|arg| arg == "--window-cycle") {
        struct DiagnosticWindow(Instant);
        impl eframe::App for DiagnosticWindow {
            fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label("Temporary graphics-memory measurement; closes automatically.");
                });
                if self.0.elapsed() > Duration::from_secs(2) {
                    ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
                }
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        eframe::run_native(
            "Token Ashtray memory probe",
            eframe::NativeOptions::default(),
            Box::new(|_| Ok(Box::new(DiagnosticWindow(Instant::now())))),
        )
        .unwrap();
        println!("Graphics window closed; measuring retained tray process.");
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        let cards = board.read().unwrap();
        *snapshot.write().unwrap() = platform::TraySnapshot {
            providers: cards
                .values()
                .map(|card| platform::TrayProvider {
                    name: card.short_name.clone(),
                    remaining: card.remaining(),
                    reset: None,
                })
                .collect(),
            primary_remaining: cards.values().find_map(|card| card.remaining()),
            any_readable: cards.values().any(|card| card.remaining().is_some()),
            ..Default::default()
        };
        drop(cards);
        tray.sync();
        if tray.poll() == platform::TrayAction::Quit {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This measurement probe is Linux-only.");
}
