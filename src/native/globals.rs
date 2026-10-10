//! Finds module-level variables that only their own module's top-level code
//! uses. Compiled module code keeps those in registers and frame slots instead
//! of the shared global table.

use crate::check::hir::{self, ExprKind, FuncId, GlobalId, Place, Program, StmtKind};
use std::collections::{HashMap, HashSet};

/// For each module's init function, the globals no other function touches.
pub fn private_globals(prog: &Program) -> HashMap<FuncId, HashSet<GlobalId>> {
    let inits: HashSet<FuncId> = prog.modules.iter().map(|m| m.init).collect();
    let mut used_elsewhere = HashSet::new();
    let mut by_init: HashMap<FuncId, HashSet<GlobalId>> = HashMap::new();
    for (i, f) in prog.funcs.iter().enumerate() {
        let id = FuncId(i as u32);
        let mut used = HashSet::new();
        block(&f.body, &mut used);
        for d in f.defaults.iter().flatten() {
            expr(d, &mut used);
        }
        if inits.contains(&id) {
            by_init.insert(id, used);
        } else {
            used_elsewhere.extend(used);
        }
    }
    let mut seen_in_inits: HashMap<GlobalId, usize> = HashMap::new();
    for used in by_init.values() {
        for g in used {
            *seen_in_inits.entry(*g).or_default() += 1;
        }
    }
    for used in by_init.values_mut() {
        used.retain(|g| !used_elsewhere.contains(g) && seen_in_inits[g] == 1);
    }
    by_init
}

fn block(stmts: &hir::Block, out: &mut HashSet<GlobalId>) {
    for s in stmts {
        stmt(s, out);
    }
}

fn place(p: &Place, out: &mut HashSet<GlobalId>) {
    match p {
        Place::Global(g) => {
            out.insert(*g);
        }
        Place::Local(_) => {}
        Place::Field { target, .. } => expr(target, out),
        Place::Index { target, index } => {
            expr(target, out);
            expr(index, out);
        }
    }
}

fn stmt(s: &hir::Stmt, out: &mut HashSet<GlobalId>) {
    match &s.kind {
        StmtKind::Expr(e) => expr(e, out),
        StmtKind::Assign {
            place: p, value, ..
        } => {
            place(p, out);
            expr(value, out);
        }
        StmtKind::Show(es) => es.iter().for_each(|e| expr(e, out)),
        StmtKind::If {
            branches,
            otherwise,
        } => {
            for (c, b) in branches {
                expr(c, out);
                block(b, out);
            }
            block(otherwise, out);
        }
        StmtKind::While { cond, body } => {
            expr(cond, out);
            block(body, out);
        }
        StmtKind::For { var, iter, body } => {
            place(var, out);
            expr(iter, out);
            block(body, out);
        }
        StmtKind::Repeat { count, body, .. } => {
            expr(count, out);
            block(body, out);
        }
        StmtKind::Return(Some(e)) => expr(e, out),
        StmtKind::Try { body, var, handler } => {
            block(body, out);
            place(var, out);
            block(handler, out);
        }
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue | StmtKind::InitModule(_) => {
        }
    }
}

fn expr(e: &hir::Expr, out: &mut HashSet<GlobalId>) {
    match &e.kind {
        ExprKind::Global(g) => {
            out.insert(*g);
        }
        ExprKind::FStr(parts) => {
            for p in parts {
                if let hir::FStrPart::Expr { expr: inner, .. } = p {
                    expr(inner, out);
                }
            }
        }
        ExprKind::List(es) | ExprKind::Tuple(es) | ExprKind::RunParallel(es) => {
            es.iter().for_each(|e| expr(e, out))
        }
        ExprKind::Map(entries) => {
            for (k, v) in entries {
                expr(k, out);
                expr(v, out);
            }
        }
        ExprKind::Closure { captures: args, .. }
        | ExprKind::CallFunc { args, .. }
        | ExprKind::CallBuiltin { args, .. }
        | ExprKind::New { args, .. } => args.iter().for_each(|e| expr(e, out)),
        ExprKind::Unary { operand: inner, .. }
        | ExprKind::Field { target: inner, .. }
        | ExprKind::Together(inner)
        | ExprKind::Await(inner)
        | ExprKind::IntToFloat(inner) => expr(inner, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr(lhs, out);
            expr(rhs, out);
        }
        ExprKind::IfElse {
            cond,
            then,
            otherwise,
        } => {
            expr(cond, out);
            expr(then, out);
            expr(otherwise, out);
        }
        ExprKind::Call { callee, args } => {
            expr(callee, out);
            args.iter().for_each(|e| expr(e, out));
        }
        ExprKind::MethodCall { recv, args, .. } => {
            expr(recv, out);
            args.iter().for_each(|e| expr(e, out));
        }
        ExprKind::Index { target, index } => {
            expr(target, out);
            expr(index, out);
        }
        ExprKind::Slice { target, start, end } => {
            expr(target, out);
            if let Some(s) = start {
                expr(s, out);
            }
            if let Some(x) = end {
                expr(x, out);
            }
        }
        ExprKind::Query { target, body, .. } => {
            expr(target, out);
            if let Some(b) = body {
                expr(b, out);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Local(_)
        | ExprKind::Func { .. }
        | ExprKind::Recurse
        | ExprKind::PyImport(_) => {}
    }
}
