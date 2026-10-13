use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample");

struct Sandbox {
    root: PathBuf,
    state: PathBuf,
    config: PathBuf,
    fake_herdr: PathBuf,
    notify_log: PathBuf,
    daemon: Child,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn sandbox() -> Sandbox {
    let base = std::env::temp_dir().join(format!("hj-daemon-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("repo");
    let state = base.join("state");
    let config = base.join("config");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&config).unwrap();
    for file in ["package.json", "math.spec.js", "src/price.js", "src/price.spec.js"] {
        fs::copy(Path::new(FIXTURE).join(file), root.join(file)).unwrap();
    }
    symlink(Path::new(FIXTURE).join("node_modules"), root.join("node_modules")).unwrap();

    let notify_log = base.join("notify.log");
    let fake_herdr = base.join("fake-herdr");
    fs::write(&fake_herdr, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", notify_log.display())).unwrap();
    fs::set_permissions(&fake_herdr, fs::Permissions::from_mode(0o755)).unwrap();

    let root = root.canonicalize().unwrap();
    let daemon = start_daemon(&root, &state, &config, &fake_herdr);
    Sandbox { root, state, config, fake_herdr, notify_log, daemon }
}

fn start_daemon(root: &Path, state: &Path, config: &Path, fake_herdr: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_herdr-jest"))
        .args(["daemon", "--root"])
        .arg(root)
        .env("HERDR_PLUGIN_STATE_DIR", state)
        .env("HERDR_PLUGIN_CONFIG_DIR", config)
        .env("HERDR_BIN_PATH", fake_herdr)
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}


struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn connect(state: &Path) -> Client {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let socket = fs::read_dir(state)
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.extension().is_some_and(|ext| ext == "sock"));
            if let Some(stream) = socket.and_then(|path| UnixStream::connect(path).ok()) {
                stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
                let writer = stream.try_clone().unwrap();
                return Client { reader: BufReader::new(stream), writer };
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn send(&mut self, request: Value) {
        writeln!(self.writer, "{request}").unwrap();
    }

    fn wait_for(&mut self, what: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).unwrap();
            assert!(read > 0, "daemon closed the connection while waiting for {what}");
            let snapshot: Value = serde_json::from_str(&line).unwrap();
            if predicate(&snapshot) {
                return snapshot;
            }
        }
    }
}

fn file<'a>(snapshot: &'a Value, root: &Path, relative: &str) -> &'a Value {
    let path = root.join(relative).display().to_string();
    snapshot["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path.as_str())
        .unwrap_or_else(|| panic!("{relative} missing from snapshot"))
}

fn statuses(node: &Value) -> Vec<(String, String)> {
    let mut out = vec![(node["name"].as_str().unwrap().to_string(), node["status"].as_str().unwrap().to_string())];
    for child in node["children"].as_array().unwrap() {
        out.extend(statuses(child));
    }
    out
}

fn notifications(sandbox: &Sandbox) -> Vec<String> {
    fs::read_to_string(&sandbox.notify_log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn saved_runs(sandbox: &Sandbox) -> u64 {
    let saved = fs::read_dir(&sandbox.state)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json") && !path.to_string_lossy().ends_with("-run.json"))
        .expect("state file");
    let snapshot: Value = serde_json::from_slice(&fs::read(saved).unwrap()).unwrap();
    snapshot["runs_completed"].as_u64().unwrap()
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn stop(sandbox: &mut Sandbox, client: &mut Client) {
    client.send(json!({ "cmd": "stop" }));
    let deadline = Instant::now() + Duration::from_secs(10);
    while sandbox.daemon.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "daemon did not stop");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn daemon_runs_on_demand_keeps_results_across_restarts_and_notifies_only_when_unwatched() {
    let mut sandbox = sandbox();
    let mut client = Client::connect(&sandbox.state);

    let discovered = client.wait_for("discovered tree", |s| s["files"].as_array().is_some_and(|f| f.len() == 2));
    assert_eq!(
        statuses(&file(&discovered, &sandbox.root, "math.spec.js")["nodes"][0]),
        vec![
            ("math".into(), "unknown".into()),
            ("adds".into(), "unknown".into()),
            ("multiplies wrong".into(), "unknown".into()),
            ("skipped one".into(), "unknown".into()),
            ("sums %i + %i".into(), "unknown".into()),
        ]
    );

    client.send(json!({ "cmd": "run", "scope": "all" }));
    let after_all = client.wait_for("first run", |s| s["runs_completed"] == 1 && s["running"].is_null());
    assert_eq!(
        statuses(&file(&after_all, &sandbox.root, "math.spec.js")["nodes"][0]),
        vec![
            ("math".into(), "failed".into()),
            ("adds".into(), "passed".into()),
            ("multiplies wrong".into(), "failed".into()),
            ("skipped one".into(), "skipped".into()),
            ("sums %i + %i".into(), "passed".into()),
        ]
    );
    assert_eq!(file(&after_all, &sandbox.root, "src/price.spec.js")["status"], "passed");
    assert_eq!(notifications(&sandbox).len(), 0, "watched run must not notify");

    client.send(json!({ "cmd": "run_failed" }));
    client.wait_for("failed-only run", |s| s["runs_completed"] == 2 && s["running"].is_null());

    fs::write(
        sandbox.root.join("src/price.js"),
        "function withTax(amount) {\n  return amount;\n}\n\nmodule.exports = { withTax };\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(saved_runs(&sandbox), 2, "saving a file must not start a run");

    let price_spec = sandbox.root.join("src/price.spec.js").display().to_string();
    client.send(json!({ "cmd": "run", "scope": { "file": price_spec } }));
    drop(client);
    wait_until("unwatched run to notify", || notifications(&sandbox).len() == 1);
    assert!(notifications(&sandbox)[0].contains("price adds 10% tax"));
    wait_until("third run saved", || saved_runs(&sandbox) == 3);

    let mut client = Client::connect(&sandbox.state);
    stop(&mut sandbox, &mut client);
    sandbox.daemon = start_daemon(&sandbox.root, &sandbox.state, &sandbox.config, &sandbox.fake_herdr);
    let mut client = Client::connect(&sandbox.state);
    let restored = client.wait_for("restored results", |s| s["files"].as_array().is_some_and(|f| f.len() == 2));
    assert_eq!(restored["runs_completed"], 3);
    assert_eq!(file(&restored, &sandbox.root, "src/price.spec.js")["status"], "failed");
    assert_eq!(file(&restored, &sandbox.root, "math.spec.js")["nodes"][0]["children"][1]["status"], "failed");
    stop(&mut sandbox, &mut client);
}
