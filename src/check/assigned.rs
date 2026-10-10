//! Definite assignment: a variable may only be read where every path to the
//! read has assigned it. Parameters and captures are assigned on entry; the
//! module's own globals are tracked through its top-level code.

use super::builtins::Builtin;
use super::hir::{self, ExprKind, GlobalId, LocalId, Place, StmtKind};
use crate::diagnostic::Diagnostic;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Var {
    Local(u32),
    Global(u32),
}

/// The variables assigned on every path so far; `None` where no path reaches.
type State = Option<HashSet<Var>>;

fn meet(a: State, b: State) -> State {
    match (a, b) {
        (None, s) | (s, None) => s,
        (Some(a), Some(b)) => Some(a.intersection(&b).copied().collect()),
    }
}

struct Pass<'a> {
    func: &'a hir::Func,
    globals: &'a [hir::Global],
    tracked_globals: &'a HashSet<GlobalId>,
    /// States at each `break` of the loops being walked, innermost last.
    breaks: Vec<State>,
    reported: HashSet<Var>,
    diags: Vec<Diagnostic>,
}

/// Reports reads of `func`'s locals, and of `tracked_globals`, that may happen before an assignment.
pub fn check(
    func: &hir::Func,
    globals: &[hir::Global],
    tracked_globals: &HashSet<GlobalId>,
) -> Vec<Diagnostic> {
    let mut entry: HashSet<Var> = (0..func.params as u32).map(Var::Local).collect();
    entry.extend(func.captures.iter().map(|l| Var::Local(l.0)));
    let mut pass = Pass {
        func,
        globals,
        tracked_globals,
        breaks: Vec::new(),
        reported: HashSet::new(),
        diags: Vec::new(),
    };
    pass.block(&func.body, Some(entry));
    pass.diags
}

impl Pass<'_> {
    fn block(&mut self, block: &hir::Block, mut state: State) -> State {
        for s in block {
            state = self.stmt(s, state);
        }
        state
    }

    fn assign(place: &Place, state: &mut State) {
        if let Some(s) = state {
            match place {
                Place::Local(l) => {
                    s.insert(Var::Local(l.0));
                }
                Place::Global(g) => {
                    s.insert(Var::Global(g.0));
                }
                _ => {}
            }
        }
    }

    fn with_local(local: LocalId, state: &State) -> State {
        state.clone().map(|mut s| {
            s.insert(Var::Local(local.0));
            s
        })
    }

    fn stmt(&mut self, stmt: &hir::Stmt, state: State) -> State {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.expr(e, &state);
                let fails = matches!(
                    &e.kind,
                    ExprKind::CallBuiltin {
                        builtin: Builtin::Fail,
                        ..
                    }
                );
                if fails { None } else { state }
            }
            StmtKind::Assign { place, op, value } => {
                self.expr(value, &state);
                match place {
                    Place::Local(l) if op.is_some() => {
                        self.read(Var::Local(l.0), stmt.span, &state)
                    }
                    Place::Global(g) if op.is_some() => {
                        self.read(Var::Global(g.0), stmt.span, &state)
                    }
                    Place::Field { target, .. } => self.expr(target, &state),
                    Place::Index { target, index } => {
                        self.expr(target, &state);
                        self.expr(index, &state);
                    }
                    _ => {}
                }
                let mut state = state;
                Self::assign(place, &mut state);
                state
            }
            StmtKind::Show(es) => {
                es.iter().for_each(|e| self.expr(e, &state));
                state
            }
            StmtKind::If {
                branches,
                otherwise,
            } => {
                let mut out = None;
                for (cond, body) in branches {
                    self.expr(cond, &state);
                    out = meet(out, self.block(body, state.clone()));
                }
                meet(out, self.block(otherwise, state))
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, &state);
                let forever = matches!(cond.kind, ExprKind::Bool(true));
                let breaks = self.in_loop(body, state.clone());
                if forever { breaks } else { state }
            }
            StmtKind::For { var, iter, body } => {
                self.expr(iter, &state);
                let mut inner = state.clone();
                Self::assign(var, &mut inner);
                self.in_loop(body, inner);
                state
            }
            StmtKind::Repeat { count, it, body } => {
                self.expr(count, &state);
                self.in_loop(body, Self::with_local(*it, &state));
                state
            }
            StmtKind::Return(value) => {
                if let Some(v) = value {
                    self.expr(v, &state);
                }
                None
            }
            StmtKind::Break => {
                if let Some(b) = self.breaks.last_mut() {
                    *b = meet(b.take(), state);
                }
                None
            }
            StmtKind::Continue => None,
            StmtKind::Try { body, catches } => {
                let mut after = self.block(body, state.clone());
                for c in catches {
                    let mut caught = state.clone();
                    Self::assign(&c.var, &mut caught);
                    after = meet(after, self.block(&c.body, caught));
                }
                after
            }
            StmtKind::InitModule(_) => state,
        }
    }

    /// Walks a loop body and returns the meet of the states at its `break`s.
    fn in_loop(&mut self, body: &hir::Block, state: State) -> State {
        self.breaks.push(None);
        self.block(body, state);
        self.breaks.pop().flatten()
    }

    fn read(&mut self, var: Var, span: crate::source::Span, state: &State) {
        let tracked = match var {
            Var::Local(_) => true,
            Var::Global(g) => self.tracked_globals.contains(&GlobalId(g)),
        };
        let Some(assigned) = state else { return };
        if !tracked || assigned.contains(&var) || !self.reported.insert(var) {
            return;
        }
        let name = match var {
            Var::Local(l) => &self.func.locals[l as usize].name,
            Var::Global(g) => &self.globals[g as usize].name,
        };
        self.diags.push(
            Diagnostic::error(span, format!("`{name}` may be used before it is assigned"))
                .with_help("assign it on every path that leads here, for example before the `if`, loop or `try`"),
        );
    }

    fn exprs(&mut self, es: &[hir::Expr], state: &State) {
        es.iter().for_each(|e| self.expr(e, state));
    }

    fn expr(&mut self, e: &hir::Expr, state: &State) {
        match &e.kind {
            ExprKind::Local(l) => self.read(Var::Local(l.0), e.span, state),
            ExprKind::Global(g) => self.read(Var::Global(g.0), e.span, state),
            ExprKind::FStr(parts) => {
                for p in parts {
                    if let hir::FStrPart::Expr { expr, .. } = p {
                        self.expr(expr, state);
                    }
                }
            }
            ExprKind::List(es) | ExprKind::Tuple(es) | ExprKind::RunParallel(es) => {
                self.exprs(es, state)
            }
            ExprKind::Closure { captures: es, .. }
            | ExprKind::CallFunc { args: es, .. }
            | ExprKind::CallBuiltin { args: es, .. }
            | ExprKind::New { args: es, .. } => self.exprs(es, state),
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    self.expr(k, state);
                    self.expr(v, state);
                }
            }
            ExprKind::Unary { operand: x, .. }
            | ExprKind::Field { target: x, .. }
            | ExprKind::Together(x)
            | ExprKind::Await(x)
            | ExprKind::IntToFloat(x)
            | ExprKind::IsVariant { value: x, .. } => self.expr(x, state),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs, state);
                self.expr(rhs, state);
            }
            ExprKind::IfElse {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond, state);
                self.expr(then, state);
                self.expr(otherwise, state);
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee, state);
                self.exprs(args, state);
            }
            ExprKind::MethodCall { recv, args, .. } => {
                self.expr(recv, state);
                self.exprs(args, state);
            }
            ExprKind::Index { target, index } => {
                self.expr(target, state);
                self.expr(index, state);
            }
            ExprKind::Slice { target, start, end } => {
                self.expr(target, state);
                start.iter().chain(end).for_each(|x| self.expr(x, state));
            }
            ExprKind::Query {
                target, item, body, ..
            } => {
                self.expr(target, state);
                if let Some(b) = body {
                    self.expr(b, &Self::with_local(*item, state));
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Func { .. }
            | ExprKind::Recurse
            | ExprKind::PyImport(_) => {}
        }
    }
}
