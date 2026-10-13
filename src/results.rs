use crate::discover::{NodeKind, TestNode};
use crate::protocol::{FileView, NodeView};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Unknown,
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    pub full_name: String,
    pub status: Status,
    pub failure_messages: Vec<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct FileModel {
    tree: Vec<TestNode>,
    by_node: HashMap<Vec<usize>, Vec<Assertion>>,
    unmatched: Vec<Assertion>,
    suite_message: Option<String>,
}

#[derive(Debug, Default)]
pub struct Model {
    files: BTreeMap<PathBuf, FileModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    test_results: Vec<SuiteReport>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuiteReport {
    name: PathBuf,
    #[serde(default)]
    message: String,
    assertion_results: Vec<AssertionReport>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssertionReport {
    ancestor_titles: Vec<String>,
    title: String,
    full_name: String,
    status: String,
    #[serde(default)]
    failure_messages: Vec<String>,
    duration: Option<u64>,
    location: Option<Location>,
}

#[derive(Deserialize)]
struct Location {
    line: u32,
}

impl Model {
    pub fn set_tree(&mut self, file: PathBuf, tree: Vec<TestNode>) {
        let model = self.files.entry(file).or_default();
        if model.tree == tree {
            return;
        }
        model.tree = tree;
        model.by_node.clear();
        model.unmatched.clear();
    }

    pub fn apply(&mut self, report_json: &str) -> Result<Vec<(PathBuf, String)>, serde_json::Error> {
        let report: Report = serde_json::from_str(report_json)?;
        let mut newly_failed = Vec::new();
        for suite in report.test_results {
            let file = self.files.entry(suite.name.clone()).or_default();
            let crashed = suite.assertion_results.is_empty() && !suite.message.trim().is_empty();
            file.suite_message = crashed.then_some(suite.message);
            for result in suite.assertion_results {
                let mut titles = result.ancestor_titles.clone();
                titles.push(result.title.clone());
                let line = result.location.as_ref().map(|location| location.line);
                let node_path = path_by_titles(&file.tree, &titles)
                    .or_else(|| line.and_then(|line| path_by_line(&file.tree, line)));
                let assertion = Assertion {
                    full_name: result.full_name,
                    status: status_from_jest(&result.status),
                    failure_messages: result.failure_messages,
                    duration_ms: result.duration,
                };
                let slot = match node_path {
                    Some(node_path) => file.by_node.entry(node_path).or_default(),
                    None => &mut file.unmatched,
                };
                let full_name = assertion.full_name.clone();
                let became_failed = assertion.status == Status::Failed;
                let previous = upsert(slot, assertion);
                if became_failed && previous != Some(Status::Failed) {
                    newly_failed.push((suite.name.clone(), full_name));
                }
            }
        }
        Ok(newly_failed)
    }

    pub fn status(&self, file: &Path, node_path: &[usize]) -> Status {
        let Some(model) = self.files.get(file) else {
            return Status::Unknown;
        };
        let Some(node) = node_at(&model.tree, node_path) else {
            return Status::Unknown;
        };
        match node.kind {
            NodeKind::Test => combine(
                model
                    .by_node
                    .get(node_path)
                    .into_iter()
                    .flatten()
                    .map(|assertion| assertion.status),
            ),
            NodeKind::Describe => combine((0..node.children.len()).map(|index| {
                let mut child = node_path.to_vec();
                child.push(index);
                self.status(file, &child)
            })),
        }
    }

    pub fn assertions(&self, file: &Path, node_path: &[usize]) -> Vec<Assertion> {
        self.files
            .get(file)
            .and_then(|model| model.by_node.get(node_path))
            .cloned()
            .unwrap_or_default()
    }
}

fn status_from_jest(status: &str) -> Status {
    match status {
        "passed" => Status::Passed,
        "failed" => Status::Failed,
        "pending" | "skipped" | "todo" | "disabled" => Status::Skipped,
        _ => Status::Unknown,
    }
}

fn combine(statuses: impl Iterator<Item = Status>) -> Status {
    let mut seen = Status::Unknown;
    for status in statuses {
        match (seen, status) {
            (_, Status::Failed) => return Status::Failed,
            (_, Status::Passed) => seen = Status::Passed,
            (Status::Unknown, Status::Skipped) => seen = Status::Skipped,
            _ => {}
        }
    }
    seen
}

fn node_at<'a>(tree: &'a [TestNode], node_path: &[usize]) -> Option<&'a TestNode> {
    let (first, rest) = node_path.split_first()?;
    let node = tree.get(*first)?;
    if rest.is_empty() {
        return Some(node);
    }
    node_at(&node.children, rest)
}

fn path_by_titles(tree: &[TestNode], titles: &[String]) -> Option<Vec<usize>> {
    let (first, rest) = titles.split_first()?;
    let index = tree.iter().position(|node| !node.dynamic && &node.name == first)?;
    if rest.is_empty() {
        return Some(vec![index]);
    }
    let mut path = vec![index];
    path.extend(path_by_titles(&tree[index].children, rest)?);
    Some(path)
}

fn path_by_line(tree: &[TestNode], line: u32) -> Option<Vec<usize>> {
    let index = tree
        .iter()
        .position(|node| node.start_line <= line && line <= node.end_line)?;
    let node = &tree[index];
    let mut path = vec![index];
    match node.kind {
        NodeKind::Test => Some(path),
        NodeKind::Describe => {
            path.extend(path_by_line(&node.children, line)?);
            Some(path)
        }
    }
}

impl Model {
    pub fn unmatched(&self, file: &Path) -> Vec<Assertion> {
        self.files
            .get(file)
            .map(|model| model.unmatched.clone())
            .unwrap_or_default()
    }

    pub fn suite_message(&self, file: &Path) -> Option<String> {
        self.files.get(file)?.suite_message.clone()
    }

    pub fn file_status(&self, file: &Path) -> Status {
        let Some(model) = self.files.get(file) else {
            return Status::Unknown;
        };
        if model.suite_message.is_some() {
            return Status::Failed;
        }
        let nodes = (0..model.tree.len()).map(|index| self.status(file, &[index]));
        let unmatched = model.unmatched.iter().map(|assertion| assertion.status);
        combine(nodes.chain(unmatched))
    }
}

fn upsert(slot: &mut Vec<Assertion>, assertion: Assertion) -> Option<Status> {
    let Some(existing) = slot.iter_mut().find(|existing| existing.full_name == assertion.full_name) else {
        slot.push(assertion);
        return None;
    };
    let previous = existing.status;
    let filtered_out = assertion.status == Status::Skipped
        && matches!(previous, Status::Passed | Status::Failed);
    if !filtered_out {
        *existing = assertion;
    }
    Some(previous)
}

impl Model {
    pub fn failed(&self) -> Vec<(PathBuf, String)> {
        let mut failed = Vec::new();
        for (file, model) in &self.files {
            let mut node_paths: Vec<&Vec<usize>> = model.by_node.keys().collect();
            node_paths.sort();
            let mapped = node_paths.into_iter().flat_map(|node_path| &model.by_node[node_path]);
            for assertion in mapped.chain(&model.unmatched) {
                if assertion.status == Status::Failed {
                    failed.push((file.clone(), assertion.full_name.clone()));
                }
            }
        }
        failed
    }
}

impl Model {
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }

    pub fn tree(&self, file: &Path) -> &[TestNode] {
        self.files.get(file).map(|model| model.tree.as_slice()).unwrap_or_default()
    }

    pub fn remove_file(&mut self, file: &Path) {
        self.files.remove(file);
    }
}

impl Model {
    pub fn restore(&mut self, views: &[FileView]) {
        for view in views {
            let mut model = FileModel {
                tree: tree_of(&view.nodes),
                unmatched: view.unmatched.clone(),
                suite_message: view.suite_message.clone(),
                ..FileModel::default()
            };
            collect_saved(&view.nodes, &[], &mut model.by_node);
            self.files.insert(view.path.clone(), model);
        }
    }
}

fn tree_of(nodes: &[NodeView]) -> Vec<TestNode> {
    nodes
        .iter()
        .map(|node| TestNode {
            name: node.name.clone(),
            kind: node.kind.clone(),
            start_line: node.line,
            end_line: node.end_line,
            dynamic: node.dynamic,
            children: tree_of(&node.children),
        })
        .collect()
}

fn collect_saved(nodes: &[NodeView], parent_path: &[usize], by_node: &mut HashMap<Vec<usize>, Vec<Assertion>>) {
    for (index, node) in nodes.iter().enumerate() {
        let mut node_path = parent_path.to_vec();
        node_path.push(index);
        if !node.assertions.is_empty() {
            by_node.insert(node_path.clone(), node.assertions.clone());
        }
        collect_saved(&node.children, &node_path, by_node);
    }
}
