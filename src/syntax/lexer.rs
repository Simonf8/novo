use super::token::{FPart, Tok, Token};
use crate::diagnostic::Diagnostic;
use crate::source::{FileId, Span};

pub fn tokenize(file: FileId, src: &str) -> Result<Vec<Token>, Diagnostic> {
    Lexer { file, src, pos: 0, depth: 0, indents: vec![0], tokens: Vec::new(), at_line_start: true }.run()
}

/// Tokenizes a slice of a larger file, used for expressions embedded in f-strings.
pub fn tokenize_fragment(file: FileId, src: &str, lo: usize, hi: usize) -> Result<Vec<Token>, Diagnostic> {
    let mut lexer = Lexer { file, src: &src[..hi], pos: lo, depth: 1, indents: vec![0], tokens: Vec::new(), at_line_start: false };
    lexer.scan()?;
    let end = Span::new(file, hi, hi);
    lexer.tokens.push(Token { tok: Tok::Eof, span: end });
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    file: FileId,
    src: &'a str,
    pos: usize,
    depth: usize,
    indents: Vec<usize>,
    tokens: Vec<Token>,
    at_line_start: bool,
}

impl<'a> Lexer<'a> {
    fn run(mut self) -> Result<Vec<Token>, Diagnostic> {
        self.scan()?;
        let end = self.span(self.src.len(), self.src.len());
        if !matches!(self.tokens.last().map(|t| &t.tok), None | Some(Tok::Newline)) {
            self.tokens.push(Token { tok: Tok::Newline, span: end });
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.tokens.push(Token { tok: Tok::Dedent, span: end });
        }
        self.tokens.push(Token { tok: Tok::Eof, span: end });
        Ok(self.tokens)
    }

    fn span(&self, lo: usize, hi: usize) -> Span {
        Span::new(self.file, lo, hi)
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn push(&mut self, tok: Tok, lo: usize) {
        let span = self.span(lo, self.pos);
        self.tokens.push(Token { tok, span });
    }

    fn scan(&mut self) -> Result<(), Diagnostic> {
        while self.pos < self.src.len() {
            if self.at_line_start && self.depth == 0 {
                self.at_line_start = false;
                self.indentation()?;
                continue;
            }
            let lo = self.pos;
            let c = self.peek().unwrap();
            match c {
                '\n' => {
                    self.bump();
                    if self.depth == 0 {
                        if !matches!(self.tokens.last().map(|t| &t.tok), None | Some(Tok::Newline)) {
                            self.push(Tok::Newline, lo);
                        }
                        self.at_line_start = true;
                    }
                }
                ' ' | '\t' | '\r' => {
                    self.bump();
                }
                '#' => self.skip_comment(),
                'f' if matches!(self.peek_at(1), Some('"') | Some('\'')) => {
                    self.bump();
                    let parts = self.fstring(lo)?;
                    self.push(Tok::FStr(parts), lo);
                }
                '"' | '\'' => {
                    let s = self.string()?;
                    self.push(Tok::Str(s), lo);
                }
                c if c.is_ascii_digit() => self.number()?,
                c if c.is_alphabetic() || c == '_' => self.ident(),
                _ => self.operator()?,
            }
        }
        Ok(())
    }

    fn indentation(&mut self) -> Result<(), Diagnostic> {
        let lo = self.pos;
        let mut width = 0;
        while let Some(c) = self.peek() {
            match c {
                ' ' => width += 1,
                '\t' => width += 4,
                _ => break,
            }
            self.bump();
        }
        match self.peek() {
            None | Some('\n') | Some('\r') | Some('#') => return Ok(()),
            _ => {}
        }
        let current = *self.indents.last().unwrap();
        if width > current {
            self.indents.push(width);
            self.push(Tok::Indent, lo);
        } else {
            while width < *self.indents.last().unwrap() {
                self.indents.pop();
                self.push(Tok::Dedent, self.pos);
            }
            if width != *self.indents.last().unwrap() {
                return Err(Diagnostic::error(self.span(lo, self.pos), "inconsistent indentation")
                    .with_help("indent each block by the same amount as the lines around it"));
            }
        }
        Ok(())
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn ident(&mut self) {
        let lo = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        let word = &self.src[lo..self.pos];
        let tok = Tok::keyword(word).unwrap_or_else(|| Tok::Ident(word.to_string()));
        self.push(tok, lo);
    }

    fn digits(&mut self, out: &mut String, radix: u32) {
        while let Some(c) = self.peek() {
            if c.is_digit(radix) {
                out.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn number(&mut self) -> Result<(), Diagnostic> {
        let lo = self.pos;
        let mut text = String::new();
        if self.peek() == Some('0') && matches!(self.peek_at(1), Some('x') | Some('b')) {
            self.bump();
            let radix = if self.bump() == Some('x') { 16 } else { 2 };
            self.digits(&mut text, radix);
            let value = i64::from_str_radix(&text, radix)
                .map_err(|_| Diagnostic::error(self.span(lo, self.pos), "invalid integer literal"))?;
            self.push(Tok::Int(value), lo);
            return Ok(());
        }

        self.digits(&mut text, 10);
        let mut is_float = false;
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            text.push('.');
            self.bump();
            self.digits(&mut text, 10);
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let sign = self.peek_at(1);
            let has_exp = match sign {
                Some('+') | Some('-') => self.peek_at(2).is_some_and(|c| c.is_ascii_digit()),
                Some(c) => c.is_ascii_digit(),
                None => false,
            };
            if has_exp {
                is_float = true;
                text.push('e');
                self.bump();
                if let Some(s @ ('+' | '-')) = self.peek() {
                    text.push(s);
                    self.bump();
                }
                self.digits(&mut text, 10);
            }
        }

        let span = self.span(lo, self.pos);
        if is_float {
            let value: f64 = text.parse().map_err(|_| Diagnostic::error(span, "invalid number literal"))?;
            self.push(Tok::Float(value), lo);
        } else {
            let value: i64 = text.parse().map_err(|_| {
                Diagnostic::error(span, "integer literal is too large").with_help("integers are 64-bit; use a float for larger values")
            })?;
            self.push(Tok::Int(value), lo);
        }
        Ok(())
    }

    fn escape(&mut self, quote_lo: usize) -> Result<char, Diagnostic> {
        let lo = self.pos;
        self.bump();
        let c = self.bump().ok_or_else(|| Diagnostic::error(self.span(quote_lo, self.pos), "unterminated string"))?;
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' => '\\',
            '"' => '"',
            '\'' => '\'',
            '{' => '{',
            '}' => '}',
            'u' => {
                if self.bump() != Some('{') {
                    return Err(Diagnostic::error(self.span(lo, self.pos), "expected `{` after `\\u`"));
                }
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.bump();
                }
                let hex = &self.src[start..self.pos];
                if self.bump() != Some('}') {
                    return Err(Diagnostic::error(self.span(lo, self.pos), "expected `}` to close unicode escape"));
                }
                u32::from_str_radix(hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| Diagnostic::error(self.span(lo, self.pos), "invalid unicode escape"))?
            }
            other => {
                return Err(Diagnostic::error(self.span(lo, self.pos), format!("unknown escape `\\{other}`"))
                    .with_help("use `\\\\` for a literal backslash"));
            }
        })
    }

    fn string(&mut self) -> Result<String, Diagnostic> {
        let lo = self.pos;
        let quote = self.bump().unwrap();
        let triple = self.peek() == Some(quote) && self.peek_at(1) == Some(quote);
        if triple {
            self.bump();
            self.bump();
        }
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(Diagnostic::error(self.span(lo, self.pos), "unterminated string")),
                Some('\n') if !triple => {
                    return Err(Diagnostic::error(self.span(lo, self.pos), "unterminated string")
                        .with_help("use triple quotes for strings that span lines"));
                }
                Some('\\') => out.push(self.escape(lo)?),
                Some(c) if c == quote => {
                    if !triple {
                        self.bump();
                        return Ok(out);
                    }
                    if self.peek_at(1) == Some(quote) && self.peek_at(2) == Some(quote) {
                        self.bump();
                        self.bump();
                        self.bump();
                        return Ok(out);
                    }
                    out.push(c);
                    self.bump();
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn fstring(&mut self, lo: usize) -> Result<Vec<FPart>, Diagnostic> {
        let quote = self.bump().unwrap();
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err(Diagnostic::error(self.span(lo, self.pos), "unterminated f-string")),
                Some('\\') => lit.push(self.escape(lo)?),
                Some(c) if c == quote => {
                    self.bump();
                    break;
                }
                Some('{') if self.peek_at(1) == Some('{') => {
                    self.bump();
                    self.bump();
                    lit.push('{');
                }
                Some('}') if self.peek_at(1) == Some('}') => {
                    self.bump();
                    self.bump();
                    lit.push('}');
                }
                Some('{') => {
                    if !lit.is_empty() {
                        parts.push(FPart::Lit(std::mem::take(&mut lit)));
                    }
                    let open = self.pos;
                    self.bump();
                    parts.push(self.fstring_expr(open, quote)?);
                }
                Some('}') => {
                    return Err(Diagnostic::error(self.span(self.pos, self.pos + 1), "unmatched `}` in f-string")
                        .with_help("write `}}` for a literal brace"));
                }
                Some(c) => {
                    lit.push(c);
                    self.bump();
                }
            }
        }
        if !lit.is_empty() {
            parts.push(FPart::Lit(lit));
        }
        Ok(parts)
    }

    fn fstring_expr(&mut self, open: usize, quote: char) -> Result<FPart, Diagnostic> {
        let lo = self.pos;
        let mut depth = 0usize;
        let mut colon = None;
        loop {
            match self.peek() {
                None | Some('\n') => return Err(Diagnostic::error(self.span(open, self.pos), "unclosed `{` in f-string")),
                Some(c) if c == quote => return Err(Diagnostic::error(self.span(open, self.pos), "unclosed `{` in f-string")),
                Some('(') | Some('[') | Some('{') => depth += 1,
                Some(')') | Some(']') => depth = depth.saturating_sub(1),
                Some('}') if depth == 0 => break,
                Some('}') => depth -= 1,
                Some(':') if depth == 0 && colon.is_none() => colon = Some(self.pos),
                Some('"') | Some('\'') => {
                    self.string()?;
                    continue;
                }
                _ => {}
            }
            self.bump();
        }
        let close = self.pos;
        self.bump();
        let (hi, spec) = match colon {
            Some(c) => (c, Some(self.src[c + 1..close].to_string())),
            None => (close, None),
        };
        if self.src[lo..hi].trim().is_empty() {
            return Err(Diagnostic::error(self.span(open, close + 1), "empty expression in f-string"));
        }
        Ok(FPart::Expr { lo, hi, spec })
    }

    fn operator(&mut self) -> Result<(), Diagnostic> {
        let lo = self.pos;
        let c = self.bump().unwrap();
        let next = self.peek();
        let two = |lexer: &mut Self, tok: Tok| {
            lexer.bump();
            tok
        };
        let tok = match (c, next) {
            ('/', Some('/')) => two(self, Tok::SlashSlash),
            ('*', Some('*')) => two(self, Tok::StarStar),
            ('=', Some('=')) => two(self, Tok::EqEq),
            ('!', Some('=')) => two(self, Tok::NotEq),
            ('<', Some('=')) => two(self, Tok::Le),
            ('>', Some('=')) => two(self, Tok::Ge),
            ('-', Some('>')) => two(self, Tok::Arrow),
            ('=', Some('>')) => two(self, Tok::FatArrow),
            ('+', Some('=')) => two(self, Tok::PlusAssign),
            ('-', Some('=')) => two(self, Tok::MinusAssign),
            ('*', Some('=')) => two(self, Tok::StarAssign),
            ('/', Some('=')) => two(self, Tok::SlashAssign),
            ('+', _) => Tok::Plus,
            ('-', _) => Tok::Minus,
            ('*', _) => Tok::Star,
            ('/', _) => Tok::Slash,
            ('%', _) => Tok::Percent,
            ('@', _) => Tok::At,
            ('=', _) => Tok::Assign,
            ('<', _) => Tok::Lt,
            ('>', _) => Tok::Gt,
            (':', _) => Tok::Colon,
            ('.', _) => Tok::Dot,
            ('|', _) => Tok::Pipe,
            (',', _) => Tok::Comma,
            ('(', _) | ('[', _) | ('{', _) => {
                self.depth += 1;
                match c {
                    '(' => Tok::LParen,
                    '[' => Tok::LBracket,
                    _ => Tok::LBrace,
                }
            }
            (')', _) | (']', _) | ('}', _) => {
                self.depth = self.depth.saturating_sub(1);
                match c {
                    ')' => Tok::RParen,
                    ']' => Tok::RBracket,
                    _ => Tok::RBrace,
                }
            }
            ('!', _) => {
                return Err(Diagnostic::error(self.span(lo, self.pos), "unexpected `!`").with_help("use `not` for negation"));
            }
            _ => {
                return Err(Diagnostic::error(self.span(lo, self.pos), format!("unexpected character `{c}`")));
            }
        };
        self.push(tok, lo);
        Ok(())
    }
}
