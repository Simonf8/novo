//! Deferred type constraints: operations whose meaning depends on a type that
//! may not be known yet. Constraints still open when a function is generalized
//! travel with its scheme and are re-checked at every use.

use super::builtins::method_signature;
use super::types::{Infer, Ty};
use super::Checker;
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::syntax::ast::BinOp;

#[derive(Debug, Clone)]
pub enum Constraint {
    Binary { op: BinOp, lhs: Ty, rhs: Ty, out: Ty, span: Span },
    Neg { operand: Ty, out: Ty, span: Span },
    Method { recv: Ty, name: String, args: Vec<Ty>, out: Ty, span: Span },
    Field { recv: Ty, name: String, out: Ty, span: Span },
    Index { target: Ty, index: Ty, out: Ty, span: Span },
    Slice { target: Ty, span: Span },
    Iter { iterable: Ty, item: Ty, span: Span },
    Len { ty: Ty, span: Span },
    Numeric { ty: Ty, span: Span },
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
            Constraint::Method { recv, args, out, .. } => std::iter::once(recv).chain(args).chain(std::iter::once(out)).collect(),
            Constraint::Field { recv, out, .. } => vec![recv, out],
            Constraint::Index { target, index, out, .. } => vec![target, index, out],
            Constraint::Slice { target, .. } => vec![target],
            Constraint::Iter { iterable, item, .. } => vec![iterable, item],
            Constraint::Len { ty, .. } | Constraint::Numeric { ty, .. } => vec![ty],
        }
    }

    pub fn substitute(&self, infer: &Infer, map: &[(u32, Ty)]) -> Constraint {
        let s = |t: &Ty| infer.substitute(t, map);
        match self {
            Constraint::Binary { op, lhs, rhs, out, span } => Constraint::Binary { op: *op, lhs: s(lhs), rhs: s(rhs), out: s(out), span: *span },
            Constraint::Neg { operand, out, span } => Constraint::Neg { operand: s(operand), out: s(out), span: *span },
            Constraint::Method { recv, name, args, out, span } => {
                Constraint::Method { recv: s(recv), name: name.clone(), args: args.iter().map(s).collect(), out: s(out), span: *span }
            }
            Constraint::Field { recv, name, out, span } => Constraint::Field { recv: s(recv), name: name.clone(), out: s(out), span: *span },
            Constraint::Index { target, index, out, span } => Constraint::Index { target: s(target), index: s(index), out: s(out), span: *span },
            Constraint::Slice { target, span } => Constraint::Slice { target: s(target), span: *span },
            Constraint::Iter { iterable, item, span } => Constraint::Iter { iterable: s(iterable), item: s(item), span: *span },
            Constraint::Len { ty, span } => Constraint::Len { ty: s(ty), span: *span },
            Constraint::Numeric { ty, span } => Constraint::Numeric { ty: s(ty), span: *span },
        }
    }
}

impl<'s> Checker<'s> {
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
        self.infer.unify(param, arg)
    }

    fn mismatch_diag(&self, span: Span, expected: &Ty, found: &Ty) -> Diagnostic {
        let msg = format!("expected `{}`, found `{}`", self.show(expected), self.show(found));
        Diagnostic::error(span, "mismatched types").with_label(msg)
    }

    fn solve(&mut self, c: &Constraint) -> Outcome {
        match c {
            Constraint::Binary { op, lhs, rhs, out, span } => self.solve_binary(*op, lhs, rhs, out, *span),
            Constraint::Neg { operand, out, span } => match self.infer.shallow(operand) {
                Ty::Var(_) => Outcome::Wait,
                t @ (Ty::Int | Ty::Float | Ty::Any) => self.settle(out, &t, *span),
                t => Outcome::Fail(Diagnostic::error(*span, format!("cannot negate a `{}`", self.show(&t)))),
            },
            Constraint::Method { recv, name, args, out, span } => self.solve_method(recv, name, args, out, *span),
            Constraint::Field { recv, name, out, span } => match self.infer.shallow(recv) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(out, &Ty::Any, *span),
                Ty::Class(cid) => {
                    let class = &self.classes[cid.0 as usize];
                    if let Some(f) = class.fields.iter().find(|f| f.name == *name) {
                        let ty = f.ty.clone();
                        return self.settle(out, &ty, *span);
                    }
                    let msg = if class.methods.iter().any(|(m, _)| m == name) {
                        format!("`{name}` is a method of `{}`; call it with `()`", class.name)
                    } else {
                        format!("`{}` has no field `{name}`", class.name)
                    };
                    Outcome::Fail(Diagnostic::error(*span, msg))
                }
                t => Outcome::Fail(Diagnostic::error(*span, format!("`{}` has no field `{name}`", self.show(&t)))),
            },
            Constraint::Index { target, index, out, span } => match self.infer.shallow(target) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(out, &Ty::Any, *span),
                Ty::List(e) => {
                    if !self.infer.unify(index, &Ty::Int) {
                        return Outcome::Fail(Diagnostic::error(*span, format!("list index must be an `Int`, found `{}`", self.show(index))));
                    }
                    self.settle(out, &e, *span)
                }
                Ty::Str => {
                    if !self.infer.unify(index, &Ty::Int) {
                        return Outcome::Fail(Diagnostic::error(*span, format!("string index must be an `Int`, found `{}`", self.show(index))));
                    }
                    self.settle(out, &Ty::Str, *span)
                }
                Ty::Map(k, v) => {
                    if !self.infer.unify(&k, index) {
                        return Outcome::Fail(self.mismatch_diag(*span, &k, index));
                    }
                    self.settle(out, &v, *span)
                }
                t => Outcome::Fail(Diagnostic::error(*span, format!("cannot index into a `{}`", self.show(&t)))),
            },
            Constraint::Slice { target, span } => match self.infer.shallow(target) {
                Ty::Var(_) => Outcome::Wait,
                Ty::List(_) | Ty::Str | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(*span, format!("cannot slice a `{}`", self.show(&t)))),
            },
            Constraint::Iter { iterable, item, span } => match self.infer.shallow(iterable) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Any => self.settle(item, &Ty::Any, *span),
                Ty::List(e) => self.settle(item, &e, *span),
                Ty::Str => self.settle(item, &Ty::Str, *span),
                Ty::Map(k, _) => self.settle(item, &k, *span),
                t => Outcome::Fail(
                    Diagnostic::error(*span, format!("cannot iterate over a `{}`", self.show(&t))).with_help("loops work over lists, strings, maps and `range(...)`"),
                ),
            },
            Constraint::Len { ty, span } => match self.infer.shallow(ty) {
                Ty::Var(_) => Outcome::Wait,
                Ty::List(_) | Ty::Str | Ty::Map(..) | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(*span, format!("`len` needs a list, string or map, found `{}`", self.show(&t)))),
            },
            Constraint::Numeric { ty, span } => match self.infer.shallow(ty) {
                Ty::Var(_) => Outcome::Wait,
                Ty::Int | Ty::Float | Ty::Any => Outcome::Done,
                t => Outcome::Fail(Diagnostic::error(*span, format!("expected a number, found `{}`", self.show(&t)))),
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
            let msg = format!("cannot apply `{}` to `{}` and `{}`", op.symbol(), c.show(&l), c.show(&r));
            Outcome::Fail(Diagnostic::error(span, msg))
        };
        let is_var = |t: &Ty| matches!(t, Ty::Var(_));
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
                if op == BinOp::Mul {
                    if let (Ty::Str, Ty::Int) | (Ty::Str, Ty::Var(_)) = (&l, &r) {
                        if self.infer.unify(&r, &Ty::Int) {
                            return self.settle(out, &Ty::Str, span);
                        }
                    }
                }
                match (&l, &r) {
                    (Ty::Int, Ty::Int) => self.settle(out, &Ty::Int, span),
                    (Ty::Int | Ty::Float, Ty::Int | Ty::Float) => self.settle(out, &Ty::Float, span),
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => Outcome::Wait,
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
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => Outcome::Wait,
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
                    (a, b) if (is_var(a) || a.is_numeric()) && (is_var(b) || b.is_numeric()) => Outcome::Wait,
                    _ => bad(self),
                }
            }
            BinOp::Eq | BinOp::Ne => {
                if !self.infer.unify(out, &Ty::Bool) {
                    return bad(self);
                }
                match (&l, &r) {
                    (a, b) if a.is_numeric() && b.is_numeric() => Outcome::Done,
                    (a, b) if (is_var(a) && b.is_numeric()) || (a.is_numeric() && is_var(b)) => Outcome::Wait,
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
                if self.infer.unify(&l, &Ty::Bool) && self.infer.unify(&r, &Ty::Bool) && self.infer.unify(out, &Ty::Bool) {
                    Outcome::Done
                } else {
                    bad(self)
                }
            }
            BinOp::MatMul => {
                if l == Ty::Any || r == Ty::Any {
                    return self.settle(out, &Ty::Any, span);
                }
                Outcome::Fail(Diagnostic::error(span, "`@` is reserved for tensors, which are not available yet"))
            }
        }
    }

    fn solve_method(&mut self, recv: &Ty, name: &str, args: &[Ty], out: &Ty, span: Span) -> Outcome {
        let recv = self.infer.shallow(recv);
        match &recv {
            Ty::Var(_) => Outcome::Wait,
            Ty::Any => self.settle(out, &Ty::Any, span),
            Ty::Class(cid) => {
                let class = &self.classes[cid.0 as usize];
                if let Some((_, fid)) = class.methods.iter().find(|(m, _)| m == name) {
                    let fid = *fid;
                    let scheme = self.func_scheme(fid);
                    let fty = self.instantiate(&scheme);
                    let Ty::Func(params, ret) = fty else { unreachable!("methods have function types") };
                    let required = 1 + self.funcs[fid.0 as usize].params.iter().filter(|p| p.default.is_none()).count();
                    if args.len() + 1 < required || args.len() + 1 > params.len() {
                        return Outcome::Fail(arity_error(span, name, required - 1, params.len() - 1, args.len()));
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
                    return if self.infer.unify(&fty, &want) { Outcome::Done } else { Outcome::Fail(self.mismatch_diag(span, &fty, &want)) };
                }
                Outcome::Fail(Diagnostic::error(span, format!("`{}` has no method `{name}`", class.name)))
            }
            Ty::Str | Ty::List(_) | Ty::Map(..) => {
                let Some((params, required, ret)) = method_signature(&recv, name) else {
                    return Outcome::Fail(Diagnostic::error(span, format!("`{}` has no method `{name}`", self.show(&recv))));
                };
                if args.len() < required || args.len() > params.len() {
                    return Outcome::Fail(arity_error(span, name, required, params.len(), args.len()));
                }
                for (p, a) in params.iter().zip(args) {
                    if !self.unify_arg(p, a) {
                        return Outcome::Fail(self.mismatch_diag(span, p, a));
                    }
                }
                self.settle(out, &ret, span)
            }
            t => Outcome::Fail(Diagnostic::error(span, format!("`{}` has no method `{name}`", self.show(t)))),
        }
    }
}

pub fn arity_error(span: Span, name: &str, required: usize, max: usize, given: usize) -> Diagnostic {
    let expected = if required == max { format!("{required}") } else { format!("{required} to {max}") };
    let plural = if max == 1 { "" } else { "s" };
    Diagnostic::error(span, format!("`{name}` takes {expected} argument{plural} but {given} were given"))
}

