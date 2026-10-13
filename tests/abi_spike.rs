use tree_sitter::Parser;

#[test]
fn typescript_grammar_loads_and_parses_describe_it() {
    let source = "describe('math', () => {\n  it('adds', () => {\n    expect(1 + 1).toBe(2);\n  });\n});\n";
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .expect("grammar ABI must match tree-sitter core");

    let tree = parser.parse(source, None).expect("parse");
    let sexp = tree.root_node().to_sexp();

    assert!(!tree.root_node().has_error());
    assert_eq!(sexp.matches("call_expression").count(), 4);
}
