//! Smart indentation on top of the component's (which only indents on Enter):
//! a `}` typed on a blank line goes back to its `{` line's indent, the first
//! character typed on an empty line lands at the block's indent, and a pasted
//! block is re-indented to where it goes. Plain text in, plain text out.

/// Columns of a line's leading whitespace (a tab counts `tab` columns).
pub fn indent_cols(line: &str, tab: usize) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { tab } else { 1 }).sum()
}

pub fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// `cols` columns of indent in the file's style.
pub fn make_indent(cols: usize, tab: usize, hard_tabs: bool) -> String {
    if hard_tabs && tab > 0 {
        format!("{}{}", "\t".repeat(cols / tab), " ".repeat(cols % tab))
    } else {
        " ".repeat(cols)
    }
}

fn opens_block(line: &str) -> bool {
    matches!(line.trim_end().chars().last(), Some('{' | '(' | '['))
}

/// The indent of the line `row` of `lines` should have from the code above:
/// the previous non-blank line's, one level deeper after an opening bracket.
/// `first` is what the line starts (or will start) with: a closing bracket
/// takes a level off.
pub fn context_indent(lines: &[&str], row: usize, first: Option<char>, tab: usize) -> usize {
    let Some(prev) = lines[..row.min(lines.len())].iter().rev().find(|l| !l.trim().is_empty()) else { return 0 };
    let mut cols = indent_cols(prev, tab);
    if opens_block(prev) {
        cols += tab;
    }
    if matches!(first, Some('}' | ')' | ']')) {
        cols = cols.saturating_sub(tab);
    }
    cols
}

/// The indent of the line holding the `{` that a `}` typed at byte `at` closes.
/// Braces in strings and comments are counted too: a rare miss, not worth a parser.
pub fn matching_open_indent(text: &str, at: usize) -> Option<&str> {
    let mut depth = 0usize;
    for (i, ch) in text[..at.min(text.len())].char_indices().rev() {
        match ch {
            '}' => depth += 1,
            '{' if depth == 0 => {
                let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
                return Some(leading_ws(&text[line_start..]));
            }
            '{' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// `pasted` re-indented to start at `target` columns, keeping its lines'
/// relative indent. The first line is often copied from mid-line (no indent
/// of its own): then the rest set the base, one level in after a `{`.
pub fn reindent_block(pasted: &str, target: usize, tab: usize, hard_tabs: bool) -> String {
    let nl = if pasted.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = pasted.split(nl).collect();
    let first = lines[0];
    let first_has_indent = !leading_ws(first).is_empty() || first.trim().is_empty();
    let rest_min = lines[1..].iter().filter(|l| !l.trim().is_empty()).map(|l| indent_cols(l, tab)).min();
    // The column the pasted block's base level was at.
    let base = match (first_has_indent, rest_min) {
        (true, Some(m)) => m.min(indent_cols(first, tab)),
        (true, None) => indent_cols(first, tab),
        // `if (a) {` then its body: the body is a level in, unless its `}` is there too.
        (false, Some(m))
            if opens_block(first)
                && !lines[1..].iter().any(|l| indent_cols(l, tab) == m && l.trim_start().starts_with(['}', ')', ']'])) =>
        {
            m.saturating_sub(tab)
        }
        (false, Some(m)) => m,
        (false, None) => 0,
    };
    let mut out = String::with_capacity(pasted.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(nl);
        }
        let body = line.trim_start_matches([' ', '\t']);
        if body.is_empty() && i > 0 {
            continue;
        }
        let own = if i == 0 && !first_has_indent { base } else { indent_cols(line, tab) };
        out.push_str(&make_indent(target + own.saturating_sub(base), tab, hard_tabs));
        out.push_str(body);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_brace_matches_its_opener() {
        let text = "fn a() {\n    if x {\n        y();\n    }\n        ";
        assert_eq!(matching_open_indent(text, text.len()), Some(""));
        let text = "fn a() {\n    if x {\n        ";
        assert_eq!(matching_open_indent(text, text.len()), Some("    "));
    }

    #[test]
    fn empty_line_takes_the_block_indent() {
        let lines = ["void f() {", "    int a;", "", "}"];
        assert_eq!(context_indent(&lines, 2, Some('b'), 4), 4);
        let lines = ["void f() {", ""];
        assert_eq!(context_indent(&lines, 1, Some('x'), 4), 4);
        assert_eq!(context_indent(&lines, 1, Some('}'), 4), 0);
        assert_eq!(context_indent(&["", "x"], 0, Some('x'), 4), 0);
    }

    #[test]
    fn pasted_block_moves_to_the_target() {
        // Copied whole lines from a deeper block.
        let pasted = "        if (a) {\n            b();\n        }";
        assert_eq!(reindent_block(pasted, 4, 4, false), "    if (a) {\n        b();\n    }");
        // Copied from mid-line: the first line has no indent of its own.
        let pasted = "if (a) {\n            b();\n        }";
        assert_eq!(reindent_block(pasted, 4, 4, false), "    if (a) {\n        b();\n    }");
        assert_eq!(reindent_block("if (a) {\n            b();", 4, 4, false), "    if (a) {\n        b();");
        // Blank lines stay empty; tabs in a tab file.
        assert_eq!(reindent_block("  a\n\n    b", 4, 4, true), "\ta\n\n\t  b");
        assert_eq!(reindent_block("a\r\n  b", 0, 2, false), "a\r\nb");
    }
}
