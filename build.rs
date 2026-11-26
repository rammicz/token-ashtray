use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/application/token-ashtray.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .join("assets/application/token-ashtray.ico");
    let mut resource = winres::WindowsResource::new();
    resource.set_icon(path.to_str().expect("UTF-8 icon path"));
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
        && !env::var("HOST").expect("Cargo HOST").contains("windows")
    {
        let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
        let prefix = format!("{arch}-w64-mingw32");
        resource
            .set_windres_path(&format!("{prefix}-windres"))
            .set_ar_path(&format!("{prefix}-ar"));
    }
    resource.compile().expect("embed Windows application icon");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        // A resource-only archive has no Rust symbol to pull it into the EXE.
        println!(
            "cargo:rustc-link-arg-bin=token-ashtray={}",
            PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
                .join("resource.o")
                .display()
        );
    }
}
