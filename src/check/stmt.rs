use super::hir::{self, Place, StmtKind};
use super::solve::Constraint;
use super::types::Ty;
use super::{Checker, FnCtx, Item, Resolved};
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::syntax::ast;
use std::collections::HashMap;

/// Collects the names a block assigns, not looking inside nested functions or classes.
pub(super) fn assigned_names(block: &[ast::Stmt], out: &mut Vec<ast::Ident>) {
    let add = |name: &ast::Ident, out: &mut Vec<ast::Ident>| {
        if !out.iter().any(|n| n.name == name.name) {
            out.push(name.clone());
        }
    };
    for stmt in block {
        match &stmt.kind {
            ast::StmtKind::Assign { target, .. } | ast::StmtKind::AugAssign { target, .. } => {
                target_names(target, out)
            }
            ast::StmtKind::If {
                branches,
                otherwise,
            } => {
                for (_, b) in branches {
                    assigned_names(b, out);
                }
                if let Some(b) = otherwise {
                    assigned_names(b, out);
                }
            }
            ast::StmtKind::While { body, .. } | ast::StmtKind::Repeat { body, .. } => {
                assigned_names(body, out)
            }
            ast::StmtKind::For { vars, body, .. } => {
                for var in vars {
                    add(var, out);
                }
                assigned_names(body, out);
            }
            ast::StmtKind::Try { body, var, handler } => {
                assigned_names(body, out);
                add(var, out);
                assigned_names(handler, out);
            }
            ast::StmtKind::Use(ast::Use::Python { module, alias }) => {
                let name = alias.clone().unwrap_or_else(|| ast::Ident {
                    name: module.split('.').next().unwrap_or(module).to_string(),
                    span: stmt.span,
                });
                add(&name, out);
            }
            _ => {}
        }
    }
}

fn target_names(target: &ast::Expr, out: &mut Vec<ast::Ident>) {
    match &target.kind {
        ast::ExprKind::Name(n) if !out.iter().any(|o| o.name == *n) => out.push(ast::Ident {
            name: n.clone(),
            span: target.span,
        }),
        ast::ExprKind::Tuple(items) => items.iter().for_each(|i| target_names(i, out)),
        _ => {}
    }
}

/// Counts the statements assigning each name and the `return`s with a value,
/// not looking inside nested functions or classes.
pub(super) fn count_assignments(
    block: &[ast::Stmt],
    names: &mut HashMap<String, usize>,
    returns: &mut usize,
) {
    let add = |name: &str, names: &mut HashMap<String, usize>| {
        *names.entry(name.to_string()).or_default() += 1
    };
    for stmt in block {
        match &stmt.kind {
            ast::StmtKind::Assign { target, .. } | ast::StmtKind::AugAssign { target, .. } => {
                let mut targets = Vec::new();
                target_names(target, &mut targets);
                targets.iter().for_each(|t| add(&t.name, names));
            }
            ast::StmtKind::Return(Some(_)) => *returns += 1,
            ast::StmtKind::If {
                branches,
                otherwise,
            } => {
                for (_, b) in branches {
                    count_assignments(b, names, returns);
                }
                if let Some(b) = otherwise {
                    count_assignments(b, names, returns);
                }
            }
            ast::StmtKind::While { body, .. } | ast::StmtKind::Repeat { body, .. } => {
                count_assignments(body, names, returns)
            }
            ast::StmtKind::For { vars, body, .. } => {
                vars.iter().for_each(|v| add(&v.name, names));
                count_assignments(body, names, returns);
            }
            ast::StmtKind::Try { body, var, handler } => {
                count_assignments(body, names, returns);
                add(&var.name, names);
                count_assignments(handler, names, returns);
            }
            _ => {}
        }
    }
}

/// Whether control can never fall off the end of `block`.
pub(super) fn always_returns(block: &[ast::Stmt]) -> bool {
    block.iter().any(|stmt| match &stmt.kind {
        ast::StmtKind::Return(_) => true,
        ast::StmtKind::If {
            branches,
            otherwise: Some(otherwise),
        } => branches.iter().all(|(_, b)| always_returns(b)) && always_returns(otherwise),
        ast::StmtKind::Try { body, handler, .. } => always_returns(body) && always_returns(handler),
        ast::StmtKind::While { cond, body } => {
            matches!(cond.kind, ast::ExprKind::Bool(true)) && !breaks(body)
        }
        ast::StmtKind::Expr(e) => is_fail_call(e),
        _ => false,
    })
}

fn is_fail_call(e: &ast::Expr) -> bool {
    matches!(&e.kind, ast::ExprKind::Call { callee, .. } if matches!(&callee.kind, ast::ExprKind::Name(n) if n == "fail"))
}

fn breaks(block: &[ast::Stmt]) -> bool {
    block.iter().any(|stmt| match &stmt.kind {
        ast::StmtKind::Break => true,
        ast::StmtKind::If {
            branches,
            otherwise,
        } => branches.iter().any(|(_, b)| breaks(b)) || otherwise.as_deref().is_some_and(breaks),
        ast::StmtKind::Try { body, handler, .. } => breaks(body) || breaks(handler),
        _ => false,
    })
}

impl Checker {
    pub(super) fn check_block(&mut self, block: &[ast::Stmt]) -> hir::Block {
        self.ctx().depth += 1;
        let mut out = Vec::with_capacity(block.len());
        for stmt in block {
            if let Some(s) = self.check_stmt(stmt, &mut out) {
                out.push(s);
            }
            self.solve_pending();
        }
        self.ctx().depth -= 1;
        out
    }

    fn check_cond(&mut self, cond: &ast::Expr, what: &str) -> hir::Expr {
        let cond = self.infer_expr(cond);
        if !self.infer.unify(&cond.ty, &Ty::Bool) {
            let msg = format!("{what} condition must be a `Bool`");
            let label = format!("this is `{}`", self.show(&cond.ty));
            self.error(
                Diagnostic::error(cond.span, msg)
                    .with_label(label)
                    .with_help("compare explicitly, for example `x != 0` or `items.length() > 0`"),
            );
        }
        cond
    }

    fn in_loop<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.ctx().loop_depth += 1;
        let out = f(self);
        self.ctx().loop_depth -= 1;
        out
    }

    /// Assigns `value` to a tuple pattern by storing it in a hidden local and
    /// assigning each element in turn.
    fn unpack(&mut self, items: &[ast::Expr], value: hir::Expr, span: Span, out: &mut hir::Block) {
        let elems: Vec<Ty> = items.iter().map(|_| self.infer.fresh()).collect();
        let want = Ty::Tuple(elems.clone());
        if !self.infer.unify(&want, &value.ty) {
            let msg = format!(
                "cannot unpack a `{}` into {} variables",
                self.show(&value.ty),
                items.len()
            );
            self.error(Diagnostic::error(value.span, msg));
            return;
        }
        let tmp = self.ctx().add_local("<tuple>", value.ty.clone());
        out.push(hir::Stmt {
            kind: StmtKind::Assign {
                place: Place::Local(tmp),
                op: None,
                value,
            },
            span,
        });
        for (i, (item, ty)) in items.iter().zip(elems).enumerate() {
            let source = hir::Expr {
                kind: hir::ExprKind::Local(tmp),
                ty: want.clone(),
                span: item.span,
            };
            let index = hir::Expr {
                kind: hir::ExprKind::Int(i as i64),
                ty: Ty::Int,
                span: item.span,
            };
            let elem = hir::Expr {
                kind: hir::ExprKind::Index {
                    target: Box::new(source),
                    index: Box::new(index),
                },
                ty,
                span: item.span,
            };
            self.assign_to(item, elem, span, out);
        }
    }

    fn assign_to(
        &mut self,
        target: &ast::Expr,
        value: hir::Expr,
        span: Span,
        out: &mut hir::Block,
    ) {
        if let ast::ExprKind::Tuple(items) = &target.kind {
            return self.unpack(items, value, span, out);
        }
        let Some(place) = self.place(target) else {
            return;
        };
        let place_ty = self.place_ty(&place, target.span);
        let value = self.store(value, &place, target);
        let value = self.coerce(value, &place_ty);
        out.push(hir::Stmt {
            kind: StmtKind::Assign {
                place,
                op: None,
                value,
            },
            span,
        });
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt, out: &mut hir::Block) -> Option<hir::Stmt> {
        let span = stmt.span;
        let kind = match &stmt.kind {
            ast::StmtKind::Expr(e) => StmtKind::Expr(self.infer_expr(e)),
            ast::StmtKind::Assign {
                target,
                ty: None,
                value,
            } if matches!(target.kind, ast::ExprKind::Tuple(_)) => {
                let value = self.infer_expr(value);
                self.assign_to(target, value, span, out);
                return None;
            }
            ast::StmtKind::Assign { target, ty, value } => {
                let annotated = ty.as_ref().map(|t| self.resolve_type(t));
                let place = self.place(target)?;
                let place_ty = self.place_ty(&place, target.span);
                if let Some(a) = &annotated {
                    self.unify_at(target.span, a, &place_ty);
                }
                let mut value = self.infer_expr(value);
                if annotated.is_none() {
                    value = self.store(value, &place, target);
                }
                let errors = self.diags.len();
                let value = self.coerce(value, &place_ty);
                if self.diags.len() > errors && matches!(place, Place::Local(_) | Place::Global(_))
                {
                    let diag = self.diags.pop().unwrap().with_help("a variable keeps the type of its first value; use a new name for a different type");
                    self.diags.push(diag);
                }
                StmtKind::Assign {
                    place,
                    op: None,
                    value,
                }
            }
            ast::StmtKind::AugAssign { target, op, value } => {
                let place = self.place(target)?;
                let place_ty = self.place_ty(&place, target.span);
                let value = self.infer_expr(value);
                let out = self.infer.fresh();
                self.add_constraint(Constraint::Binary {
                    op: *op,
                    lhs: place_ty.clone(),
                    rhs: value.ty.clone(),
                    out: out.clone(),
                    span,
                });
                if !self.infer.unify(&place_ty, &out) {
                    let msg = format!(
                        "`{}=` would change this variable from `{}` to `{}`",
                        op.symbol(),
                        self.show(&place_ty),
                        self.show(&out)
                    );
                    self.error(Diagnostic::error(span, msg).with_help("a variable keeps the type of its first value; start it as a Float, like `0.0`"));
                }
                StmtKind::Assign {
                    place,
                    op: Some(*op),
                    value,
                }
            }
            ast::StmtKind::Show(values) => {
                StmtKind::Show(values.iter().map(|v| self.infer_expr(v)).collect())
            }
            ast::StmtKind::If {
                branches,
                otherwise,
            } => {
                let branches = branches
                    .iter()
                    .map(|(c, b)| (self.check_cond(c, "an `if`"), self.check_block(b)))
                    .collect();
                let otherwise = otherwise
                    .as_ref()
                    .map(|b| self.check_block(b))
                    .unwrap_or_default();
                StmtKind::If {
                    branches,
                    otherwise,
                }
            }
            ast::StmtKind::While { cond, body } => {
                let cond = self.check_cond(cond, "a `while`");
                let body = self.in_loop(|c| c.check_block(body));
                StmtKind::While { cond, body }
            }
            ast::StmtKind::Repeat { count, body } => {
                let count = self.infer_expr(count);
                let count = self.coerce(count, &Ty::Int);
                let it = self.ctx().add_local("it", Ty::Int);
                self.ctx().scopes.push(("it".into(), it));
                let body = self.in_loop(|c| c.check_block(body));
                self.ctx().scopes.pop();
                StmtKind::Repeat { count, it, body }
            }
            ast::StmtKind::For { vars, iter, body } => {
                let iter = self.infer_expr(iter);
                if let [var] = vars.as_slice() {
                    let place = self.name_place(&var.name, var.span)?;
                    let item = self.place_ty(&place, var.span);
                    let iterable = if self.ctx().varied.contains(&var.name) {
                        self.unshaped(&iter.ty)
                    } else {
                        iter.ty.clone()
                    };
                    self.add_constraint(Constraint::Iter {
                        iterable,
                        item,
                        span: iter.span,
                    });
                    let body = self.in_loop(|c| c.check_block(body));
                    StmtKind::For {
                        var: place,
                        iter,
                        body,
                    }
                } else {
                    let item = self.infer.fresh();
                    let tmp = self.ctx().add_local("<item>", item.clone());
                    self.add_constraint(Constraint::Iter {
                        iterable: iter.ty.clone(),
                        item: item.clone(),
                        span: iter.span,
                    });
                    let targets: Vec<ast::Expr> = vars
                        .iter()
                        .map(|v| ast::Expr {
                            kind: ast::ExprKind::Name(v.name.clone()),
                            span: v.span,
                        })
                        .collect();
                    let mut block = Vec::new();
                    let current = hir::Expr {
                        kind: hir::ExprKind::Local(tmp),
                        ty: item,
                        span: iter.span,
                    };
                    self.unpack(&targets, current, span, &mut block);
                    block.extend(self.in_loop(|c| c.check_block(body)));
                    StmtKind::For {
                        var: Place::Local(tmp),
                        iter,
                        body: block,
                    }
                }
            }
            ast::StmtKind::Function(def) => {
                if self.ctx().is_toplevel {
                    if !self.is_module_level() {
                        self.error(Diagnostic::error(
                            def.name.span,
                            "functions must be defined at the top level or inside another function",
                        ));
                    }
                    return None;
                }
                let value = self.nested_function(def);
                let place = self.name_place(&def.name.name, def.name.span)?;
                let place_ty = self.place_ty(&place, def.name.span);
                self.unify_at(def.name.span, &place_ty, &value.ty);
                StmtKind::Assign {
                    place,
                    op: None,
                    value,
                }
            }
            ast::StmtKind::Class(def) => {
                if !self.is_module_level() {
                    self.error(Diagnostic::error(
                        def.name.span,
                        "classes must be defined at the top level",
                    ));
                }
                return None;
            }
            ast::StmtKind::Return(value) => {
                let Some(ret) = self.ctx().ret.clone() else {
                    self.error(Diagnostic::error(span, "`return` outside of a function"));
                    return None;
                };
                match value {
                    Some(v) => {
                        let mut v = self.infer_expr(v);
                        if self.ctx().varied.contains("return") {
                            v = self.unshape(v);
                        }
                        let v = self.coerce(v, &ret);
                        StmtKind::Return(Some(v))
                    }
                    None => {
                        if !self.infer.unify(&ret, &Ty::None) {
                            let msg = format!(
                                "this function returns `{}`, so `return` needs a value",
                                self.show(&ret)
                            );
                            self.error(Diagnostic::error(span, msg));
                        }
                        StmtKind::Return(None)
                    }
                }
            }
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                if self.ctx().loop_depth == 0 {
                    let word = if matches!(stmt.kind, ast::StmtKind::Break) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.error(Diagnostic::error(
                        span,
                        format!("`{word}` outside of a loop"),
                    ));
                    return None;
                }
                if matches!(stmt.kind, ast::StmtKind::Break) {
                    StmtKind::Break
                } else {
                    StmtKind::Continue
                }
            }
            ast::StmtKind::Pass => return None,
            ast::StmtKind::Try { body, var, handler } => {
                let body = self.check_block(body);
                let place = self.name_place(&var.name, var.span)?;
                let place_ty = self.place_ty(&place, var.span);
                self.unify_at(var.span, &Ty::Str, &place_ty);
                let handler = self.check_block(handler);
                StmtKind::Try {
                    body,
                    var: place,
                    handler,
                }
            }
            ast::StmtKind::Use(u) => return self.check_use(u, span),
        };
        Some(hir::Stmt { kind, span })
    }

    /// Whether the statement being checked sits directly in a module body.
    fn is_module_level(&self) -> bool {
        let ctx = self.stack.last().expect("inside a function");
        ctx.is_toplevel && !ctx.is_const && ctx.depth == 1
    }

    fn check_use(&mut self, u: &ast::Use, span: Span) -> Option<hir::Stmt> {
        match u {
            ast::Use::Python { module, alias } => {
                let name = alias
                    .as_ref()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| module.split('.').next().unwrap_or(module).to_string());
                let place = self.name_place(&name, span)?;
                let place_ty = self.place_ty(&place, span);
                self.unify_at(span, &place_ty, &Ty::Any);
                let value = hir::Expr {
                    kind: hir::ExprKind::PyImport(module.clone()),
                    ty: Ty::Any,
                    span,
                };
                Some(hir::Stmt {
                    kind: StmtKind::Assign {
                        place,
                        op: None,
                        value,
                    },
                    span,
                })
            }
            ast::Use::File { path, alias } => {
                if !self.is_module_level() {
                    self.error(Diagnostic::error(
                        span,
                        "`use` of a file must be at the top level",
                    ));
                    return None;
                }
                let module = self.module_of_ctx();
                let name = alias.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| {
                    std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path)
                        .to_string()
                });
                match self.modules[module.0 as usize].names.get(&name) {
                    Some(Item::Module(m)) => Some(hir::Stmt {
                        kind: StmtKind::InitModule(*m),
                        span,
                    }),
                    _ => None,
                }
            }
            ast::Use::Module { .. } => {
                if !self.is_module_level() {
                    self.error(Diagnostic::error(span, "`use` must be at the top level"));
                }
                None
            }
        }
    }

    fn name_place(&mut self, name: &str, span: Span) -> Option<Place> {
        match self.lookup(name) {
            Some(Resolved::Local(l)) => Some(Place::Local(l)),
            Some(Resolved::Global(g)) => Some(Place::Global(g)),
            Some(_) => {
                self.error(Diagnostic::error(
                    span,
                    format!(
                        "cannot assign to `{name}` because it names a function, class or module"
                    ),
                ));
                None
            }
            None => {
                let ty = self.infer.fresh();
                let l = self.ctx().add_local(name, ty);
                self.ctx().names.insert(name.to_string(), l);
                Some(Place::Local(l))
            }
        }
    }

    fn place(&mut self, target: &ast::Expr) -> Option<Place> {
        match &target.kind {
            ast::ExprKind::Name(name) => self.name_place(name, target.span),
            ast::ExprKind::Field { target: obj, name } => {
                let obj = self.infer_expr(obj);
                Some(Place::Field {
                    target: obj,
                    name: name.name.clone(),
                })
            }
            ast::ExprKind::Index { target: obj, items } => {
                let obj = self.infer_expr(obj);
                match items.as_slice() {
                    [ast::IndexItem::Single(i)] => {
                        let index = self.infer_expr(i);
                        Some(Place::Index { target: obj, index })
                    }
                    _ => {
                        self.error(Diagnostic::error(
                            target.span,
                            "only single elements can be assigned",
                        ));
                        None
                    }
                }
            }
            _ => {
                self.error(Diagnostic::error(
                    target.span,
                    "cannot assign to this expression",
                ));
                None
            }
        }
    }

    /// Prepares a value for storing: fields, elements and variables assigned in
    /// more than one place may hold tensors of different shapes over time, so
    /// values stored there keep only their type, not their shape.
    fn store(&mut self, value: hir::Expr, place: &Place, target: &ast::Expr) -> hir::Expr {
        let varied = match (place, &target.kind) {
            (Place::Field { .. } | Place::Index { .. }, _) => true,
            (Place::Global(_), _) if !self.ctx().is_toplevel => true,
            (_, ast::ExprKind::Name(n)) => self.ctx().varied.contains(n),
            _ => false,
        };
        if varied { self.unshape(value) } else { value }
    }

    fn place_ty(&mut self, place: &Place, span: Span) -> Ty {
        match place {
            Place::Local(l) => self.ctx().locals[l.0 as usize].ty.clone(),
            Place::Global(g) => self.globals[g.0 as usize].ty.clone(),
            Place::Field { target, name } => {
                let out = self.infer.fresh();
                self.add_constraint(Constraint::Field {
                    recv: target.ty.clone(),
                    name: name.clone(),
                    out: out.clone(),
                    span,
                });
                out
            }
            Place::Index { target, index } => {
                if self.infer.shallow(&target.ty) == Ty::Str {
                    self.error(
                        Diagnostic::error(span, "strings cannot be changed in place")
                            .with_help("build a new string instead"),
                    );
                    return Ty::Any;
                }
                let out = self.infer.fresh();
                self.add_constraint(Constraint::Index {
                    target: target.ty.clone(),
                    index: index.ty.clone(),
                    out: out.clone(),
                    literal: None,
                    span,
                });
                out
            }
        }
    }

    fn nested_function(&mut self, def: &ast::FunctionDef) -> hir::Expr {
        let module = self.module_of_ctx();
        let mut ctx = FnCtx {
            module: module.0,
            is_closure: true,
            ..FnCtx::default()
        };
        let mut param_tys = Vec::new();
        for p in &def.params {
            if p.default.is_some() {
                self.error(
                    Diagnostic::error(p.name.span, "nested functions cannot have default values")
                        .with_help("move the function to the top level"),
                );
            }
            let ty = match &p.ty {
                Some(t) => self.resolve_type(t),
                None => self.infer.fresh(),
            };
            let local = ctx.add_local(&p.name.name, ty.clone());
            ctx.names.insert(p.name.name.clone(), local);
            param_tys.push(ty);
        }
        let ret = match &def.ret {
            Some(t) => self.resolve_type(t),
            None => self.infer.fresh(),
        };
        let fty = Ty::func(param_tys, ret.clone());
        ctx.ret = Some(ret);
        ctx.self_name = Some((def.name.name.clone(), fty.clone()));
        self.declare_locals(&mut ctx, &def.body);
        self.stack.push(ctx);
        let body = self.check_function_body(&def.body, def.name.span, &def.name.name);
        self.solve_pending();
        self.finish_closure(&def.name.name, def.params.len(), fty, body, def.span)
    }
}
