//! The `aeon` command-line tool, built with the app so the installers include it.
//! A console program (no `windows_subsystem`), unlike the app's main binary.

fn main() -> std::process::ExitCode {
    aeon_cli::run()
}
