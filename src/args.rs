use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    All,
    File(PathBuf),
    Files(Vec<PathBuf>),
    Describe { file: PathBuf, full_name: String },
    Test { file: PathBuf, full_name: String },
    Failed(Vec<(PathBuf, String)>),
    SavedTestFile(PathBuf),
    SavedSource(PathBuf),
}

pub fn build(scope: &Scope, output_file: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--testLocationInResults".into(),
        "--json".into(),
        "--useStderr".into(),
        "--outputFile".into(),
        output_file.display().to_string(),
        "--no-color".into(),
        "--watchAll=false".into(),
    ];
    match scope {
        Scope::Test { file, full_name } => {
            push_by_path(&mut args, [file]);
            args.push("--testNamePattern".into());
            args.push(format!("{}$", escape(full_name)));
        }
        Scope::Describe { file, full_name } => {
            push_by_path(&mut args, [file]);
            args.push("--testNamePattern".into());
            args.push(escape(full_name));
        }
        Scope::All => {}
        Scope::File(file) | Scope::SavedTestFile(file) => push_by_path(&mut args, [file]),
        Scope::Files(files) => push_by_path(&mut args, files),
        Scope::SavedSource(file) => {
            args.push("--findRelatedTests".into());
            args.push(file.display().to_string());
        }
        Scope::Failed(failures) => {
            let mut files: Vec<&PathBuf> = Vec::new();
            for (file, _) in failures {
                if !files.contains(&file) {
                    files.push(file);
                }
            }
            push_by_path(&mut args, files);

            let anchored: Vec<String> = failures
                .iter()
                .map(|(_, full_name)| format!("{}$", escape(full_name)))
                .collect();
            args.push("--testNamePattern".into());
            args.push(format!("({})", anchored.join("|")));
        }
    }
    args
}

fn push_by_path<'a>(args: &mut Vec<String>, files: impl IntoIterator<Item = &'a PathBuf>) {
    args.push("--runTestsByPath".into());
    args.extend(files.into_iter().map(|file| file.display().to_string()));
}

fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c == '\n' {
            out.push_str("\\n");
            continue;
        }
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
