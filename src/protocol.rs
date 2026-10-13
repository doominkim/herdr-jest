use crate::args::Scope;
use crate::discover::{NodeKind, TestNode};
use crate::results::{Assertion, Model, Status};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Run { scope: Scope },
    RunFailed,
    Stop,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: PathBuf,
    pub running: Option<String>,
    pub running_files: Vec<PathBuf>,
    pub runs_completed: u64,
    #[serde(default)]
    pub last_duration_ms: Option<u64>,
    pub last_error: Option<String>,
    pub files: Vec<FileView>,
    #[serde(default)]
    pub history: Vec<RunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub finished_at: u64,
    pub branch: Option<String>,
    pub label: String,
    #[serde(default)]
    pub scope: Option<Scope>,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub duration_ms: Option<u64>,
    pub error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileView {
    pub path: PathBuf,
    pub status: Status,
    pub suite_message: Option<String>,
    pub nodes: Vec<NodeView>,
    pub unmatched: Vec<Assertion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    pub name: String,
    pub full_name: String,
    pub kind: NodeKind,
    pub line: u32,
    #[serde(default)]
    pub end_line: u32,
    pub dynamic: bool,
    pub status: Status,
    pub assertions: Vec<Assertion>,
    pub children: Vec<NodeView>,
}

pub fn file_views(model: &Model) -> Vec<FileView> {
    model
        .files()
        .map(|file| FileView {
            path: file.to_path_buf(),
            status: model.file_status(file),
            suite_message: model.suite_message(file),
            nodes: node_views(model, file, model.tree(file), &[], ""),
            unmatched: model.unmatched(file),
        })
        .collect()
}

fn node_views(model: &Model, file: &Path, nodes: &[TestNode], parent_path: &[usize], parent_name: &str) -> Vec<NodeView> {
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let mut node_path = parent_path.to_vec();
            node_path.push(index);
            let full_name = if parent_name.is_empty() {
                node.name.clone()
            } else {
                format!("{parent_name} {}", node.name)
            };
            NodeView {
                name: node.name.clone(),
                kind: node.kind.clone(),
                line: node.start_line,
                end_line: node.end_line,
                dynamic: node.dynamic,
                status: model.status(file, &node_path),
                assertions: model.assertions(file, &node_path),
                children: node_views(model, file, &node.children, &node_path, &full_name),
                full_name,
            }
        })
        .collect()
}
