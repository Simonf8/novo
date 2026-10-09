//! Lowers one function instance from typed HIR to Cranelift IR.

use super::runtime::{code, MAX_DEPTH, STATUS_OFFSET};
use super::runtime::math;
use super::{Batch, Compiler, Key, Scalar};
use crate::check::builtins::Builtin;
use crate::check::hir::{self, ExprKind, Place, Program, StmtKind};
use crate::check::types::Ty;
use crate::source::Span;
use crate::syntax::ast::{BinOp, UnaryOp};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{self, types, Block, InstBuilder, MemFlagsData};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId as ClifFunc, Module};
use std::collections::HashMap;

type R<T> = Result<T, String>;

pub fn substitute(ty: &Ty, subst: &[(u32, Ty)]) -> Ty {
    match ty {
        Ty::Var(v) => subst.iter().find(|(k, _)| k == v).map(|(_, t)| t.clone()).unwrap_or(Ty::Var(*v)),
        Ty::List(t) => Ty::list(substitute(t, subst)),
        Ty::Task(t) => Ty::task(substitute(t, subst)),
        Ty::Map(k, v) => Ty::map(substitute(k, subst), substitute(v, subst)),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| substitute(t, subst)).collect()),
        Ty::Func(ps, r) => Ty::func(ps.iter().map(|p| substitute(p, subst)).collect(), substitute(r, subst)),
        other => other.clone(),
    }
}

static SPACE: &str = " ";

struct Lower<'a, 'b> {
    c: &'a mut Compiler,
    prog: &'a Program,
    batch: &'a mut Batch,
    subst: Vec<(u32, Ty)>,
    b: FunctionBuilder<'b>,
    ctx: ir::Value,
    /// This call's recursion depth, passed on to callees.
    depth: ir::Value,
    locals: Vec<(Variable, Scalar)>,
    ret: Scalar,
    ret_var: Variable,
    exit: Block,
    fail: Block,
    loops: Vec<(Block, Block)>,
    refs: HashMap<ClifFunc, ir::FuncRef>,
}

pub(super) fn lower(c: &mut Compiler, prog: &Program, batch: &mut Batch, key: &Key, out: &mut ir::Function, fctx: &mut FunctionBuilderContext) -> R<()> {
    let func = &prog.funcs[key.0 .0 as usize];
    if !func.captures.is_empty() {
        return Err("closures are interpreted".into());
    }
    let subst: Vec<(u32, Ty)> = func.generics.iter().copied().zip(key.1.iter().cloned()).collect();
    let (_, params, ret) = batch.pending[key].clone();
    let mut b = FunctionBuilder::new(out, fctx);

    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    b.seal_block(entry);
    let ctx = b.block_params(entry)[0];
    let caller_depth = b.block_params(entry)[1];
    let args: Vec<ir::Value> = b.block_params(entry)[2..].to_vec();
    let depth = b.ins().iadd_imm_s(caller_depth, 1);

    let mut locals = Vec::with_capacity(func.locals.len());
    for (i, local) in func.locals.iter().enumerate() {
        let ty = substitute(&local.ty, &subst);
        let scalar = Scalar::of(&ty).ok_or_else(|| format!("local `{}` holds a {}", local.name, ty.display(&[])))?;
        let var = b.declare_var(scalar.clif());
        let init = if i < params.len() { args[i] } else { zero(&mut b, scalar) };
        b.def_var(var, init);
        locals.push((var, scalar));
    }
    let ret_var = b.declare_var(ret.clif());
    let init = zero(&mut b, ret);
    b.def_var(ret_var, init);

    let exit = b.create_block();
    let fail = b.create_block();
    let mut l = Lower { c, prog, batch, subst, b, ctx, depth, locals, ret, ret_var, exit, fail, loops: Vec::new(), refs: HashMap::new() };

    let too_deep = l.b.ins().icmp_imm_s(IntCC::UnsignedGreaterThan, depth, MAX_DEPTH as i64);
    l.fail_if(too_deep, code::DEPTH, func_span(func))?;

    l.block(&func.body)?;
    if !l.terminated() {
        l.b.ins().jump(l.exit, &[]);
    }

    for (block, value) in [(l.exit, None), (l.fail, Some(()))] {
        l.b.switch_to_block(block);
        let result = match value {
            None => l.b.use_var(l.ret_var),
            Some(()) => zero(&mut l.b, l.ret),
        };
        l.b.ins().return_(&[result]);
    }
    l.b.seal_all_blocks();
    let config = l.c.module.isa().frontend_config();
    l.b.finalize(config);
    Ok(())
}

fn func_span(func: &hir::Func) -> Span {
    func.body.first().map(|s| s.span).unwrap_or_default()
}

fn zero(b: &mut FunctionBuilder, s: Scalar) -> ir::Value {
    match s {
        Scalar::Int | Scalar::None => b.ins().iconst(types::I64, 0),
        Scalar::Float => b.ins().f64const(0.0),
        Scalar::Bool => b.ins().iconst(types::I8, 0),
    }
}

impl Lower<'_, '_> {
    fn terminated(&self) -> bool {
        let block = self.b.current_block().expect("positioned in a block");
        self.b.func.layout.last_inst(block).is_some_and(|i| self.b.func.dfg.insts[i].opcode().is_terminator())
    }

    /// Starts a fresh block after a terminator so later statements have somewhere to go.
    fn after_jump(&mut self) {
        let block = self.b.create_block();
        self.b.switch_to_block(block);
    }

    fn helper(&mut self, id: ClifFunc) -> ir::FuncRef {
        if let Some(r) = self.refs.get(&id) {
            return *r;
        }
        let r = self.c.module.declare_func_in_func(id, self.b.func);
        self.refs.insert(id, r);
        r
    }

    fn call(&mut self, id: ClifFunc, args: &[ir::Value]) -> Option<ir::Value> {
        let r = self.helper(id);
        let inst = self.b.ins().call(r, args);
        self.b.inst_results(inst).first().copied()
    }

    fn span_args(&mut self, span: Span) -> [ir::Value; 3] {
        [
            self.b.ins().iconst(types::I32, i64::from(span.file.0)),
            self.b.ins().iconst(types::I32, i64::from(span.lo)),
            self.b.ins().iconst(types::I32, i64::from(span.hi)),
        ]
    }

    /// Raises error `code` when `cond` is true.
    fn fail_if(&mut self, cond: ir::Value, code: u32, span: Span) -> R<()> {
        let raise = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(cond, raise, &[], ok, &[]);
        self.b.switch_to_block(raise);
        let code = self.b.ins().iconst(types::I32, i64::from(code));
        let [file, lo, hi] = self.span_args(span);
        let fail = self.c.helpers.fail;
        self.call(fail, &[self.ctx, code, file, lo, hi]);
        self.b.ins().jump(self.fail, &[]);
        self.b.switch_to_block(ok);
        Ok(())
    }

    /// Propagates an error raised by a call.
    fn check_status(&mut self) {
        let status = self.b.ins().load(types::I8, MemFlagsData::trusted(), self.ctx, STATUS_OFFSET);
        let ok = self.b.create_block();
        self.b.ins().brif(status, self.fail, &[], ok, &[]);
        self.b.switch_to_block(ok);
    }

    // ---- statements ----------------------------------------------------------------------

    fn block(&mut self, block: &hir::Block) -> R<()> {
        for stmt in block {
            self.stmt(stmt)?;
        }
        Ok(())
    }

    fn local(&self, place: &Place) -> R<(Variable, Scalar)> {
        match place {
            Place::Local(l) => Ok(self.locals[l.0 as usize]),
            Place::Global(_) => Err("assigns a global variable".into()),
            _ => Err("assigns into a field or element".into()),
        }
    }

    fn stmt(&mut self, stmt: &hir::Stmt) -> R<()> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.expr(e)?;
            }
            StmtKind::Assign { place, op, value } => {
                let (var, scalar) = self.local(place)?;
                let (v, s) = self.expr(value)?;
                let v = match op {
                    None => v,
                    Some(op) => {
                        let cur = self.b.use_var(var);
                        self.binary(*op, (cur, scalar), (v, s), stmt.span)?.0
                    }
                };
                let v = self.convert(v, s, scalar)?;
                self.b.def_var(var, v);
            }
            StmtKind::Show(values) => {
                for (i, e) in values.iter().enumerate() {
                    if i > 0 {
                        self.show_str(SPACE);
                    }
                    self.show(e)?;
                }
                let end = self.c.helpers.show_end;
                self.call(end, &[self.ctx]);
            }
            StmtKind::If { branches, otherwise } => {
                let done = self.b.create_block();
                for (cond, body) in branches {
                    let (c, _) = self.expr(cond)?;
                    let then = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(c, then, &[], next, &[]);
                    self.b.switch_to_block(then);
                    self.block(body)?;
                    if !self.terminated() {
                        self.b.ins().jump(done, &[]);
                    }
                    self.b.switch_to_block(next);
                }
                self.block(otherwise)?;
                if !self.terminated() {
                    self.b.ins().jump(done, &[]);
                }
                self.b.switch_to_block(done);
            }
            StmtKind::While { cond, body } => {
                let head = self.b.create_block();
                let body_block = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let (c, _) = self.expr(cond)?;
                self.b.ins().brif(c, body_block, &[], exit, &[]);
                self.b.switch_to_block(body_block);
                self.loop_body(body, head, exit)?;
                self.b.switch_to_block(exit);
            }
            StmtKind::Repeat { count, it, body } => {
                let (n, _) = self.expr(count)?;
                let it_var = self.locals[it.0 as usize].0;
                let zero = self.b.ins().iconst(types::I64, 0);
                self.counted_loop(zero, n, None, it_var, body)?;
            }
            StmtKind::For { var, iter, body } => {
                let ExprKind::CallBuiltin { builtin: Builtin::Range, args } = &iter.kind else {
                    return Err("loops over a collection".into());
                };
                let (var, _) = self.local(var)?;
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.expr(a)?.0);
                }
                let (start, end, step) = match vals.as_slice() {
                    [end] => (self.b.ins().iconst(types::I64, 0), *end, None),
                    [start, end] => (*start, *end, None),
                    [start, end, step] => (*start, *end, Some(*step)),
                    _ => return Err("unexpected range arguments".into()),
                };
                if let Some(step) = step {
                    let zero_step = self.b.ins().icmp_imm_s(IntCC::Equal, step, 0);
                    self.fail_if(zero_step, code::RANGE_STEP, iter.span)?;
                }
                self.counted_loop(start, end, step, var, body)?;
            }
            StmtKind::Return(value) => {
                let v = match value {
                    Some(e) => {
                        let (v, s) = self.expr(e)?;
                        self.convert(v, s, self.ret)?
                    }
                    None => zero(&mut self.b, self.ret),
                };
                self.b.def_var(self.ret_var, v);
                self.b.ins().jump(self.exit, &[]);
                self.after_jump();
            }
            StmtKind::Break | StmtKind::Continue => {
                let (cont, brk) = *self.loops.last().ok_or("break outside a loop")?;
                let target = if matches!(stmt.kind, StmtKind::Break) { brk } else { cont };
                self.b.ins().jump(target, &[]);
                self.after_jump();
            }
            StmtKind::Try { .. } => return Err("uses try/catch".into()),
            StmtKind::InitModule(_) => return Err("imports a module".into()),
        }
        Ok(())
    }

    fn loop_body(&mut self, body: &hir::Block, cont: Block, exit: Block) -> R<()> {
        self.loops.push((cont, exit));
        self.block(body)?;
        self.loops.pop();
        if !self.terminated() {
            self.b.ins().jump(cont, &[]);
        }
        Ok(())
    }

    /// Runs `body` with `var` set to `start`, `start + step`, ... while short of `end`.
    fn counted_loop(&mut self, start: ir::Value, end: ir::Value, step: Option<ir::Value>, var: Variable, body: &hir::Block) -> R<()> {
        let counter = self.b.declare_var(types::I64);
        self.b.def_var(counter, start);
        let head = self.b.create_block();
        let body_block = self.b.create_block();
        let next = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let i = self.b.use_var(counter);
        let more = match step {
            None => self.b.ins().icmp(IntCC::SignedLessThan, i, end),
            Some(step) => {
                let up = self.b.ins().icmp(IntCC::SignedLessThan, i, end);
                let down = self.b.ins().icmp(IntCC::SignedGreaterThan, i, end);
                let positive = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, step, 0);
                self.b.ins().select(positive, up, down)
            }
        };
        self.b.ins().brif(more, body_block, &[], exit, &[]);
        self.b.switch_to_block(body_block);
        let i = self.b.use_var(counter);
        self.b.def_var(var, i);
        self.loop_body(body, next, exit)?;
        self.b.switch_to_block(next);
        let i = self.b.use_var(counter);
        let i = match step {
            None => self.b.ins().iadd_imm_s(i, 1),
            Some(step) => {
                let (sum, overflow) = self.b.ins().sadd_overflow(i, step);
                let stop = self.b.create_block();
                let go = self.b.create_block();
                self.b.ins().brif(overflow, stop, &[], go, &[]);
                self.b.switch_to_block(stop);
                self.b.ins().jump(exit, &[]);
                self.b.switch_to_block(go);
                sum
            }
        };
        self.b.def_var(counter, i);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(exit);
        Ok(())
    }

    fn show_str(&mut self, text: &str) {
        let ptr = self.b.ins().iconst(types::I64, text.as_ptr() as i64);
        let len = self.b.ins().iconst(types::I64, text.len() as i64);
        let f = self.c.helpers.show_str;
        self.call(f, &[self.ctx, ptr, len]);
    }

    fn show(&mut self, e: &hir::Expr) -> R<()> {
        match &e.kind {
            ExprKind::Str(s) => {
                self.show_str(s);
                return Ok(());
            }
            ExprKind::FStr(parts) => {
                for part in parts {
                    match part {
                        hir::FStrPart::Lit(s) => self.show_str(s),
                        hir::FStrPart::Expr { expr, spec: None } => self.show(expr)?,
                        hir::FStrPart::Expr { .. } => return Err("formats with a spec".into()),
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let (v, s) = self.expr(e)?;
        let (helper, args) = match s {
            Scalar::Int => (self.c.helpers.show_int, vec![self.ctx, v]),
            Scalar::Float => (self.c.helpers.show_float, vec![self.ctx, v]),
            Scalar::Bool => (self.c.helpers.show_bool, vec![self.ctx, v]),
            Scalar::None => {
                self.show_str("none");
                return Ok(());
            }
        };
        self.call(helper, &args);
        Ok(())
    }

    // ---- expressions ---------------------------------------------------------------------

    fn convert(&mut self, v: ir::Value, from: Scalar, to: Scalar) -> R<ir::Value> {
        Ok(match (from, to) {
            (a, b) if a == b => v,
            (Scalar::Int, Scalar::Float) => self.b.ins().fcvt_from_sint(types::F64, v),
            _ => return Err(format!("mixes {from:?} and {to:?}")),
        })
    }

    fn float(&mut self, (v, s): (ir::Value, Scalar)) -> ir::Value {
        match s {
            Scalar::Int => self.b.ins().fcvt_from_sint(types::F64, v),
            _ => v,
        }
    }

    fn expr(&mut self, e: &hir::Expr) -> R<(ir::Value, Scalar)> {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Int(i) => (self.b.ins().iconst(types::I64, *i), Scalar::Int),
            ExprKind::Float(f) => (self.b.ins().f64const(*f), Scalar::Float),
            ExprKind::Bool(v) => (self.b.ins().iconst(types::I8, i64::from(*v)), Scalar::Bool),
            ExprKind::None => (self.b.ins().iconst(types::I64, 0), Scalar::None),
            ExprKind::Local(l) => {
                let (var, s) = self.locals[l.0 as usize];
                (self.b.use_var(var), s)
            }
            ExprKind::IntToFloat(inner) => {
                let v = self.expr(inner)?;
                (self.float(v), Scalar::Float)
            }
            ExprKind::Unary { op: UnaryOp::Not, operand } => {
                let (v, _) = self.expr(operand)?;
                (self.b.ins().icmp_imm_s(IntCC::Equal, v, 0), Scalar::Bool)
            }
            ExprKind::Unary { op: UnaryOp::Neg, operand } => {
                let (v, s) = self.expr(operand)?;
                match s {
                    Scalar::Int => {
                        let min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                        self.fail_if(min, code::OVERFLOW, span)?;
                        (self.b.ins().ineg(v), Scalar::Int)
                    }
                    Scalar::Float => (self.b.ins().fneg(v), Scalar::Float),
                    _ => return Err("negates a non-number".into()),
                }
            }
            ExprKind::Binary { op: op @ (BinOp::And | BinOp::Or), lhs, rhs } => {
                let result = self.b.declare_var(types::I8);
                let (l, _) = self.expr(lhs)?;
                self.b.def_var(result, l);
                let eval_rhs = self.b.create_block();
                let done = self.b.create_block();
                if *op == BinOp::And {
                    self.b.ins().brif(l, eval_rhs, &[], done, &[]);
                } else {
                    self.b.ins().brif(l, done, &[], eval_rhs, &[]);
                }
                self.b.switch_to_block(eval_rhs);
                let (r, _) = self.expr(rhs)?;
                self.b.def_var(result, r);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                (self.b.use_var(result), Scalar::Bool)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                self.binary(*op, l, r, span)?
            }
            ExprKind::CallFunc { func, args, inst, .. } => {
                let inst: Vec<Ty> = inst.iter().map(|t| substitute(t, &self.subst)).collect();
                if inst.iter().any(super::has_vars) {
                    return Err("calls a function whose types are not known".into());
                }
                let (id, params, ret) = self.c.request(self.prog, &(*func, inst), self.batch)?;
                let mut vals = vec![self.ctx, self.depth];
                for (a, p) in args.iter().zip(&params) {
                    let (v, s) = self.expr(a)?;
                    vals.push(self.convert(v, s, *p)?);
                }
                let result = self.call(id, &vals).expect("compiled functions return a value");
                self.check_status();
                (result, ret)
            }
            ExprKind::CallBuiltin { builtin, args } => self.builtin(*builtin, args, span)?,
            ExprKind::Str(_) | ExprKind::FStr(_) => return Err("uses strings".into()),
            other => return Err(format!("uses {}", describe(other))),
        })
    }

    fn binary(&mut self, op: BinOp, l: (ir::Value, Scalar), r: (ir::Value, Scalar), span: Span) -> R<(ir::Value, Scalar)> {
        let ints = l.1 == Scalar::Int && r.1 == Scalar::Int;
        let numeric = |s: Scalar| matches!(s, Scalar::Int | Scalar::Float);
        if !matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) && !(numeric(l.1) && numeric(r.1)) {
            return Err(format!("applies `{}` to non-numbers", op.symbol()));
        }
        Ok(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul if ints => {
                let (v, of) = match op {
                    BinOp::Add => self.b.ins().sadd_overflow(l.0, r.0),
                    BinOp::Sub => self.b.ins().ssub_overflow(l.0, r.0),
                    _ => self.b.ins().smul_overflow(l.0, r.0),
                };
                self.fail_if(of, code::OVERFLOW, span)?;
                (v, Scalar::Int)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let (a, b) = (self.float(l), self.float(r));
                let v = match op {
                    BinOp::Add => self.b.ins().fadd(a, b),
                    BinOp::Sub => self.b.ins().fsub(a, b),
                    _ => self.b.ins().fmul(a, b),
                };
                (v, Scalar::Float)
            }
            BinOp::Div => {
                let (a, b) = (self.float(l), self.float(r));
                let zero = self.b.ins().f64const(0.0);
                let is_zero = self.b.ins().fcmp(FloatCC::Equal, b, zero);
                self.fail_if(is_zero, code::DIV_ZERO, span)?;
                (self.b.ins().fdiv(a, b), Scalar::Float)
            }
            BinOp::FloorDiv | BinOp::Mod if ints => {
                let (a, b) = (l.0, r.0);
                let is_zero = self.b.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.fail_if(is_zero, code::DIV_ZERO, span)?;
                let a_min = self.b.ins().icmp_imm_s(IntCC::Equal, a, i64::MIN);
                let b_neg1 = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let edge = self.b.ins().band(a_min, b_neg1);
                if op == BinOp::FloorDiv {
                    self.fail_if(edge, code::OVERFLOW, span)?;
                    let q = self.b.ins().sdiv(a, b);
                    let rem = self.b.ins().srem(a, b);
                    let adjust = self.sign_differs(rem, b);
                    let q1 = self.b.ins().iadd_imm_s(q, -1);
                    (self.b.ins().select(adjust, q1, q), Scalar::Int)
                } else {
                    let one = self.b.ins().iconst(types::I64, 1);
                    let safe_b = self.b.ins().select(edge, one, b);
                    let rem = self.b.ins().srem(a, safe_b);
                    let adjust = self.sign_differs(rem, b);
                    let fixed = self.b.ins().iadd(rem, b);
                    (self.b.ins().select(adjust, fixed, rem), Scalar::Int)
                }
            }
            BinOp::FloorDiv | BinOp::Mod => {
                let (a, b) = (self.float(l), self.float(r));
                let zero = self.b.ins().f64const(0.0);
                let is_zero = self.b.ins().fcmp(FloatCC::Equal, b, zero);
                self.fail_if(is_zero, code::DIV_ZERO, span)?;
                if op == BinOp::FloorDiv {
                    let q = self.b.ins().fdiv(a, b);
                    (self.b.ins().floor(q), Scalar::Float)
                } else {
                    let f = self.c.helpers.fmod;
                    (self.call(f, &[a, b]).unwrap(), Scalar::Float)
                }
            }
            BinOp::Pow if ints => {
                let [file, lo, hi] = self.span_args(span);
                let f = self.c.helpers.ipow;
                let v = self.call(f, &[self.ctx, l.0, r.0, file, lo, hi]).unwrap();
                self.check_status();
                (v, Scalar::Int)
            }
            BinOp::Pow => {
                let (a, b) = (self.float(l), self.float(r));
                let f = self.c.helpers.powf;
                (self.call(f, &[a, b]).unwrap(), Scalar::Float)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => self.compare(op, l, r)?,
            BinOp::And | BinOp::Or => unreachable!("handled with short-circuiting"),
            BinOp::In | BinOp::MatMul => return Err(format!("uses `{}`", op.symbol())),
        })
    }

    /// Whether `rem` is non-zero and has the opposite sign of `divisor`.
    fn sign_differs(&mut self, rem: ir::Value, divisor: ir::Value) -> ir::Value {
        let nonzero = self.b.ins().icmp_imm_s(IntCC::NotEqual, rem, 0);
        let x = self.b.ins().bxor(rem, divisor);
        let opposite = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, x, 0);
        self.b.ins().band(nonzero, opposite)
    }

    fn compare(&mut self, op: BinOp, l: (ir::Value, Scalar), r: (ir::Value, Scalar)) -> R<(ir::Value, Scalar)> {
        let v = match (l.1, r.1) {
            (Scalar::None, Scalar::None) => self.b.ins().iconst(types::I8, i64::from(matches!(op, BinOp::Eq | BinOp::Le | BinOp::Ge))),
            (Scalar::Int, Scalar::Int) | (Scalar::Bool, Scalar::Bool) => {
                let signed = l.1 == Scalar::Int;
                let cc = match op {
                    BinOp::Eq => IntCC::Equal,
                    BinOp::Ne => IntCC::NotEqual,
                    BinOp::Lt if signed => IntCC::SignedLessThan,
                    BinOp::Le if signed => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt if signed => IntCC::SignedGreaterThan,
                    BinOp::Ge if signed => IntCC::SignedGreaterThanOrEqual,
                    BinOp::Lt => IntCC::UnsignedLessThan,
                    BinOp::Le => IntCC::UnsignedLessThanOrEqual,
                    BinOp::Gt => IntCC::UnsignedGreaterThan,
                    _ => IntCC::UnsignedGreaterThanOrEqual,
                };
                self.b.ins().icmp(cc, l.0, r.0)
            }
            (a, b) if matches!(a, Scalar::Int | Scalar::Float) && matches!(b, Scalar::Int | Scalar::Float) => {
                let (x, y) = (self.float(l), self.float(r));
                let cc = match op {
                    BinOp::Eq => FloatCC::Equal,
                    BinOp::Ne => FloatCC::NotEqual,
                    BinOp::Lt => FloatCC::LessThan,
                    BinOp::Le => FloatCC::LessThanOrEqual,
                    BinOp::Gt => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                self.b.ins().fcmp(cc, x, y)
            }
            _ => return Err("compares values of different kinds".into()),
        };
        Ok((v, Scalar::Bool))
    }

    fn builtin(&mut self, b: Builtin, args: &[hir::Expr], span: Span) -> R<(ir::Value, Scalar)> {
        let lowered = if b == Builtin::Assert { &args[..1] } else { args };
        let mut vals = Vec::with_capacity(lowered.len());
        for a in lowered {
            vals.push(self.expr(a)?);
        }
        let f64_of = |me: &mut Self, i: usize| me.float(vals[i]);
        Ok(match b {
            Builtin::Abs => match vals[0].1 {
                Scalar::Int => {
                    let v = vals[0].0;
                    let min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                    self.fail_if(min, code::OVERFLOW, span)?;
                    let neg = self.b.ins().ineg(v);
                    let is_neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, v, 0);
                    (self.b.ins().select(is_neg, neg, v), Scalar::Int)
                }
                _ => {
                    let v = f64_of(self, 0);
                    (self.b.ins().fabs(v), Scalar::Float)
                }
            },
            Builtin::Min | Builtin::Max if vals.len() >= 2 => {
                let s = vals[0].1;
                let mut acc = vals[0].0;
                for (v, vs) in &vals[1..] {
                    if *vs != s {
                        return Err("mixes kinds in min/max".into());
                    }
                    let take = match (s, b) {
                        (Scalar::Int, Builtin::Max) => self.b.ins().icmp(IntCC::SignedGreaterThan, *v, acc),
                        (Scalar::Int, _) => self.b.ins().icmp(IntCC::SignedLessThan, *v, acc),
                        (Scalar::Float, Builtin::Max) => self.b.ins().fcmp(FloatCC::GreaterThan, *v, acc),
                        (Scalar::Float, _) => self.b.ins().fcmp(FloatCC::LessThan, *v, acc),
                        _ => return Err("min/max of non-numbers".into()),
                    };
                    acc = self.b.ins().select(take, *v, acc);
                }
                (acc, s)
            }
            Builtin::ToFloat => match vals[0].1 {
                Scalar::Bool => {
                    let w = self.b.ins().uextend(types::I64, vals[0].0);
                    (self.b.ins().fcvt_from_sint(types::F64, w), Scalar::Float)
                }
                Scalar::Int | Scalar::Float => (f64_of(self, 0), Scalar::Float),
                Scalar::None => return Err("converts none".into()),
            },
            Builtin::ToInt => match vals[0].1 {
                Scalar::Int => vals[0],
                Scalar::Bool => (self.b.ins().uextend(types::I64, vals[0].0), Scalar::Int),
                Scalar::Float => {
                    let v = vals[0].0;
                    let finite = self.finite(v);
                    let not_finite = self.b.ins().icmp_imm_s(IntCC::Equal, finite, 0);
                    self.fail_if(not_finite, code::FLOAT_TO_INT, span)?;
                    let t = self.b.ins().trunc(v);
                    (self.b.ins().fcvt_to_sint_sat(types::I64, t), Scalar::Int)
                }
                Scalar::None => return Err("converts none".into()),
            },
            Builtin::ToBool => match vals[0].1 {
                Scalar::Bool => vals[0],
                Scalar::Int => (self.b.ins().icmp_imm_s(IntCC::NotEqual, vals[0].0, 0), Scalar::Bool),
                Scalar::Float => {
                    let zero = self.b.ins().f64const(0.0);
                    (self.b.ins().fcmp(FloatCC::NotEqual, vals[0].0, zero), Scalar::Bool)
                }
                Scalar::None => (self.b.ins().iconst(types::I8, 0), Scalar::Bool),
            },
            Builtin::Round if vals.len() == 1 => {
                let v = f64_of(self, 0);
                let r = self.b.ins().nearest(v);
                (self.b.ins().fcvt_to_sint_sat(types::I64, r), Scalar::Int)
            }
            Builtin::Round => {
                let v = f64_of(self, 0);
                let f = self.c.helpers.round_digits;
                (self.call(f, &[v, vals[1].0]).unwrap(), Scalar::Float)
            }
            Builtin::MathPi => (self.b.ins().f64const(std::f64::consts::PI), Scalar::Float),
            Builtin::MathE => (self.b.ins().f64const(std::f64::consts::E), Scalar::Float),
            Builtin::MathSqrt => {
                let v = f64_of(self, 0);
                (self.b.ins().sqrt(v), Scalar::Float)
            }
            Builtin::MathFloor | Builtin::MathCeil => {
                let v = f64_of(self, 0);
                let r = if b == Builtin::MathFloor { self.b.ins().floor(v) } else { self.b.ins().ceil(v) };
                (self.b.ins().fcvt_to_sint_sat(types::I64, r), Scalar::Int)
            }
            Builtin::MathSin | Builtin::MathCos | Builtin::MathTan | Builtin::MathLog | Builtin::MathExp => {
                let v = f64_of(self, 0);
                let op = match b {
                    Builtin::MathSin => math::SIN,
                    Builtin::MathCos => math::COS,
                    Builtin::MathTan => math::TAN,
                    Builtin::MathLog => math::LOG,
                    _ => math::EXP,
                };
                let op = self.b.ins().iconst(types::I32, i64::from(op));
                let f = self.c.helpers.math;
                (self.call(f, &[op, v]).unwrap(), Scalar::Float)
            }
            Builtin::MathPow => {
                let (x, y) = (f64_of(self, 0), f64_of(self, 1));
                let f = self.c.helpers.powf;
                (self.call(f, &[x, y]).unwrap(), Scalar::Float)
            }
            Builtin::TimeNow => {
                let f = self.c.helpers.time_now;
                (self.call(f, &[]).unwrap(), Scalar::Float)
            }
            Builtin::TimeNowMs => {
                let f = self.c.helpers.time_now_ms;
                (self.call(f, &[]).unwrap(), Scalar::Int)
            }
            Builtin::Assert => {
                let message = match args.get(1).map(|a| &a.kind) {
                    None => "assertion failed".to_string(),
                    Some(ExprKind::Str(m)) => format!("assertion failed: {m}"),
                    Some(_) => return Err("asserts with a computed message".into()),
                };
                let text: &'static str = Box::leak(message.into_boxed_str());
                let raise = self.b.create_block();
                let ok = self.b.create_block();
                self.b.ins().brif(vals[0].0, ok, &[], raise, &[]);
                self.b.switch_to_block(raise);
                let ptr = self.b.ins().iconst(types::I64, text.as_ptr() as i64);
                let len = self.b.ins().iconst(types::I64, text.len() as i64);
                let [file, lo, hi] = self.span_args(span);
                let f = self.c.helpers.fail_msg;
                self.call(f, &[self.ctx, ptr, len, file, lo, hi]);
                self.b.ins().jump(self.fail, &[]);
                self.b.switch_to_block(ok);
                (self.b.ins().iconst(types::I64, 0), Scalar::None)
            }
            other => return Err(format!("calls `{}`", other.name())),
        })
    }

    fn finite(&mut self, v: ir::Value) -> ir::Value {
        let diff = self.b.ins().fsub(v, v);
        self.b.ins().fcmp(FloatCC::Equal, diff, diff)
    }
}

fn describe(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::List(_) | ExprKind::Tuple(_) => "lists and tuples",
        ExprKind::Map(_) => "maps",
        ExprKind::Global(_) => "global variables",
        ExprKind::Func { .. } | ExprKind::Closure { .. } | ExprKind::Recurse | ExprKind::Call { .. } => "function values",
        ExprKind::MethodCall { .. } => "methods",
        ExprKind::New { .. } | ExprKind::Field { .. } => "classes",
        ExprKind::Index { .. } | ExprKind::Slice { .. } => "indexing",
        ExprKind::Query { .. } => "queries",
        ExprKind::Together(_) | ExprKind::Await(_) | ExprKind::RunParallel(_) => "concurrency",
        ExprKind::PyImport(_) => "Python",
        _ => "an unsupported expression",
    }
}
