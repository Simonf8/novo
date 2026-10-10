use super::builtins::Builtin;
use super::hir::{self, ExprKind, FuncId, LocalId};
use super::shape::{self, Lit};
use super::solve::{Constraint, arity_error};
use super::types::{ClassId, Ty};
use super::{Checker, FnCtx, FnState, Item, Resolved, Scheme};
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::syntax::ast::{self, BinOp, QueryOp, UnaryOp};

type Arg = (Option<ast::Ident>, hir::Expr);

fn expr(kind: ExprKind, ty: Ty, span: Span) -> hir::Expr {
    hir::Expr { kind, ty, span }
}

pub(super) fn error_expr(span: Span) -> hir::Expr {
    expr(ExprKind::None, Ty::Any, span)
}

/// What a dotted access like `a.b` refers to before any value is computed.
enum Member {
    Item(Item),
    Builtin(Builtin),
    Value(hir::Expr),
}

impl Checker {
    pub(super) fn infer_expr(&mut self, e: &ast::Expr) -> hir::Expr {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::Int(n) => expr(ExprKind::Int(*n), Ty::Int, span),
            ast::ExprKind::Float(f) => expr(ExprKind::Float(*f), Ty::Float, span),
            ast::ExprKind::Str(s) => expr(ExprKind::Str(s.as_str().into()), Ty::Str, span),
            ast::ExprKind::Bool(b) => expr(ExprKind::Bool(*b), Ty::Bool, span),
            ast::ExprKind::None => expr(ExprKind::None, Ty::None, span),
            ast::ExprKind::FStr(parts) => {
                let parts = parts
                    .iter()
                    .map(|p| match p {
                        ast::FStrPart::Lit(s) => hir::FStrPart::Lit(s.clone()),
                        ast::FStrPart::Expr { expr, spec } => hir::FStrPart::Expr {
                            expr: self.infer_expr(expr),
                            spec: spec.clone(),
                        },
                    })
                    .collect();
                expr(ExprKind::FStr(parts), Ty::Str, span)
            }
            ast::ExprKind::Name(name) => self.infer_name(name, span),
            ast::ExprKind::List(items) => self.infer_list(items, span),
            ast::ExprKind::Tuple(items) => {
                let items: Vec<hir::Expr> = items.iter().map(|i| self.infer_expr(i)).collect();
                let ty = Ty::Tuple(items.iter().map(|i| i.ty.clone()).collect());
                expr(ExprKind::Tuple(items), ty, span)
            }
            ast::ExprKind::Map(entries) => {
                let (keys, values): (Vec<_>, Vec<_>) = entries
                    .iter()
                    .map(|(k, v)| {
                        let (k, v) = (self.infer_expr(k), self.infer_expr(v));
                        (self.unshape(k), self.unshape(v))
                    })
                    .unzip();
                let (k, v) = (self.element_type(&keys), self.element_type(&values));
                let entries = keys
                    .into_iter()
                    .zip(values)
                    .map(|(key, value)| (self.coerce(key, &k), self.coerce(value, &v)))
                    .collect();
                expr(ExprKind::Map(entries), Ty::map(k, v), span)
            }
            ast::ExprKind::Unary { op, operand } => {
                let operand = self.infer_expr(operand);
                match op {
                    UnaryOp::Not => {
                        self.expect_bool(&operand, "`not` needs a `Bool`");
                        expr(
                            ExprKind::Unary {
                                op: *op,
                                operand: Box::new(operand),
                            },
                            Ty::Bool,
                            span,
                        )
                    }
                    UnaryOp::Neg => {
                        let out = self.infer.fresh();
                        self.add_constraint(Constraint::Neg {
                            operand: operand.ty.clone(),
                            out: out.clone(),
                            span,
                        });
                        expr(
                            ExprKind::Unary {
                                op: *op,
                                operand: Box::new(operand),
                            },
                            out,
                            span,
                        )
                    }
                }
            }
            ast::ExprKind::Binary { op, lhs, rhs } => {
                let lhs = self.infer_expr(lhs);
                let rhs = self.infer_expr(rhs);
                self.binary(*op, lhs, rhs, span)
            }
            ast::ExprKind::Call { callee, args } => self.infer_call(callee, args, span),
            ast::ExprKind::Field { target, name } => match self.member(target, name) {
                Some(Member::Value(v)) => v,
                Some(Member::Builtin(b)) if b.is_constant() => {
                    self.call_builtin(b, Vec::new(), span)
                }
                Some(Member::Builtin(b)) => {
                    self.not_a_value(&format!("built-in `{}`", b.name()), span)
                }
                Some(Member::Item(item)) => self.item_value(item, &name.name, span),
                None => {
                    let target = self.infer_expr(target);
                    self.field(target, name, span)
                }
            },
            ast::ExprKind::Index { target, items } => {
                let target = self.infer_expr(target);
                self.index(target, items, span)
            }
            ast::ExprKind::Lambda {
                params,
                types,
                body,
            } => self.lambda(params, types, body, span),
            ast::ExprKind::Query {
                op,
                target,
                body,
                parallel,
            } => self.query(*op, target, body.as_deref(), *parallel, span),
            ast::ExprKind::Together(inner) => {
                let module = self.module_of_ctx();
                self.stack.push(FnCtx {
                    module: module.0,
                    is_closure: true,
                    ..FnCtx::default()
                });
                let inner = self.infer_expr(inner);
                let ty = inner.ty.clone();
                let block = vec![hir::Stmt {
                    span: inner.span,
                    kind: hir::StmtKind::Return(Some(inner)),
                }];
                let closure =
                    self.finish_closure("<task>", 0, Ty::func(vec![], ty.clone()), block, span);
                expr(ExprKind::Together(Box::new(closure)), Ty::task(ty), span)
            }
            ast::ExprKind::Await(inner) => {
                let inner = self.infer_expr(inner);
                let out = self.infer.fresh();
                if !self.infer.unify(&inner.ty, &Ty::task(out.clone())) {
                    let msg = format!("`await` needs a `Task`, found `{}`", self.show(&inner.ty));
                    self.error(
                        Diagnostic::error(inner.span, msg)
                            .with_help("create a task with `together f(x)`"),
                    );
                }
                expr(ExprKind::Await(Box::new(inner)), out, span)
            }
            ast::ExprKind::RunParallel(items) => {
                let elem = self.infer.fresh();
                let module = self.module_of_ctx();
                let items = items
                    .iter()
                    .map(|i| {
                        self.stack.push(FnCtx {
                            module: module.0,
                            is_closure: true,
                            ..FnCtx::default()
                        });
                        let item = self.infer_expr(i);
                        let item = self.coerce(item, &elem);
                        let block = vec![hir::Stmt {
                            span: item.span,
                            kind: hir::StmtKind::Return(Some(item)),
                        }];
                        self.finish_closure(
                            "<parallel>",
                            0,
                            Ty::func(vec![], elem.clone()),
                            block,
                            i.span,
                        )
                    })
                    .collect();
                expr(ExprKind::RunParallel(items), Ty::list(elem), span)
            }
        }
    }

    fn expect_bool(&mut self, e: &hir::Expr, message: &str) {
        if !self.infer.unify(&e.ty, &Ty::Bool) {
            let label = format!("this is `{}`", self.show(&e.ty));
            self.error(
                Diagnostic::error(e.span, message.to_string())
                    .with_label(label)
                    .with_help("compare explicitly, for example `x != 0` or `items.length() > 0`"),
            );
        }
    }

    pub(super) fn binary(
        &mut self,
        op: BinOp,
        lhs: hir::Expr,
        rhs: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        if matches!(op, BinOp::And | BinOp::Or) {
            let msg = format!("`{}` needs `Bool` operands", op.symbol());
            self.expect_bool(&lhs, &msg);
            self.expect_bool(&rhs, &msg);
            return expr(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                Ty::Bool,
                span,
            );
        }
        let out = self.infer.fresh();
        self.add_constraint(Constraint::Binary {
            op,
            lhs: lhs.ty.clone(),
            rhs: rhs.ty.clone(),
            out: out.clone(),
            span,
        });
        expr(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            out,
            span,
        )
    }

    /// The element type for a literal's items: their common type, `Float` when
    /// `Int`s and `Float`s mix, or `Any` when their kinds conflict.
    fn element_type(&mut self, items: &[hir::Expr]) -> Ty {
        let heads: Vec<Ty> = items
            .iter()
            .map(|i| self.infer.shallow(&i.ty))
            .filter(|t| !matches!(t, Ty::Var(_)))
            .collect();
        let kind = |t: &Ty| std::mem::discriminant(t);
        let numeric = heads.iter().all(Ty::is_numeric);
        if numeric && heads.contains(&Ty::Float) {
            return Ty::Float;
        }
        if heads.windows(2).any(|w| {
            kind(&w[0]) != kind(&w[1])
                || matches!((&w[0], &w[1]), (Ty::Class(a), Ty::Class(b)) if a != b)
        }) {
            return Ty::Any;
        }
        self.infer.fresh()
    }

    fn infer_list(&mut self, items: &[ast::Expr], span: Span) -> hir::Expr {
        let items: Vec<hir::Expr> = items
            .iter()
            .map(|i| {
                let item = self.infer_expr(i);
                self.unshape(item)
            })
            .collect();
        let elem = self.element_type(&items);
        let items = items.into_iter().map(|i| self.coerce(i, &elem)).collect();
        expr(ExprKind::List(items), Ty::list(elem), span)
    }

    fn not_a_value(&mut self, what: &str, span: Span) -> hir::Expr {
        self.error(
            Diagnostic::error(span, format!("{what} cannot be used as a value"))
                .with_help("call it, or wrap it in a closure like `|x| f(x)`"),
        );
        error_expr(span)
    }

    pub(super) fn infer_name(&mut self, name: &str, span: Span) -> hir::Expr {
        match self.lookup(name) {
            Some(Resolved::Local(l)) => {
                let ty = self.ctx().locals[l.0 as usize].ty.clone();
                expr(ExprKind::Local(l), ty, span)
            }
            Some(Resolved::Global(g)) => self.item_value(Item::Global(g), name, span),
            Some(Resolved::Func(f)) => self.item_value(Item::Func(f), name, span),
            Some(Resolved::Class(c)) => self.item_value(Item::Class(c), name, span),
            Some(Resolved::Module(m)) => self.item_value(Item::Module(m), name, span),
            Some(Resolved::Std(s)) => self.item_value(Item::Std(s), name, span),
            Some(Resolved::Builtin(b)) => {
                self.not_a_value(&format!("built-in `{}`", b.name()), span)
            }
            Some(Resolved::Recurse(ty)) => {
                if self.stack.last().is_some_and(|c| c.self_name.is_none()) {
                    let msg = format!("cannot refer to `{name}` from a closure inside it");
                    self.error(Diagnostic::error(span, msg));
                    return error_expr(span);
                }
                expr(ExprKind::Recurse, ty, span)
            }
            None => {
                let mut diag = Diagnostic::error(span, format!("unknown name `{name}`"));
                if name == "it" {
                    diag = diag.with_help(
                        "`it` is only available inside `repeat` blocks and query bodies",
                    );
                } else if let Some(similar) = self.similar_name(name) {
                    diag = diag.with_help(format!("did you mean `{similar}`?"));
                }
                self.error(diag);
                error_expr(span)
            }
        }
    }

    fn similar_name(&self, name: &str) -> Option<String> {
        let ctx = self.stack.last()?;
        let candidates = ctx
            .names
            .keys()
            .chain(self.modules[ctx.module as usize].names.keys())
            .filter(|c| edit_distance(c, name) <= 2.min(name.len() / 2).max(1));
        candidates.min_by_key(|c| edit_distance(c, name)).cloned()
    }

    fn item_value(&mut self, item: Item, name: &str, span: Span) -> hir::Expr {
        match item {
            Item::Global(g) => {
                let ty = self.globals[g.0 as usize].ty.clone();
                expr(ExprKind::Global(g), ty, span)
            }
            Item::Func(f) => {
                let scheme = self.func_scheme(f);
                let (ty, inst) = self.instantiate(&scheme);
                expr(ExprKind::Func { func: f, inst }, ty, span)
            }
            Item::Class(_) => {
                self.error(
                    Diagnostic::error(span, format!("class `{name}` cannot be used as a value"))
                        .with_help(format!("create an instance with `{name}(...)`")),
                );
                error_expr(span)
            }
            Item::Module(_) | Item::Std(_) => {
                self.error(
                    Diagnostic::error(span, format!("module `{name}` cannot be used as a value"))
                        .with_help(format!("use its members, like `{name}.something`")),
                );
                error_expr(span)
            }
        }
    }

    /// Resolves `target.name` statically when `target` names a module.
    fn member(&mut self, target: &ast::Expr, name: &ast::Ident) -> Option<Member> {
        let ast::ExprKind::Name(module_name) = &target.kind else {
            return None;
        };
        match self.lookup(module_name)? {
            Resolved::Module(m) => {
                match self.modules[m.0 as usize].names.get(&name.name).copied() {
                    Some(item) => Some(Member::Item(item)),
                    None => {
                        let msg = format!("module `{module_name}` has no member `{}`", name.name);
                        self.error(Diagnostic::error(name.span, msg));
                        Some(Member::Value(error_expr(name.span)))
                    }
                }
            }
            Resolved::Std(s) => match s.member(&name.name) {
                Some(b) => Some(Member::Builtin(b)),
                None => {
                    let msg = format!("module `{module_name}` has no member `{}`", name.name);
                    self.error(Diagnostic::error(name.span, msg));
                    Some(Member::Value(error_expr(name.span)))
                }
            },
            _ => None,
        }
    }

    fn field(&mut self, target: hir::Expr, name: &ast::Ident, span: Span) -> hir::Expr {
        let out = self.infer.fresh();
        self.add_constraint(Constraint::Field {
            recv: target.ty.clone(),
            name: name.name.clone(),
            out: out.clone(),
            span: name.span,
        });
        expr(
            ExprKind::Field {
                target: Box::new(target),
                name: name.name.clone(),
            },
            out,
            span,
        )
    }

    fn index(&mut self, target: hir::Expr, items: &[ast::IndexItem], span: Span) -> hir::Expr {
        if items.len() != 1 {
            self.error(Diagnostic::error(
                span,
                "multi-dimensional indexing needs tensors, which are not available yet",
            ));
            return error_expr(span);
        }
        match &items[0] {
            ast::IndexItem::Single(i) => {
                let index = self.infer_expr(i);
                let literal = match index.kind {
                    ExprKind::Int(n) => Some(n),
                    _ => None,
                };
                let out = self.infer.fresh();
                self.add_constraint(Constraint::Index {
                    target: target.ty.clone(),
                    index: index.ty.clone(),
                    out: out.clone(),
                    literal,
                    span,
                });
                expr(
                    ExprKind::Index {
                        target: Box::new(target),
                        index: Box::new(index),
                    },
                    out,
                    span,
                )
            }
            ast::IndexItem::Slice { start, end } => {
                let bound = |c: &mut Self, e: &Option<ast::Expr>| {
                    e.as_ref().map(|e| {
                        let v = c.infer_expr(e);
                        Box::new(c.coerce(v, &Ty::Int))
                    })
                };
                let start = bound(self, start);
                let end = bound(self, end);
                self.add_constraint(Constraint::Slice {
                    target: target.ty.clone(),
                    span,
                });
                let ty = match self.infer.zonk(&target.ty) {
                    Ty::Tensor(Some(d)) if !d.is_empty() => Ty::Tensor(Some(
                        std::iter::once(Ty::Any)
                            .chain(d[1..].iter().cloned())
                            .collect(),
                    )),
                    _ => target.ty.clone(),
                };
                expr(
                    ExprKind::Slice {
                        target: Box::new(target),
                        start,
                        end,
                    },
                    ty,
                    span,
                )
            }
        }
    }

    fn infer_args(&mut self, args: &[ast::Arg]) -> Vec<Arg> {
        args.iter()
            .map(|a| (a.name.clone(), self.infer_expr(&a.value)))
            .collect()
    }

    fn no_named_args(&mut self, args: &[Arg], what: &str) -> bool {
        if let Some((Some(n), _)) = args.iter().find(|(n, _)| n.is_some()) {
            self.error(Diagnostic::error(
                n.span,
                format!("{what} does not take named arguments"),
            ));
            return false;
        }
        true
    }

    fn infer_call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span) -> hir::Expr {
        if let ast::ExprKind::Name(name) = &callee.kind {
            match self.lookup(name) {
                Some(Resolved::Func(f)) => {
                    let args = self.infer_args(args);
                    return self.call_func(f, args, None, span);
                }
                Some(Resolved::Class(c)) => {
                    let args = self.infer_args(args);
                    return self.construct(c, args, span);
                }
                Some(Resolved::Builtin(b)) => {
                    let args = self.infer_args(args);
                    return self.builtin_call(b, args, span);
                }
                _ => {}
            }
        }
        if let ast::ExprKind::Field { target, name } = &callee.kind {
            match self.member(target, name) {
                Some(Member::Item(Item::Func(f))) => {
                    let args = self.infer_args(args);
                    return self.call_func(f, args, None, span);
                }
                Some(Member::Item(Item::Class(c))) => {
                    let args = self.infer_args(args);
                    return self.construct(c, args, span);
                }
                Some(Member::Builtin(b)) => {
                    let args = self.infer_args(args);
                    return self.builtin_call(b, args, span);
                }
                Some(Member::Item(item)) => {
                    let callee = self.item_value(item, &name.name, callee.span);
                    let args = self.infer_args(args);
                    return self.call_value(callee, args, span);
                }
                Some(Member::Value(v)) => return v,
                None => {
                    let recv = self.infer_expr(target);
                    let args = self.infer_args(args);
                    return self.method_call(recv, name, args, span);
                }
            }
        }
        let callee = self.infer_expr(callee);
        let args = self.infer_args(args);
        self.call_value(callee, args, span)
    }

    fn call_value(&mut self, callee: hir::Expr, args: Vec<Arg>, span: Span) -> hir::Expr {
        self.no_named_args(&args, "a function value");
        let args: Vec<hir::Expr> = args.into_iter().map(|(_, e)| e).collect();
        let ret = if self.infer.shallow(&callee.ty) == Ty::Any {
            Ty::Any
        } else {
            let ret = self.infer.fresh();
            let want = Ty::func(args.iter().map(|a| a.ty.clone()).collect(), ret.clone());
            if !self.infer.unify(&callee.ty, &want) {
                let msg = format!(
                    "cannot call a `{}` with these arguments",
                    self.show(&callee.ty)
                );
                let label = format!("arguments are `{}`", self.show(&want));
                self.error(Diagnostic::error(span, msg).with_label(label));
            }
            ret
        };
        expr(
            ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
            ret,
            span,
        )
    }

    /// Matches positional and named arguments to parameters. `None` marks a parameter left to its default.
    fn bind_args(
        &mut self,
        what: &str,
        params: &[(String, bool)],
        args: Vec<Arg>,
        span: Span,
    ) -> Option<Vec<Option<hir::Expr>>> {
        let mut slots: Vec<Option<hir::Expr>> = vec![None; params.len()];
        let mut ok = true;
        let mut next = 0;
        let given = args.len();
        for (name, value) in args {
            match name {
                None => {
                    if next >= params.len() {
                        let required = params.iter().filter(|(_, d)| !d).count();
                        self.error(arity_error(span, what, required, params.len(), given));
                        return None;
                    }
                    slots[next] = Some(value);
                    next += 1;
                }
                Some(n) => match params.iter().position(|(p, _)| *p == n.name) {
                    Some(i) if slots[i].is_some() => {
                        self.error(Diagnostic::error(
                            n.span,
                            format!("argument `{}` is given twice", n.name),
                        ));
                        ok = false;
                    }
                    Some(i) => slots[i] = Some(value),
                    None => {
                        self.error(Diagnostic::error(
                            n.span,
                            format!("`{what}` has no parameter named `{}`", n.name),
                        ));
                        ok = false;
                    }
                },
            }
        }
        for (i, (name, has_default)) in params.iter().enumerate() {
            if slots[i].is_none() && !has_default {
                self.error(Diagnostic::error(
                    span,
                    format!("missing argument `{name}` in call to `{what}`"),
                ));
                ok = false;
            }
        }
        ok.then_some(slots)
    }

    pub(super) fn call_func(
        &mut self,
        f: FuncId,
        args: Vec<Arg>,
        recv: Option<hir::Expr>,
        span: Span,
    ) -> hir::Expr {
        let scheme = self.func_scheme(f);
        let (fty, inst) = self.instantiate(&scheme);
        let Ty::Func(param_tys, ret) = fty else {
            unreachable!("functions have function types")
        };
        let params: Vec<(String, bool)> = self.funcs[f.0 as usize]
            .params
            .iter()
            .map(|p| (p.name.clone(), p.default.is_some()))
            .collect();
        let name = self.funcs[f.0 as usize].name.clone();
        let Some(bound) = self.bind_args(&name, &params, args, span) else {
            return expr(ExprKind::None, *ret, span);
        };
        let offset = usize::from(recv.is_some());
        let mut out = Vec::with_capacity(bound.len() + offset);
        if let Some(r) = recv {
            self.infer.unify(&param_tys[0], &r.ty);
            out.push(r);
        }
        for (i, slot) in bound.into_iter().enumerate() {
            let arg = match slot {
                Some(e) => self.coerce(e, &param_tys[i + offset]),
                None => self.funcs[f.0 as usize].params[i]
                    .default
                    .clone()
                    .expect("default exists"),
            };
            out.push(arg);
        }
        expr(
            ExprKind::CallFunc {
                func: f,
                args: out,
                inst,
                native: Default::default(),
            },
            *ret,
            span,
        )
    }

    fn construct(&mut self, c: ClassId, args: Vec<Arg>, span: Span) -> hir::Expr {
        let class = &self.classes[c.0 as usize];
        let name = class.name.clone();
        let params: Vec<(String, bool)> = class
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.default.is_some()))
            .collect();
        let Some(bound) = self.bind_args(&name, &params, args, span) else {
            return expr(ExprKind::None, Ty::Class(c), span);
        };
        let mut out = Vec::with_capacity(bound.len());
        for (i, slot) in bound.into_iter().enumerate() {
            let field_ty = self.classes[c.0 as usize].fields[i].ty.clone();
            let arg = match slot {
                Some(e) => {
                    let e = self.unshape(e);
                    self.coerce(e, &field_ty)
                }
                None => self.classes[c.0 as usize].fields[i]
                    .default
                    .clone()
                    .expect("default exists"),
            };
            out.push(arg);
        }
        expr(
            ExprKind::New {
                class: c,
                args: out,
            },
            Ty::Class(c),
            span,
        )
    }

    fn method_call(
        &mut self,
        recv: hir::Expr,
        name: &ast::Ident,
        args: Vec<Arg>,
        span: Span,
    ) -> hir::Expr {
        if let Ty::Class(c) = self.infer.shallow(&recv.ty) {
            let class = &self.classes[c.0 as usize];
            if let Some(m) = class
                .methods
                .iter()
                .find(|(n, _)| *n == name.name)
                .map(|(_, f)| *f)
            {
                return self.call_func(m, args, Some(recv), span);
            }
            if class.fields.iter().any(|f| f.name == name.name) {
                let callee = self.field(recv, name, name.span);
                return self.call_value(callee, args, span);
            }
            let msg = format!("`{}` has no method `{}`", class.name, name.name);
            self.error(Diagnostic::error(name.span, msg));
            return error_expr(span);
        }
        self.no_named_args(&args, "this method");
        let args: Vec<hir::Expr> = args.into_iter().map(|(_, e)| e).collect();
        let out = self.infer.fresh();
        self.add_constraint(Constraint::Method {
            recv: recv.ty.clone(),
            name: name.name.clone(),
            args: args.iter().map(|a| a.ty.clone()).collect(),
            lits: args.iter().map(Lit::of).collect(),
            out: out.clone(),
            span: name.span,
        });
        expr(
            ExprKind::MethodCall {
                recv: Box::new(recv),
                name: name.name.clone(),
                args,
            },
            out,
            span,
        )
    }

    fn builtin_call(&mut self, b: Builtin, args: Vec<Arg>, span: Span) -> hir::Expr {
        if !self.no_named_args(&args, &format!("`{}`", b.name())) {
            return error_expr(span);
        }
        let args = args.into_iter().map(|(_, e)| e).collect();
        self.call_builtin(b, args, span)
    }

    /// The shaped tensor type a tensor-creating builtin returns, when its arguments tell.
    fn builtin_shape(&self, b: Builtin, args: &[hir::Expr]) -> Option<Ty> {
        let sizes = |e: &hir::Expr| match Lit::of(e) {
            Lit::Ints(dims) if dims.iter().all(|d| *d >= 0) => {
                Some(dims.iter().map(|d| *d as u64).collect::<Vec<_>>())
            }
            _ => None,
        };
        let size = |e: &hir::Expr| match Lit::of(e) {
            Lit::Int(n) if n >= 0 => Some(n as u64),
            _ => None,
        };
        let first = args.first()?;
        match b {
            Builtin::NnZeros | Builtin::NnOnes | Builtin::NnRandn | Builtin::NnRand => {
                sizes(first).map(Ty::shaped)
            }
            Builtin::NnEye | Builtin::NnTril => size(first).map(|n| Ty::shaped([n, n])),
            Builtin::NnArange => size(first).map(|n| Ty::shaped([n])),
            Builtin::NnTensor => shape::of_literal(first).map(|d| Ty::Tensor(Some(d))),
            Builtin::NnParam => Some(self.infer.zonk(&first.ty)).filter(Ty::is_tensor),
            Builtin::NnMse | Builtin::NnCrossEntropy => Some(Ty::shaped([])),
            _ => None,
        }
    }

    /// Forgets the tensor shapes in a value about to be stored somewhere that
    /// may hold tensors of different shapes over time.
    pub(super) fn unshape(&self, mut value: hir::Expr) -> hir::Expr {
        value.ty = self.unshaped(&value.ty);
        value
    }

    fn call_builtin(&mut self, b: Builtin, args: Vec<hir::Expr>, span: Span) -> hir::Expr {
        let arity = |c: &mut Self, min: usize, max: usize| -> bool {
            if args.len() < min || args.len() > max {
                c.error(arity_error(span, b.name(), min, max, args.len()));
                return false;
            }
            true
        };
        if let Some((params, required, ret)) = b.fixed_signature() {
            if !arity(self, required, params.len()) {
                return error_expr(span);
            }
            let args: Vec<hir::Expr> = args
                .into_iter()
                .zip(&params)
                .map(|(a, p)| self.coerce(a, p))
                .collect();
            let ret = self.builtin_shape(b, &args).unwrap_or(ret);
            return expr(ExprKind::CallBuiltin { builtin: b, args }, ret, span);
        }
        let ty = match b {
            Builtin::Len => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                self.add_constraint(Constraint::Len {
                    ty: args[0].ty.clone(),
                    span,
                });
                Ty::Int
            }
            Builtin::Range => {
                if !arity(self, 1, 3) {
                    return error_expr(span);
                }
                let args = args.into_iter().map(|a| self.coerce(a, &Ty::Int)).collect();
                return expr(
                    ExprKind::CallBuiltin { builtin: b, args },
                    Ty::list(Ty::Int),
                    span,
                );
            }
            Builtin::ToStr | Builtin::TypeOf | Builtin::JsonStringify => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                Ty::Str
            }
            Builtin::ToInt | Builtin::ToFloat | Builtin::ToBool => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                match b {
                    Builtin::ToInt => Ty::Int,
                    Builtin::ToFloat => Ty::Float,
                    _ => Ty::Bool,
                }
            }
            Builtin::Abs => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                self.add_constraint(Constraint::Numeric {
                    ty: args[0].ty.clone(),
                    span,
                });
                args[0].ty.clone()
            }
            Builtin::Round => {
                if !arity(self, 1, 2) {
                    return error_expr(span);
                }
                self.add_constraint(Constraint::Numeric {
                    ty: args[0].ty.clone(),
                    span,
                });
                if args.len() == 2 {
                    let mut args = args;
                    let digits = args.pop().unwrap();
                    let digits = self.coerce(digits, &Ty::Int);
                    args.push(digits);
                    return expr(ExprKind::CallBuiltin { builtin: b, args }, Ty::Float, span);
                }
                Ty::Int
            }
            Builtin::Min | Builtin::Max => {
                if args.is_empty() {
                    arity(self, 1, usize::MAX);
                    return error_expr(span);
                }
                if args.len() == 1 {
                    let item = self.infer.fresh();
                    self.unify_at(args[0].span, &Ty::list(item.clone()), &args[0].ty);
                    item
                } else {
                    let has_float = args.iter().any(|a| self.infer.shallow(&a.ty) == Ty::Float);
                    let ty = if has_float {
                        Ty::Float
                    } else {
                        self.infer.fresh()
                    };
                    let args = args.into_iter().map(|a| self.coerce(a, &ty)).collect();
                    return expr(ExprKind::CallBuiltin { builtin: b, args }, ty, span);
                }
            }
            Builtin::AssertEq => {
                if !arity(self, 2, 3) {
                    return error_expr(span);
                }
                self.add_constraint(Constraint::Binary {
                    op: BinOp::Eq,
                    lhs: args[0].ty.clone(),
                    rhs: args[1].ty.clone(),
                    out: Ty::Bool,
                    span,
                });
                if args.len() == 3 {
                    self.unify_at(args[2].span, &Ty::Str, &args[2].ty);
                }
                Ty::None
            }
            Builtin::Enumerate | Builtin::Reversed | Builtin::Sum => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                let item = self.infer.fresh();
                self.unify_at(args[0].span, &Ty::list(item.clone()), &args[0].ty);
                match b {
                    Builtin::Enumerate => Ty::list(Ty::Tuple(vec![Ty::Int, item])),
                    Builtin::Reversed => Ty::list(item),
                    _ => {
                        self.add_constraint(Constraint::Summable {
                            ty: item.clone(),
                            span,
                        });
                        item
                    }
                }
            }
            Builtin::Zip => {
                if !arity(self, 2, 2) {
                    return error_expr(span);
                }
                let (a, b) = (self.infer.fresh(), self.infer.fresh());
                self.unify_at(args[0].span, &Ty::list(a.clone()), &args[0].ty);
                self.unify_at(args[1].span, &Ty::list(b.clone()), &args[1].ty);
                Ty::list(Ty::Tuple(vec![a, b]))
            }
            Builtin::Sorted => {
                if !arity(self, 1, 2) {
                    return error_expr(span);
                }
                let item = self.infer.fresh();
                self.unify_at(args[0].span, &Ty::list(item.clone()), &args[0].ty);
                if let Some(key) = args.get(1) {
                    let k = self.infer.fresh();
                    self.unify_at(key.span, &Ty::func(vec![item.clone()], k), &key.ty);
                }
                Ty::list(item)
            }
            Builtin::RandomChoice | Builtin::RandomShuffle => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                let item = self.infer.fresh();
                self.unify_at(args[0].span, &Ty::list(item.clone()), &args[0].ty);
                if b == Builtin::RandomChoice {
                    item
                } else {
                    Ty::None
                }
            }
            Builtin::Fail => {
                if !arity(self, 1, 1) {
                    return error_expr(span);
                }
                self.unify_at(args[0].span, &Ty::Str, &args[0].ty);
                self.infer.fresh()
            }
            _ => unreachable!("builtin `{}` has a fixed signature", b.name()),
        };
        expr(ExprKind::CallBuiltin { builtin: b, args }, ty, span)
    }

    fn lambda(
        &mut self,
        params: &[ast::Ident],
        types: &[Option<ast::TypeExpr>],
        body: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let module = self.module_of_ctx();
        let mut ctx = FnCtx {
            module: module.0,
            is_closure: true,
            ..FnCtx::default()
        };
        let mut param_tys = Vec::new();
        for (p, annotation) in params.iter().zip(types) {
            let ty = match annotation {
                Some(t) => self.resolve_type(t),
                None => self.infer.fresh(),
            };
            let local = ctx.add_local(&p.name, ty.clone());
            ctx.names.insert(p.name.clone(), local);
            param_tys.push(ty);
        }
        self.stack.push(ctx);
        let body = self.infer_expr(body);
        let ret = body.ty.clone();
        let body_span = body.span;
        let block = vec![hir::Stmt {
            kind: hir::StmtKind::Return(Some(body)),
            span: body_span,
        }];
        self.finish_closure(
            "<closure>",
            params.len(),
            Ty::func(param_tys, ret),
            block,
            span,
        )
    }

    /// Pops a closure context and returns the expression that creates the closure.
    pub(super) fn finish_closure(
        &mut self,
        name: &str,
        params: usize,
        ty: Ty,
        body: hir::Block,
        span: Span,
    ) -> hir::Expr {
        let ctx = self.stack.pop().expect("closure context");
        self.ctx().pending.extend(ctx.pending);
        let module = super::hir::ModuleId(ctx.module);
        let func = self.add_func(name, module, None, None);
        let captures = ctx
            .captures
            .iter()
            .map(|(_, outer)| {
                let ty = self.ctx().locals[outer.0 as usize].ty.clone();
                expr(ExprKind::Local(*outer), ty, span)
            })
            .collect();
        let slot = &mut self.funcs[func.0 as usize];
        slot.state = FnState::Done(Scheme {
            vars: vec![],
            ty: ty.clone(),
            constraints: vec![],
        });
        slot.hir = Some(hir::Func {
            name: name.to_string(),
            generics: Vec::new(),
            params,
            defaults: Vec::new(),
            captures: ctx.captures.iter().map(|(inner, _)| *inner).collect(),
            locals: ctx.locals,
            body,
            ty: ty.clone(),
        });
        expr(ExprKind::Closure { func, captures }, ty, span)
    }

    fn query(
        &mut self,
        op: QueryOp,
        target: &ast::Expr,
        body: Option<&ast::Expr>,
        parallel: bool,
        span: Span,
    ) -> hir::Expr {
        let target = self.infer_expr(target);
        let item_ty = self.infer.fresh();
        if let Ty::Tensor(shape) = self.infer.zonk(&target.ty) {
            let row = shape.map(|d| d.get(1..).unwrap_or_default().to_vec());
            self.infer.unify(&item_ty, &Ty::Tensor(row));
        } else if !self.infer.unify(&target.ty, &Ty::list(item_ty.clone())) {
            let msg = format!(
                "`{}` works on lists and tensors, found `{}`",
                op.word(),
                self.show(&target.ty)
            );
            self.error(Diagnostic::error(target.span, msg));
        }
        let item = self.ctx().add_local("it", item_ty.clone());
        if parallel && op == QueryOp::Select {
            return self.parallel_select(target, item, item_ty, body, span);
        }
        let body = body.map(|b| self.query_body(b, item, &item_ty));
        let elem = body
            .as_ref()
            .map(|b| b.ty.clone())
            .unwrap_or_else(|| item_ty.clone());
        let ty = match op {
            QueryOp::Where => {
                if let Some(b) = &body {
                    self.expect_bool(b, "a `where` condition must be a `Bool`");
                }
                Ty::list(item_ty)
            }
            QueryOp::Select => Ty::list(elem),
            QueryOp::Sum => {
                self.add_constraint(Constraint::Summable {
                    ty: elem.clone(),
                    span,
                });
                elem
            }
            QueryOp::Average => {
                self.add_constraint(Constraint::Numeric { ty: elem, span });
                Ty::Float
            }
            QueryOp::Min | QueryOp::Max => elem,
        };
        expr(
            ExprKind::Query {
                op,
                target: Box::new(target),
                item,
                body: body.map(Box::new),
                parallel,
            },
            ty,
            span,
        )
    }

    /// A parallel `select` runs its body as a one-parameter closure on many threads.
    fn parallel_select(
        &mut self,
        target: hir::Expr,
        item: LocalId,
        item_ty: Ty,
        body: Option<&ast::Expr>,
        span: Span,
    ) -> hir::Expr {
        let module = self.module_of_ctx();
        let mut ctx = FnCtx {
            module: module.0,
            is_closure: true,
            ..FnCtx::default()
        };
        let param = ctx.add_local("it", item_ty.clone());
        self.stack.push(ctx);
        let value = match body {
            Some(b) => self.query_body(b, param, &item_ty),
            None => expr(ExprKind::Local(param), item_ty.clone(), span),
        };
        let out = value.ty.clone();
        let block = vec![hir::Stmt {
            span: value.span,
            kind: hir::StmtKind::Return(Some(value)),
        }];
        let closure = self.finish_closure(
            "<select>",
            1,
            Ty::func(vec![item_ty], out.clone()),
            block,
            span,
        );
        let kind = ExprKind::Query {
            op: QueryOp::Select,
            target: Box::new(target),
            item,
            body: Some(Box::new(closure)),
            parallel: true,
        };
        expr(kind, Ty::list(out), span)
    }

    /// Checks a query body with the current item bound to `item`, as `it` or as a closure parameter.
    fn query_body(&mut self, body: &ast::Expr, item: LocalId, item_ty: &Ty) -> hir::Expr {
        if let ast::ExprKind::Lambda {
            params,
            body: inner,
            ..
        } = &body.kind
        {
            if params.len() != 1 {
                self.error(Diagnostic::error(
                    body.span,
                    "a query closure takes exactly one parameter",
                ));
                return error_expr(body.span);
            }
            self.ctx().scopes.push((params[0].name.clone(), item));
            let out = self.infer_expr(inner);
            self.ctx().scopes.pop();
            return out;
        }
        self.ctx().scopes.push(("it".into(), item));
        let out = self.infer_expr(body);
        self.ctx().scopes.pop();
        if let Ty::Func(..) = self.infer.shallow(&out.ty) {
            let arg = expr(ExprKind::Local(item), item_ty.clone(), out.span);
            let span = out.span;
            return self.call_value(out, vec![(None, arg)], span);
        }
        out
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != *cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}
