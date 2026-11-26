//! QA ONLY (#52): stages a locally built setup as a pending update for a QA
//! build of the app, signed with the public TEST release key. It never makes
//! a request and needs the `qa-offline-update` feature, which no release
//! build enables. See docs/WINDOWS-ACCEPTANCE.md.
//!
//! ```text
//! cargo run --release --locked --features qa-offline-update --example update_seed_probe -- \
//!     --installed 0.2.1 --version 0.2.2 --setup <Setup N+1.exe> --app <N+1 token-ashtray.exe>
//! ```
use std::path::PathBuf;
use std::process::ExitCode;

use token_ashtray::updates::{Version, qa};

const USAGE: &str =
    "usage: update_seed_probe --installed X.Y.Z --version X.Y.Z --setup PATH --app PATH";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|index| arguments.get(index + 1))
    };
    let (Some(installed), Some(version), Some(setup), Some(app)) = (
        value("--installed").and_then(|text| Version::parse(text).ok()),
        value("--version").and_then(|text| Version::parse(text).ok()),
        value("--setup").map(PathBuf::from),
        value("--app").map(PathBuf::from),
    ) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match qa::seed(installed, version, &setup, &app) {
        Ok(staged) => {
            println!("Staged {version} for {installed} at {}", staged.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("update_seed_probe: {error}");
            ExitCode::FAILURE
        }
    }
}
