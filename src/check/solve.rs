//! Deferred type constraints: operations whose meaning depends on a type that
//! may not be known yet. Constraints still open when a function is generalized
//! travel with its scheme and are re-checked at every use.

use super::Checker;
use super::builtins::method_signature;
use super::shape::{self, Lit};
use super::types::{Infer, Ty};
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::syntax::ast::BinOp;

#[derive(Debug, Clone)]
pub enum Constraint {
    Binary {
        op: BinOp,
        lhs: Ty,
        rhs: Ty,
        out: Ty,
        span: Span,
    },
    Neg {
        operand: Ty,
        out: Ty,
        span: Span,
    },
    Method {
        recv: Ty,
        name: String,
        args: Vec<Ty>,
        lits: Vec<Lit>,
        out: Ty,
        span: Span,
    },
    Field {
        recv: Ty,
        name: String,
        out: Ty,
        span: Span,
    },
    /// `literal` is the index when it is written as a number, which tuples require.
    Index {
        target: Ty,
        index: Ty,
        out: Ty,
        literal: Option<i64>,
        span: Span,
    },
    Slice {
        target: Ty,
        span: Span,
    },
    Iter {
        iterable: Ty,
        item: Ty,
        span: Span,
    },
    Len {
        ty: Ty,
        span: Span,
    },
    Numeric {
        ty: Ty,
        span: Span,
    },
    /// A number or a tensor, as `sum` accepts.
    Summable {
        ty: Ty,
        span: Span,
    },
}

enum Outcome {
    Done,
    Wait,
    Fail(Diagnostic),
}

impl Constraint {
    pub fn tys(&self) -> Vec<&Ty> {
        match self {
            Constraint::Binary { lhs, rhs, out, .. } => vec![lhs, rhs, out],
            Constraint::Neg { operand, out, .. } => vec![operand, out],
            Constraint::Method {
                recv, args, out, ..
            } => std::iter::once(recv)
                .chain(args)
                .chain(std::iter::once(out))
                .collect(),
            Constraint::Field { recv, out, .. } => vec![recv, out],
            Constraint::Index {
                target, index, out, ..
            } => vec![target, index, out],
            Constraint::Slice { target, .. } => vec![target],
            Constraint::Iter { iterable, item, .. } => vec![iterable, item],
            Constraint::Len { ty, .. }
            | Constraint::Numeric { ty, .. }
            | Constraint::Summable { ty, .. } => vec![ty],
        }
    }

    pub fn substitute(&self, infer: &Infer, map: &[(u32, Ty)]) -> Constraint {
        let s = |t: &Ty| infer.substitute(t, map);
        match self {
            Constraint::Binary {
                op,
                lhs,
                rhs,
                out,
                span,
            } => Constraint::Binary {
                op: *op,
                lhs: s(lhs),
                rhs: s(rhs),
                out: s(out),
                span: *span,
            },
            Constraint::Neg { operand, out, span } => Constraint::Neg {
                operand: s(operand),
                out: s(out),
                span: *span,
            },
            Constraint::Method {
                recv,
                name,
                args,
                lits,
                out,
                span,
            } => Constraint::Method {
                recv: s(recv),
                name: name.clone(),
                args: args.iter().map(s).collect(),
                lits: lits.clone(),
                out: s(out),
                span: *span,
            },
            Constraint::Field {
                recv,
                name,
                out,
                span,
            } => Constraint::Field {
                recv: s(recv),
                name: name.clone(),
                out: s(out),
                span: *span,
            },
            Constraint::Index {
                target,
                index,
                out,
                literal,
                span,
            } => Constraint::Index {
                target: s(target),
                index: s(index),
                out: s(out),
                literal: *literal,
                span: *span,
            },
            Constraint::Slice { target, span } => Constraint::Slice {
                target: s(target),
                span: *span,
            },
            Constraint::Iter {
                iterable,
                item,
                span,
            } => Constraint::Iter {
                iterable: s(iterable),
                item: s(item),
                span: *span,
            },
            Constraint::Len { ty, span } => Constraint::Len {
                ty: s(ty),
                span: *span,
            },
            Constraint::Numeric { ty, span } => Constraint::Numeric {
                ty: s(ty),
                span: *span,
            },
            Constraint::Summable { ty, span } => Constraint::Summable {
                ty: s(ty),
                span: *span,
            },
        }
    }
}

impl Checker {
    pub(super) fn add_constraint(&mut self, c: Constraint) {
        match self.solve(&c) {
            Outcome::Done => {}
            Outcome::Wait => self.ctx().pending.push(c),
            Outcome::Fail(d) => self.error(d),
        }
    }

    pub(super) fn solve_pending(&mut self) {
        loop {
            let pending = std::mem::take(&mut self.ctx().pending);
            let mut progress = false;
            let mut waiting = Vec::new();
            for c in pending {
                match self.solve(&c) {
                    Outcome::Done => progress = true,
                    Outcome::Wait => waiting.push(c),
                    Outcome::Fail(d) => {
                        progress = true;
                        self.error(d);
                    }
                }
            }
            self.ctx().pending.extend(waiting);
            if !progress {
                break;
            }
        }
    }

    /// Unifies an argument with a parameter, letting an `Int` stand in for a `Float`.
    fn unify_arg(&mut self, param: &Ty, arg: &Ty) -> bool {
        if self.infer.shallow(arg) == Ty::Int && self.infer.shallow(param) == Ty::Float {
            return true;
        }
        self.upcasts(arg, param) || self.infer.unify(param, arg)
    }

    fn mismatch_diag(&self, span: Span, expected: &Ty, found: &Ty) -> Diagnostic {
        let msg = format!(
            "expected `{}`, found `{}`",
            self.show(expected),
            self.show(found)
        );
        Diagnostic::error(span, "mismatched types").with_label(msg)
    }

    fn solve(&mut self, c: &Constraint) -> Outcome {
        match c {
            Constraint::Binary {
                op,
                lhs,
                rhs,
                out,
                span,
            } => self.solve_binary(*op, lhs, rhs, out, *span),
            Constraint::Neg { operand, out, span } => match self.infer.shallow(operand) {
                Ty::Var(_) => Outcome::Wait,
                t @ (Ty::Int | Ty::Float | Ty::Tensor(_) | Ty::Any) => self.settle(out, &t, *span),
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!("cannot negate a `{}`", self.show(&t)),
                )),
            },
            Constraint::Method {
                recv,
                name,
                args,
                lits,
                out,
                span,
            } => self.solve_method(recv, name, args, lits, out, *span),
            Constraint::Field {
                recv,
                name,
                out,
                span,
            } => match self.infer.shallow(recv) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(out, &Ty::Any, *span),
                Ty::Class(cid) => {
                    let class = &self.classes[cid.0 as usize];
                    if let Some(f) = class.fields.iter().find(|f| f.name == *name) {
                        let ty = f.ty.clone();
                        return self.settle(out, &ty, *span);
                    }
                    let msg = if class.methods.iter().any(|(m, _)| m == name) {
                        format!(
                            "`{name}` is a method of `{}`; call it with `()`",
                            class.name
                        )
                    } else {
                        format!("`{}` has no field `{name}`", class.name)
                    };
                    Outcome::Fail(Diagnostic::error(*span, msg))
                }
                t => {
                    let mut diag = Diagnostic::error(
                        *span,
                        format!("`{}` has no field `{name}`", self.show(&t)),
                    );
                    if method_signature(&t, name).is_some() {
                        diag = diag
                            .with_help(format!("`{name}` is a method; call it with `{name}()`"));
                    }
                    Outcome::Fail(diag)
                }
            },
            Constraint::Index {
                target,
                index,
                out,
                literal,
                span,
            } => match self.infer.shallow(target) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(out, &Ty::Any, *span),
                Ty::Tuple(items) => {
                    let n = items.len() as i64;
                    match literal {
                        Some(i) if (-n..n).contains(i) => {
                            let at = if *i < 0 { i + n } else { *i } as usize;
                            self.settle(out, &items[at], *span)
                        }
                        Some(i) => Outcome::Fail(Diagnostic::error(
                            *span,
                            format!("index {i} is out of range for a tuple of {n}"),
                        )),
                        None => Outcome::Fail(
                            Diagnostic::error(
                                *span,
                                "a tuple index must be a number written in the code",
                            )
                            .with_help("write `pair[0]`, or unpack it: `a, b = pair`"),
                        ),
                    }
                }
                Ty::List(e) => {
                    if !self.infer.unify(index, &Ty::Int) {
                        return Outcome::Fail(Diagnostic::error(
                            *span,
                            format!("list index must be an `Int`, found `{}`", self.show(index)),
                        ));
                    }
                    self.settle(out, &e, *span)
                }
                t @ (Ty::Str | Ty::Tensor(_)) => {
                    if !self.infer.unify(index, &Ty::Int) {
                        let msg = format!(
                            "{} index must be an `Int`, found `{}`",
                            if t == Ty::Str { "string" } else { "tensor" },
                            self.show(index)
                        );
                        return Outcome::Fail(Diagnostic::error(*span, msg));
                    }
                    let item = match t {
                        Ty::Tensor(Some(d)) if d.is_empty() => {
                            return Outcome::Fail(Diagnostic::error(
                                *span,
                                "cannot index into a tensor with no dimensions",
                            ));
                        }
                        Ty::Tensor(Some(d)) => Ty::Tensor(Some(d[1..].to_vec())),
                        t => t,
                    };
                    self.settle(out, &item, *span)
                }
                Ty::Map(k, v) => {
                    if !self.infer.unify(&k, index) {
                        return Outcome::Fail(self.mismatch_diag(*span, &k, index));
                    }
                    self.settle(out, &v, *span)
                }
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!("cannot index into a `{}`", self.show(&t)),
                )),
            },
            Constraint::Slice { target, span } => match self.infer.shallow(target) {
                Ty::Var(_) => Outcome::Wait,
                Ty::List(_) | Ty::Str | Ty::Tensor(_) | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!("cannot slice a `{}`", self.show(&t)),
                )),
            },
            Constraint::Iter {
                iterable,
                item,
                span,
            } => match self.infer.shallow(iterable) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(item, &Ty::Any, *span),
                Ty::List(e) => self.settle(item, &e, *span),
                Ty::Str => self.settle(item, &Ty::Str, *span),
                Ty::Tensor(shape) => {
                    let row = shape.map(|d| d.get(1..).unwrap_or_default().to_vec());
                    self.settle(item, &Ty::Tensor(row), *span)
                }
                Ty::Map(k, _) => self.settle(item, &k, *span),
                t => Outcome::Fail(
                    Diagnostic::error(*span, format!("cannot iterate over a `{}`", self.show(&t)))
                        .with_help("loops work over lists, strings, maps and `range(...)`"),
                ),
            },
            Constraint::Len { ty, span } => match self.infer.shallow(ty) {
                Ty::Var(_) => Outcome::Wait,
                Ty::List(_) | Ty::Str | Ty::Map(..) | Ty::Tuple(_) | Ty::Tensor(_) | Ty::Any => {
                    Outcome::Done
                }
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!(
                        "`len` needs a list, string, map or tensor, found `{}`",
                        self.show(&t)
                    ),
                )),
            },
            Constraint::Numeric { ty, span } => match self.infer.shallow(ty) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Int | Ty::Float | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!("expected a number, found `{}`", self.show(&t)),
                )),
            },
            Constraint::Summable { ty, span } => match self.infer.shallow(ty) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Int | Ty::Float | Ty::Tensor(_) | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(
                    *span,
                    format!(
                        "expected numbers or tensors to sum, found `{}`",
                        self.show(&t)
                    ),
                )),
            },
        }
    }

    fn settle(&mut self, out: &Ty, ty: &Ty, span: Span) -> Outcome {
        if self.infer.unify(out, ty) {
            Outcome::Done
        } else {
            Outcome::Fail(self.mismatch_diag(span, out, ty))
        }
    }

    fn solve_binary(&mut self, op: BinOp, lhs: &Ty, rhs: &Ty, out: &Ty, span: Span) -> Outcome {
        let l = self.infer.shallow(lhs);
        let r = self.infer.shallow(rhs);
        let bad = |c: &Self| {
            let msg = format!(
                "cannot apply `{}` to `{}` and `{}`",
                op.symbol(),
                c.show(&l),
                c.show(&r)
            );
            Outcome::Fail(Diagnostic::error(span, msg))
        };
        let is_var = |t: &Ty| matches!(t, Ty::Var(_));
        if l.is_tensor() || r.is_tensor() {
            return self.solve_tensor_binary(op, &l, &r, out, span);
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod | BinOp::FloorDiv | BinOp::Pow => {
                if l == Ty::Any || r == Ty::Any {
                    return self.settle(out, &Ty::Any, span);
                }
                if op == BinOp::Add {
                    match (&l, &r) {
                        (Ty::Str, _) | (_, Ty::Str) => {
                            if self.infer.unify(&l, &Ty::Str) && self.infer.unify(&r, &Ty::Str) {
                                return self.settle(out, &Ty::Str, span);
                            }
                            return bad(self);
                        }
                        (Ty::List(_), _) | (_, Ty::List(_)) => {
                            if self.infer.unify(&l, &r) {
                                return self.settle(out, &l, span);
                            }
                            return bad(self);
                        }
                        _ => {}
                    }
                }
                if op == BinOp::Mul
                    && let (Ty::Str, Ty::Int) | (Ty::Str, Ty::Var(_)) = (&l, &r)
                    && self.infer.unify(&r, &Ty::Int)
                {
                    return self.settle(out, &Ty::Str, span);
                }
                match (&l, &r) {
                    (Ty::Int, Ty::Int) => self.settle(out, &Ty::Int, span),
                    (Ty::Int | Ty::Float, Ty::Int | Ty::Float) => {
                        self.settle(out, &Ty::Float, span)
                    }
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => {
                        Outcome::Wait
                    }
                    _ => bad(self),
                }
            }
            BinOp::Div => {
                if !self.infer.unify(out, &Ty::Float) && self.infer.shallow(out) != Ty::Any {
                    return bad(self);
                }
                match (&l, &r) {
                    (Ty::Any, _) | (_, Ty::Any) => Outcome::Done,
                    (a, b) if a.is_numeric() && b.is_numeric() => Outcome::Done,
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => {
                        Outcome::Wait
                    }
                    _ => bad(self),
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if !self.infer.unify(out, &Ty::Bool) {
                    return bad(self);
                }
                match (&l, &r) {
                    (Ty::Any, _) | (_, Ty::Any) => Outcome::Done,
                    (a, b) if a.is_numeric() && b.is_numeric() => Outcome::Done,
                    (Ty::Str, _) | (_, Ty::Str) => {
                        if self.infer.unify(&l, &Ty::Str) && self.infer.unify(&r, &Ty::Str) {
                            Outcome::Done
                        } else {
                            bad(self)
                        }
                    }
                    (Ty::Tuple(_) | Ty::List(_), _) | (_, Ty::Tuple(_) | Ty::List(_)) => {
                        if self.infer.unify(&l, &r) {
                            Outcome::Done
                        } else {
                            bad(self)
                        }
                    }
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => {
                        Outcome::Wait
                    }
                    _ => bad(self),
                }
            }
            BinOp::Eq | BinOp::Ne => {
                if !self.infer.unify(out, &Ty::Bool) {
                    return bad(self);
                }
                match (&l, &r) {
                    (a, b) if a.is_numeric() && b.is_numeric() => Outcome::Done,
                    (a, b) if (is_var(a) && b.is_numeric()) || (a.is_numeric() && is_var(b)) => {
                        Outcome::Wait
                    }
                    _ => {
                        if self.infer.unify(&l, &r) {
                            Outcome::Done
                        } else {
                            bad(self)
                        }
                    }
                }
            }
            BinOp::In => {
                if !self.infer.unify(out, &Ty::Bool) {
                    return bad(self);
                }
                let ok = match &r {
                    Ty::Var(_) => return Outcome::Wait,
                    Ty::Any => true,
                    Ty::List(e) => self.infer.unify(e, &l),
                    Ty::Map(k, _) => self.infer.unify(k, &l),
                    Ty::Str => self.infer.unify(&l, &Ty::Str),
                    _ => false,
                };
                if ok { Outcome::Done } else { bad(self) }
            }
            BinOp::And | BinOp::Or => {
                if self.infer.unify(&l, &Ty::Bool)
                    && self.infer.unify(&r, &Ty::Bool)
                    && self.infer.unify(out, &Ty::Bool)
                {
                    Outcome::Done
                } else {
                    bad(self)
                }
            }
            BinOp::MatMul => {
                if l == Ty::Any || r == Ty::Any {
                    return self.settle(out, &Ty::Any, span);
                }
                if is_var(&l) && is_var(&r) {
                    return Outcome::Wait;
                }
                bad(self)
            }
        }
    }

    /// Operators with at least one `Tensor` operand: arithmetic broadcasts against
    /// tensors and numbers, `**` takes a number exponent, and `@` needs two tensors.
    fn solve_tensor_binary(&mut self, op: BinOp, l: &Ty, r: &Ty, out: &Ty, span: Span) -> Outcome {
        let tensor_or_number = |t: &Ty| matches!(t, Ty::Tensor(_) | Ty::Int | Ty::Float | Ty::Any);
        let shape_of = |c: &Self, t: &Ty| match c.infer.zonk(t) {
            Ty::Tensor(s) => s,
            _ => Some(Vec::new()),
        };
        let ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let other = if l.is_tensor() { r } else { l };
                if matches!(other, Ty::Var(_)) {
                    return Outcome::Wait;
                }
                tensor_or_number(other)
            }
            BinOp::Pow => l.is_tensor() && (r.is_numeric() || matches!(r, Ty::Var(_) | Ty::Any)),
            BinOp::MatMul => {
                self.infer.unify(l, &Ty::tensor()) && self.infer.unify(r, &Ty::tensor())
            }
            _ => false,
        };
        if !ok {
            let msg = format!(
                "cannot apply `{}` to `{}` and `{}`",
                op.symbol(),
                self.show(l),
                self.show(r)
            );
            let mut diag = Diagnostic::error(span, msg);
            if matches!(
                op,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
            ) {
                diag = diag.with_help("compare tensors element-wise with `a.eq(b)`");
            }
            return Outcome::Fail(diag);
        }
        let result = match (op, shape_of(self, l), shape_of(self, r)) {
            (BinOp::Pow, a, _) => Ok(a),
            (BinOp::MatMul, Some(a), Some(b)) => shape::matmul(&mut self.infer, &a, &b).map(Some),
            (BinOp::MatMul, ..) => Ok(None),
            (_, Some(a), Some(b)) => shape::broadcast(&self.infer, &a, &b).map(Some),
            _ => Ok(None),
        };
        match result {
            Ok(shape) => self.settle(out, &Ty::Tensor(shape), span),
            Err(msg) => Outcome::Fail(Diagnostic::error(span, msg)),
        }
    }

    fn solve_method(
        &mut self,
        recv: &Ty,
        name: &str,
        args: &[Ty],
        lits: &[Lit],
        out: &Ty,
        span: Span,
    ) -> Outcome {
        let recv = self.infer.shallow(recv);
        match &recv {
            Ty::Var(_) => Outcome::Wait,
            Ty::Any => self.settle(out, &Ty::Any, span),
            Ty::Class(cid) if self.classes[cid.0 as usize].interface.is_some() => {
                match self.interface_method(*cid, name) {
                    Some(Ty::Func(params, ret)) => {
                        if args.len() != params.len() {
                            return Outcome::Fail(arity_error(
                                span,
                                name,
                                params.len(),
                                params.len(),
                                args.len(),
                            ));
                        }
                        for (p, a) in params.iter().zip(args) {
                            if !self.unify_arg(p, a) {
                                return Outcome::Fail(self.mismatch_diag(span, p, a));
                            }
                        }
                        self.settle(out, &ret, span)
                    }
                    _ => Outcome::Fail(Diagnostic::error(
                        span,
                        format!(
                            "`{}` has no method `{name}`",
                            self.classes[cid.0 as usize].name
                        ),
                    )),
                }
            }
            Ty::Class(cid) => {
                let class = &self.classes[cid.0 as usize];
                if let Some((_, fid)) = class.methods.iter().find(|(m, _)| m == name) {
                    let fid = *fid;
                    let scheme = self.func_scheme(fid);
                    let (fty, _) = self.instantiate(&scheme);
                    let Ty::Func(params, ret) = fty else {
                        unreachable!("methods have function types")
                    };
                    let required = 1 + self.funcs[fid.0 as usize]
                        .params
                        .iter()
                        .filter(|p| p.default.is_none())
                        .count();
                    if args.len() + 1 < required || args.len() + 1 > params.len() {
                        return Outcome::Fail(arity_error(
                            span,
                            name,
                            required - 1,
                            params.len() - 1,
                            args.len(),
                        ));
                    }
                    self.infer.unify(&params[0], &recv);
                    for (p, a) in params[1..].iter().zip(args) {
                        if !self.unify_arg(p, a) {
                            return Outcome::Fail(self.mismatch_diag(span, p, a));
                        }
                    }
                    return self.settle(out, &ret, span);
                }
                if let Some(f) = class.fields.iter().find(|f| f.name == name) {
                    let fty = f.ty.clone();
                    let want = Ty::func(args.to_vec(), out.clone());
                    return if self.infer.unify(&fty, &want) {
                        Outcome::Done
                    } else {
                        Outcome::Fail(self.mismatch_diag(span, &fty, &want))
                    };
                }
                Outcome::Fail(Diagnostic::error(
                    span,
                    format!("`{}` has no method `{name}`", class.name),
                ))
            }
            Ty::Str | Ty::List(_) | Ty::Map(..) | Ty::Tensor(_) | Ty::Optimizer | Ty::Tokenizer => {
                let Some((params, required, ret)) = method_signature(&recv, name) else {
                    return Outcome::Fail(Diagnostic::error(
                        span,
                        format!("`{}` has no method `{name}`", self.show(&recv)),
                    ));
                };
                if args.len() < required || args.len() > params.len() {
                    return Outcome::Fail(arity_error(
                        span,
                        name,
                        required,
                        params.len(),
                        args.len(),
                    ));
                }
                let stores = matches!(recv, Ty::List(_) | Ty::Map(..));
                for (p, a) in params.iter().zip(args) {
                    // Collections hold tensors of any shape.
                    let a = if stores { self.unshaped(a) } else { a.clone() };
                    if !self.unify_arg(p, &a) {
                        return Outcome::Fail(self.mismatch_diag(span, p, &a));
                    }
                }
                let ret = match (&recv, &ret) {
                    (Ty::Tensor(_), Ty::Tensor(_)) => {
                        match self.tensor_method_shape(&recv, name, args, lits) {
                            Ok(shape) => Ty::Tensor(shape),
                            Err(msg) => return Outcome::Fail(Diagnostic::error(span, msg)),
                        }
                    }
                    _ => ret,
                };
                self.settle(out, &ret, span)
            }
            t => Outcome::Fail(Diagnostic::error(
                span,
                format!("`{}` has no method `{name}`", self.show(t)),
            )),
        }
    }
}

impl Checker {
    /// The type of an interface's method, fresh for one call.
    pub(super) fn interface_method(&mut self, iid: super::ClassId, name: &str) -> Option<Ty> {
        let sig = self.classes[iid.0 as usize]
            .interface
            .as_ref()?
            .iter()
            .find(|(n, ..)| n == name)
            .map(|(_, t, _)| t.clone())?;
        Some(self.fresh_signature(&sig))
    }

    /// A type with tensor shapes forgotten, unless it is not yet known.
    pub(super) fn unshaped(&self, ty: &Ty) -> Ty {
        match self.infer.shallow(ty) {
            Ty::Var(_) => ty.clone(),
            _ => self.infer.zonk(ty).unshaped(),
        }
    }

    fn tensor_method_shape(
        &mut self,
        recv: &Ty,
        name: &str,
        args: &[Ty],
        lits: &[Lit],
    ) -> Result<Option<shape::Dims>, String> {
        let Ty::Tensor(Some(d)) = self.infer.zonk(recv) else {
            return Ok(None);
        };
        let arg = |c: &Self, i: usize| match args.get(i).map(|a| c.infer.zonk(a)) {
            Some(Ty::Tensor(s)) => s,
            _ => None,
        };
        match name {
            "matmul" => match arg(self, 0) {
                Some(b) => shape::matmul(&mut self.infer, &d, &b).map(Some),
                None => Ok(None),
            },
            "eq" => match arg(self, 0) {
                Some(b) => shape::broadcast(&self.infer, &d, &b).map(Some),
                None => Ok(None),
            },
            "embed" if d.len() == 2 => {
                Ok(arg(self, 0).map(|ids| ids.into_iter().chain([d[1].clone()]).collect()))
            }
            _ => shape::method(&self.infer, name, &d, lits),
        }
    }
}

pub fn arity_error(
    span: Span,
    name: &str,
    required: usize,
    max: usize,
    given: usize,
) -> Diagnostic {
    let expected = if required == max {
        format!("{required}")
    } else {
        format!("{required} to {max}")
    };
    let plural = if max == 1 { "" } else { "s" };
    Diagnostic::error(
        span,
        format!("`{name}` takes {expected} argument{plural} but {given} were given"),
    )
}
