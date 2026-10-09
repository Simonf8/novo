use super::ast::*;
use super::lexer;
use super::token::{FPart, Tok, Token};
use crate::diagnostic::Diagnostic;
use crate::source::{FileId, Span};

type PResult<T> = Result<T, Diagnostic>;

pub fn parse(file: FileId, src: &str) -> PResult<Module> {
    let tokens = lexer::tokenize(file, src)?;
    let mut parser = Parser { tokens, pos: 0, file, src };
    let mut stmts = Vec::new();
    parser.skip_newlines();
    while !parser.at(&Tok::Eof) {
        stmts.push(parser.statement()?);
        parser.skip_newlines();
    }
    Ok(Module { stmts })
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    file: FileId,
    src: &'a str,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_nth(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn at(&self, tok: &Tok) -> bool {
        self.peek() == tok
    }

    fn at_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(w) if w == word)
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.at(tok) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &Tok, context: &str) -> PResult<Span> {
        if self.at(tok) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("expected {tok} {context}")))
        }
    }

    fn unexpected(&self, message: &str) -> Diagnostic {
        Diagnostic::error(self.span(), message.to_string()).with_label(format!("found {}", self.peek()))
    }

    fn skip_newlines(&mut self) {
        while self.eat(&Tok::Newline) {}
    }

    fn ident(&mut self, context: &str) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            _ => Err(self.unexpected(&format!("expected a name {context}"))),
        }
    }

    fn end_of_statement(&mut self) -> PResult<()> {
        match self.peek() {
            Tok::Newline => {
                self.bump();
                Ok(())
            }
            Tok::Eof | Tok::Dedent => Ok(()),
            _ => Err(self.unexpected("expected end of line")),
        }
    }

    fn block(&mut self) -> PResult<Block> {
        self.expect(&Tok::Colon, "to start a block")?;
        if !self.at(&Tok::Newline) {
            return Ok(vec![self.simple_statement()?]);
        }
        self.bump();
        self.skip_newlines();
        if !self.eat(&Tok::Indent) {
            return Err(self.unexpected("expected an indented block").with_help("indent the lines that belong to this block"));
        }
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::Dedent) || self.at(&Tok::Eof) {
                break;
            }
            stmts.push(self.statement()?);
        }
        Ok(stmts)
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let lo = self.span();
        match self.peek() {
            Tok::If => self.if_statement(),
            Tok::While => {
                self.bump();
                let cond = self.expression()?;
                let body = self.block()?;
                Ok(Stmt { kind: StmtKind::While { cond, body }, span: lo.to(self.prev_span()) })
            }
            Tok::Repeat => {
                self.bump();
                let count = self.expression()?;
                if !self.at_ident("times") {
                    return Err(self.unexpected("expected `times` after the repeat count"));
                }
                self.bump();
                let body = self.block()?;
                Ok(Stmt { kind: StmtKind::Repeat { count, body }, span: lo.to(self.prev_span()) })
            }
            Tok::For => {
                self.bump();
                let mut vars = vec![self.ident("for the loop variable")?];
                while self.eat(&Tok::Comma) {
                    vars.push(self.ident("for the loop variable")?);
                }
                self.expect(&Tok::In, "after the loop variable")?;
                let iter = self.expression()?;
                let body = self.block()?;
                Ok(Stmt { kind: StmtKind::For { vars, iter, body }, span: lo.to(self.prev_span()) })
            }
            Tok::Function | Tok::Task => {
                let def = self.function_def()?;
                let span = def.span;
                Ok(Stmt { kind: StmtKind::Function(def), span })
            }
            Tok::Class => self.class_def(),
            Tok::Try => {
                self.bump();
                let body = self.block()?;
                self.skip_newlines();
                self.expect(&Tok::Catch, "after the try block")?;
                let var = self.ident("for the caught error")?;
                let handler = self.block()?;
                Ok(Stmt { kind: StmtKind::Try { body, var, handler }, span: lo.to(self.prev_span()) })
            }
            _ => {
                let stmt = self.simple_statement()?;
                Ok(stmt)
            }
        }
    }

    fn simple_statement(&mut self) -> PResult<Stmt> {
        let lo = self.span();
        let kind = match self.peek() {
            Tok::Show => {
                self.bump();
                let mut values = vec![self.expression()?];
                while self.eat(&Tok::Comma) {
                    values.push(self.expression()?);
                }
                StmtKind::Show(values)
            }
            Tok::Return => {
                self.bump();
                if matches!(self.peek(), Tok::Newline | Tok::Dedent | Tok::Eof) {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expression_list()?))
                }
            }
            Tok::Break => {
                self.bump();
                StmtKind::Break
            }
            Tok::Continue => {
                self.bump();
                StmtKind::Continue
            }
            Tok::Pass => {
                self.bump();
                StmtKind::Pass
            }
            Tok::Use => self.use_statement()?,
            _ => self.expression_statement()?,
        };
        let span = lo.to(self.prev_span());
        self.end_of_statement()?;
        Ok(Stmt { kind, span })
    }

    /// Parses `a` or `a, b, ...`, the latter as a tuple.
    fn expression_list(&mut self) -> PResult<Expr> {
        let first = self.expression()?;
        if !self.at(&Tok::Comma) {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(&Tok::Comma) {
            if !self.starts_expression() {
                break;
            }
            items.push(self.expression()?);
        }
        let span = items[0].span.to(self.prev_span());
        Ok(Expr { kind: ExprKind::Tuple(items), span })
    }

    fn expression_statement(&mut self) -> PResult<StmtKind> {
        let target = self.expression_list()?;
        fn check_target(p: &Parser, e: &Expr) -> PResult<()> {
            match &e.kind {
                ExprKind::Name(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => Ok(()),
                ExprKind::Tuple(items) => items.iter().try_for_each(|i| check_target(p, i)),
                _ => Err(Diagnostic::error(e.span, "cannot assign to this expression")
                    .with_label("not a variable, field or element")
                    .with_help(if matches!(p.peek(), Tok::Assign) { "use `==` to compare values" } else { "assign to a variable instead" })),
            }
        }
        let aug = match self.peek() {
            Tok::PlusAssign => Some(BinOp::Add),
            Tok::MinusAssign => Some(BinOp::Sub),
            Tok::StarAssign => Some(BinOp::Mul),
            Tok::SlashAssign => Some(BinOp::Div),
            _ => None,
        };
        if let Some(op) = aug {
            if matches!(target.kind, ExprKind::Tuple(_)) {
                return Err(Diagnostic::error(target.span, "cannot update several variables at once").with_help("update each one on its own line"));
            }
            check_target(self, &target)?;
            self.bump();
            let value = self.expression()?;
            return Ok(StmtKind::AugAssign { target, op, value });
        }
        if self.at(&Tok::Colon) && matches!(target.kind, ExprKind::Name(_)) {
            self.bump();
            let ty = self.type_expr()?;
            self.expect(&Tok::Assign, "after the type annotation")?;
            let value = self.expression()?;
            return Ok(StmtKind::Assign { target, ty: Some(ty), value });
        }
        if self.at(&Tok::Assign) {
            check_target(self, &target)?;
            self.bump();
            let value = self.expression_list()?;
            return Ok(StmtKind::Assign { target, ty: None, value });
        }
        Ok(StmtKind::Expr(target))
    }

    fn use_statement(&mut self) -> PResult<StmtKind> {
        self.bump();
        let alias = |p: &mut Self| -> PResult<Option<Ident>> {
            if p.eat(&Tok::As) { Ok(Some(p.ident("after `as`")?)) } else { Ok(None) }
        };
        if self.eat(&Tok::Python) {
            let mut module = self.ident("for the Python module")?.name;
            while self.eat(&Tok::Dot) {
                module.push('.');
                module.push_str(&self.ident("in the module path")?.name);
            }
            let alias = alias(self)?;
            return Ok(StmtKind::Use(Use::Python { module, alias }));
        }
        if let Tok::Str(path) = self.peek().clone() {
            self.bump();
            let alias = alias(self)?;
            return Ok(StmtKind::Use(Use::File { path, alias }));
        }
        let name = self.ident("of the module to use")?;
        let alias = alias(self)?;
        Ok(StmtKind::Use(Use::Module { name, alias }))
    }

    fn if_statement(&mut self) -> PResult<Stmt> {
        let lo = self.span();
        self.bump();
        let mut branches = vec![(self.expression()?, self.block()?)];
        let mut otherwise = None;
        loop {
            let save = self.pos;
            self.skip_newlines();
            if !self.eat(&Tok::Otherwise) {
                self.pos = save;
                break;
            }
            if self.eat(&Tok::If) {
                branches.push((self.expression()?, self.block()?));
            } else {
                otherwise = Some(self.block()?);
                break;
            }
        }
        Ok(Stmt { kind: StmtKind::If { branches, otherwise }, span: lo.to(self.prev_span()) })
    }

    fn function_def(&mut self) -> PResult<FunctionDef> {
        let lo = self.span();
        self.bump();
        let name = self.ident("for the function")?;
        self.expect(&Tok::LParen, "to start the parameter list")?;
        let mut params = Vec::new();
        while !self.at(&Tok::RParen) {
            let pname = self.ident("for the parameter")?;
            let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
            let default = if self.eat(&Tok::Assign) { Some(self.expression()?) } else { None };
            params.push(Param { name: pname, ty, default });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "to close the parameter list")?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        let body = if self.at(&Tok::FatArrow) {
            let arrow = self.bump().span;
            let value = self.expression()?;
            let span = arrow.to(value.span);
            self.end_of_statement()?;
            vec![Stmt { kind: StmtKind::Return(Some(value)), span }]
        } else {
            self.block()?
        };
        Ok(FunctionDef { name, params, ret, body, span: lo.to(self.prev_span()) })
    }

    fn class_def(&mut self) -> PResult<Stmt> {
        let lo = self.span();
        self.bump();
        let name = self.ident("for the class")?;
        self.expect(&Tok::Colon, "after the class name")?;
        self.expect(&Tok::Newline, "after `:`")?;
        self.skip_newlines();
        if !self.eat(&Tok::Indent) {
            return Err(self.unexpected("expected an indented class body"));
        }
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::Dedent) || self.at(&Tok::Eof) {
                break;
            }
            match self.peek() {
                Tok::Function | Tok::Task => methods.push(self.function_def()?),
                Tok::Pass => {
                    self.bump();
                    self.end_of_statement()?;
                }
                Tok::Ident(_) => {
                    let fname = self.ident("for the field")?;
                    let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                    let default = if self.eat(&Tok::Assign) { Some(self.expression()?) } else { None };
                    if ty.is_none() && default.is_none() {
                        return Err(Diagnostic::error(fname.span, "field needs a type or a default value")
                            .with_help(format!("write `{}: Int` or `{} = 0`", fname.name, fname.name)));
                    }
                    self.end_of_statement()?;
                    fields.push(FieldDef { name: fname, ty, default });
                }
                _ => return Err(self.unexpected("expected a field or a method in the class body")),
            }
        }
        let span = lo.to(self.prev_span());
        Ok(Stmt { kind: StmtKind::Class(ClassDef { name, fields, methods }), span })
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let lo = self.span();
        if self.eat(&Tok::LParen) {
            let mut args = vec![self.type_expr()?];
            while self.eat(&Tok::Comma) {
                if self.at(&Tok::RParen) {
                    break;
                }
                args.push(self.type_expr()?);
            }
            self.expect(&Tok::RParen, "to close the tuple type")?;
            return Ok(TypeExpr { name: "Tuple".into(), args, span: lo.to(self.prev_span()) });
        }
        let name = match self.peek().clone() {
            Tok::Ident(n) => n,
            Tok::None => "None".to_string(),
            _ => return Err(self.unexpected("expected a type")),
        };
        self.bump();
        let mut args = Vec::new();
        if self.eat(&Tok::LBracket) {
            loop {
                args.push(self.type_expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBracket, "to close the type arguments")?;
        }
        Ok(TypeExpr { name, args, span: lo.to(self.prev_span()) })
    }

    pub fn expression(&mut self) -> PResult<Expr> {
        let mut expr = self.or_expr()?;
        loop {
            if let Tok::Ident(word) = self.peek() {
                if let Some(op) = QueryOp::from_word(word) {
                    self.bump();
                    let body = if self.starts_expression() { Some(Box::new(self.or_expr()?)) } else { None };
                    if body.is_none() && matches!(op, QueryOp::Where | QueryOp::Select) {
                        return Err(self.unexpected(&format!("expected an expression after `{}`", op.word())));
                    }
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Query { op, target: Box::new(expr), body, parallel: false }, span };
                    continue;
                }
            }
            if self.at(&Tok::Together) {
                let tspan = self.bump().span;
                let span = expr.span.to(tspan);
                expr = match expr.kind {
                    ExprKind::Query { op: QueryOp::Select, target, body, .. } => {
                        Expr { kind: ExprKind::Query { op: QueryOp::Select, target, body, parallel: true }, span }
                    }
                    ExprKind::List(items) => Expr { kind: ExprKind::RunParallel(items), span },
                    _ => {
                        return Err(Diagnostic::error(span, "postfix `together` needs a list or a `select` query")
                            .with_help("use `together f(x)` to run a single call in the background"));
                    }
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn starts_expression(&self) -> bool {
        match self.peek() {
            Tok::Ident(w) => QueryOp::from_word(w).is_none(),
            Tok::Int(_) | Tok::Float(_) | Tok::Str(_) | Tok::FStr(_) | Tok::True | Tok::False | Tok::None => true,
            Tok::LParen | Tok::LBracket | Tok::LBrace | Tok::Pipe | Tok::Minus | Tok::Not | Tok::Await | Tok::Together => true,
            _ => false,
        }
    }

    fn binary(lhs: Expr, op: BinOp, rhs: Expr) -> Expr {
        let span = lhs.span.to(rhs.span);
        Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span }
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.and_expr()?;
        while self.eat(&Tok::Or) {
            let rhs = self.and_expr()?;
            lhs = Self::binary(lhs, BinOp::Or, rhs);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.not_expr()?;
        while self.eat(&Tok::And) {
            let rhs = self.not_expr()?;
            lhs = Self::binary(lhs, BinOp::And, rhs);
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> PResult<Expr> {
        if self.at(&Tok::Not) {
            let lo = self.bump().span;
            let operand = self.not_expr()?;
            let span = lo.to(operand.span);
            return Ok(Expr { kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(operand) }, span });
        }
        self.comparison()
    }

    fn comparison_op(&self) -> Option<(BinOp, bool)> {
        Some(match self.peek() {
            Tok::EqEq => (BinOp::Eq, false),
            Tok::NotEq => (BinOp::Ne, false),
            Tok::Lt => (BinOp::Lt, false),
            Tok::Le => (BinOp::Le, false),
            Tok::Gt => (BinOp::Gt, false),
            Tok::Ge => (BinOp::Ge, false),
            Tok::In => (BinOp::In, false),
            Tok::Not if self.peek_nth(1) == &Tok::In => (BinOp::In, true),
            _ => return None,
        })
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let lhs = self.additive()?;
        let Some((op, negated)) = self.comparison_op() else { return Ok(lhs) };
        self.bump();
        if negated {
            self.bump();
        }
        let rhs = self.additive()?;
        let mut expr = Self::binary(lhs, op, rhs);
        if negated {
            let span = expr.span;
            expr = Expr { kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(expr) }, span };
        }
        if self.comparison_op().is_some() {
            return Err(Diagnostic::error(self.span(), "comparisons cannot be chained")
                .with_help("combine them with `and`, for example `a < b and b < c`"));
        }
        Ok(expr)
    }

    fn additive(&mut self) -> PResult<Expr> {
        let mut lhs = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.multiplicative()?;
            lhs = Self::binary(lhs, op, rhs);
        }
        Ok(lhs)
    }

    fn multiplicative(&mut self) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::SlashSlash => BinOp::FloorDiv,
                Tok::Percent => BinOp::Mod,
                Tok::At => BinOp::MatMul,
                _ => break,
            };
            self.bump();
            let rhs = self.unary()?;
            lhs = Self::binary(lhs, op, rhs);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let lo = self.span();
        match self.peek() {
            Tok::Minus => {
                self.bump();
                let operand = self.unary()?;
                let span = lo.to(operand.span);
                let kind = match operand.kind {
                    ExprKind::Int(n) => ExprKind::Int(n.wrapping_neg()),
                    ExprKind::Float(f) => ExprKind::Float(-f),
                    _ => ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) },
                };
                Ok(Expr { kind, span })
            }
            Tok::Together => {
                self.bump();
                let operand = self.unary()?;
                let span = lo.to(operand.span);
                Ok(Expr { kind: ExprKind::Together(Box::new(operand)), span })
            }
            Tok::Await => {
                self.bump();
                let operand = self.unary()?;
                let span = lo.to(operand.span);
                Ok(Expr { kind: ExprKind::Await(Box::new(operand)), span })
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> PResult<Expr> {
        let base = self.postfix()?;
        if self.eat(&Tok::StarStar) {
            let exp = self.unary()?;
            return Ok(Self::binary(base, BinOp::Pow, exp));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.atom()?;
        loop {
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let args = self.call_args()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Call { callee: Box::new(expr), args }, span };
                }
                Tok::LBracket => {
                    self.bump();
                    let items = self.index_items()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Index { target: Box::new(expr), items }, span };
                }
                Tok::Dot => {
                    self.bump();
                    let name = match self.peek().keyword_text() {
                        Some(word) => Ident { name: word.to_string(), span: self.bump().span },
                        None => self.ident("after `.`")?,
                    };
                    let span = expr.span.to(name.span);
                    expr = Expr { kind: ExprKind::Field { target: Box::new(expr), name }, span };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn call_args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        let mut seen_named = false;
        while !self.at(&Tok::RParen) {
            if matches!(self.peek(), Tok::Ident(_)) && self.peek_nth(1) == &Tok::Assign {
                let name = self.ident("for the argument")?;
                self.bump();
                let value = self.expression()?;
                seen_named = true;
                args.push(Arg { name: Some(name), value });
            } else {
                let value = self.expression()?;
                if seen_named {
                    return Err(Diagnostic::error(value.span, "positional argument after a named argument")
                        .with_help("put positional arguments first"));
                }
                args.push(Arg { name: None, value });
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "to close the argument list")?;
        Ok(args)
    }

    fn index_items(&mut self) -> PResult<Vec<IndexItem>> {
        let mut items = Vec::new();
        loop {
            let start = if self.at(&Tok::Colon) { None } else { Some(self.expression()?) };
            if self.eat(&Tok::Colon) {
                let end = if matches!(self.peek(), Tok::Comma | Tok::RBracket) { None } else { Some(self.expression()?) };
                items.push(IndexItem::Slice { start, end });
            } else {
                items.push(IndexItem::Single(start.expect("index expression")));
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBracket, "to close the index")?;
        Ok(items)
    }

    fn atom(&mut self) -> PResult<Expr> {
        let lo = self.span();
        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                ExprKind::Int(n)
            }
            Tok::Float(f) => {
                self.bump();
                ExprKind::Float(f)
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::FStr(parts) => {
                self.bump();
                ExprKind::FStr(self.fstring(parts)?)
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::None => {
                self.bump();
                ExprKind::None
            }
            Tok::Ident(name) if name == "run" && self.peek_nth(1) == &Tok::LBracket => {
                self.bump();
                self.bump();
                let items = self.sequence(&Tok::RBracket)?;
                self.expect(&Tok::In, "after the task list")?;
                if !self.at_ident("parallel") {
                    return Err(self.unexpected("expected `parallel` after `in`"));
                }
                self.bump();
                ExprKind::RunParallel(items)
            }
            Tok::Ident(name) => {
                self.bump();
                ExprKind::Name(name)
            }
            Tok::LParen => {
                self.bump();
                let inner = self.expression()?;
                if self.eat(&Tok::Comma) {
                    let mut items = vec![inner];
                    while !self.at(&Tok::RParen) {
                        items.push(self.expression()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "to close the tuple")?;
                    return Ok(Expr { kind: ExprKind::Tuple(items), span: lo.to(self.prev_span()) });
                }
                self.expect(&Tok::RParen, "to close the parenthesis")?;
                return Ok(Expr { kind: inner.kind, span: lo.to(self.prev_span()) });
            }
            Tok::LBracket => {
                self.bump();
                ExprKind::List(self.sequence(&Tok::RBracket)?)
            }
            Tok::LBrace => {
                self.bump();
                ExprKind::Map(self.map_entries()?)
            }
            Tok::Pipe => {
                self.bump();
                let mut params = Vec::new();
                while !self.at(&Tok::Pipe) {
                    params.push(self.ident("for the closure parameter")?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::Pipe, "to close the closure parameters")?;
                let body = self.expression()?;
                ExprKind::Lambda { params, body: Box::new(body) }
            }
            _ => return Err(self.unexpected("expected an expression")),
        };
        Ok(Expr { kind, span: lo.to(self.prev_span()) })
    }

    fn sequence(&mut self, close: &Tok) -> PResult<Vec<Expr>> {
        let mut items = Vec::new();
        while !self.at(close) {
            items.push(self.expression()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(close, "to close the list")?;
        Ok(items)
    }

    fn map_entries(&mut self) -> PResult<Vec<(Expr, Expr)>> {
        let mut entries = Vec::new();
        while !self.at(&Tok::RBrace) {
            let key = match self.peek().clone() {
                Tok::Ident(name) if self.peek_nth(1) == &Tok::Colon => {
                    let span = self.bump().span;
                    Expr { kind: ExprKind::Str(name), span }
                }
                _ => self.expression()?,
            };
            self.expect(&Tok::Colon, "after the map key")?;
            let value = self.expression()?;
            entries.push((key, value));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace, "to close the map")?;
        Ok(entries)
    }

    fn fstring(&mut self, parts: Vec<FPart>) -> PResult<Vec<FStrPart>> {
        let mut out = Vec::new();
        for part in parts {
            match part {
                FPart::Lit(s) => out.push(FStrPart::Lit(s)),
                FPart::Expr { lo, hi, spec } => {
                    let tokens = lexer::tokenize_fragment(self.file, self.src, lo, hi)?;
                    let mut sub = Parser { tokens, pos: 0, file: self.file, src: self.src };
                    let expr = sub.expression()?;
                    if !sub.at(&Tok::Eof) {
                        return Err(sub.unexpected("unexpected text in f-string expression"));
                    }
                    out.push(FStrPart::Expr { expr, spec });
                }
            }
        }
        Ok(out)
    }
}
