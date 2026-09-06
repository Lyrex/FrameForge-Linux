fn main() -> std::process::ExitCode {
    match warframe_companion_lib::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FrameForge cannot start: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
