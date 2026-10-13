use std::path::Path;

pub fn failure_line(message: &str, file: &Path) -> Option<usize> {
    message.lines().find_map(|line| {
        let frame = line.trim_start().strip_prefix("at ")?;
        let location = frame.rsplit_once('(').map_or(frame, |(_, rest)| rest.trim_end_matches(')'));
        let mut parts = location.rsplitn(3, ':');
        let _column = parts.next()?;
        let line_number = parts.next()?.parse().ok()?;
        (Path::new(parts.next()?) == file).then_some(line_number)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Function,
}

pub fn highlight(source: &str) -> Vec<Vec<(Token, String)>> {
    let mut parser = tree_sitter::Parser::new();
    let mut ranges = Vec::new();
    if parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).is_ok()
        && let Some(tree) = parser.parse(source, None)
    {
        collect(tree.root_node(), &mut ranges);
    }
    ranges.sort_by_key(|(start, _, _)| *start);

    let mut lines = vec![Vec::new()];
    let mut cursor = 0;
    for (start, end, token) in ranges {
        push_text(&mut lines, Token::Plain, &source[cursor..start]);
        push_text(&mut lines, token, &source[start..end]);
        cursor = end;
    }
    push_text(&mut lines, Token::Plain, &source[cursor..]);
    if source.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn collect(node: tree_sitter::Node, out: &mut Vec<(usize, usize, Token)>) {
    let token = match node.kind() {
        "string" | "template_string" | "regex" => Some(Token::String),
        "comment" => Some(Token::Comment),
        "number" | "true" | "false" | "null" | "undefined" => Some(Token::Number),
        kind if !node.is_named() && kind.chars().all(|c| c.is_ascii_lowercase()) => Some(Token::Keyword),
        _ => None,
    };
    if let Some(token) = token {
        out.push((node.start_byte(), node.end_byte(), token));
        return;
    }
    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
    {
        let name = if function.kind() == "member_expression" { function.child_by_field_name("property") } else { Some(function) };
        if let Some(name) = name.filter(|name| matches!(name.kind(), "identifier" | "property_identifier")) {
            out.push((name.start_byte(), name.end_byte(), Token::Function));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, out);
    }
}

fn push_text(lines: &mut Vec<Vec<(Token, String)>>, token: Token, text: &str) {
    for (index, part) in text.split('\n').enumerate() {
        if index > 0 {
            lines.push(Vec::new());
        }
        if part.is_empty() {
            continue;
        }
        if let Some(line) = lines.last_mut() {
            line.push((token, part.to_string()));
        }
    }
}
