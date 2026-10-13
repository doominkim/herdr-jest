use crate::args::{Scope, build};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct RunOutput {
    pub report: Option<String>,
    pub error: Option<String>,
}

pub fn list_tests(root: &Path, command: &[String]) -> Result<Vec<PathBuf>, String> {
    let (program, base) = command.split_first().ok_or("empty jest command")?;
    let output = Command::new(program)
        .args(base)
        .args(["--listTests", "--json"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_line = stdout.lines().rev().find(|line| line.trim_start().starts_with('['));
    json_line
        .and_then(|line| serde_json::from_str::<Vec<PathBuf>>(line).ok())
        .ok_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string())
}

pub fn run(root: &Path, command: &[String], scope: &Scope, output_file: &Path, mut on_file_done: impl FnMut(PathBuf)) -> RunOutput {
    let Some((program, base)) = command.split_first() else {
        return RunOutput { report: None, error: Some("empty jest command".into()) };
    };
    let _ = std::fs::remove_file(output_file);
    let spawned = Command::new(program)
        .args(base)
        .args(build(scope, output_file))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return RunOutput { report: None, error: Some(format!("{program}: {error}")) },
    };

    let mut tail: VecDeque<String> = VecDeque::new();
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(file) = finished_file(root, &line) {
                on_file_done(file);
            }
            tail.push_back(line);
            if tail.len() > 40 {
                tail.pop_front();
            }
        }
    }
    let _ = child.wait();

    match std::fs::read_to_string(output_file) {
        Ok(report) => RunOutput { report: Some(report), error: None },
        Err(_) => RunOutput { report: None, error: Some(Vec::from(tail).join("\n")) },
    }
}

fn finished_file(root: &Path, line: &str) -> Option<PathBuf> {
    let rest = line.strip_prefix("PASS ").or_else(|| line.strip_prefix("FAIL "))?;
    let relative = rest.split_whitespace().last()?;
    let relative = relative.strip_prefix("./").unwrap_or(relative);
    Some(root.join(relative))
}
