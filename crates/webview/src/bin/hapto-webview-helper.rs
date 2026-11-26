//! Runs the page's helper process on Linux.

fn main() -> std::process::ExitCode {
    #[cfg(hapto_helper)]
    {
        hapto_webview::helper::run()
    }
    #[cfg(not(hapto_helper))]
    {
        eprintln!("{} runs only on Linux", env!("CARGO_BIN_NAME"));
        std::process::ExitCode::FAILURE
    }
}
