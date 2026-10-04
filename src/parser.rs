use crate::ast::*;
use crate::lexer::{Token, TokenType};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        if self.pos >= self.tokens.len() {
            &self.tokens[self.tokens.len() - 1]
        } else {
            &self.tokens[self.pos]
        }
    }

    fn peek_next(&self) -> Option<&Token> {
        if self.pos + 1 < self.tokens.len() {
            Some(&self.tokens[self.pos + 1])
        } else {
            None
        }
    }

    fn advance(&mut self) -> &Token {
        let _token = self.peek();
        self.pos += 1;
        &self.tokens[self.pos - 1]
    }

    fn match_token(&mut self, expected: &TokenType) -> bool {
        if self.peek().token_type == *expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error<T>(&self, message: &str) -> Result<T, String> {
        let token = self.peek();
        Err(format!("{} at line {}, col {}", message, token.line, token.column))
    }

    fn expect(&mut self, expected: &TokenType) -> Result<Token, String> {
        if self.peek().token_type == *expected {
            let tok = self.advance().clone();
            Ok(tok)
        } else {
            self.error(&format!("Expected {:?}, got {:?}", expected, self.peek().token_type))
        }
    }

    pub fn parse(&mut self) -> Result<Program, String> {
        let mut statements = Vec::new();
        while self.peek().token_type != TokenType::Eof {
            if self.match_token(&TokenType::Dedent) {
                continue;
            }
            statements.push(self.parse_statement()?);
        }
        Ok(Program { statements })
    }

    fn parse_block(&mut self) -> Result<Block, String> {
        self.expect(&TokenType::Indent)?;
        let mut statements = Vec::new();
        while self.peek().token_type != TokenType::Dedent && self.peek().token_type != TokenType::Eof {
            statements.push(self.parse_statement()?);
        }
        if self.peek().token_type == TokenType::Dedent {
            self.advance();
        }
        Ok(Block { statements })
    }

    fn parse_statement(&mut self) -> Result<Statement, String> {
        let token = self.peek().clone();
        match &token.token_type {
            TokenType::Keyword(k) if k == "use" => self.parse_use_statement(),
            TokenType::Keyword(k) if k == "show" => self.parse_show_statement(),
            TokenType::Keyword(k) if k == "if" => self.parse_if_statement(),
            TokenType::Keyword(k) if k == "repeat" => self.parse_repeat_statement(),
            TokenType::Keyword(k) if k == "function" => self.parse_function_declaration(),
            TokenType::Keyword(k) if k == "return" => self.parse_return_statement(),
            TokenType::Keyword(k) if k == "save" => self.parse_save_statement(),
            TokenType::Identifier(_) => self.parse_assignment_or_expression(),
            _ => self.error(&format!("Unexpected token {:?}", token.token_type)),
        }
    }

    fn parse_use_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("use".to_string()))?;
        self.expect(&TokenType::Keyword("python".to_string()))?;
        
        let mut module = match self.advance().token_type.clone() {
            TokenType::Identifier(n) => n,
            TokenType::Keyword(k) => k,
            _ => return self.error("Expected python module name"),
        };
        
        while self.match_token(&TokenType::Op(".".to_string())) {
            let next_part = match self.advance().token_type.clone() {
                TokenType::Identifier(n) => n,
                TokenType::Keyword(k) => k,
                _ => return self.error("Expected identifier after '.' in module name"),
            };
            module.push('.');
            module.push_str(&next_part);
        }
        
        let mut alias = None;
        if self.match_token(&TokenType::Keyword("as".to_string())) {
            let alias_tok = self.advance().clone();
            if let TokenType::Identifier(n) = alias_tok.token_type {
                alias = Some(n);
            } else {
                return self.error("Expected alias after 'as'");
            }
        }
        
        Ok(Statement::UsePython { module, alias })
    }

    fn parse_show_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("show".to_string()))?;
        let expr = self.parse_expression()?;
        Ok(Statement::Show { value: expr })
    }

    fn parse_save_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("save".to_string()))?;
        let data = self.parse_expression()?;
        self.expect(&TokenType::Keyword("to".to_string()))?;
        let destination = self.parse_expression()?;
        Ok(Statement::Save { data, destination })
    }

    fn parse_if_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("if".to_string()))?;
        let condition = self.parse_expression()?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let body = self.parse_block()?;
        
        let mut otherwise = None;
        if self.match_token(&TokenType::Keyword("otherwise".to_string())) {
            if self.match_token(&TokenType::Op(":".to_string())) {
                otherwise = Some(Box::new(Statement::If {
                    condition: Expression::Boolean(true),
                    body: self.parse_block()?,
                    otherwise: None,
                }));
            } else if self.peek().token_type == TokenType::Keyword("if".to_string()) {
                otherwise = Some(Box::new(self.parse_if_statement()?));
            } else {
                return self.error("Expected ':' or 'if' after 'otherwise'");
            }
        }
        
        Ok(Statement::If { condition, body, otherwise })
    }

    fn parse_repeat_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("repeat".to_string()))?;
        let count = self.parse_expression()?;
        self.expect(&TokenType::Keyword("times".to_string()))?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let body = self.parse_block()?;
        Ok(Statement::Repeat { count, body })
    }

    fn parse_assignment_or_expression(&mut self) -> Result<Statement, String> {
        if let Some(next) = self.peek_next() {
            if next.token_type == TokenType::Op("=".to_string()) || next.token_type == TokenType::Op(":".to_string()) {
                return self.parse_assignment();
            }
        }
        
        let expr = self.parse_expression()?;
        if let Expression::Call { .. } = expr {
            // Treat function calls as statements if they stand alone (by mapping them to dummy assignment or we can just return it if we add ExprStmt)
            // Let's add it as a dummy assignment to '_' for now, to keep AST simple.
            return Ok(Statement::Assignment {
                name: "_".to_string(),
                value: expr,
                type_annotation: None,
            });
        }
        self.error("Expressions as statements not fully supported yet")
    }

    fn parse_assignment(&mut self) -> Result<Statement, String> {
        let name_tok = self.advance().clone();
        let name = match name_tok.token_type {
            TokenType::Identifier(n) => n,
            _ => return self.error("Expected identifier"),
        };
        
        let mut type_annotation = None;
        if self.match_token(&TokenType::Op(":".to_string())) {
            let type_tok = self.advance().clone();
            if let TokenType::Identifier(t) = type_tok.token_type {
                type_annotation = Some(t);
            } else {
                return self.error("Expected type identifier");
            }
        }
        
        self.expect(&TokenType::Op("=".to_string()))?;
        let value = self.parse_expression()?;
        
        Ok(Statement::Assignment { name, value, type_annotation })
    }

    fn parse_function_declaration(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("function".to_string()))?;
        let name_tok = self.advance().clone();
        let name = match name_tok.token_type {
            TokenType::Identifier(n) => n,
            _ => return self.error("Expected function name"),
        };
        
        self.expect(&TokenType::Punct("(".to_string()))?;
        
        let mut params = Vec::new();
        if self.peek().token_type != TokenType::Punct(")".to_string()) {
            loop {
                let param_name_tok = self.advance().clone();
                let param_name = match param_name_tok.token_type {
                    TokenType::Identifier(n) => n,
                    _ => return self.error("Expected parameter name"),
                };
                
                let mut param_type = None;
                if self.match_token(&TokenType::Op(":".to_string())) {
                    let type_tok = self.advance().clone();
                    if let TokenType::Identifier(t) = type_tok.token_type {
                        param_type = Some(t);
                    } else {
                        return self.error("Expected parameter type");
                    }
                }
                
                params.push((param_name, param_type));
                
                if self.match_token(&TokenType::Punct(",".to_string())) {
                    continue;
                } else {
                    break;
                }
            }
        }
        self.expect(&TokenType::Punct(")".to_string()))?;
        
        let mut return_type = None;
        if self.match_token(&TokenType::Op("->".to_string())) {
            let type_tok = self.advance().clone();
            if let TokenType::Identifier(t) = type_tok.token_type {
                return_type = Some(t);
            } else {
                return Err("Expected return type".to_string());
            }
        }
        
        let body;
        if self.match_token(&TokenType::Op("=>".to_string())) {
            let expr = self.parse_expression()?;
            body = Block { statements: vec![Statement::Return { value: Some(expr) }] };
        } else {
            self.expect(&TokenType::Op(":".to_string()))?;
            body = self.parse_block()?;
        }
        
        Ok(Statement::FunctionDeclaration { name, params, return_type, body })
    }

    fn parse_return_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("return".to_string()))?;
        let peek_type = &self.peek().token_type;
        let value = match peek_type {
            TokenType::Dedent | TokenType::Eof => None,
            TokenType::Keyword(k) if k != "true" && k != "false" => None,
            _ => Some(self.parse_expression()?),
        };
        Ok(Statement::Return { value })
    }

    fn parse_expression(&mut self) -> Result<Expression, String> {
        self.parse_query()
    }

    fn parse_query(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_logical_or()?;
        
        loop {
            if self.match_token(&TokenType::Keyword("where".to_string())) {
                let condition = self.parse_logical_or()?;
                left = Expression::Where { target: Box::new(left), condition: Box::new(condition) };
            } else if self.match_token(&TokenType::Keyword("select".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = Expression::Select { target: Box::new(left), mapping: Box::new(mapping) };
            } else if self.match_token(&TokenType::Keyword("average".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = Expression::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "average".to_string() };
            } else if self.match_token(&TokenType::Keyword("sum".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = Expression::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "sum".to_string() };
            } else if self.match_token(&TokenType::Keyword("min".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = Expression::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "min".to_string() };
            } else if self.match_token(&TokenType::Keyword("max".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = Expression::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "max".to_string() };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_logical_or(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_logical_and()?;
        while self.match_token(&TokenType::Keyword("or".to_string())) {
            let right = self.parse_logical_and()?;
            left = Expression::BinaryOp { left: Box::new(left), operator: "or".to_string(), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_comparison()?;
        while self.match_token(&TokenType::Keyword("and".to_string())) {
            let right = self.parse_comparison()?;
            left = Expression::BinaryOp { left: Box::new(left), operator: "and".to_string(), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_addition()?;
        
        loop {
            if let TokenType::Op(op) = &self.peek().token_type {
                if ["==", "!=", "<", ">", "<=", ">="].contains(&op.as_str()) {
                    let operator = op.clone();
                    self.advance();
                    let right = self.parse_addition()?;
                    left = Expression::BinaryOp { left: Box::new(left), operator, right: Box::new(right) };
                    continue;
                }
            } else if let TokenType::Keyword(kw) = &self.peek().token_type {
                if ["starts_with", "ends_with"].contains(&kw.as_str()) {
                    let operator = kw.clone();
                    self.advance();
                    let right = self.parse_addition()?;
                    left = Expression::BinaryOp { left: Box::new(left), operator, right: Box::new(right) };
                    continue;
                }
            }
            break;
        }
        Ok(left)
    }

    fn parse_addition(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_multiplication()?;
        
        while let TokenType::Op(op) = &self.peek().token_type {
            if ["+", "-"].contains(&op.as_str()) {
                let operator = op.clone();
                self.advance();
                let right = self.parse_multiplication()?;
                left = Expression::BinaryOp { left: Box::new(left), operator, right: Box::new(right) };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_primary()?;
        
        while let TokenType::Op(op) = &self.peek().token_type {
            if ["*", "/", "%"].contains(&op.as_str()) {
                let operator = op.clone();
                self.advance();
                let right = self.parse_primary()?;
                left = Expression::BinaryOp { left: Box::new(left), operator, right: Box::new(right) };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expression, String> {
        let mut expr = self.parse_atom()?;
        
        loop {
            if self.match_token(&TokenType::Punct("(".to_string())) {
                let mut arguments = Vec::new();
                if self.peek().token_type != TokenType::Punct(")".to_string()) {
                    loop {
                        arguments.push(self.parse_expression()?);
                        if self.match_token(&TokenType::Punct(",".to_string())) {
                            continue;
                        } else {
                            break;
                        }
                    }
                }
                self.expect(&TokenType::Punct(")".to_string()))?;
                expr = Expression::Call { callee: Box::new(expr), arguments };
            } else if self.match_token(&TokenType::Punct("[".to_string())) {
                let index = self.parse_expression()?;
                self.expect(&TokenType::Punct("]".to_string()))?;
                expr = Expression::IndexAccess { target: Box::new(expr), index: Box::new(index) };
            } else if self.match_token(&TokenType::Op(".".to_string())) {
                let prop_tok = self.advance().clone();
                let prop = match prop_tok.token_type {
                    TokenType::Identifier(p) => p,
                    TokenType::Keyword(p) => p,
                    _ => return self.error("Expected identifier after '.'"),
                };
                expr = Expression::PropertyAccess { target: Box::new(expr), property: prop };
            } else if self.match_token(&TokenType::Keyword("together".to_string())) {
                expr = Expression::Concurrent(Box::new(expr));
            } else if self.match_token(&TokenType::Keyword("as".to_string())) {
                let type_name = match self.advance().token_type.clone() {
                    TokenType::Identifier(n) => n,
                    TokenType::Keyword(k) => k,
                    _ => return self.error("Expected type after 'as'"),
                };
                expr = Expression::Cast(Box::new(expr), type_name);
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_atom(&mut self) -> Result<Expression, String> {
        if self.match_token(&TokenType::Keyword("read".to_string())) {
            if self.match_token(&TokenType::Keyword("json".to_string())) {
                let expr = self.parse_atom()?;
                return Ok(Expression::Cast(
                    Box::new(Expression::Read(Box::new(expr))),
                    "json".to_string()
                ));
            } else {
                let expr = self.parse_atom()?;
                return Ok(Expression::Read(Box::new(expr)));
            }
        }
        
        if self.match_token(&TokenType::Keyword("get".to_string())) {
            let expr = self.parse_atom()?;
            return Ok(Expression::Get(Box::new(expr)));
        }

        if self.match_token(&TokenType::Punct("(".to_string())) {
            let expr = self.parse_expression()?;
            self.expect(&TokenType::Punct(")".to_string()))?;
            return Ok(expr);
        }

        if self.match_token(&TokenType::Punct("{".to_string())) {
            let mut items = Vec::new();
            loop {
                // Skip layout tokens inside { }
                while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
                
                if self.peek().token_type == TokenType::Punct("}".to_string()) {
                    break;
                }
                
                let key_tok = self.advance().clone();
                let key = match key_tok.token_type {
                    TokenType::Identifier(k) => k,
                    TokenType::String(k) => k,
                    _ => return self.error(&format!("Expected identifier or string as map key, got {:?}", key_tok.token_type)),
                };
                self.expect(&TokenType::Op(":".to_string()))?;
                let value = self.parse_expression()?;
                items.push((key, value));
                
                while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
                if self.match_token(&TokenType::Punct(",".to_string())) {
                    continue;
                } else {
                    break;
                }
            }
            while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
            self.expect(&TokenType::Punct("}".to_string()))?;
            return Ok(Expression::Map(items));
        }

        if self.match_token(&TokenType::Punct("[".to_string())) {
            let mut items = Vec::new();
            loop {
                while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
                
                if self.peek().token_type == TokenType::Punct("]".to_string()) {
                    break;
                }
                
                items.push(self.parse_expression()?);
                
                while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
                if self.match_token(&TokenType::Punct(",".to_string())) {
                    continue;
                } else {
                    break;
                }
            }
            while self.match_token(&TokenType::Indent) || self.match_token(&TokenType::Dedent) {}
            self.expect(&TokenType::Punct("]".to_string()))?;
            return Ok(Expression::List(items));
        }

        let token = self.advance().clone();
        match token.token_type {
            TokenType::Integer(i) => Ok(Expression::Integer(i)),
            TokenType::Float(f) => Ok(Expression::Float(f)),
            TokenType::String(s) => Ok(Expression::String(s)),
            TokenType::Identifier(id) => Ok(Expression::Identifier(id)),
            TokenType::Keyword(k) if k == "true" => Ok(Expression::Boolean(true)),
            TokenType::Keyword(k) if k == "false" => Ok(Expression::Boolean(false)),
            TokenType::Keyword(k) if k == "json" => Ok(Expression::Identifier(k)),
            _ => self.error(&format!("Unexpected token in expression: {:?}", token.token_type)),
        }
    }
}
