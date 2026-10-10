//! Lowers one function instance from typed HIR to Cranelift IR.
//!
//! Scalars live in Cranelift variables. Every other value lives in a frame
//! slot: locals have one slot each, and every expression producing such a value
//! gets a temporary slot. Slots are initialized on entry and dropped on exit.

use super::runtime::{DEPTH_OFFSET, MAX_DEPTH, STATUS_OFFSET, VALUE_SIZE, code, math, pack};
use super::{Batch, Compiler, Key, Rep, Scalar};
use crate::check::builtins::Builtin;
use crate::check::hir::{self, ExprKind, Place, Program, StmtKind};
use crate::check::types::Ty;
use crate::source::Span;
use crate::syntax::ast::{BinOp, QueryOp, UnaryOp};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    self, Block, Inst, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId as ClifFunc, Module};
use std::collections::HashMap;
use std::sync::Arc;

type R<T> = Result<T, String>;

pub fn substitute(ty: &Ty, subst: &[(u32, Ty)]) -> Ty {
    match ty {
        Ty::Var(v) => subst
            .iter()
            .find(|(k, _)| k == v)
            .map(|(_, t)| t.clone())
            .unwrap_or(Ty::Var(*v)),
        Ty::List(t) => Ty::list(substitute(t, subst)),
        Ty::Task(t) => Ty::task(substitute(t, subst)),
        Ty::Map(k, v) => Ty::map(substitute(k, subst), substitute(v, subst)),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| substitute(t, subst)).collect()),
        Ty::Func(ps, r) => Ty::func(
            ps.iter().map(|p| substitute(p, subst)).collect(),
            substitute(r, subst),
        ),
        other => other.clone(),
    }
}

static SPACE: &str = " ";

#[derive(Clone, Copy)]
enum Local {
    Scalar(Variable, Scalar),
    Obj(u32),
}

/// A lowered value: a scalar in a register, or the address of a slot. An owned
/// slot is a temporary whose contents may be moved out.
#[derive(Clone, Copy)]
enum Val {
    S(ir::Value, Scalar),
    O(ir::Value, bool),
}

struct Lower<'a, 'b> {
    c: &'a mut Compiler,
    prog: &'a Program,
    batch: &'a mut Batch,
    func: &'a hir::Func,
    subst: Vec<(u32, Ty)>,
    b: FunctionBuilder<'b>,
    ctx: ir::Value,
    depth: ir::Value,
    ret_slot: Option<ir::Value>,
    locals: Vec<Local>,
    /// Module variables this module's code keeps to itself, stored like locals.
    private: HashMap<u32, Local>,
    ret: Rep,
    ret_var: Variable,
    exit: Block,
    fail: Block,
    loops: Vec<(Block, Block)>,
    refs: HashMap<ClifFunc, ir::FuncRef>,
    base: ir::Value,
    slots: u32,
}

pub(super) fn lower(
    c: &mut Compiler,
    prog: &Program,
    batch: &mut Batch,
    key: &Key,
    out: &mut ir::Function,
    fctx: &mut FunctionBuilderContext,
) -> R<()> {
    let func = &prog.funcs[key.0.0 as usize];
    let param_slots: HashMap<usize, usize> = param_locals(func)
        .into_iter()
        .enumerate()
        .map(|(arg, local)| (local, arg))
        .collect();
    let subst: Vec<(u32, Ty)> = func
        .generics
        .iter()
        .copied()
        .zip(key.1.iter().cloned())
        .collect();
    let compiled = batch.pending[key].clone();
    let mut b = FunctionBuilder::new(out, fctx);

    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    b.seal_block(entry);
    let params = b.block_params(entry).to_vec();
    let (ctx, caller_depth) = (params[0], params[1]);
    let (ret_slot, args) = if compiled.ret == Rep::Obj {
        (Some(params[2]), &params[3..])
    } else {
        (None, &params[2..])
    };
    let args = args.to_vec();
    let depth = b.ins().iadd_imm_s(caller_depth, 1);
    let base = b.ins().iconst(types::I64, 0);
    let count = b.ins().iconst(types::I64, 0);
    let base_inst = inst_of(&b, base);
    let count_inst = inst_of(&b, count);
    let ret_var = b.declare_var(match compiled.ret {
        Rep::Scalar(s) => s.clif(),
        Rep::Obj => types::I64,
    });
    let zero_ret = zero(&mut b, compiled.ret);
    b.def_var(ret_var, zero_ret);

    let exit = b.create_block();
    let fail = b.create_block();
    let mut l = Lower {
        c,
        prog,
        batch,
        func,
        subst,
        b,
        ctx,
        depth,
        ret_slot,
        locals: Vec::new(),
        private: HashMap::new(),
        ret: compiled.ret,
        ret_var,
        exit,
        fail,
        loops: Vec::new(),
        refs: HashMap::new(),
        base,
        slots: 0,
    };
    let init = l.c.helpers.slots_init;
    l.call(init, &[base, count]);
    let mut frame_calls = vec![l.last_inst()];

    for (i, local) in func.locals.iter().enumerate() {
        let rep = Rep::of(&substitute(&local.ty, &l.subst));
        let slot = match rep {
            Rep::Scalar(s) => {
                let var = l.b.declare_var(s.clif());
                let init = match param_slots.get(&i) {
                    Some(&arg) => args[arg],
                    None => zero(&mut l.b, rep),
                };
                l.b.def_var(var, init);
                Local::Scalar(var, s)
            }
            Rep::Obj => {
                let index = l.slots;
                l.slots += 1;
                if let Some(&arg) = param_slots.get(&i) {
                    let dst = l.slot_addr(index);
                    let clone = l.c.helpers.clone;
                    l.call(clone, &[dst, args[arg]]);
                }
                Local::Obj(index)
            }
        };
        l.locals.push(slot);
    }

    let mut private: Vec<_> =
        l.c.private_globals
            .get(&key.0)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();
    private.sort_by_key(|g| g.0);
    for g in private {
        let local = match Rep::of(&prog.globals[g.0 as usize].ty) {
            Rep::Scalar(s) => {
                let var = l.b.declare_var(s.clif());
                let init = zero(&mut l.b, Rep::Scalar(s));
                l.b.def_var(var, init);
                Local::Scalar(var, s)
            }
            Rep::Obj => {
                let index = l.slots;
                l.slots += 1;
                Local::Obj(index)
            }
        };
        l.private.insert(g.0, local);
    }

    let too_deep =
        l.b.ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThan, depth, MAX_DEPTH as i64);
    l.fail_if(
        too_deep,
        code::DEPTH,
        func.body.first().map(|s| s.span).unwrap_or_default(),
    );

    l.block(&func.body)?;
    if !l.terminated() {
        l.b.ins().jump(l.exit, &[]);
    }

    for (block, failed) in [(l.exit, false), (l.fail, true)] {
        l.b.switch_to_block(block);
        let drop = l.c.helpers.slots_drop;
        l.call(drop, &[base, count]);
        frame_calls.push(l.last_inst());
        let result = if failed {
            zero(&mut l.b, l.ret)
        } else {
            l.b.use_var(l.ret_var)
        };
        l.b.ins().return_(&[result]);
    }

    let slots = l.slots;
    if slots == 0 {
        for inst in frame_calls {
            l.b.func.layout.remove_inst(inst);
        }
    }
    let frame = l.b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        (slots * VALUE_SIZE).max(8),
        3,
    ));
    l.b.func.replace(base_inst).stack_addr(types::I64, frame, 0);
    l.b.func
        .replace(count_inst)
        .iconst(types::I64, i64::from(slots));
    l.b.seal_all_blocks();
    let config = l.c.module.isa().frontend_config();
    l.b.finalize(config);
    Ok(())
}

/// The locals that receive arguments: the parameters, then a closure's captures.
pub fn param_locals(func: &hir::Func) -> Vec<usize> {
    (0..func.params)
        .chain(func.captures.iter().map(|l| l.0 as usize))
        .collect()
}

fn inst_of(b: &FunctionBuilder, v: ir::Value) -> Inst {
    b.func.dfg.value_def(v).unwrap_inst()
}

fn zero(b: &mut FunctionBuilder, rep: Rep) -> ir::Value {
    match rep {
        Rep::Scalar(Scalar::Float) => b.ins().f64const(0.0),
        Rep::Scalar(Scalar::Bool) => b.ins().iconst(types::I8, 0),
        _ => b.ins().iconst(types::I64, 0),
    }
}

fn describe(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Recurse => "a nested function calling itself",
        _ => "an unsupported expression",
    }
}

impl Lower<'_, '_> {
    fn last_inst(&self) -> Inst {
        let block = self.b.current_block().expect("positioned in a block");
        self.b
            .func
            .layout
            .last_inst(block)
            .expect("an instruction was just added")
    }

    fn terminated(&self) -> bool {
        let block = self.b.current_block().expect("positioned in a block");
        self.b
            .func
            .layout
            .last_inst(block)
            .is_some_and(|i| self.b.func.dfg.insts[i].opcode().is_terminator())
    }

    /// Starts a fresh block after a terminator so later statements have somewhere to go.
    fn after_jump(&mut self) {
        let block = self.b.create_block();
        self.b.switch_to_block(block);
    }

    fn rep(&self, ty: &Ty) -> Rep {
        Rep::of(&substitute(ty, &self.subst))
    }

    fn func_ref(&mut self, id: ClifFunc) -> ir::FuncRef {
        if let Some(r) = self.refs.get(&id) {
            return *r;
        }
        let r = self.c.module.declare_func_in_func(id, self.b.func);
        self.refs.insert(id, r);
        r
    }

    fn call(&mut self, id: ClifFunc, args: &[ir::Value]) -> Option<ir::Value> {
        let r = self.func_ref(id);
        let inst = self.b.ins().call(r, args);
        self.b.inst_results(inst).first().copied()
    }

    fn iconst(&mut self, v: i64) -> ir::Value {
        self.b.ins().iconst(types::I64, v)
    }

    fn i32const(&mut self, v: u32) -> ir::Value {
        self.b.ins().iconst(types::I32, i64::from(v))
    }

    fn span(&mut self, span: Span) -> ir::Value {
        self.iconst(pack(span))
    }

    /// The address and length of text that lives as long as the program.
    fn text(&mut self, s: &str) -> (ir::Value, ir::Value) {
        (self.iconst(s.as_ptr() as i64), self.iconst(s.len() as i64))
    }

    fn slot_addr(&mut self, index: u32) -> ir::Value {
        self.b
            .ins()
            .iadd_imm_s(self.base, i64::from(index * VALUE_SIZE))
    }

    fn temp(&mut self) -> ir::Value {
        let index = self.slots;
        self.slots += 1;
        self.slot_addr(index)
    }

    /// Lets interpreted code called from here see this call's recursion depth.
    fn publish_depth(&mut self) {
        self.b
            .ins()
            .store(MemFlagsData::trusted(), self.depth, self.ctx, DEPTH_OFFSET);
    }

    /// Raises error `code` when `cond` is true.
    fn fail_if(&mut self, cond: ir::Value, code: u32, span: Span) {
        let raise = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(cond, raise, &[], ok, &[]);
        self.b.switch_to_block(raise);
        let code = self.i32const(code);
        let span = self.span(span);
        let fail = self.c.helpers.fail;
        self.call(fail, &[self.ctx, code, span]);
        self.b.ins().jump(self.fail, &[]);
        self.b.switch_to_block(ok);
    }

    /// Propagates an error raised by a call to the enclosing `try` or out of the function.
    fn check_status(&mut self) {
        let status = self
            .b
            .ins()
            .load(types::I8, MemFlagsData::trusted(), self.ctx, STATUS_OFFSET);
        let ok = self.b.create_block();
        self.b.ins().brif(status, self.fail, &[], ok, &[]);
        self.b.switch_to_block(ok);
    }

    /// Stores `addrs` in a fresh stack array and returns its address.
    fn ptr_array(&mut self, addrs: &[ir::Value]) -> ir::Value {
        let slot = self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (addrs.len() as u32 * 8).max(8),
            3,
        ));
        for (i, a) in addrs.iter().enumerate() {
            self.b
                .ins()
                .stack_store(types::I64, *a, slot, (i * 8) as i32);
        }
        self.b.ins().stack_addr(types::I64, slot, 0)
    }

    // ---- converting between representations -----------------------------------------------

    /// The address of a slot holding `v`, boxing scalars into a temporary.
    fn boxed(&mut self, v: Val) -> ir::Value {
        match v {
            Val::O(addr, _) => addr,
            Val::S(x, s) => {
                let t = self.temp();
                let h = &self.c.helpers;
                let (f, args) = match s {
                    Scalar::Int => (h.set_int, vec![t, x]),
                    Scalar::Float => (h.set_float, vec![t, x]),
                    Scalar::Bool => (h.set_bool, vec![t, x]),
                    Scalar::None => (h.set_none, vec![t]),
                };
                self.call(f, &args);
                t
            }
        }
    }

    /// `v` as a scalar of kind `want`, reading it out of its slot if needed.
    fn scalar(&mut self, v: Val, want: Scalar) -> ir::Value {
        match v {
            Val::S(x, s) if s == want => x,
            Val::S(x, Scalar::Int) if want == Scalar::Float => {
                self.b.ins().fcvt_from_sint(types::F64, x)
            }
            Val::S(_, _) => zero(&mut self.b, Rep::Scalar(want)),
            Val::O(addr, _) => {
                let h = &self.c.helpers;
                let f = match want {
                    Scalar::Int => h.get_int,
                    Scalar::Float => h.get_float,
                    Scalar::Bool => h.get_bool,
                    Scalar::None => return self.iconst(0),
                };
                self.call(f, &[addr]).expect("getters return a value")
            }
        }
    }

    /// Interprets a slot written by a helper according to the expression's type.
    fn result(&mut self, addr: ir::Value, ty: &Ty) -> Val {
        match self.rep(ty) {
            Rep::Scalar(s) => Val::S(self.scalar(Val::O(addr, true), s), s),
            Rep::Obj => Val::O(addr, true),
        }
    }

    /// Writes `v` into the slot at `dst`.
    fn write_slot(&mut self, dst: ir::Value, v: Val) {
        match v {
            Val::O(src, owned) => {
                let f = if owned {
                    self.c.helpers.take
                } else {
                    self.c.helpers.clone
                };
                self.call(f, &[dst, src]);
            }
            Val::S(..) => {
                let src = self.boxed(v);
                let take = self.c.helpers.take;
                self.call(take, &[dst, src]);
            }
        }
    }

    fn store_local(&mut self, local: Local, v: Val) {
        match local {
            Local::Scalar(var, s) => {
                let x = self.scalar(v, s);
                self.b.def_var(var, x);
            }
            Local::Obj(index) => {
                let dst = self.slot_addr(index);
                self.write_slot(dst, v);
            }
        }
    }

    fn load_local(&mut self, local: Local) -> Val {
        match local {
            Local::Scalar(var, s) => Val::S(self.b.use_var(var), s),
            Local::Obj(index) => Val::O(self.slot_addr(index), false),
        }
    }

    // ---- statements ----------------------------------------------------------------------

    fn block(&mut self, block: &hir::Block) -> R<()> {
        for stmt in block {
            self.stmt(stmt)?;
        }
        Ok(())
    }

    fn store(&mut self, place: &Place, v: Val, span: Span) -> R<()> {
        match place {
            Place::Local(l) => self.store_local(self.locals[l.0 as usize], v),
            Place::Global(g) if self.private.contains_key(&g.0) => {
                self.store_local(self.private[&g.0], v)
            }
            Place::Global(g) => {
                let src = self.boxed(v);
                let g = self.i32const(g.0);
                let set = self.c.helpers.global_set;
                self.call(set, &[self.ctx, g, src]);
            }
            Place::Field { target, name } => {
                let obj = self.expr(target)?;
                let obj = self.boxed(obj);
                let src = self.boxed(v);
                let (ptr, len) = self.text(name);
                let span = self.span(span);
                let set = self.c.helpers.set_field;
                self.call(set, &[self.ctx, obj, ptr, len, src, span]);
                self.check_status();
            }
            Place::Index { target, index }
                if self.is_list(&target.ty) && self.rep(&index.ty) == Rep::Scalar(Scalar::Int) =>
            {
                let obj = self.expr(target)?;
                let obj = self.boxed(obj);
                let idx = self.expr(index)?;
                let idx = self.scalar(idx, Scalar::Int);
                let src = self.boxed(v);
                let span = self.span(span);
                let set = self.c.helpers.list_set;
                self.call(set, &[self.ctx, obj, idx, src, span]);
                self.check_status();
            }
            Place::Index { target, index } => {
                let obj = self.expr(target)?;
                let obj = self.boxed(obj);
                let idx = self.expr(index)?;
                let idx = self.boxed(idx);
                let src = self.boxed(v);
                let span = self.span(span);
                let set = self.c.helpers.set_index;
                self.call(set, &[self.ctx, obj, idx, src, span]);
                self.check_status();
            }
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &hir::Stmt) -> R<()> {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.expr(e)?;
            }
            StmtKind::Assign {
                place,
                op: None,
                value,
            } => {
                let v = self.expr(value)?;
                self.store(place, v, span)?;
            }
            StmtKind::Assign {
                place,
                op: Some(op),
                value,
            } => {
                let current = match place {
                    Place::Local(l) => self.load_local(self.locals[l.0 as usize]),
                    Place::Global(g) => match self.private.get(&g.0) {
                        Some(local) => self.load_local(*local),
                        None => self.global(*g, &self.prog.globals[g.0 as usize].ty.clone())?,
                    },
                    Place::Field { target, name } => self.field(target, name, &Ty::Any, span)?,
                    Place::Index { target, index } => self.index(target, index, &Ty::Any)?,
                };
                let rhs = self.expr(value)?;
                let result_ty = match place {
                    Place::Local(l) => self.func.locals[l.0 as usize].ty.clone(),
                    Place::Global(g) => self.prog.globals[g.0 as usize].ty.clone(),
                    _ => Ty::Any,
                };
                let v = self.binary(*op, current, rhs, &result_ty, span)?;
                self.store(place, v, span)?;
            }
            StmtKind::Show(values) => {
                for (i, e) in values.iter().enumerate() {
                    if i > 0 {
                        self.show_text(SPACE);
                    }
                    self.show(e)?;
                }
                let end = self.c.helpers.show_end;
                self.call(end, &[self.ctx]);
            }
            StmtKind::If {
                branches,
                otherwise,
            } => {
                let done = self.b.create_block();
                for (cond, body) in branches {
                    let c = self.expr(cond)?;
                    let c = self.scalar(c, Scalar::Bool);
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
                let c = self.expr(cond)?;
                let c = self.scalar(c, Scalar::Bool);
                self.b.ins().brif(c, body_block, &[], exit, &[]);
                self.b.switch_to_block(body_block);
                self.loop_body(body, head, exit)?;
                self.b.switch_to_block(exit);
            }
            StmtKind::Repeat { count, it, body } => {
                let n = self.expr(count)?;
                let n = self.scalar(n, Scalar::Int);
                let zero = self.iconst(0);
                let var = Place::Local(*it);
                self.counted_loop(zero, n, None, &var, span, |_, _| Ok(()), body)?;
            }
            StmtKind::For { var, iter, body } => {
                if let ExprKind::CallBuiltin {
                    builtin: Builtin::Range,
                    args,
                } = &iter.kind
                {
                    let mut vals = Vec::new();
                    for a in args {
                        let v = self.expr(a)?;
                        vals.push(self.scalar(v, Scalar::Int));
                    }
                    let (start, end, step) = match vals.as_slice() {
                        [end] => (self.iconst(0), *end, None),
                        [start, end] => (*start, *end, None),
                        [start, end, step] => (*start, *end, Some(*step)),
                        _ => return Err("unexpected range arguments".into()),
                    };
                    if let Some(step) = step {
                        let zero_step = self.b.ins().icmp_imm_s(IntCC::Equal, step, 0);
                        self.fail_if(zero_step, code::RANGE_STEP, iter.span);
                    }
                    self.counted_loop(start, end, step, var, span, |_, _| Ok(()), body)?;
                } else {
                    let items = self.items(iter)?;
                    let n = self.list_len(items);
                    let zero = self.iconst(0);
                    let item = self.temp();
                    self.counted_loop(
                        zero,
                        n,
                        None,
                        &Place::Local(hir::LocalId(u32::MAX)),
                        span,
                        |l, i| {
                            let get = l.c.helpers.list_get;
                            l.call(get, &[item, items, i]);
                            l.store(var, Val::O(item, true), span)
                        },
                        body,
                    )?;
                }
            }
            StmtKind::Return(value) => {
                match (value, self.ret) {
                    (Some(e), Rep::Scalar(s)) => {
                        let v = self.expr(e)?;
                        let x = self.scalar(v, s);
                        self.b.def_var(self.ret_var, x);
                    }
                    (Some(e), Rep::Obj) => {
                        let v = self.expr(e)?;
                        let dst = self.ret_slot.expect("object results have a slot");
                        self.write_slot(dst, v);
                    }
                    (None, _) => {}
                }
                self.b.ins().jump(self.exit, &[]);
                self.after_jump();
            }
            StmtKind::Break | StmtKind::Continue => {
                let (cont, brk) = *self.loops.last().ok_or("break outside a loop")?;
                let target = if matches!(stmt.kind, StmtKind::Break) {
                    brk
                } else {
                    cont
                };
                self.b.ins().jump(target, &[]);
                self.after_jump();
            }
            StmtKind::Try { body, var, handler } => {
                let catch = self.b.create_block();
                let done = self.b.create_block();
                let outer = std::mem::replace(&mut self.fail, catch);
                self.block(body)?;
                self.fail = outer;
                if !self.terminated() {
                    self.b.ins().jump(done, &[]);
                }
                self.b.switch_to_block(catch);
                let message = self.temp();
                let take = self.c.helpers.take_error;
                self.call(take, &[self.ctx, message]);
                self.store(var, Val::O(message, true), span)?;
                self.block(handler)?;
                if !self.terminated() {
                    self.b.ins().jump(done, &[]);
                }
                self.b.switch_to_block(done);
            }
            StmtKind::InitModule(m) => {
                self.publish_depth();
                let m = self.i32const(m.0);
                let span = self.span(span);
                let init = self.c.helpers.init_module;
                self.call(init, &[self.ctx, m, span]);
                self.check_status();
            }
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

    /// Runs `body` for `i` from `start` toward `end` by `step` (default 1),
    /// storing `i` in `var` unless `before` handles the iteration variable.
    #[allow(clippy::too_many_arguments)]
    fn counted_loop(
        &mut self,
        start: ir::Value,
        end: ir::Value,
        step: Option<ir::Value>,
        var: &Place,
        span: Span,
        before: impl FnOnce(&mut Self, ir::Value) -> R<()>,
        body: &hir::Block,
    ) -> R<()> {
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
        if !matches!(var, Place::Local(l) if l.0 == u32::MAX) {
            self.store(var, Val::S(i, Scalar::Int), span)?;
        }
        before(self, i)?;
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

    /// Evaluates `iter` and returns a slot holding its items as a list.
    fn items(&mut self, iter: &hir::Expr) -> R<ir::Value> {
        let v = self.expr(iter)?;
        let src = self.boxed(v);
        let list = self.temp();
        let span = self.span(iter.span);
        let items = self.c.helpers.items;
        self.call(items, &[self.ctx, list, src, span]);
        self.check_status();
        Ok(list)
    }

    fn list_len(&mut self, list: ir::Value) -> ir::Value {
        let len = self.c.helpers.list_len;
        self.call(len, &[list]).expect("length")
    }

    fn show_text(&mut self, text: &str) {
        let (ptr, len) = self.text(text);
        let f = self.c.helpers.show_str;
        self.call(f, &[self.ctx, ptr, len]);
    }

    fn show(&mut self, e: &hir::Expr) -> R<()> {
        if let ExprKind::Str(s) = &e.kind {
            let s: &str = s;
            let (ptr, len) = (self.iconst(s.as_ptr() as i64), self.iconst(s.len() as i64));
            let f = self.c.helpers.show_str;
            self.call(f, &[self.ctx, ptr, len]);
            return Ok(());
        }
        let v = self.expr(e)?;
        match v {
            Val::S(_, Scalar::None) => self.show_text("none"),
            Val::S(x, s) => {
                let h = &self.c.helpers;
                let f = match s {
                    Scalar::Int => h.show_int,
                    Scalar::Float => h.show_float,
                    _ => h.show_bool,
                };
                self.call(f, &[self.ctx, x]);
            }
            Val::O(addr, _) => {
                let f = self.c.helpers.show_value;
                self.call(f, &[self.ctx, addr]);
            }
        }
        Ok(())
    }

    // ---- expressions ---------------------------------------------------------------------

    fn convert(&mut self, v: ir::Value, from: Scalar, to: Scalar) -> ir::Value {
        match (from, to) {
            (Scalar::Int, Scalar::Float) => self.b.ins().fcvt_from_sint(types::F64, v),
            _ => v,
        }
    }

    fn float(&mut self, (v, s): (ir::Value, Scalar)) -> ir::Value {
        self.convert(v, s, Scalar::Float)
    }

    fn global(&mut self, g: hir::GlobalId, ty: &Ty) -> R<Val> {
        let t = self.temp();
        let g = self.i32const(g.0);
        let get = self.c.helpers.global_get;
        self.call(get, &[self.ctx, t, g]);
        Ok(self.result(t, ty))
    }

    /// Calls a helper of the form `f(ctx, dst, ...args, span)` that may fail, returning `dst`.
    fn helper_into(
        &mut self,
        f: ClifFunc,
        args: &[ir::Value],
        span: Span,
        runs_user_code: bool,
    ) -> ir::Value {
        if runs_user_code {
            self.publish_depth();
        }
        let t = self.temp();
        let span = self.span(span);
        let mut all = vec![self.ctx, t];
        all.extend_from_slice(args);
        all.push(span);
        self.call(f, &all);
        self.check_status();
        t
    }

    fn boxed_all(&mut self, exprs: &[hir::Expr]) -> R<Vec<ir::Value>> {
        let mut out = Vec::with_capacity(exprs.len());
        for e in exprs {
            let v = self.expr(e)?;
            out.push(self.boxed(v));
        }
        Ok(out)
    }

    fn expr(&mut self, e: &hir::Expr) -> R<Val> {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Int(i) => Val::S(self.iconst(*i), Scalar::Int),
            ExprKind::Float(f) => Val::S(self.b.ins().f64const(*f), Scalar::Float),
            ExprKind::Bool(v) => {
                Val::S(self.b.ins().iconst(types::I8, i64::from(*v)), Scalar::Bool)
            }
            ExprKind::None => Val::S(self.iconst(0), Scalar::None),
            ExprKind::Str(s) => {
                let t = self.temp();
                let ptr = self.iconst(s as *const Arc<str> as i64);
                let set = self.c.helpers.set_str;
                self.call(set, &[t, ptr]);
                Val::O(t, true)
            }
            ExprKind::FStr(parts) => {
                let begin = self.c.helpers.fstr_begin;
                self.call(begin, &[self.ctx]);
                for part in parts {
                    match part {
                        hir::FStrPart::Lit(s) => {
                            let (ptr, len) = self.text(s);
                            let lit = self.c.helpers.fstr_lit;
                            self.call(lit, &[self.ctx, ptr, len]);
                        }
                        hir::FStrPart::Expr { expr, spec } => {
                            let v = self.expr(expr)?;
                            let addr = self.boxed(v);
                            let (ptr, len) = match spec {
                                Some(spec) => self.text(spec),
                                None => (self.iconst(0), self.iconst(0)),
                            };
                            let span = self.span(expr.span);
                            let value = self.c.helpers.fstr_value;
                            self.call(value, &[self.ctx, addr, ptr, len, span]);
                            self.check_status();
                        }
                    }
                }
                let t = self.temp();
                let end = self.c.helpers.fstr_end;
                self.call(end, &[self.ctx, t]);
                Val::O(t, true)
            }
            ExprKind::List(items) | ExprKind::Tuple(items) => {
                let addrs = self.boxed_all(items)?;
                let arr = self.ptr_array(&addrs);
                let n = self.iconst(addrs.len() as i64);
                let t = self.temp();
                let f = if matches!(e.kind, ExprKind::List(_)) {
                    self.c.helpers.make_list
                } else {
                    self.c.helpers.make_tuple
                };
                self.call(f, &[t, arr, n]);
                Val::O(t, true)
            }
            ExprKind::Map(entries) => {
                let mut keys = Vec::new();
                let mut vals = Vec::new();
                for (k, v) in entries {
                    let k = self.expr(k)?;
                    keys.push(self.boxed(k));
                    let v = self.expr(v)?;
                    vals.push(self.boxed(v));
                }
                let (ka, va) = (self.ptr_array(&keys), self.ptr_array(&vals));
                let n = self.iconst(keys.len() as i64);
                let f = self.c.helpers.make_map;
                let t = self.helper_into(f, &[ka, va, n], span, false);
                Val::O(t, true)
            }
            ExprKind::Local(l) => self.load_local(self.locals[l.0 as usize]),
            ExprKind::Global(g) => match self.private.get(&g.0) {
                Some(local) => self.load_local(*local),
                None => self.global(*g, &e.ty)?,
            },
            ExprKind::Func { func, .. } => self.closure(func.0, &[])?,
            ExprKind::Closure { func, captures } => self.closure(func.0, captures)?,
            ExprKind::IntToFloat(inner) => {
                let v = self.expr(inner)?;
                Val::S(self.scalar(v, Scalar::Float), Scalar::Float)
            }
            ExprKind::Unary {
                op: UnaryOp::Not,
                operand,
            } => {
                let v = self.expr(operand)?;
                let v = self.scalar(v, Scalar::Bool);
                Val::S(self.b.ins().icmp_imm_s(IntCC::Equal, v, 0), Scalar::Bool)
            }
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => match self.expr(operand)? {
                Val::S(v, Scalar::Int) => {
                    let min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                    self.fail_if(min, code::OVERFLOW, span);
                    Val::S(self.b.ins().ineg(v), Scalar::Int)
                }
                Val::S(v, Scalar::Float) => Val::S(self.b.ins().fneg(v), Scalar::Float),
                other => {
                    let a = self.boxed(other);
                    let f = self.c.helpers.negate;
                    let t = self.helper_into(f, &[a], span, false);
                    self.result(t, &e.ty)
                }
            },
            ExprKind::Binary {
                op: op @ (BinOp::And | BinOp::Or),
                lhs,
                rhs,
            } => {
                let result = self.b.declare_var(types::I8);
                let l = self.expr(lhs)?;
                let l = self.scalar(l, Scalar::Bool);
                self.b.def_var(result, l);
                let eval_rhs = self.b.create_block();
                let done = self.b.create_block();
                if *op == BinOp::And {
                    self.b.ins().brif(l, eval_rhs, &[], done, &[]);
                } else {
                    self.b.ins().brif(l, done, &[], eval_rhs, &[]);
                }
                self.b.switch_to_block(eval_rhs);
                let r = self.expr(rhs)?;
                let r = self.scalar(r, Scalar::Bool);
                self.b.def_var(result, r);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Val::S(self.b.use_var(result), Scalar::Bool)
            }
            ExprKind::IfElse {
                cond,
                then,
                otherwise,
            } => {
                let c = self.expr(cond)?;
                let c = self.scalar(c, Scalar::Bool);
                let (then_block, else_block, done) = (
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                );
                let rep = self.rep(&e.ty);
                let dst = matches!(rep, Rep::Obj).then(|| self.temp());
                self.b.ins().brif(c, then_block, &[], else_block, &[]);
                match rep {
                    Rep::Scalar(s) => {
                        let result = self.b.declare_var(s.clif());
                        for (block, branch) in [(then_block, then), (else_block, otherwise)] {
                            self.b.switch_to_block(block);
                            let v = self.expr(branch)?;
                            let v = self.scalar(v, s);
                            self.b.def_var(result, v);
                            self.b.ins().jump(done, &[]);
                        }
                        self.b.switch_to_block(done);
                        Val::S(self.b.use_var(result), s)
                    }
                    Rep::Obj => {
                        let dst = dst.expect("allocated for objects");
                        for (block, branch) in [(then_block, then), (else_block, otherwise)] {
                            self.b.switch_to_block(block);
                            let v = self.expr(branch)?;
                            self.write_slot(dst, v);
                            self.b.ins().jump(done, &[]);
                        }
                        self.b.switch_to_block(done);
                        Val::O(dst, true)
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                self.binary(*op, l, r, &e.ty, span)?
            }
            ExprKind::Call { callee, args } => {
                let f = self.expr(callee)?;
                let f = self.boxed(f);
                let addrs = self.boxed_all(args)?;
                let arr = self.ptr_array(&addrs);
                let n = self.iconst(addrs.len() as i64);
                let call = self.c.helpers.call_value;
                let t = self.helper_into(call, &[f, arr, n], span, true);
                self.result(t, &e.ty)
            }
            ExprKind::CallFunc {
                func, args, inst, ..
            } => self.call_func(*func, args, inst, &e.ty, span)?,
            ExprKind::CallBuiltin { builtin, args } => self.builtin(*builtin, args, &e.ty, span)?,
            ExprKind::MethodCall { recv, name, args }
                if self.fast_method(&recv.ty, name, args.len()) =>
            {
                self.method_fast(recv, name, args, &e.ty, span)?
            }
            ExprKind::MethodCall { recv, name, args } => {
                let r = self.expr(recv)?;
                let r = self.boxed(r);
                let addrs = self.boxed_all(args)?;
                let arr = self.ptr_array(&addrs);
                let n = self.iconst(addrs.len() as i64);
                let (ptr, len) = self.text(name);
                let method = self.c.helpers.method;
                let t = self.helper_into(method, &[r, ptr, len, arr, n], span, true);
                self.result(t, &e.ty)
            }
            ExprKind::New { class, args } => {
                let addrs = self.boxed_all(args)?;
                let arr = self.ptr_array(&addrs);
                let n = self.iconst(addrs.len() as i64);
                let class = self.i32const(class.0);
                let t = self.temp();
                let new = self.c.helpers.new;
                self.call(new, &[t, class, arr, n]);
                Val::O(t, true)
            }
            ExprKind::Field { target, name } => self.field(target, name, &e.ty, span)?,
            ExprKind::Index { target, index } => self.index(target, index, &e.ty)?,
            ExprKind::Slice { target, start, end } => {
                let obj = self.expr(target)?;
                let obj = self.boxed(obj);
                let bound =
                    |l: &mut Self, b: &Option<Box<hir::Expr>>| -> R<(ir::Value, ir::Value)> {
                        Ok(match b {
                            Some(e) => {
                                let v = l.expr(e)?;
                                (l.b.ins().iconst(types::I8, 1), l.scalar(v, Scalar::Int))
                            }
                            None => (l.b.ins().iconst(types::I8, 0), l.iconst(0)),
                        })
                    };
                let (has_start, s) = bound(self, start)?;
                let (has_end, t) = bound(self, end)?;
                let f = self.c.helpers.slice;
                let dst = self.helper_into(f, &[obj, has_start, s, has_end, t], span, false);
                Val::O(dst, true)
            }
            ExprKind::Query {
                target,
                body: Some(body),
                parallel: true,
                ..
            } => {
                let items = self.items(target)?;
                let f = self.expr(body)?;
                let f = self.boxed(f);
                let map = self.c.helpers.parallel_map;
                let t = self.helper_into(map, &[f, items], span, true);
                Val::O(t, true)
            }
            ExprKind::Query {
                op,
                target,
                item,
                body,
                ..
            } => self.query(*op, target, *item, body.as_deref(), &e.ty, span)?,
            ExprKind::RunParallel(items) => {
                let addrs = self.boxed_all(items)?;
                let arr = self.ptr_array(&addrs);
                let n = self.iconst(addrs.len() as i64);
                let run = self.c.helpers.run_parallel;
                let t = self.helper_into(run, &[arr, n], span, true);
                Val::O(t, true)
            }
            ExprKind::Together(closure) => {
                let c = self.expr(closure)?;
                let c = self.boxed(c);
                let f = self.c.helpers.together;
                let t = self.helper_into(f, &[c], span, false);
                Val::O(t, true)
            }
            ExprKind::Await(task) => {
                let task = self.expr(task)?;
                let task = self.boxed(task);
                let f = self.c.helpers.await_task;
                let t = self.helper_into(f, &[task], span, false);
                self.result(t, &e.ty)
            }
            ExprKind::PyImport(_) => {
                self.publish_depth();
                let t = self.temp();
                let ptr = self.iconst(e as *const hir::Expr as i64);
                let f = self.c.helpers.eval_closed;
                self.call(f, &[self.ctx, t, ptr]);
                self.check_status();
                Val::O(t, true)
            }
            other => return Err(format!("uses {}", describe(other))),
        })
    }

    /// Reads a field; `name` must belong to the program so the code can point at it.
    fn field(&mut self, target: &hir::Expr, name: &str, ty: &Ty, span: Span) -> R<Val> {
        let obj = self.expr(target)?;
        let obj = self.boxed(obj);
        let (ptr, len) = self.text(name);
        let field = self.c.helpers.field;
        let t = self.helper_into(field, &[obj, ptr, len], span, false);
        Ok(self.result(t, ty))
    }

    fn is_list(&self, ty: &Ty) -> bool {
        matches!(substitute(ty, &self.subst), Ty::List(_))
    }

    /// Whether a method has a dedicated helper for the receiver's static type.
    fn fast_method(&self, recv: &Ty, name: &str, args: usize) -> bool {
        match (substitute(recv, &self.subst), name, args) {
            (Ty::List(_), "push" | "append", 1) => true,
            (Ty::List(_) | Ty::Str | Ty::Map(..), "length" | "len", 0) => true,
            (Ty::Map(..), "get", 2) | (Ty::Map(..), "has" | "contains", 1) => true,
            _ => false,
        }
    }

    fn method_fast(
        &mut self,
        recv: &hir::Expr,
        name: &str,
        args: &[hir::Expr],
        ty: &Ty,
        span: Span,
    ) -> R<Val> {
        let r = self.expr(recv)?;
        let r = self.boxed(r);
        let mut addrs = Vec::new();
        for a in args {
            let v = self.expr(a)?;
            addrs.push(self.boxed(v));
        }
        let span_v = self.span(span);
        Ok(match name {
            "push" | "append" => {
                let push = self.c.helpers.list_push;
                self.call(push, &[r, addrs[0]]);
                Val::S(self.iconst(0), Scalar::None)
            }
            "length" | "len" => {
                let len = self.c.helpers.len;
                let n = self.call(len, &[self.ctx, r, span_v]).expect("length");
                self.check_status();
                Val::S(n, Scalar::Int)
            }
            "get" => {
                let t = self.temp();
                let get = self.c.helpers.map_get;
                self.call(get, &[self.ctx, t, r, addrs[0], addrs[1], span_v]);
                self.check_status();
                self.result(t, ty)
            }
            _ => {
                let has = self.c.helpers.map_has;
                let found = self
                    .call(has, &[self.ctx, r, addrs[0], span_v])
                    .expect("found");
                self.check_status();
                Val::S(found, Scalar::Bool)
            }
        })
    }

    fn index(&mut self, target: &hir::Expr, index: &hir::Expr, ty: &Ty) -> R<Val> {
        if self.is_list(&target.ty) && self.rep(&index.ty) == Rep::Scalar(Scalar::Int) {
            let list = self.expr(target)?;
            let list = self.boxed(list);
            let i = self.expr(index)?;
            let i = self.scalar(i, Scalar::Int);
            let t = self.temp();
            let span = self.span(index.span);
            let get = self.c.helpers.list_index;
            self.call(get, &[self.ctx, t, list, i, span]);
            self.check_status();
            return Ok(self.result(t, ty));
        }
        let obj = self.expr(target)?;
        let obj = self.boxed(obj);
        let idx = self.expr(index)?;
        let idx = self.boxed(idx);
        let f = self.c.helpers.index;
        let t = self.helper_into(f, &[obj, idx], index.span, false);
        Ok(self.result(t, ty))
    }

    fn closure(&mut self, func: u32, captures: &[hir::Expr]) -> R<Val> {
        let addrs = self.boxed_all(captures)?;
        let arr = self.ptr_array(&addrs);
        let n = self.iconst(addrs.len() as i64);
        let func = self.i32const(func);
        let t = self.temp();
        let f = self.c.helpers.make_closure;
        self.call(f, &[t, func, arr, n]);
        Ok(Val::O(t, true))
    }

    fn call_func(
        &mut self,
        func: hir::FuncId,
        args: &[hir::Expr],
        inst: &[Ty],
        ty: &Ty,
        span: Span,
    ) -> R<Val> {
        let inst: Vec<Ty> = inst.iter().map(|t| substitute(t, &self.subst)).collect();
        let target = if inst.iter().any(super::has_vars) {
            None
        } else {
            self.c.request(self.prog, &(func, inst), self.batch)?
        };
        let Some(target) = target else {
            let addrs = self.boxed_all(args)?;
            let arr = self.ptr_array(&addrs);
            let n = self.iconst(addrs.len() as i64);
            let id = self.i32const(func.0);
            let f = self.c.helpers.call_interp;
            let t = self.helper_into(f, &[id, arr, n], span, true);
            return Ok(self.result(t, ty));
        };
        let mut vals = vec![self.ctx, self.depth];
        let result_slot = (target.ret == Rep::Obj).then(|| self.temp());
        if let Some(slot) = result_slot {
            vals.push(slot);
        }
        for (a, p) in args.iter().zip(&target.params) {
            let v = self.expr(a)?;
            vals.push(match p {
                Rep::Scalar(s) => self.scalar(v, *s),
                Rep::Obj => self.boxed(v),
            });
        }
        let result = self
            .call(target.id, &vals)
            .expect("compiled functions return a value");
        self.check_status();
        Ok(match (target.ret, result_slot) {
            (Rep::Scalar(s), _) => Val::S(result, s),
            (Rep::Obj, Some(slot)) => self.result(slot, ty),
            (Rep::Obj, None) => unreachable!("object results have a slot"),
        })
    }

    fn binary(&mut self, op: BinOp, l: Val, r: Val, ty: &Ty, span: Span) -> R<Val> {
        if let (Val::S(a, sa), Val::S(b, sb)) = (l, r)
            && let Some(v) = self.scalar_binary(op, (a, sa), (b, sb), span)
        {
            return Ok(v);
        }
        let a = self.boxed(l);
        let b = self.boxed(r);
        let op = self.i32const(op.index());
        let f = self.c.helpers.binary;
        let t = self.helper_into(f, &[op, a, b], span, false);
        Ok(self.result(t, ty))
    }

    /// Arithmetic and comparisons on scalars; `None` when the operands need the generic path.
    fn scalar_binary(
        &mut self,
        op: BinOp,
        l: (ir::Value, Scalar),
        r: (ir::Value, Scalar),
        span: Span,
    ) -> Option<Val> {
        let ints = l.1 == Scalar::Int && r.1 == Scalar::Int;
        let numeric = |s: Scalar| matches!(s, Scalar::Int | Scalar::Float);
        let comparison = matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        if !comparison && !(numeric(l.1) && numeric(r.1)) {
            return None;
        }
        Some(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul if ints => {
                let (v, of) = match op {
                    BinOp::Add => self.b.ins().sadd_overflow(l.0, r.0),
                    BinOp::Sub => self.b.ins().ssub_overflow(l.0, r.0),
                    _ => self.b.ins().smul_overflow(l.0, r.0),
                };
                self.fail_if(of, code::OVERFLOW, span);
                Val::S(v, Scalar::Int)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let (a, b) = (self.float(l), self.float(r));
                let v = match op {
                    BinOp::Add => self.b.ins().fadd(a, b),
                    BinOp::Sub => self.b.ins().fsub(a, b),
                    _ => self.b.ins().fmul(a, b),
                };
                Val::S(v, Scalar::Float)
            }
            BinOp::Div => {
                let (a, b) = (self.float(l), self.float(r));
                let zero = self.b.ins().f64const(0.0);
                let is_zero = self.b.ins().fcmp(FloatCC::Equal, b, zero);
                self.fail_if(is_zero, code::DIV_ZERO, span);
                Val::S(self.b.ins().fdiv(a, b), Scalar::Float)
            }
            BinOp::FloorDiv | BinOp::Mod if ints => {
                let (a, b) = (l.0, r.0);
                let is_zero = self.b.ins().icmp_imm_s(IntCC::Equal, b, 0);
                self.fail_if(is_zero, code::DIV_ZERO, span);
                let a_min = self.b.ins().icmp_imm_s(IntCC::Equal, a, i64::MIN);
                let b_neg1 = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                let edge = self.b.ins().band(a_min, b_neg1);
                if op == BinOp::FloorDiv {
                    self.fail_if(edge, code::OVERFLOW, span);
                    let q = self.b.ins().sdiv(a, b);
                    let rem = self.b.ins().srem(a, b);
                    let adjust = self.sign_differs(rem, b);
                    let q1 = self.b.ins().iadd_imm_s(q, -1);
                    Val::S(self.b.ins().select(adjust, q1, q), Scalar::Int)
                } else {
                    let one = self.iconst(1);
                    let safe_b = self.b.ins().select(edge, one, b);
                    let rem = self.b.ins().srem(a, safe_b);
                    let adjust = self.sign_differs(rem, b);
                    let fixed = self.b.ins().iadd(rem, b);
                    Val::S(self.b.ins().select(adjust, fixed, rem), Scalar::Int)
                }
            }
            BinOp::FloorDiv | BinOp::Mod => {
                let (a, b) = (self.float(l), self.float(r));
                let zero = self.b.ins().f64const(0.0);
                let is_zero = self.b.ins().fcmp(FloatCC::Equal, b, zero);
                self.fail_if(is_zero, code::DIV_ZERO, span);
                if op == BinOp::FloorDiv {
                    let q = self.b.ins().fdiv(a, b);
                    Val::S(self.b.ins().floor(q), Scalar::Float)
                } else {
                    let f = self.c.helpers.fmod;
                    Val::S(self.call(f, &[a, b]).unwrap(), Scalar::Float)
                }
            }
            BinOp::Pow if ints => {
                let span = self.span(span);
                let f = self.c.helpers.ipow;
                let v = self.call(f, &[self.ctx, l.0, r.0, span]).unwrap();
                self.check_status();
                Val::S(v, Scalar::Int)
            }
            BinOp::Pow => {
                let (a, b) = (self.float(l), self.float(r));
                let f = self.c.helpers.powf;
                Val::S(self.call(f, &[a, b]).unwrap(), Scalar::Float)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                return self.compare(op, l, r);
            }
            BinOp::And | BinOp::Or | BinOp::In | BinOp::MatMul => return None,
        })
    }

    /// Whether `rem` is non-zero and has the opposite sign of `divisor`.
    fn sign_differs(&mut self, rem: ir::Value, divisor: ir::Value) -> ir::Value {
        let nonzero = self.b.ins().icmp_imm_s(IntCC::NotEqual, rem, 0);
        let x = self.b.ins().bxor(rem, divisor);
        let opposite = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, x, 0);
        self.b.ins().band(nonzero, opposite)
    }

    fn compare(
        &mut self,
        op: BinOp,
        l: (ir::Value, Scalar),
        r: (ir::Value, Scalar),
    ) -> Option<Val> {
        let v = match (l.1, r.1) {
            (Scalar::None, Scalar::None) => self.b.ins().iconst(
                types::I8,
                i64::from(matches!(op, BinOp::Eq | BinOp::Le | BinOp::Ge)),
            ),
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
            (a, b)
                if matches!(a, Scalar::Int | Scalar::Float)
                    && matches!(b, Scalar::Int | Scalar::Float) =>
            {
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
            _ => return None,
        };
        Some(Val::S(v, Scalar::Bool))
    }

    fn builtin(&mut self, b: Builtin, args: &[hir::Expr], ty: &Ty, span: Span) -> R<Val> {
        if b == Builtin::Len
            && matches!(
                substitute(&args[0].ty, &self.subst),
                Ty::List(_) | Ty::Str | Ty::Map(..) | Ty::Tuple(_)
            )
        {
            let v = self.expr(&args[0])?;
            let v = self.boxed(v);
            let span = self.span(span);
            let len = self.c.helpers.len;
            let n = self.call(len, &[self.ctx, v, span]).expect("length");
            self.check_status();
            return Ok(Val::S(n, Scalar::Int));
        }
        let all_scalar = args
            .iter()
            .all(|a| matches!(self.rep(&a.ty), Rep::Scalar(_)));
        if (all_scalar || b == Builtin::Assert)
            && let Some(v) = self.scalar_builtin(b, args, span)?
        {
            return Ok(v);
        }
        let addrs = self.boxed_all(args)?;
        let arr = self.ptr_array(&addrs);
        let n = self.iconst(addrs.len() as i64);
        let id = self.i32const(b.index());
        let f = self.c.helpers.builtin;
        let t = self.helper_into(f, &[id, arr, n], span, true);
        Ok(self.result(t, ty))
    }

    /// Builtins compiled inline on scalars; `None` when the generic path is needed.
    fn scalar_builtin(&mut self, b: Builtin, args: &[hir::Expr], span: Span) -> R<Option<Val>> {
        let inline = matches!(
            b,
            Builtin::Abs
                | Builtin::Min
                | Builtin::Max
                | Builtin::ToFloat
                | Builtin::ToInt
                | Builtin::ToBool
                | Builtin::Round
                | Builtin::MathPi
                | Builtin::MathE
                | Builtin::MathSqrt
                | Builtin::MathFloor
                | Builtin::MathCeil
                | Builtin::MathSin
                | Builtin::MathCos
                | Builtin::MathTan
                | Builtin::MathLog
                | Builtin::MathExp
                | Builtin::MathPow
                | Builtin::TimeNow
                | Builtin::TimeNowMs
                | Builtin::Assert
        );
        if !inline || (matches!(b, Builtin::Min | Builtin::Max) && args.len() < 2) {
            return Ok(None);
        }
        if b == Builtin::Assert
            && !matches!(args.get(1).map(|a| &a.kind), None | Some(ExprKind::Str(_)))
        {
            return Ok(None);
        }
        let lowered = if b == Builtin::Assert {
            &args[..1]
        } else {
            args
        };
        let mut vals = Vec::with_capacity(lowered.len());
        for a in lowered {
            match self.expr(a)? {
                Val::S(x, s) => vals.push((x, s)),
                Val::O(addr, owned) if b == Builtin::Assert => {
                    vals.push((self.scalar(Val::O(addr, owned), Scalar::Bool), Scalar::Bool))
                }
                Val::O(..) => return Err("internal: object argument to a scalar builtin".into()),
            }
        }
        let f64_of = |me: &mut Self, i: usize| me.float(vals[i]);
        Ok(Some(match b {
            Builtin::Abs => match vals[0].1 {
                Scalar::Int => {
                    let v = vals[0].0;
                    let min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                    self.fail_if(min, code::OVERFLOW, span);
                    let neg = self.b.ins().ineg(v);
                    let is_neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, v, 0);
                    Val::S(self.b.ins().select(is_neg, neg, v), Scalar::Int)
                }
                _ => {
                    let v = f64_of(self, 0);
                    Val::S(self.b.ins().fabs(v), Scalar::Float)
                }
            },
            Builtin::Min | Builtin::Max => {
                let s = vals[0].1;
                let mut acc = vals[0].0;
                for (v, vs) in &vals[1..] {
                    if *vs != s {
                        return Ok(None);
                    }
                    let take = match (s, b) {
                        (Scalar::Int, Builtin::Max) => {
                            self.b.ins().icmp(IntCC::SignedGreaterThan, *v, acc)
                        }
                        (Scalar::Int, _) => self.b.ins().icmp(IntCC::SignedLessThan, *v, acc),
                        (Scalar::Float, Builtin::Max) => {
                            self.b.ins().fcmp(FloatCC::GreaterThan, *v, acc)
                        }
                        (Scalar::Float, _) => self.b.ins().fcmp(FloatCC::LessThan, *v, acc),
                        _ => return Ok(None),
                    };
                    acc = self.b.ins().select(take, *v, acc);
                }
                Val::S(acc, s)
            }
            Builtin::ToFloat => match vals[0].1 {
                Scalar::Bool => {
                    let w = self.b.ins().uextend(types::I64, vals[0].0);
                    Val::S(self.b.ins().fcvt_from_sint(types::F64, w), Scalar::Float)
                }
                Scalar::Int | Scalar::Float => Val::S(f64_of(self, 0), Scalar::Float),
                Scalar::None => return Ok(None),
            },
            Builtin::ToInt => match vals[0].1 {
                Scalar::Int => Val::S(vals[0].0, Scalar::Int),
                Scalar::Bool => Val::S(self.b.ins().uextend(types::I64, vals[0].0), Scalar::Int),
                Scalar::Float => {
                    let v = vals[0].0;
                    let diff = self.b.ins().fsub(v, v);
                    let finite = self.b.ins().fcmp(FloatCC::Equal, diff, diff);
                    let not_finite = self.b.ins().icmp_imm_s(IntCC::Equal, finite, 0);
                    self.fail_if(not_finite, code::FLOAT_TO_INT, span);
                    let t = self.b.ins().trunc(v);
                    Val::S(self.b.ins().fcvt_to_sint_sat(types::I64, t), Scalar::Int)
                }
                Scalar::None => return Ok(None),
            },
            Builtin::ToBool => match vals[0].1 {
                Scalar::Bool => Val::S(vals[0].0, Scalar::Bool),
                Scalar::Int => Val::S(
                    self.b.ins().icmp_imm_s(IntCC::NotEqual, vals[0].0, 0),
                    Scalar::Bool,
                ),
                Scalar::Float => {
                    let zero = self.b.ins().f64const(0.0);
                    Val::S(
                        self.b.ins().fcmp(FloatCC::NotEqual, vals[0].0, zero),
                        Scalar::Bool,
                    )
                }
                Scalar::None => Val::S(self.b.ins().iconst(types::I8, 0), Scalar::Bool),
            },
            Builtin::Round if vals.len() == 1 => {
                let v = f64_of(self, 0);
                let r = self.b.ins().nearest(v);
                Val::S(self.b.ins().fcvt_to_sint_sat(types::I64, r), Scalar::Int)
            }
            Builtin::Round => {
                let v = f64_of(self, 0);
                let f = self.c.helpers.round_digits;
                Val::S(self.call(f, &[v, vals[1].0]).unwrap(), Scalar::Float)
            }
            Builtin::MathPi => Val::S(self.b.ins().f64const(std::f64::consts::PI), Scalar::Float),
            Builtin::MathE => Val::S(self.b.ins().f64const(std::f64::consts::E), Scalar::Float),
            Builtin::MathSqrt => {
                let v = f64_of(self, 0);
                Val::S(self.b.ins().sqrt(v), Scalar::Float)
            }
            Builtin::MathFloor | Builtin::MathCeil => {
                let v = f64_of(self, 0);
                let r = if b == Builtin::MathFloor {
                    self.b.ins().floor(v)
                } else {
                    self.b.ins().ceil(v)
                };
                Val::S(self.b.ins().fcvt_to_sint_sat(types::I64, r), Scalar::Int)
            }
            Builtin::MathSin
            | Builtin::MathCos
            | Builtin::MathTan
            | Builtin::MathLog
            | Builtin::MathExp => {
                let v = f64_of(self, 0);
                let op = match b {
                    Builtin::MathSin => math::SIN,
                    Builtin::MathCos => math::COS,
                    Builtin::MathTan => math::TAN,
                    Builtin::MathLog => math::LOG,
                    _ => math::EXP,
                };
                let op = self.i32const(op);
                let f = self.c.helpers.math;
                Val::S(self.call(f, &[op, v]).unwrap(), Scalar::Float)
            }
            Builtin::MathPow => {
                let (x, y) = (f64_of(self, 0), f64_of(self, 1));
                let f = self.c.helpers.powf;
                Val::S(self.call(f, &[x, y]).unwrap(), Scalar::Float)
            }
            Builtin::TimeNow => {
                let f = self.c.helpers.time_now;
                Val::S(self.call(f, &[]).unwrap(), Scalar::Float)
            }
            Builtin::TimeNowMs => {
                let f = self.c.helpers.time_now_ms;
                Val::S(self.call(f, &[]).unwrap(), Scalar::Int)
            }
            Builtin::Assert => {
                let message = match args.get(1).map(|a| &a.kind) {
                    Some(ExprKind::Str(m)) => format!("assertion failed: {m}"),
                    _ => "assertion failed".to_string(),
                };
                let text: &'static str = Box::leak(message.into_boxed_str());
                let raise = self.b.create_block();
                let ok = self.b.create_block();
                self.b.ins().brif(vals[0].0, ok, &[], raise, &[]);
                self.b.switch_to_block(raise);
                let (ptr, len) = self.text(text);
                let span = self.span(span);
                let f = self.c.helpers.fail_msg;
                self.call(f, &[self.ctx, ptr, len, span]);
                self.b.ins().jump(self.fail, &[]);
                self.b.switch_to_block(ok);
                Val::S(self.iconst(0), Scalar::None)
            }
            _ => return Ok(None),
        }))
    }

    /// Lowers `where`, `select` and the aggregates as loops over the items.
    fn query(
        &mut self,
        op: QueryOp,
        target: &hir::Expr,
        item: hir::LocalId,
        body: Option<&hir::Expr>,
        ty: &Ty,
        span: Span,
    ) -> R<Val> {
        let items = self.items(target)?;
        let n = self.list_len(items);
        let local = self.locals[item.0 as usize];
        let collected = matches!(
            op,
            QueryOp::Where | QueryOp::Select | QueryOp::Min | QueryOp::Max
        );
        let out = if collected {
            let t = self.temp();
            let arr = self.ptr_array(&[]);
            let zero = self.iconst(0);
            let make = self.c.helpers.make_list;
            self.call(make, &[t, arr, zero]);
            Some(t)
        } else {
            None
        };
        let sum_rep = match op {
            QueryOp::Sum => self.rep(ty),
            QueryOp::Average => self.element_rep(target, body),
            _ => Rep::Obj,
        };
        let acc_var = match sum_rep {
            Rep::Scalar(s @ (Scalar::Int | Scalar::Float))
                if matches!(op, QueryOp::Sum | QueryOp::Average) =>
            {
                let var = self.b.declare_var(s.clif());
                let z = zero(&mut self.b, Rep::Scalar(s));
                self.b.def_var(var, z);
                Some((var, s))
            }
            _ => None,
        };
        let acc_slot = if matches!(op, QueryOp::Sum | QueryOp::Average) && acc_var.is_none() {
            let t = self.temp();
            let h = &self.c.helpers;
            if *ty == Ty::Float && op == QueryOp::Sum {
                let (f, z) = (h.set_float, self.b.ins().f64const(0.0));
                self.call(f, &[t, z]);
            } else {
                let (f, z) = (h.set_int, self.iconst(0));
                self.call(f, &[t, z]);
            }
            Some(t)
        } else {
            None
        };

        let zero_i = self.iconst(0);
        let element = self.temp();
        let marker = Place::Local(hir::LocalId(u32::MAX));
        let empty_body: hir::Block = Vec::new();
        self.counted_loop(
            zero_i,
            n,
            None,
            &marker,
            span,
            |l, i| {
                let get = l.c.helpers.list_get;
                l.call(get, &[element, items, i]);
                l.store_local(local, Val::O(element, true));
                let value = match body {
                    Some(b) => l.expr(b)?,
                    None => l.load_local(local),
                };
                match op {
                    QueryOp::Where => {
                        let keep = l.scalar(value, Scalar::Bool);
                        let yes = l.b.create_block();
                        let no = l.b.create_block();
                        l.b.ins().brif(keep, yes, &[], no, &[]);
                        l.b.switch_to_block(yes);
                        let current = l.load_local(local);
                        let addr = l.boxed(current);
                        let push = l.c.helpers.list_push;
                        l.call(push, &[out.unwrap(), addr]);
                        l.b.ins().jump(no, &[]);
                        l.b.switch_to_block(no);
                    }
                    QueryOp::Select | QueryOp::Min | QueryOp::Max => {
                        let addr = l.boxed(value);
                        let push = l.c.helpers.list_push;
                        l.call(push, &[out.unwrap(), addr]);
                    }
                    QueryOp::Sum | QueryOp::Average => match (acc_var, acc_slot) {
                        (Some((var, s)), _) => {
                            let acc = l.b.use_var(var);
                            let item = l.scalar(value, s);
                            let next = l
                                .scalar_binary(BinOp::Add, (acc, s), (item, s), span)
                                .unwrap_or(Val::S(acc, s));
                            let x = l.scalar(next, s);
                            l.b.def_var(var, x);
                        }
                        (None, Some(slot)) => {
                            let addr = l.boxed(value);
                            let op = l.i32const(BinOp::Add.index());
                            let f = l.c.helpers.binary;
                            let t = l.helper_into(f, &[op, slot, addr], span, false);
                            let take = l.c.helpers.take;
                            l.call(take, &[slot, t]);
                        }
                        _ => unreachable!("sums have an accumulator"),
                    },
                }
                Ok(())
            },
            &empty_body,
        )?;

        Ok(match op {
            QueryOp::Where | QueryOp::Select => Val::O(out.unwrap(), true),
            QueryOp::Min | QueryOp::Max => {
                let arr = self.ptr_array(&[out.unwrap()]);
                let one = self.iconst(1);
                let b = if op == QueryOp::Max {
                    Builtin::Max
                } else {
                    Builtin::Min
                };
                let id = self.i32const(b.index());
                let f = self.c.helpers.builtin;
                let t = self.helper_into(f, &[id, arr, one], span, false);
                self.result(t, ty)
            }
            QueryOp::Sum => match (acc_var, acc_slot) {
                (Some((var, s)), _) => Val::S(self.b.use_var(var), s),
                (None, Some(slot)) => self.result(slot, ty),
                _ => unreachable!(),
            },
            QueryOp::Average => {
                let empty = self.b.ins().icmp_imm_s(IntCC::Equal, n, 0);
                let raise = self.b.create_block();
                let ok = self.b.create_block();
                self.b.ins().brif(empty, raise, &[], ok, &[]);
                self.b.switch_to_block(raise);
                let (ptr, len) = self.text("average of an empty list");
                let span_v = self.span(span);
                let f = self.c.helpers.fail_msg;
                self.call(f, &[self.ctx, ptr, len, span_v]);
                self.b.ins().jump(self.fail, &[]);
                self.b.switch_to_block(ok);
                let count = self.b.ins().fcvt_from_sint(types::F64, n);
                match (acc_var, acc_slot) {
                    (Some((var, s)), _) => {
                        let acc = self.b.use_var(var);
                        let total = self.float((acc, s));
                        Val::S(self.b.ins().fdiv(total, count), Scalar::Float)
                    }
                    (None, Some(slot)) => {
                        let total = self.scalar(Val::O(slot, false), Scalar::Float);
                        Val::S(self.b.ins().fdiv(total, count), Scalar::Float)
                    }
                    _ => unreachable!(),
                }
            }
        })
    }

    /// The representation of the values a query aggregates.
    fn element_rep(&self, target: &hir::Expr, body: Option<&hir::Expr>) -> Rep {
        match body {
            Some(b) => self.rep(&b.ty),
            None => match substitute(&target.ty, &self.subst) {
                Ty::List(t) => Rep::of(&t),
                _ => Rep::Obj,
            },
        }
    }
}
