use herdr_jest::source::{failure_line, highlight, Token};
use std::path::Path;

const SPEC: &str = "/repo/src/cart/cart.spec.ts";

#[test]
fn failure_line_is_the_first_stack_frame_in_the_spec_file() {
    let message = "Error: expect(received).toBe(expected) // Object.is equality\n\n\
Expected: 90\nReceived: 100\n    \
at Object.toBe (/repo/node_modules/expect/build/index.js:300:10)\n    \
at Object.<anonymous> (/repo/src/cart/cart.spec.ts:20:28)\n    \
at Object.<anonymous> (/repo/src/cart/cart.spec.ts:31:5)";
    assert_eq!(failure_line(message, Path::new(SPEC)), Some(20));
}

#[test]
fn failure_line_reads_frames_without_a_function_name() {
    let message = "Error: boom\n    at /repo/src/cart/cart.spec.ts:7:11";
    assert_eq!(failure_line(message, Path::new(SPEC)), Some(7));
}

#[test]
fn failure_line_is_none_when_no_frame_is_in_the_spec_file() {
    let message = "Error: boom\n    at helper (/repo/src/cart/helper.ts:3:9)\n    \
at Object.<anonymous> (/repo/src/cart/cart.spec.ts.snap:4:1)";
    assert_eq!(failure_line(message, Path::new(SPEC)), None);
}

fn spans(line: &[(Token, String)]) -> Vec<(Token, &str)> {
    line.iter().map(|(token, text)| (*token, text.as_str())).collect()
}

#[test]
fn highlight_marks_calls_strings_numbers_and_comments() {
    let lines = highlight("it('adds', () => 1); // done");
    assert_eq!(
        spans(&lines[0]),
        vec![
            (Token::Function, "it"),
            (Token::Plain, "("),
            (Token::String, "'adds'"),
            (Token::Plain, ", () => "),
            (Token::Number, "1"),
            (Token::Plain, "); "),
            (Token::Comment, "// done"),
        ]
    );
}

#[test]
fn highlight_marks_the_method_name_of_a_member_call() {
    let lines = highlight("expect(total).toBe(90)");
    assert_eq!(
        spans(&lines[0]),
        vec![
            (Token::Function, "expect"),
            (Token::Plain, "(total)."),
            (Token::Function, "toBe"),
            (Token::Plain, "("),
            (Token::Number, "90"),
            (Token::Plain, ")"),
        ]
    );
}

#[test]
fn highlight_splits_multiline_tokens_into_source_lines() {
    let lines = highlight("const a = `x\ny`;\nawait run();\n");
    assert_eq!(lines.len(), 3);
    assert_eq!(spans(&lines[0]), vec![(Token::Keyword, "const"), (Token::Plain, " a = "), (Token::String, "`x")]);
    assert_eq!(spans(&lines[1]), vec![(Token::String, "y`"), (Token::Plain, ";")]);
    assert_eq!(
        spans(&lines[2]),
        vec![(Token::Keyword, "await"), (Token::Plain, " "), (Token::Function, "run"), (Token::Plain, "();")]
    );
}
