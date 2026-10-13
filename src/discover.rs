use serde::{Deserialize, Serialize};
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Describe,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestNode {
    pub name: String,
    pub kind: NodeKind,
    pub start_line: u32,
    pub end_line: u32,
    pub dynamic: bool,
    pub children: Vec<TestNode>,
}

pub fn parse(_path: &Path, source: &str) -> Vec<TestNode> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .expect("typescript grammar");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    collect(tree.root_node(), source, &mut nodes);
    nodes
}

fn collect(node: Node, source: &str, out: &mut Vec<TestNode>) {
    if let Some(mut found) = test_call(node, source) {
        if let Some(args) = node.child_by_field_name("arguments") {
            collect(args, source, &mut found.children);
        }
        out.push(found);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, out);
    }
}

fn test_call(node: Node, source: &str) -> Option<TestNode> {
    if node.kind() != "call_expression" {
        return None;
    }
    let callee = node.child_by_field_name("function")?;
    let (kind, is_each) = match callee.kind() {
        "call_expression" => (each_kind(callee, source)?, true),
        _ => (callee_kind(callee, source)?, false),
    };
    let args = node.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    let (name, dynamic_name) = name_of(first, source);
    Some(TestNode {
        name,
        kind,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        dynamic: is_each || dynamic_name,
        children: Vec::new(),
    })
}

fn each_kind(table_call: Node, source: &str) -> Option<NodeKind> {
    let member = table_call.child_by_field_name("function")?;
    if member.kind() != "member_expression" {
        return None;
    }
    if text(member.child_by_field_name("property")?, source) != "each" {
        return None;
    }
    callee_kind(member.child_by_field_name("object")?, source)
}

fn name_of(arg: Node, source: &str) -> (String, bool) {
    let raw = text(arg, source);
    match arg.kind() {
        "string" => (unescape(&raw[1..raw.len() - 1]), false),
        "template_string" => {
            let mut cursor = arg.walk();
            let has_substitution = arg
                .named_children(&mut cursor)
                .any(|child| child.kind() == "template_substitution");
            (raw[1..raw.len() - 1].to_string(), has_substitution)
        }
        _ => (raw.to_string(), true),
    }
}

fn unescape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    let mut chars = literal.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn callee_kind(callee: Node, source: &str) -> Option<NodeKind> {
    let base = match callee.kind() {
        "identifier" => callee,
        "member_expression" => {
            let modifier = text(callee.child_by_field_name("property")?, source);
            if !matches!(modifier, "only" | "skip" | "todo") {
                return None;
            }
            callee.child_by_field_name("object")?
        }
        _ => return None,
    };
    if base.kind() != "identifier" {
        return None;
    }
    match text(base, source) {
        "describe" | "fdescribe" | "xdescribe" => Some(NodeKind::Describe),
        "it" | "test" | "fit" | "xit" | "xtest" => Some(NodeKind::Test),
        _ => None,
    }
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
