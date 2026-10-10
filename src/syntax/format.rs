//! Source formatting. Works on the token stream rather than the syntax tree so
//! comments, blank lines and the author's line breaks inside brackets survive;
//! indentation and the spacing between tokens are normalized, and lines too
//! long to read are split one item per line inside their widest brackets.

use super::ast::QueryOp;
use super::lexer::tokenize;
use super::token::{Tok, Token};
use crate::diagnostic::Diagnostic;
use crate::source::FileId;

const INDENT: &str = "    ";
const MAX_BLANK_LINES: usize = 2;
const MAX_WIDTH: usize = 100;

/// Formats a whole file. Fails only when the file does not tokenize.
pub fn format(file: FileId, src: &str) -> Result<String, Diagnostic> {
    let tokens = tokenize(file, src)?;
    let mut f = Formatter {
        src,
        tokens: &tokens,
        out: String::new(),
        level: 0,
        brackets: Vec::new(),
        line: 0,
        blank: 0,
        line_start: true,
        closure_params: None,
    };
    let mut prev_end = 0;
    for (i, t) in tokens.iter().enumerate() {
        let (lo, hi) = (t.span.lo as usize, t.span.hi as usize);
        f.gap(&src[prev_end..lo.max(prev_end)], i);
        f.token(i);
        prev_end = prev_end.max(hi);
    }
    if !f.line_start {
        f.out.push('\n');
    }
    let mut wrapped = String::with_capacity(f.out.len());
    for line in f.out.lines() {
        wrap_line(line, &mut wrapped);
    }
    f.out = wrapped;
    // Keep Windows line endings in files that use them.
    let formatted = if src.contains("\r\n") {
        f.out.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        f.out
    };
    let after = tokenize(file, &formatted).ok();
    let text = |src: &str, t: &Token| match t.tok {
        Tok::FStr(_) => Some(src[t.span.lo as usize..t.span.hi as usize].to_string()),
        _ => None,
    };
    let same = after.is_some_and(|after| {
        after.len() == tokens.len()
            && after.iter().zip(&tokens).all(|(a, b)| {
                a.tok == b.tok || text(&formatted, a) == text(src, b) && text(src, b).is_some()
            })
    });
    if same {
        Ok(formatted)
    } else {
        Err(Diagnostic::error(
            tokens[0].span,
            "the formatter would change this file's meaning; it was left unchanged",
        ))
    }
}

/// Writes `line`, splitting it inside its widest brackets if it is too long
/// and those brackets hold a comma-separated list.
fn wrap_line(line: &str, out: &mut String) {
    let line = line.trim_end_matches('\r');
    let push = |out: &mut String, text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    if line.chars().count() <= MAX_WIDTH {
        return push(out, line);
    }
    let code = line.trim_start();
    let indent = &line[..line.len() - code.len()];
    let Some((open, commas, close)) = widest_list(code, indent.len()) else {
        return push(out, line);
    };
    let inner = format!("{indent}{INDENT}");
    wrap_line(&format!("{indent}{}", &code[..open]), out);
    let mut start = open;
    for &comma in commas.iter().chain(std::iter::once(&close)) {
        let item = code[start..comma].trim();
        let sep = if comma == close { "" } else { "," };
        wrap_line(&format!("{inner}{item}{sep}"), out);
        start = comma + 1;
    }
    wrap_line(&format!("{indent}{}", &code[close..]), out);
}

/// In one line of code, the brackets with the most text between them that
/// hold a comma at their own level: the byte offsets just after the opener,
/// of those commas, and of the closer. Brackets holding less than half the
/// line, or whose split would leave the text around them too long, are not
/// worth splitting.
fn widest_list(code: &str, indent: usize) -> Option<(usize, Vec<usize>, usize)> {
    let tokens = tokenize(FileId(0), code).ok()?;
    let last = tokens
        .iter()
        .rev()
        .find(|t| !matches!(t.tok, Tok::Newline | Tok::Eof | Tok::Dedent))?;
    if code[last.span.hi as usize..].contains('#') {
        return None;
    }
    let mut best: Option<(usize, Vec<usize>, usize)> = None;
    let mut depth = 0usize;
    let mut open = 0;
    let mut commas = Vec::new();
    let mut in_params = false;
    for t in &tokens {
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => {
                if depth == 0 {
                    open = t.span.hi as usize;
                    commas.clear();
                }
                depth += 1;
            }
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                depth = depth.checked_sub(1)?;
                let close = t.span.lo as usize;
                let wider = best.as_ref().is_none_or(|b| close - open > b.2 - b.0);
                let fits = indent + open <= MAX_WIDTH && indent + code.len() - close <= MAX_WIDTH;
                let big = 2 * (close - open) >= code.len();
                if depth == 0 && !commas.is_empty() && wider && fits && big {
                    best = Some((open, std::mem::take(&mut commas), close));
                }
            }
            Tok::Pipe if depth == 1 => in_params = !in_params,
            Tok::Comma if depth == 1 && !in_params => commas.push(t.span.lo as usize),
            _ => {}
        }
    }
    best.filter(|_| depth == 0)
}

struct Formatter<'a> {
    src: &'a str,
    tokens: &'a [Token],
    out: String,
    level: usize,
    /// Open brackets, each with the output line it was opened on.
    brackets: Vec<(Tok, usize)>,
    line: usize,
    blank: usize,
    line_start: bool,
    /// Bracket depth of the closure parameter list being written, if any.
    closure_params: Option<usize>,
}

impl Formatter<'_> {
    /// Writes the comments and line breaks found between two tokens.
    fn gap(&mut self, gap: &str, next: usize) {
        for (k, segment) in gap.split('\n').enumerate() {
            if k > 0 {
                if self.line_start {
                    self.blank += 1;
                } else {
                    self.break_line();
                }
            }
            if let Some(at) = segment.find('#') {
                let comment = segment[at..].trim_end();
                if self.line_start {
                    let column = segment[..at].chars().count();
                    let level = self.comment_level(next, column);
                    self.begin_line(level);
                } else {
                    self.out.push_str("  ");
                }
                self.out.push_str(comment);
            }
        }
    }

    /// Indentation for a comment on its own line: that of the next line of code,
    /// unless the comment sits deeper than it, at the end of a block.
    fn comment_level(&self, next: usize, column: usize) -> usize {
        if !self.brackets.is_empty() {
            return self.level + self.continuation(usize::MAX);
        }
        let mut level = self.level;
        let mut i = next;
        while let Some(t) = self.tokens.get(i) {
            match t.tok {
                Tok::Indent => level += 1,
                Tok::Dedent => level = level.saturating_sub(1),
                _ => break,
            }
            i += 1;
        }
        let code_column = self
            .tokens
            .get(i)
            .map_or(0, |t| self.column(t.span.lo as usize));
        if level < self.level && column > code_column {
            self.level
        } else {
            level
        }
    }

    fn column(&self, pos: usize) -> usize {
        let start = self.src[..pos].rfind('\n').map_or(0, |n| n + 1);
        self.src[start..pos].chars().count()
    }

    fn break_line(&mut self) {
        self.out.push('\n');
        self.line += 1;
        self.line_start = true;
    }

    fn begin_line(&mut self, level: usize) {
        if !self.out.is_empty() {
            for _ in 0..self.blank.min(MAX_BLANK_LINES) {
                self.out.push('\n');
                self.line += 1;
            }
        }
        self.blank = 0;
        for _ in 0..level {
            self.out.push_str(INDENT);
        }
        self.line_start = false;
    }

    fn token(&mut self, i: usize) {
        let t = &self.tokens[i];
        match t.tok {
            Tok::Indent => return self.level += 1,
            Tok::Dedent => return self.level = self.level.saturating_sub(1),
            Tok::Eof => return,
            Tok::Newline => {
                if !self.line_start {
                    self.break_line();
                }
                return;
            }
            _ => {}
        }
        let closer = matches!(t.tok, Tok::RParen | Tok::RBracket | Tok::RBrace);
        if self.line_start {
            let below = if closer {
                self.brackets.last().map_or(0, |b| b.1)
            } else {
                usize::MAX
            };
            self.begin_line(self.level + self.continuation(below));
        } else if self.space_before(i) {
            self.out.push(' ');
        }
        self.out
            .push_str(&self.src[t.span.lo as usize..t.span.hi as usize]);
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => {
                self.brackets.push((t.tok.clone(), self.line))
            }
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                self.brackets.pop();
            }
            Tok::Pipe if self.closure_params == Some(self.brackets.len()) => {
                self.closure_params = None
            }
            Tok::Pipe => self.closure_params = Some(self.brackets.len()),
            _ => {}
        }
    }

    /// Continuation indent inside brackets: one step per line that opened
    /// brackets still open, counting only lines before `below`.
    fn continuation(&self, below: usize) -> usize {
        let mut lines: Vec<usize> = self
            .brackets
            .iter()
            .map(|b| b.1)
            .filter(|l| *l < below)
            .collect();
        lines.dedup();
        lines.len()
    }

    fn space_before(&self, i: usize) -> bool {
        let prev = &self.tokens[i - 1].tok;
        let cur = &self.tokens[i].tok;
        let innermost = self.brackets.last().map(|b| &b.0);
        let in_brackets = innermost == Some(&Tok::LBracket);
        if innermost == Some(&Tok::LParen) && (*cur == Tok::Assign || *prev == Tok::Assign) {
            let name = if *cur == Tok::Assign { i - 1 } else { i - 2 };
            let named = matches!(self.tokens[name].tok, Tok::Ident(_))
                && matches!(self.tokens[name - 1].tok, Tok::LParen | Tok::Comma);
            if named {
                return false;
            }
        }
        if *cur == Tok::LBracket
            && matches!(prev, Tok::Ident(w) if w == "run")
            && self.is_unary(i - 1)
        {
            return true;
        }
        match cur {
            Tok::RParen | Tok::RBracket | Tok::RBrace | Tok::Comma | Tok::Dot => return false,
            Tok::Colon => return false,
            Tok::LParen | Tok::LBracket if self.ends_value(i - 1) => return false,
            Tok::Pipe if self.closure_params == Some(self.brackets.len()) => return false,
            _ => {}
        }
        match prev {
            Tok::LParen | Tok::LBracket | Tok::LBrace | Tok::Dot => false,
            Tok::Colon => !in_brackets,
            Tok::Pipe => self.closure_params != Some(self.brackets.len()),
            Tok::Minus | Tok::Plus if self.is_unary(i - 1) => false,
            _ => true,
        }
    }

    /// Whether the token ends an operand, so a following `(` or `[` calls or indexes it.
    fn ends_value(&self, i: usize) -> bool {
        let after_dot = i > 0 && self.tokens[i - 1].tok == Tok::Dot;
        match &self.tokens[i].tok {
            Tok::Ident(w) => !(QueryOp::from_word(w).is_some() && i > 0 && self.ends_value(i - 1)),
            Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::FStr(_)
            | Tok::True
            | Tok::False
            | Tok::None => true,
            Tok::RParen | Tok::RBracket | Tok::RBrace => true,
            t => after_dot && t.keyword_text().is_some(),
        }
    }

    fn is_unary(&self, i: usize) -> bool {
        i == 0 || !self.ends_value(i - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format(FileId(0), src).unwrap()
    }

    #[test]
    fn normalizes_spacing_and_indentation() {
        let src = "function f( a:Int,b : Int )->Int:\n  if a>b :\n        return -a+b*2\n  return f(b,a)[0:2]\nx={\"a\":1}\n";
        let want = "function f(a: Int, b: Int) -> Int:\n    if a > b:\n        return -a + b * 2\n    return f(b, a)[0:2]\nx = {\"a\": 1}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn keeps_comments_blank_lines_and_breaks() {
        let src = "# head\n\n\n\n\nx = [1,\n       2]   # two\nif x:\n  y = 1\n  # inner\n# outer\nz = |a| a + 1\n";
        let want = "# head\n\n\nx = [1,\n    2]  # two\nif x:\n    y = 1\n    # inner\n# outer\nz = |a| a + 1\n";
        assert_eq!(fmt(src), want);
        assert_eq!(fmt(want), want);
    }

    #[test]
    fn wraps_long_lines() {
        let src = format!(
            "model = build(layers.linear({}, 10), layers.linear(10, 2), name=\"classifier\", seed=42, dropout=0.1)\n",
            "x".repeat(30)
        );
        let want = format!(
            "model = build(\n    layers.linear({}, 10),\n    layers.linear(10, 2),\n    name=\"classifier\",\n    seed=42,\n    dropout=0.1\n)\n",
            "x".repeat(30)
        );
        assert_eq!(fmt(&src), want);
        assert_eq!(fmt(&want), want);
        let closure = format!("ys = map(xs, |a, b| a + b, {})\n", "z".repeat(100));
        assert_eq!(
            fmt(&closure),
            format!(
                "ys = map(\n    xs,\n    |a, b| a + b,\n    {}\n)\n",
                "z".repeat(100)
            )
        );
    }

    #[test]
    fn keeps_windows_line_endings() {
        assert_eq!(
            fmt("x=1\r\nif x:\r\n  y = 2 # two\r\n"),
            "x = 1\r\nif x:\r\n    y = 2  # two\r\n"
        );
    }
}
