import os
import re

def process_ast():
    with open('src/ast.rs', 'r') as f:
        content = f.read()
    
    span_def = """
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl Span {
    pub fn merge(&self, other: &Span) -> Span {
        if self.line == other.line {
            Span {
                line: self.line,
                column: self.column,
                length: (other.column + other.length) - self.column,
            }
        } else {
            self.clone()
        }
    }
}
"""
    content = span_def + "\n" + content
    content = content.replace("pub enum Expression {", "pub enum ExpressionKind {")
    expr_struct = """
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}
"""
    content = content.replace("pub enum ExpressionKind {", expr_struct + "\npub enum ExpressionKind {")
    
    content = content.replace("pub enum Statement {", "pub enum StatementKind {")
    stmt_struct = """
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}
"""
    content = content.replace("pub enum StatementKind {", stmt_struct + "\npub enum StatementKind {")
    
    with open('src/ast.rs', 'w') as f:
        f.write(content)

def process_lexer():
    with open('src/lexer.rs', 'r') as f:
        content = f.read()
    
    content = content.replace("pub column: usize,\n}", "pub column: usize,\n    pub length: usize,\n}")
    content = re.sub(r'Token \{ token_type: (.*?), line: (.*?), column: (.*?) \}', r'Token { token_type: \1, line: \2, column: \3, length: 1 }', content)
    
    content = content.replace(
        "Token { token_type, line: self.line, column: start_col, length: 1 }",
        "Token { token_type, line: self.line, column: start_col, length: val.len() }"
    )
    content = content.replace(
        "Ok(Token { token_type: TokenType::Float(f), line: self.line, column: start_col, length: 1 })",
        "Ok(Token { token_type: TokenType::Float(f), line: self.line, column: start_col, length: val.len() })"
    )
    content = content.replace(
        "Ok(Token { token_type: TokenType::Integer(i), line: self.line, column: start_col, length: 1 })",
        "Ok(Token { token_type: TokenType::Integer(i), line: self.line, column: start_col, length: val.len() })"
    )
    content = content.replace(
        "Ok(Token { token_type: TokenType::String(val), line: self.line, column: start_col, length: 1 })",
        "Ok(Token { token_type: TokenType::String(val.clone()), line: self.line, column: start_col, length: val.len() + 2 })"
    )
    content = content.replace(
        "return Ok(Token { token_type: TokenType::Op(op), line: self.line, column: start_col, length: 1 });",
        "return Ok(Token { token_type: TokenType::Op(op.clone()), line: self.line, column: start_col, length: op.len() });"
    )
    content = content.replace(
        "return Ok(Token { token_type: TokenType::Punct(op), line: self.line, column: start_col, length: 1 });",
        "return Ok(Token { token_type: TokenType::Punct(op.clone()), line: self.line, column: start_col, length: op.len() });"
    )
    
    with open('src/lexer.rs', 'w') as f:
        f.write(content)

def process_parser_and_interpreter():
    # Basic textual replace for Expression:: -> ExpressionKind:: and Statement:: -> StatementKind::
    for filename in ['src/parser.rs', 'src/interpreter.rs']:
        with open(filename, 'r') as f:
            content = f.read()
            
        content = content.replace("Expression::", "ExpressionKind::")
        content = content.replace("Statement::", "StatementKind::")
        
        # in parser, we need to wrap ExpressionKind with Expression
        # this is tricky, so maybe we just do the basic enum rename and fix compilation manually
        with open(filename, 'w') as f:
            f.write(content)

if __name__ == '__main__':
    process_ast()
    process_lexer()
    process_parser_and_interpreter()
