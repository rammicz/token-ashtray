# Contributing to Token Ashtray

## Development

Token Ashtray is a Rust desktop application for Linux x86_64 and Windows x64.
[Build and run from source](docs/RELEASE.md#build-and-run-from-source) lists
the toolchain, platform libraries and release build steps. The patched
`eframe` and `winit` crates under `vendor/` are part of the build; see
[EFRAME-PATCH.md](docs/EFRAME-PATCH.md) and [WINIT-PATCH.md](docs/WINIT-PATCH.md).

## Validation

GitHub Actions are disabled for now, so run the checks locally. Run the focused
tests for each change, and run the full set before submitting:

```sh
cargo fmt --check
cargo clippy --locked -- -D warnings
cargo test --locked
python3 tests/test_release_scripts.py
cargo build --release --locked --bin token-ashtray
```

Tests use synthetic fixtures. Never commit real provider credentials, account
captures, local paths or personal data; see
[the fixture provenance notes](tests/fixtures/PROVENANCE.md).

A Linux run or a Windows GNU compatibility build does not establish native
Windows behavior. Use the [native Windows checklist](docs/WINDOWS-ACCEPTANCE.md)
for that. Release preparation, third-party notices, measurements and packages
follow [the release procedure](docs/RELEASE.md).
