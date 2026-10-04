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
            TokenType::Keyword(k) if k == "class" => self.parse_class_def(),
            TokenType::Keyword(k) if k == "try" => self.parse_try_catch(),
            TokenType::Keyword(k) if k == "while" => self.parse_while(),
            TokenType::Keyword(k) if k == "break" => self.parse_break(),
            TokenType::Keyword(k) if k == "continue" => self.parse_continue(),
            TokenType::Identifier(_) => self.parse_assignment_or_expression(),
            _ => self.error(&format!("Unexpected token {:?}", token.token_type)),
        }
    }

    fn parse_use_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("use".to_string()))?;
        
        if self.match_token(&TokenType::Keyword("python".to_string())) {
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
            
            Ok(StatementNode::UsePython { module, alias }.into())
        } else if let TokenType::String(path) = self.peek().token_type.clone() {
            self.advance();
            let mut alias = None;
            if self.match_token(&TokenType::Keyword("as".to_string())) {
                let alias_tok = self.advance().clone();
                if let TokenType::Identifier(n) = alias_tok.token_type {
                    alias = Some(n);
                } else {
                    return self.error("Expected alias after 'as'");
                }
            }
            Ok(StatementNode::UseModule { path, alias }.into())
        } else {
            self.error("Expected 'python' or string literal after 'use'")
        }
    }

    fn parse_show_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("show".to_string()))?;
        let expr = self.parse_expression()?;
        Ok(StatementNode::Show { value: expr }.into())
    }

    fn parse_save_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("save".to_string()))?;
        let data = self.parse_expression()?;
        self.expect(&TokenType::Keyword("to".to_string()))?;
        let destination = self.parse_expression()?;
        Ok(StatementNode::Save { data, destination }.into())
    }

    fn parse_if_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("if".to_string()))?;
        let condition = self.parse_expression()?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let body = self.parse_block()?;
        
        let mut otherwise = None;
        if self.match_token(&TokenType::Keyword("otherwise".to_string())) {
            if self.match_token(&TokenType::Op(":".to_string())) {
                otherwise = Some(Box::new(StatementNode::If {
                    condition: ExpressionNode::Boolean(true).into(),
                    body: self.parse_block()?,
                    otherwise: None,
                }.into()));
            } else if self.peek().token_type == TokenType::Keyword("if".to_string()) {
                otherwise = Some(Box::new(self.parse_if_statement()?));
            } else {
                return self.error("Expected ':' or 'if' after 'otherwise'");
            }
        }
        
        Ok(StatementNode::If { condition, body, otherwise }.into())
    }

    fn parse_repeat_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("repeat".to_string()))?;
        let count = self.parse_expression()?;
        self.expect(&TokenType::Keyword("times".to_string()))?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let body = self.parse_block()?;
        Ok(StatementNode::Repeat { count, body }.into())
    }

    fn parse_class_def(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("class".to_string()))?;
        let name_tok = self.advance().clone();
        let name = match name_tok.token_type {
            TokenType::Identifier(n) => n,
            _ => return self.error("Expected class name"),
        };
        self.expect(&TokenType::Op(":".to_string()))?;
        
        self.expect(&TokenType::Indent)?;
        let mut methods = Vec::new();
        while self.peek().token_type != TokenType::Dedent && self.peek().token_type != TokenType::Eof {
            let stmt = self.parse_statement()?;
            if let StatementNode::FunctionDeclaration { .. } = stmt.node {
                methods.push(stmt);
            } else {
                return self.error("Classes can only contain function declarations");
            }
        }
        if self.peek().token_type == TokenType::Dedent {
            self.advance();
        }
        Ok(StatementNode::ClassDef { name, methods }.into())
    }

    fn parse_try_catch(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("try".to_string()))?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let try_block = self.parse_block()?;
        
        self.expect(&TokenType::Keyword("catch".to_string()))?;
        let catch_var_tok = self.advance().clone();
        let catch_var = match catch_var_tok.token_type {
            TokenType::Identifier(n) => n,
            _ => return self.error("Expected identifier for catch variable"),
        };
        self.expect(&TokenType::Op(":".to_string()))?;
        let catch_block = self.parse_block()?;
        
        Ok(StatementNode::TryCatch { try_block, catch_var, catch_block }.into())
    }

    fn parse_while(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("while".to_string()))?;
        let condition = self.parse_expression()?;
        self.expect(&TokenType::Op(":".to_string()))?;
        let body = self.parse_block()?;
        Ok(StatementNode::While { condition, body }.into())
    }

    fn parse_break(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("break".to_string()))?;
        Ok(StatementNode::Break.into())
    }

    fn parse_continue(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("continue".to_string()))?;
        Ok(StatementNode::Continue.into())
    }

    fn parse_assignment_or_expression(&mut self) -> Result<Statement, String> {
        if let Some(next) = self.peek_next() {
            if next.token_type == TokenType::Op("=".to_string()) || next.token_type == TokenType::Op(":".to_string()) {
                return self.parse_assignment();
            }
        }
        
        let expr = self.parse_expression()?;
        if let ExpressionNode::Call { .. } = expr.node {
            // Treat function calls as statements if they stand alone (by mapping them to dummy assignment or we can just return it if we add ExprStmt)
            // Let's add it as a dummy assignment to '_' for now, to keep AST simple.
            return Ok(StatementNode::Assignment {
                name: "_".to_string(),
                value: expr,
                type_annotation: None,
            }.into());
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
        
        Ok(StatementNode::Assignment { name, value, type_annotation }.into())
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
            body = Block { statements: vec![StatementNode::Return { value: Some(expr) }.into()] };
        } else {
            self.expect(&TokenType::Op(":".to_string()))?;
            body = self.parse_block()?;
        }
        
        Ok(StatementNode::FunctionDeclaration { name, params, return_type, body }.into())
    }

    fn parse_return_statement(&mut self) -> Result<Statement, String> {
        self.expect(&TokenType::Keyword("return".to_string()))?;
        let peek_type = &self.peek().token_type;
        let value = match peek_type {
            TokenType::Dedent | TokenType::Eof => None,
            TokenType::Keyword(k) if k != "true" && k != "false" => None,
            _ => Some(self.parse_expression()?),
        };
        Ok(StatementNode::Return { value }.into())
    }

    fn parse_expression(&mut self) -> Result<Expression, String> {
        self.parse_query()
    }

    fn parse_query(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_logical_or()?;
        
        loop {
            if self.match_token(&TokenType::Keyword("where".to_string())) {
                let condition = self.parse_logical_or()?;
                left = ExpressionNode::Where { target: Box::new(left), condition: Box::new(condition) }.into();
            } else if self.match_token(&TokenType::Keyword("select".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = ExpressionNode::Select { target: Box::new(left), mapping: Box::new(mapping) }.into();
            } else if self.match_token(&TokenType::Keyword("average".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = ExpressionNode::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "average".to_string() }.into();
            } else if self.match_token(&TokenType::Keyword("sum".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = ExpressionNode::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "sum".to_string() }.into();
            } else if self.match_token(&TokenType::Keyword("min".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = ExpressionNode::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "min".to_string() }.into();
            } else if self.match_token(&TokenType::Keyword("max".to_string())) {
                let mapping = self.parse_logical_or()?;
                left = ExpressionNode::Aggregate { target: Box::new(left), mapping: Box::new(mapping), op: "max".to_string() }.into();
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
            left = ExpressionNode::BinaryOp { left: Box::new(left), operator: "or".to_string(), right: Box::new(right) }.into();
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_comparison()?;
        while self.match_token(&TokenType::Keyword("and".to_string())) {
            let right = self.parse_comparison()?;
            left = ExpressionNode::BinaryOp { left: Box::new(left), operator: "and".to_string(), right: Box::new(right) }.into();
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
                    let span = left.span.merge(&right.span);
                    left = Expression::from(ExpressionNode::BinaryOp { left: Box::new(left), operator, right: Box::new(right) }).with_span(span);
                    continue;
                }
            } else if let TokenType::Keyword(kw) = &self.peek().token_type {
                if ["starts_with", "ends_with"].contains(&kw.as_str()) {
                    let operator = kw.clone();
                    self.advance();
                    let right = self.parse_addition()?;
                    let span = left.span.merge(&right.span);
                    left = Expression::from(ExpressionNode::BinaryOp { left: Box::new(left), operator, right: Box::new(right) }).with_span(span);
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
                let span = left.span.merge(&right.span);
                left = Expression::from(ExpressionNode::BinaryOp { left: Box::new(left), operator, right: Box::new(right) }).with_span(span);
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
                let span = left.span.merge(&right.span);
                left = Expression::from(ExpressionNode::BinaryOp { left: Box::new(left), operator, right: Box::new(right) }).with_span(span);
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
                expr = ExpressionNode::Call { callee: Box::new(expr), arguments }.into();
            } else if self.match_token(&TokenType::Punct("[".to_string())) {
                let index = self.parse_expression()?;
                self.expect(&TokenType::Punct("]".to_string()))?;
                expr = ExpressionNode::IndexAccess { target: Box::new(expr), index: Box::new(index) }.into();
            } else if self.match_token(&TokenType::Op(".".to_string())) {
                let prop_tok = self.advance().clone();
                let prop = match prop_tok.token_type {
                    TokenType::Identifier(p) => p,
                    TokenType::Keyword(p) => p,
                    _ => return self.error("Expected identifier after '.'"),
                };
                expr = ExpressionNode::PropertyAccess { target: Box::new(expr), property: prop }.into();
            } else if self.match_token(&TokenType::Keyword("together".to_string())) {
                expr = ExpressionNode::Concurrent(Box::new(expr)).into();
            } else if self.match_token(&TokenType::Keyword("as".to_string())) {
                let type_name = match self.advance().token_type.clone() {
                    TokenType::Identifier(n) => n,
                    TokenType::Keyword(k) => k,
                    _ => return self.error("Expected type after 'as'"),
                };
                expr = ExpressionNode::Cast(Box::new(expr), type_name).into();
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
                return Ok(ExpressionNode::Cast(
                    Box::new(ExpressionNode::Read(Box::new(expr)).into()),
                    "json".to_string()
                ).into());
            } else {
                let expr = self.parse_atom()?;
                return Ok(ExpressionNode::Read(Box::new(expr)).into());
            }
        }
        
        if self.match_token(&TokenType::Keyword("get".to_string())) {
            let expr = self.parse_atom()?;
            return Ok(ExpressionNode::Get(Box::new(expr)).into());
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
            return Ok(ExpressionNode::Map(items).into());
        }

        if self.match_token(&TokenType::Op("|".to_string())) {
            let mut params = Vec::new();
            if self.peek().token_type != TokenType::Op("|".to_string()) {
                loop {
                    let param_tok = self.advance().clone();
                    if let TokenType::Identifier(p) = param_tok.token_type {
                        params.push(p);
                    } else {
                        return self.error("Expected parameter name in closure");
                    }
                    if self.match_token(&TokenType::Punct(",".to_string())) {
                        continue;
                    } else {
                        break;
                    }
                }
            }
            self.expect(&TokenType::Op("|".to_string()))?;
            let body = self.parse_expression()?;
            return Ok(ExpressionNode::Closure { params, body: Box::new(body) }.into());
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
            return Ok(ExpressionNode::List(items).into());
        }

        let token = self.advance().clone();
        let span = Span { line: token.line, column: token.column, length: token.length() };
        match token.token_type {
            TokenType::Integer(i) => Ok(Expression::from(ExpressionNode::Integer(i)).with_span(span)),
            TokenType::Float(f) => Ok(Expression::from(ExpressionNode::Float(f)).with_span(span)),
            TokenType::String(s) => Ok(Expression::from(ExpressionNode::String(s)).with_span(span)),
            TokenType::FString(s) => self.parse_fstring(s, span),
            TokenType::Identifier(id) => Ok(Expression::from(ExpressionNode::Identifier(id)).with_span(span)),
            TokenType::Keyword(k) if k == "true" => Ok(Expression::from(ExpressionNode::Boolean(true)).with_span(span)),
            TokenType::Keyword(k) if k == "false" => Ok(Expression::from(ExpressionNode::Boolean(false)).with_span(span)),
            TokenType::Keyword(k) if k == "json" => Ok(Expression::from(ExpressionNode::Identifier(k)).with_span(span)),
            _ => self.error(&format!("Unexpected token in expression: {:?}", token.token_type)),
        }
    }

    fn parse_fstring(&mut self, s: String, span: Span) -> Result<Expression, String> {
        let mut parts = Vec::new();
        let mut current_str = String::new();
        let mut in_expr = false;
        let mut chars = s.chars().peekable();
        
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    current_str.push(next);
                }
            } else if c == '{' && !in_expr {
                if !current_str.is_empty() {
                    parts.push(ExpressionNode::String(current_str.clone()).into());
                    current_str.clear();
                }
                in_expr = true;
            } else if c == '}' && in_expr {
                if !current_str.is_empty() {
                    let mut lexer = crate::lexer::Lexer::new(&current_str);
                    let tokens = lexer.tokenize()?;
                    if tokens.len() > 1 {
                        let mut parser = Parser::new(tokens);
                        let expr = parser.parse_expression()?;
                        parts.push(expr);
                    }
                    current_str.clear();
                }
                in_expr = false;
            } else {
                current_str.push(c);
            }
        }
        if !current_str.is_empty() {
            parts.push(ExpressionNode::String(current_str).into());
        }
        
        Ok(Expression::from(ExpressionNode::StringInterpolation(parts)).with_span(span))
    }
}
