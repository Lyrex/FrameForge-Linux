fn main() {
    // The Linux bundles carry their own Tesseract model, declared as a bundle
    // resource in `tauri.linux.conf.json`. `tauri_build::build()` below refuses
    // to compile the crate when a declared resource is missing, so the model has
    // to be on disk before that call.
    //
    // This is the only placement early enough. `beforeBundleCommand`, the obvious
    // home, fires after the crate compiles: on a checkout without the model (it
    // is untracked, at 23 MB) the build fails right here, before that hook
    // ever runs. Doing it in the build script also covers a plain `cargo run`,
    // which no Tauri hook reaches at all.
    //
    // Shelling out keeps the download and its SHA256 check in one place instead
    // of duplicating them in Rust. That would mean an HTTP client and a hasher in
    // `[build-dependencies]`, a separate graph from `[dependencies]`, so `ureq`
    // and its TLS stack would compile a second time for the host. A missing
    // `curl` here also fails the build loudly, on the machine of whoever can fix
    // it, rather than failing on a user's machine the way a runtime shell-out
    // would.
    //
    // Cargo sets `CARGO_CFG_TARGET_OS` to the target, so this stays correct under
    // cross-compilation, where `cfg!(target_os)` would describe the host instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let script = "../scripts/fetch-tessdata.sh";
        let model = "tessdata/eng.traineddata";

        // Without the model as a trigger, deleting it would leave this build
        // script cached and the absence unnoticed until the bundler failed much
        // later.
        println!("cargo:rerun-if-changed={script}");
        println!("cargo:rerun-if-changed={model}");

        let status = std::process::Command::new("bash")
            .arg(script)
            .status()
            .unwrap_or_else(|e| panic!("{script} needs bash on PATH: {e}"));
        assert!(
            status.success(),
            "{script} failed, so {model} is missing; see its output above"
        );
    }

    tauri_build::build()
}
