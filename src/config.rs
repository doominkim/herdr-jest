use crate::paths::home;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    repo: Vec<RepoRule>,
}

#[derive(Deserialize)]
struct RepoRule {
    path: String,
    command: String,
}

pub fn command_for(root: &Path) -> Vec<String> {
    let default = vec!["npx".to_string(), "jest".to_string()];
    let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") else {
        return default;
    };
    let Ok(text) = std::fs::read_to_string(PathBuf::from(dir).join("config.toml")) else {
        return default;
    };
    let config: Config = toml::from_str(&text).unwrap_or_default();
    config
        .repo
        .iter()
        .find(|rule| matches(&rule.path, root))
        .map(|rule| rule.command.split_whitespace().map(str::to_string).collect())
        .unwrap_or(default)
}

fn matches(pattern: &str, root: &Path) -> bool {
    let expanded = match pattern.strip_prefix("~/") {
        Some(rest) => home().join(rest).display().to_string(),
        None => pattern.to_string(),
    };
    glob::Pattern::new(&expanded).is_ok_and(|glob| glob.matches_path(root))
}
