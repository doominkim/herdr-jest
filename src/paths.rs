use std::path::{Path, PathBuf};

pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") {
        return PathBuf::from(dir);
    }
    home().join(".local/state/herdr-jest")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn socket_path(root: &Path) -> PathBuf {
    state_dir().join(format!("{}.sock", root_hash(root)))
}

pub fn state_file(root: &Path) -> PathBuf {
    state_dir().join(format!("{}.json", root_hash(root)))
}

pub fn view_file(root: &Path) -> PathBuf {
    state_dir().join(format!("{}.view.json", root_hash(root)))
}

pub fn run_output(root: &Path) -> PathBuf {
    state_dir().join(format!("{}-run.json", root_hash(root)))
}

pub fn log_file(root: &Path) -> PathBuf {
    state_dir().join(format!("{}.log", root_hash(root)))
}

fn root_hash(root: &Path) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn find_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| has_jest_config(dir)).map(Path::to_path_buf)
}

fn has_jest_config(dir: &Path) -> bool {
    let config_files = ["jest.config.js", "jest.config.ts", "jest.config.mjs", "jest.config.cjs", "jest.config.json"];
    if config_files.iter().any(|name| dir.join(name).is_file()) {
        return true;
    }
    let Ok(manifest) = std::fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&manifest).is_ok_and(|json| json.get("jest").is_some())
}
