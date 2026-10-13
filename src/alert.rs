use std::path::PathBuf;
use std::process::{Command, Stdio};

pub fn newly_failed(failures: &[(PathBuf, String)]) {
    let Some((_, first)) = failures.first() else {
        return;
    };
    let herdr = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let title = match failures.len() {
        1 => "herdr-jest: 1 new failure".to_string(),
        count => format!("herdr-jest: {count} new failures"),
    };
    let _ = Command::new(herdr)
        .args(["notification", "show", &title, "--body", first, "--sound", "request"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
