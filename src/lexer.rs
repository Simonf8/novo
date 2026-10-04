#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Identifier(String),
    Integer(i64),
    Float(f64),
    String(String),
    FString(String),
    Keyword(String),
    Op(String),
    Punct(String),
    Indent,
    Dedent,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub token_type: TokenType,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn length(&self) -> usize {
        match &self.token_type {
            TokenType::Identifier(s) => s.len(),
            TokenType::Integer(i) => i.to_string().len(),
            TokenType::Float(f) => f.to_string().len(),
            TokenType::String(s) => s.len() + 2,
            TokenType::FString(s) => s.len() + 3,
            TokenType::Keyword(k) => k.len(),
            TokenType::Op(o) => o.len(),
            TokenType::Punct(p) => p.len(),
            _ => 1,
        }
    }
}

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    indent_stack: Vec<usize>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            indent_stack: vec![0],
        }
    }

    fn peek(&self) -> char {
        if self.pos >= self.source.len() {
            '\0'
        } else {
            self.source[self.pos]
        }
    }

    #[allow(dead_code)]
    fn peek_next(&self) -> char {
        if self.pos + 1 >= self.source.len() {
            '\0'
        } else {
            self.source[self.pos + 1]
        }
    }

    fn advance(&mut self) -> char {
        let char = self.peek();
        if char == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        self.pos += 1;
        char
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        while self.pos < self.source.len() {
            let c = self.peek();
            
            if c == '\n' {
                self.advance();
                let indents = self.handle_indentation()?;
                tokens.extend(indents);
            } else if c.is_whitespace() {
                self.advance();
            } else if c == '#' {
                self.skip_comment();
            } else if c.is_alphabetic() || c == '_' {
                tokens.push(self.read_identifier());
            } else if c.is_ascii_digit() {
                tokens.push(self.read_number()?);
            } else if c == 'f' && self.peek_next() == '"' {
                tokens.push(self.read_fstring()?);
            } else if c == '"' {
                tokens.push(self.read_string()?);
            } else {
                tokens.push(self.read_operator_or_punct()?);
            }
        }
        
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            tokens.push(Token { token_type: TokenType::Dedent, line: self.line, column: self.column });
        }
        tokens.push(Token { token_type: TokenType::Eof, line: self.line, column: self.column });
        Ok(tokens)
    }

    fn handle_indentation(&mut self) -> Result<Vec<Token>, String> {
        let mut indent = 0;
        let mut tokens = Vec::new();
        
        while self.peek() == ' ' || self.peek() == '\t' {
            if self.peek() == '\t' {
                indent += 4;
            } else {
                indent += 1;
            }
            self.advance();
        }
        
        if self.peek() == '\n' || self.peek() == '#' || self.peek() == '\0' {
            return Ok(tokens); // ignore empty lines
        }
        
        let current_indent = *self.indent_stack.last().unwrap();
        if indent > current_indent {
            self.indent_stack.push(indent);
            tokens.push(Token { token_type: TokenType::Indent, line: self.line, column: self.column });
        } else if indent < current_indent {
            while let Some(&last) = self.indent_stack.last() {
                if indent < last {
                    self.indent_stack.pop();
                    tokens.push(Token { token_type: TokenType::Dedent, line: self.line, column: self.column });
                } else if indent == last {
                    break;
                } else {
                    return Err(format!("Inconsistent indentation at line {}, col {}", self.line, self.column));
                }
            }
        }
        Ok(tokens)
    }

    fn skip_comment(&mut self) {
        while self.peek() != '\n' && self.peek() != '\0' {
            self.advance();
        }
    }

    fn read_identifier(&mut self) -> Token {
        let start_col = self.column;
        let mut val = String::new();
        while self.peek().is_alphanumeric() || self.peek() == '_' {
            val.push(self.advance());
        }
        let token_type = match val.as_str() {
            "show" | "if" | "otherwise" | "repeat" | "times" | "for" | "in" | "while" | 
            "function" | "return" | "true" | "false" | "record" | "object" | "where" | 
            "select" | "average" | "sum" | "min" | "max" | "task" | "run" | "parallel" | "use" | "python" | "as" | "and" | "or" | "not" | "starts_with" | "ends_with" |
            "together" | "read" | "json" | "get" | "save" | "to" | "try" | "catch" | "break" | "continue" | "class" => {
                TokenType::Keyword(val)
            },
            _ => TokenType::Identifier(val),
        };
        
        Token { token_type, line: self.line, column: start_col }
    }

    fn read_number(&mut self) -> Result<Token, String> {
        let start_col = self.column;
        let mut val = String::new();
        let mut is_float = false;
        
        while self.peek().is_ascii_digit() || (self.peek() == '.' && !is_float) {
            if self.peek() == '.' {
                is_float = true;
            }
            val.push(self.advance());
        }
        
        if is_float {
            let f = val.parse::<f64>().map_err(|_| "Invalid float")?;
            Ok(Token { token_type: TokenType::Float(f), line: self.line, column: start_col })
        } else {
            let i = val.parse::<i64>().map_err(|_| "Invalid integer")?;
            Ok(Token { token_type: TokenType::Integer(i), line: self.line, column: start_col })
        }
    }

    fn read_string(&mut self) -> Result<Token, String> {
        let start_col = self.column;
        let start_line = self.line;
        self.advance(); // skip quote

        let mut val = String::new();
        
        while self.peek() != '"' && self.peek() != '\0' {
            if self.peek() == '\\' {
                self.advance();
                let c = self.advance();
                match c {
                    'n' => val.push('\n'),
                    't' => val.push('\t'),
                    '"' => val.push('"'),
                    '\\' => val.push('\\'),
                    _ => val.push(c),
                }
            } else {
                val.push(self.advance());
            }
        }
        
        if self.peek() == '\0' {
            return Err(format!("Unterminated string at line {}, col {}", start_line, start_col));
        }
        self.advance(); // skip quote
        
        Ok(Token { token_type: TokenType::String(val), line: self.line, column: start_col })
    }

    fn read_fstring(&mut self) -> Result<Token, String> {
        let start_col = self.column;
        let start_line = self.line;
        self.advance(); // skip f
        self.advance(); // skip quote

        let mut val = String::new();
        
        while self.peek() != '"' && self.peek() != '\0' {
            if self.peek() == '\\' {
                self.advance();
                let c = self.advance();
                match c {
                    'n' => val.push('\n'),
                    't' => val.push('\t'),
                    '"' => val.push('"'),
                    '\\' => val.push('\\'),
                    '{' => val.push('{'),
                    '}' => val.push('}'),
                    _ => val.push(c),
                }
            } else {
                val.push(self.advance());
            }
        }
        
        if self.peek() == '\0' {
            return Err(format!("Unterminated f-string at line {}, col {}", start_line, start_col));
        }
        self.advance(); // skip quote
        
        Ok(Token { token_type: TokenType::FString(val), line: self.line, column: start_col })
    }

    fn read_operator_or_punct(&mut self) -> Result<Token, String> {
        let start_col = self.column;
        let char = self.advance();
        let next_char = self.peek();
        
        let mut op = String::new();
        op.push(char);
        
        if char == '=' && next_char == '=' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if char == '!' && next_char == '=' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if char == '<' && next_char == '=' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if char == '>' && next_char == '=' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if char == '-' && next_char == '>' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if char == '=' && next_char == '>' {
            op.push(self.advance());
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        
        if "+-*/%=<>:.".contains(char) {
            return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col });
        }
        if "()[]{},".contains(char) {
            return Ok(Token { token_type: TokenType::Punct(op), line: self.line, column: start_col });
        }
        
        Err(format!("Unexpected character: {}", char))
    }
}
