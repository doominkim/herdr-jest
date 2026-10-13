use herdr_jest::discover::parse;
use herdr_jest::results::{Assertion, Model, Status};
use serde_json::json;
use std::path::{Path, PathBuf};

const FILE: &str = "/repo/cart.spec.ts";

const SOURCE: &str = "\
describe('cart', () => {
  it('adds', () => {});
  it('removes', () => {});
});
";

fn model() -> Model {
    let mut model = Model::default();
    model.set_tree(PathBuf::from(FILE), parse(Path::new(FILE), SOURCE));
    model
}

fn assertion(ancestors: &[&str], title: &str, status: &str, line: u32, failures: &[&str]) -> serde_json::Value {
    let mut full = ancestors.to_vec();
    full.push(title);
    json!({
        "ancestorTitles": ancestors,
        "title": title,
        "fullName": full.join(" "),
        "status": status,
        "location": { "line": line, "column": 3 },
        "failureMessages": failures,
        "duration": 4
    })
}

fn report(assertions: Vec<serde_json::Value>) -> String {
    json!({
        "success": false,
        "numTotalTests": assertions.len(),
        "testResults": [{
            "name": FILE,
            "status": "failed",
            "message": "",
            "assertionResults": assertions
        }]
    })
    .to_string()
}

#[test]
fn results_map_to_nodes_by_title_path_and_describe_aggregates_children() {
    let mut model = model();

    model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "passed", 2, &[]),
            assertion(&["cart"], "removes", "failed", 3, &["Error: expect(received).toBe(expected)\n\nExpected: 1\nReceived: 2\n    at Object.<anonymous> (cart.spec.ts:3:5)"]),
        ]))
        .unwrap();

    let file = Path::new(FILE);
    assert_eq!(model.status(file, &[0, 0]), Status::Passed);
    assert_eq!(model.status(file, &[0, 1]), Status::Failed);
    assert_eq!(model.status(file, &[0]), Status::Failed);
    assert_eq!(
        model.assertions(file, &[0, 1]),
        vec![Assertion {
            full_name: "cart removes".into(),
            status: Status::Failed,
            failure_messages: vec!["Error: expect(received).toBe(expected)\n\nExpected: 1\nReceived: 2\n    at Object.<anonymous> (cart.spec.ts:3:5)".into()],
            duration_ms: Some(4),
        }]
    );
}

const EACH_FILE: &str = "/repo/math.spec.ts";

const EACH_SOURCE: &str = "\
describe('math', () => {
  it.each([[1, 1], [2, 2], [3, 3]])('adds %i + %i', (a, b) => {
    expect(a + b).toBeGreaterThan(0);
  });
});
";

fn each_report(assertions: Vec<serde_json::Value>) -> String {
    json!({ "testResults": [{ "name": EACH_FILE, "status": "failed", "message": "", "assertionResults": assertions }] }).to_string()
}

#[test]
fn each_expansions_collect_on_one_dynamic_node_by_location_line() {
    let mut model = Model::default();
    model.set_tree(PathBuf::from(EACH_FILE), parse(Path::new(EACH_FILE), EACH_SOURCE));

    model
        .apply(&each_report(vec![
            assertion(&["math"], "adds 1 + 1", "passed", 2, &[]),
            assertion(&["math"], "adds 2 + 2", "failed", 2, &["boom"]),
            assertion(&["math"], "adds 3 + 3", "passed", 2, &[]),
        ]))
        .unwrap();

    let file = Path::new(EACH_FILE);
    let names: Vec<String> = model.assertions(file, &[0, 0]).into_iter().map(|a| a.full_name).collect();
    assert_eq!(names, vec!["math adds 1 + 1", "math adds 2 + 2", "math adds 3 + 3"]);
    assert_eq!(model.status(file, &[0, 0]), Status::Failed);
}

#[test]
fn results_matching_no_node_are_kept_as_unmatched_for_the_file() {
    let mut model = model();

    model
        .apply(&report(vec![assertion(&["ghost"], "appears", "failed", 99, &["nope"])]))
        .unwrap();

    let unmatched: Vec<String> = model.unmatched(Path::new(FILE)).into_iter().map(|a| a.full_name).collect();
    assert_eq!(unmatched, vec!["ghost appears"]);
}

#[test]
fn suite_level_failure_message_is_exposed_when_file_cannot_run() {
    let mut model = model();
    let crashed = json!({
        "testResults": [{
            "name": FILE,
            "status": "failed",
            "message": "  ● Test suite failed to run\n\n    TS2304: Cannot find name 'foo'.",
            "assertionResults": []
        }]
    })
    .to_string();

    model.apply(&crashed).unwrap();

    assert_eq!(
        model.suite_message(Path::new(FILE)),
        Some("  ● Test suite failed to run\n\n    TS2304: Cannot find name 'foo'.".to_string())
    );
    assert_eq!(model.file_status(Path::new(FILE)), Status::Failed);
}

#[test]
fn filtered_out_pending_result_keeps_previous_outcome_but_marks_unseen_test_skipped() {
    let mut model = model();
    model.apply(&report(vec![assertion(&["cart"], "adds", "passed", 2, &[])])).unwrap();

    model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "pending", 2, &[]),
            assertion(&["cart"], "removes", "pending", 3, &[]),
        ]))
        .unwrap();

    let file = Path::new(FILE);
    assert_eq!(model.status(file, &[0, 0]), Status::Passed);
    assert_eq!(model.status(file, &[0, 1]), Status::Skipped);
}

#[test]
fn apply_returns_only_tests_that_newly_failed() {
    let mut model = model();
    model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "passed", 2, &[]),
            assertion(&["cart"], "removes", "failed", 3, &["x"]),
        ]))
        .unwrap();

    let newly_failed = model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "failed", 2, &["y"]),
            assertion(&["cart"], "removes", "failed", 3, &["x"]),
            assertion(&["ghost"], "fresh", "failed", 99, &["z"]),
        ]))
        .unwrap();

    assert_eq!(
        newly_failed,
        vec![(PathBuf::from(FILE), "cart adds".to_string()), (PathBuf::from(FILE), "ghost fresh".to_string())]
    );
}

#[test]
fn failed_lists_every_currently_failing_test_including_unmatched() {
    let mut model = model();
    model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "failed", 2, &["a"]),
            assertion(&["cart"], "removes", "passed", 3, &[]),
            assertion(&["ghost"], "row 7", "failed", 99, &["b"]),
        ]))
        .unwrap();

    assert_eq!(
        model.failed(),
        vec![(PathBuf::from(FILE), "cart adds".to_string()), (PathBuf::from(FILE), "ghost row 7".to_string())]
    );
}

#[test]
fn ordinary_failing_suite_message_is_not_treated_as_a_crash() {
    let mut model = model();
    let failing = json!({
        "testResults": [{
            "name": FILE,
            "status": "failed",
            "message": "  ● cart › removes\n\n    expect(received).toBe(expected)",
            "assertionResults": [assertion(&["cart"], "removes", "failed", 3, &["x"])]
        }]
    })
    .to_string();

    model.apply(&failing).unwrap();

    assert_eq!(model.suite_message(Path::new(FILE)), None);
}

#[test]
fn results_without_location_still_map_by_title_path() {
    let mut model = model();
    let no_location = json!({
        "testResults": [{
            "name": FILE,
            "status": "passed",
            "message": "",
            "assertionResults": [{
                "ancestorTitles": ["cart"],
                "title": "removes",
                "fullName": "cart removes",
                "status": "passed",
                "failureMessages": []
            }]
        }]
    })
    .to_string();

    model.apply(&no_location).unwrap();

    assert_eq!(model.status(Path::new(FILE), &[0, 1]), Status::Passed);
}

#[test]
fn rerun_outcome_replaces_previous_outcome() {
    let mut model = model();
    model.apply(&report(vec![assertion(&["cart"], "adds", "passed", 2, &[])])).unwrap();

    model.apply(&report(vec![assertion(&["cart"], "adds", "failed", 2, &["now broken"])])).unwrap();

    let file = Path::new(FILE);
    assert_eq!(model.status(file, &[0, 0]), Status::Failed);
    assert_eq!(model.assertions(file, &[0, 0])[0].failure_messages, vec!["now broken".to_string()]);
}

#[test]
fn reparsed_tree_clears_stale_results_only_when_the_tree_changed() {
    let mut model = model();
    model.apply(&report(vec![assertion(&["cart"], "adds", "passed", 2, &[])])).unwrap();
    let file = Path::new(FILE);

    model.set_tree(PathBuf::from(FILE), parse(file, SOURCE));
    assert_eq!(model.status(file, &[0, 0]), Status::Passed);

    let edited = "\
describe('cart', () => {
  it('empties', () => {});
  it('adds', () => {});
});
";
    model.set_tree(PathBuf::from(FILE), parse(file, edited));
    assert_eq!(model.status(file, &[0, 0]), Status::Unknown);
    assert_eq!(model.status(file, &[0, 1]), Status::Unknown);
}

#[test]
fn restoring_saved_views_reproduces_results_until_the_tree_changes() {
    let mut model = model();
    model
        .apply(&report(vec![
            assertion(&["cart"], "adds", "passed", 2, &[]),
            assertion(&["cart"], "removes", "failed", 3, &["broken"]),
            assertion(&["ghost"], "row", "failed", 99, &["lost"]),
        ]))
        .unwrap();
    let saved = herdr_jest::protocol::file_views(&model);

    let mut restored = Model::default();
    restored.restore(&saved);
    restored.set_tree(PathBuf::from(FILE), parse(Path::new(FILE), SOURCE));

    let file = Path::new(FILE);
    assert_eq!(restored.status(file, &[0, 0]), Status::Passed);
    assert_eq!(restored.status(file, &[0, 1]), Status::Failed);
    assert_eq!(restored.assertions(file, &[0, 1])[0].failure_messages, vec!["broken".to_string()]);
    assert_eq!(restored.failed(), model.failed());
}

#[test]
fn removed_file_disappears_with_its_failures() {
    let mut model = model();
    model.apply(&report(vec![assertion(&["cart"], "adds", "failed", 2, &["x"])])).unwrap();

    model.remove_file(Path::new(FILE));

    assert_eq!(model.files().count(), 0);
    assert_eq!(model.failed(), vec![]);
}

#[test]
fn restoring_keeps_a_crashed_suite_message() {
    let mut model = model();
    let crashed = json!({
        "testResults": [{ "name": FILE, "status": "failed", "message": "Cannot find module", "assertionResults": [] }]
    })
    .to_string();
    model.apply(&crashed).unwrap();

    let mut restored = Model::default();
    restored.restore(&herdr_jest::protocol::file_views(&model));

    assert_eq!(restored.suite_message(Path::new(FILE)), Some("Cannot find module".to_string()));
}
