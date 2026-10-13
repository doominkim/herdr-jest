use herdr_jest::discover::{parse, NodeKind, TestNode};
use std::path::Path;

fn node(name: &str, kind: NodeKind, start_line: u32, end_line: u32, children: Vec<TestNode>) -> TestNode {
    TestNode { name: name.into(), kind, start_line, end_line, dynamic: false, children }
}

#[test]
fn describe_with_nested_tests_yields_tree_with_one_indexed_line_ranges() {
    let source = "\
describe('math', () => {
  it('adds', () => {
    expect(1 + 1).toBe(2);
  });
  test('subtracts', () => {});
});
";
    let tree = parse(Path::new("math.spec.ts"), source);

    assert_eq!(
        tree,
        vec![node(
            "math",
            NodeKind::Describe,
            1,
            6,
            vec![
                node("adds", NodeKind::Test, 2, 4, vec![]),
                node("subtracts", NodeKind::Test, 5, 5, vec![]),
            ],
        )]
    );
}

#[test]
fn focus_skip_and_todo_variants_are_recognized_by_kind() {
    let source = "\
fdescribe('a', () => {});
xdescribe('b', () => {});
describe.only('c', () => {});
describe.skip('d', () => {});
fit('e', () => {});
xit('f', () => {});
xtest('g', () => {});
it.only('h', () => {});
test.skip('i', () => {});
test.todo('j');
it.todo('k');
";
    let tree = parse(Path::new("v.spec.ts"), source);
    let summary: Vec<(String, NodeKind)> = tree.into_iter().map(|n| (n.name, n.kind)).collect();

    assert_eq!(
        summary,
        vec![
            ("a".into(), NodeKind::Describe),
            ("b".into(), NodeKind::Describe),
            ("c".into(), NodeKind::Describe),
            ("d".into(), NodeKind::Describe),
            ("e".into(), NodeKind::Test),
            ("f".into(), NodeKind::Test),
            ("g".into(), NodeKind::Test),
            ("h".into(), NodeKind::Test),
            ("i".into(), NodeKind::Test),
            ("j".into(), NodeKind::Test),
            ("k".into(), NodeKind::Test),
        ]
    );
}

#[test]
fn unrelated_calls_and_member_calls_are_ignored() {
    let source = "\
beforeEach(() => {});
foo.describe('x', () => {});
expect.it('y');
helper('z', () => {});
";
    assert_eq!(parse(Path::new("n.spec.ts"), source), vec![]);
}

#[test]
fn each_tables_become_one_dynamic_node_named_after_the_pattern() {
    let source = "\
describe.each([[1], [2]])('group %i', (n) => {
  it.each([[1, 1, 2]])('adds %i + %i', (a, b, sum) => {});
});
test.each`
  a    | b
  ${1} | ${2}
`('table $a $b', ({ a, b }) => {});
";
    let tree = parse(Path::new("e.spec.ts"), source);

    assert_eq!(
        tree,
        vec![
            TestNode {
                name: "group %i".into(),
                kind: NodeKind::Describe,
                start_line: 1,
                end_line: 3,
                dynamic: true,
                children: vec![TestNode {
                    name: "adds %i + %i".into(),
                    kind: NodeKind::Test,
                    start_line: 2,
                    end_line: 2,
                    dynamic: true,
                    children: vec![],
                }],
            },
            TestNode {
                name: "table $a $b".into(),
                kind: NodeKind::Test,
                start_line: 4,
                end_line: 7,
                dynamic: true,
                children: vec![],
            },
        ]
    );
}

#[test]
fn template_and_variable_names_are_dynamic_but_plain_backticks_are_literal() {
    let source = "\
test(`plain`, () => {});
test(`user ${id}`, () => {});
test(caseName, () => {});
";
    let tree = parse(Path::new("d.spec.ts"), source);
    let summary: Vec<(String, bool)> = tree.into_iter().map(|n| (n.name, n.dynamic)).collect();

    assert_eq!(
        summary,
        vec![
            ("plain".into(), false),
            ("user ${id}".into(), true),
            ("caseName".into(), true),
        ]
    );
}

#[test]
fn escaped_characters_in_names_are_unescaped_like_jest_sees_them() {
    let source = r#"
it('it\'s fine', () => {});
it("say \"hi\"", () => {});
it('back\\slash', () => {});
"#;
    let names: Vec<String> = parse(Path::new("s.spec.ts"), source).into_iter().map(|n| n.name).collect();

    assert_eq!(names, vec!["it's fine", "say \"hi\"", "back\\slash"]);
}

#[test]
fn tsx_files_with_jsx_are_parsed() {
    let source = "\
describe('Button', () => {
  it('renders', () => {
    render(<Button label=\"ok\" />);
  });
});
";
    let tree = parse(Path::new("Button.spec.tsx"), source);

    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children[0].name, "renders");
}
