use herdr_jest::args::{build, Scope};
use std::path::{Path, PathBuf};

const OUT: &str = "/tmp/hj/out.json";

fn base() -> Vec<String> {
    [
        "--testLocationInResults",
        "--json",
        "--useStderr",
        "--outputFile",
        OUT,
        "--no-color",
        "--watchAll=false",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn with(extra: &[&str]) -> Vec<String> {
    let mut args = base();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

#[test]
fn single_test_escapes_regex_specials_and_anchors_the_end() {
    let scope = Scope::Test {
        file: PathBuf::from("/repo/a.spec.ts"),
        full_name: "price (KRW) is 1.5 * $x? [a|b] {n} ^z \\ end".into(),
    };

    assert_eq!(
        build(&scope, Path::new(OUT)),
        with(&[
            "--runTestsByPath",
            "/repo/a.spec.ts",
            "--testNamePattern",
            "price \\(KRW\\) is 1\\.5 \\* \\$x\\? \\[a\\|b\\] \\{n\\} \\^z \\\\ end$",
        ])
    );
}

#[test]
fn describe_matches_by_prefix_without_end_anchor() {
    let scope = Scope::Describe { file: PathBuf::from("/repo/a.spec.ts"), full_name: "cart total".into() };

    assert_eq!(
        build(&scope, Path::new(OUT)),
        with(&["--runTestsByPath", "/repo/a.spec.ts", "--testNamePattern", "cart total"])
    );
}

#[test]
fn newline_in_name_becomes_literal_backslash_n() {
    let scope = Scope::Test { file: PathBuf::from("/repo/a.spec.ts"), full_name: "line one\nline two".into() };

    assert_eq!(
        build(&scope, Path::new(OUT)),
        with(&["--runTestsByPath", "/repo/a.spec.ts", "--testNamePattern", "line one\\nline two$"])
    );
}

#[test]
fn all_file_and_save_scopes_map_to_their_jest_selectors() {
    let file = PathBuf::from("/repo/a.spec.ts");
    let source = PathBuf::from("/repo/src/price.ts");

    assert_eq!(build(&Scope::All, Path::new(OUT)), base());
    assert_eq!(build(&Scope::File(file.clone()), Path::new(OUT)), with(&["--runTestsByPath", "/repo/a.spec.ts"]));
    assert_eq!(build(&Scope::SavedTestFile(file), Path::new(OUT)), with(&["--runTestsByPath", "/repo/a.spec.ts"]));
    assert_eq!(
        build(&Scope::SavedSource(source), Path::new(OUT)),
        with(&["--findRelatedTests", "/repo/src/price.ts"])
    );
}

#[test]
fn failed_only_groups_files_once_and_alternates_anchored_names() {
    let scope = Scope::Failed(vec![
        (PathBuf::from("/repo/a.spec.ts"), "cart adds (1)".into()),
        (PathBuf::from("/repo/b.spec.ts"), "login works".into()),
        (PathBuf::from("/repo/a.spec.ts"), "cart removes".into()),
    ]);

    assert_eq!(
        build(&scope, Path::new(OUT)),
        with(&[
            "--runTestsByPath",
            "/repo/a.spec.ts",
            "/repo/b.spec.ts",
            "--testNamePattern",
            "(cart adds \\(1\\)$|login works$|cart removes$)",
        ])
    );
}

#[test]
fn folder_scope_runs_each_listed_file_by_path() {
    let scope = Scope::Files(vec![PathBuf::from("/repo/a/one.spec.ts"), PathBuf::from("/repo/a/b/two.spec.ts")]);

    assert_eq!(
        build(&scope, Path::new(OUT)),
        with(&["--runTestsByPath", "/repo/a/one.spec.ts", "/repo/a/b/two.spec.ts"])
    );
}
