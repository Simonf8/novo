//! Reference evaluator: walks the typed HIR directly. It defines the observable
//! behavior that compiled code must match.

pub mod builtins;
mod ops;
mod tensor;
pub mod value;

use crate::check::hir::{self, Block, ExprKind, FuncId, ModuleId, Place, Program, StmtKind};
use crate::check::types::Ty;
use crate::source::Span;
use crate::syntax::ast::{BinOp, QueryOp, UnaryOp};
use parking_lot::{Mutex, RwLock};
use std::cell::Cell;
use std::io::{BufWriter, Stdout, Write};
use std::sync::Arc;
use value::{Closure, Dict, Instance, Key, TaskHandle, Value};

const MAX_DEPTH: usize = 10_000;
const MAIN_STACK: usize = 1 << 30;
const TASK_STACK: usize = 256 << 20;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug)]
pub struct RtError {
    pub message: String,
    pub span: Span,
    /// Calls the error passed through, innermost first: the function and where it was called.
    pub trace: Vec<(String, Span)>,
}

const MAX_TRACE: usize = 24;

impl RtError {
    fn through(mut self, function: &str, call: Span) -> RtError {
        if self.trace.len() < MAX_TRACE {
            self.trace.push((function.to_string(), call));
        }
        self
    }
}

type Eval = Result<Value, RtError>;

enum Flow {
    Next,
    Break,
    Continue,
    Return(Value),
}

#[derive(Clone)]
struct Frame {
    locals: Vec<Value>,
    closure: Option<Arc<Closure>>,
}

#[derive(Clone)]
pub struct Interp {
    prog: Arc<Program>,
    globals: Arc<Vec<RwLock<Value>>>,
    initialized: Arc<Mutex<Vec<bool>>>,
    out: Arc<Mutex<BufWriter<Stdout>>>,
    args: Arc<Vec<String>>,
    native: Option<Arc<crate::native::Native>>,
    tensors: Arc<tensor::State>,
}

/// Runs a checked program on a thread with a large stack, compiling eligible
/// functions to native code when `native` is set.
pub fn run(prog: Program, args: Vec<String>, native: bool) -> Result<(), RtError> {
    let globals = (0..prog.globals.len()).map(|_| RwLock::new(Value::None)).collect();
    let initialized = vec![false; prog.modules.len()];
    let entry = prog.entry;
    let prog = Arc::new(prog);
    let native = if native { crate::native::Native::new(prog.clone()).map(Arc::new) } else { None };
    let interp = Interp {
        prog,
        globals: Arc::new(globals),
        initialized: Arc::new(Mutex::new(initialized)),
        out: Arc::new(Mutex::new(BufWriter::new(std::io::stdout()))),
        args: Arc::new(args),
        native,
        tensors: Arc::default(),
    };
    let handle = std::thread::Builder::new()
        .stack_size(MAIN_STACK)
        .spawn(move || {
            let result = interp.init_module(entry, Span::default());
            interp.flush();
            result
        })
        .expect("spawn the main thread");
    handle.join().unwrap_or_else(|_| Err(RtError { message: "the interpreter panicked".into(), span: Span::default(), trace: Vec::new() }))
}

fn fail<T>(message: impl Into<String>, span: Span) -> Result<T, RtError> {
    Err(RtError { message: message.into(), span, trace: Vec::new() })
}

trait At<T> {
    fn at(self, span: Span) -> Result<T, RtError>;
}

impl<T> At<T> for Result<T, String> {
    fn at(self, span: Span) -> Result<T, RtError> {
        self.map_err(|message| RtError { message, span, trace: Vec::new() })
    }
}

impl Interp {
    pub(crate) fn display(&self, v: &Value) -> String {
        v.display(&self.prog)
    }

    pub(crate) fn write_out(&self, text: &str) {
        let _ = self.out.lock().write_all(text.as_bytes());
    }

    pub(crate) fn flush(&self) {
        let _ = self.out.lock().flush();
    }

    fn init_module(&self, m: ModuleId, span: Span) -> Result<(), RtError> {
        {
            let mut done = self.initialized.lock();
            if done[m.0 as usize] {
                return Ok(());
            }
            done[m.0 as usize] = true;
        }
        let init = self.prog.modules[m.0 as usize].init;
        self.call_func(init, Vec::new(), None, span).map(|_| ())
    }

    fn call_func(&self, fid: FuncId, args: Vec<Value>, closure: Option<Arc<Closure>>, span: Span) -> Eval {
        let f = &self.prog.funcs[fid.0 as usize];
        let depth = DEPTH.with(|d| {
            d.set(d.get() + 1);
            d.get()
        });
        let _guard = DepthGuard;
        if depth > MAX_DEPTH {
            return fail("maximum recursion depth exceeded", span);
        }
        if args.len() > f.params {
            return fail(format!("`{}` takes at most {} arguments but {} were given", f.name, f.params, args.len()), span);
        }
        let mut locals = vec![Value::None; f.locals.len()];
        let given = args.len();
        for (slot, a) in locals.iter_mut().zip(args) {
            *slot = a;
        }
        let first_default = f.params - f.defaults.len();
        for i in given..f.params {
            match f.defaults.get(i.wrapping_sub(first_default)).and_then(Option::as_ref) {
                Some(default) if i >= first_default => {
                    let mut empty = Frame { locals: Vec::new(), closure: None };
                    locals[i] = self.eval(default, &mut empty)?;
                }
                _ => return fail(format!("missing argument `{}` in call to `{}`", f.locals[i].name, f.name), span),
            }
        }
        if let Some(c) = &closure {
            for (slot, v) in f.captures.iter().zip(&c.captures) {
                locals[slot.0 as usize] = v.clone();
            }
        }
        let mut frame = Frame { locals, closure };
        match self.exec_block(&f.body, &mut frame) {
            Ok(Flow::Return(v)) => Ok(v),
            Ok(_) => Ok(Value::None),
            Err(e) if f.name == "<module>" => Err(e),
            Err(e) => Err(e.through(&f.name, span)),
        }
    }

    fn call_native(&self, entry: &crate::native::Entry, args: Vec<Value>, func: FuncId, span: Span) -> Eval {
        use crate::native::Scalar;
        let mut words = Vec::with_capacity(args.len());
        for (v, p) in args.iter().zip(&entry.params) {
            words.push(match (v, p) {
                (Value::Int(i), Scalar::Int) => *i as u64,
                (Value::Float(f), Scalar::Float) => f.to_bits(),
                (Value::Bool(b), Scalar::Bool) => u64::from(*b),
                (Value::None, Scalar::None) => 0,
                _ => return self.call_func(func, args, None, span),
            });
        }
        let depth = DEPTH.with(Cell::get);
        let name = &self.prog.funcs[func.0 as usize].name;
        let word = entry.call(&words, depth, &self.out).map_err(|(message, inner)| RtError { message, span: inner, trace: Vec::new() }.through(name, span))?;
        Ok(match entry.ret {
            Scalar::Int => Value::Int(word as i64),
            Scalar::Float => Value::Float(f64::from_bits(word)),
            Scalar::Bool => Value::Bool(word != 0),
            Scalar::None => Value::None,
        })
    }

    fn call_value(&self, callee: &Value, args: Vec<Value>, span: Span) -> Eval {
        match callee {
            Value::Func(c) => self.call_func(c.func, args, Some(c.clone()), span),
            Value::Py(obj) => crate::python::call(obj, &args).at(span),
            other => fail(format!("a {} cannot be called", other.type_name()), span),
        }
    }

    // ---- statements ----------------------------------------------------------------------

    fn exec_block(&self, block: &Block, frame: &mut Frame) -> Result<Flow, RtError> {
        for stmt in block {
            match self.exec(stmt, frame)? {
                Flow::Next => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Next)
    }

    fn cond(&self, e: &hir::Expr, frame: &mut Frame) -> Result<bool, RtError> {
        match self.eval(e, frame)? {
            Value::Bool(b) => Ok(b),
            other => fail(format!("a condition must be a Bool, found a {}", other.type_name()), e.span),
        }
    }

    fn loop_body(&self, body: &Block, frame: &mut Frame) -> Result<Option<Flow>, RtError> {
        Ok(match self.exec_block(body, frame)? {
            Flow::Break => Some(Flow::Next),
            Flow::Return(v) => Some(Flow::Return(v)),
            Flow::Next | Flow::Continue => None,
        })
    }

    fn exec(&self, stmt: &hir::Stmt, frame: &mut Frame) -> Result<Flow, RtError> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.eval(e, frame)?;
            }
            StmtKind::Assign { place, op: None, value } => {
                let v = self.eval(value, frame)?;
                self.store(place, v, frame)?;
            }
            StmtKind::Assign { place, op: Some(op), value } => self.update(place, *op, value, frame, stmt.span)?,
            StmtKind::Show(values) => {
                let mut line = String::new();
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        line.push(' ');
                    }
                    let v = self.eval(v, frame)?;
                    line.push_str(&self.display(&v));
                }
                line.push('\n');
                self.write_out(&line);
            }
            StmtKind::If { branches, otherwise } => {
                for (cond, body) in branches {
                    if self.cond(cond, frame)? {
                        return self.exec_block(body, frame);
                    }
                }
                return self.exec_block(otherwise, frame);
            }
            StmtKind::While { cond, body } => {
                while self.cond(cond, frame)? {
                    if let Some(flow) = self.loop_body(body, frame)? {
                        return Ok(flow);
                    }
                }
            }
            StmtKind::For { var, iter, body } => {
                let items = self.items(&self.eval(iter, frame)?).at(iter.span)?;
                for item in items {
                    self.store(var, item, frame)?;
                    if let Some(flow) = self.loop_body(body, frame)? {
                        return Ok(flow);
                    }
                }
            }
            StmtKind::Repeat { count, it, body } => {
                let n = match self.eval(count, frame)? {
                    Value::Int(n) => n,
                    other => return fail(format!("`repeat` needs an Int count, found a {}", other.type_name()), count.span),
                };
                for i in 0..n.max(0) {
                    frame.locals[it.0 as usize] = Value::Int(i);
                    if let Some(flow) = self.loop_body(body, frame)? {
                        return Ok(flow);
                    }
                }
            }
            StmtKind::Return(value) => {
                let v = match value {
                    Some(e) => self.eval(e, frame)?,
                    None => Value::None,
                };
                return Ok(Flow::Return(v));
            }
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Try { body, var, handler } => match self.exec_block(body, frame) {
                Ok(flow) => return Ok(flow),
                Err(e) => {
                    self.store(var, Value::Str(e.message.into()), frame)?;
                    return self.exec_block(handler, frame);
                }
            },
            StmtKind::InitModule(m) => self.init_module(*m, stmt.span)?,
        }
        Ok(Flow::Next)
    }

    fn items(&self, v: &Value) -> Result<Vec<Value>, String> {
        match v {
            Value::List(l) => Ok(l.read().clone()),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string().into())).collect()),
            Value::Map(m) => Ok(m.read().iter().map(|(k, _)| k.to_value()).collect()),
            Value::Py(obj) => crate::python::to_list(obj),
            Value::Tensor(t) => tensor::rows(t),
            other => Err(format!("cannot iterate over a {}", other.type_name())),
        }
    }

    fn store(&self, place: &Place, v: Value, frame: &mut Frame) -> Result<(), RtError> {
        match place {
            Place::Local(l) => frame.locals[l.0 as usize] = v,
            Place::Global(g) => *self.globals[g.0 as usize].write() = v,
            Place::Field { target, name } => {
                let obj = self.eval(target, frame)?;
                self.set_field(&obj, name, v).at(target.span)?;
            }
            Place::Index { target, index } => {
                let obj = self.eval(target, frame)?;
                let idx = self.eval(index, frame)?;
                self.set_index(&obj, &idx, v).at(index.span)?;
            }
        }
        Ok(())
    }

    fn update(&self, place: &Place, op: BinOp, value: &hir::Expr, frame: &mut Frame, span: Span) -> Result<(), RtError> {
        match place {
            Place::Local(l) => {
                let rhs = self.eval(value, frame)?;
                let cur = &frame.locals[l.0 as usize];
                frame.locals[l.0 as usize] = self.binary_values(op, cur, &rhs).at(span)?;
            }
            Place::Global(g) => {
                let rhs = self.eval(value, frame)?;
                let mut slot = self.globals[g.0 as usize].write();
                *slot = self.binary_values(op, &slot, &rhs).at(span)?;
            }
            Place::Field { target, name } => {
                let obj = self.eval(target, frame)?;
                let cur = self.get_field(&obj, name).at(target.span)?;
                let rhs = self.eval(value, frame)?;
                let new = self.binary_values(op, &cur, &rhs).at(span)?;
                self.set_field(&obj, name, new).at(target.span)?;
            }
            Place::Index { target, index } => {
                let obj = self.eval(target, frame)?;
                let idx = self.eval(index, frame)?;
                let cur = self.get_index(&obj, &idx).at(index.span)?;
                let rhs = self.eval(value, frame)?;
                let new = self.binary_values(op, &cur, &rhs).at(span)?;
                self.set_index(&obj, &idx, new).at(index.span)?;
            }
        }
        Ok(())
    }

    // ---- element access ------------------------------------------------------------------

    fn get_field(&self, obj: &Value, name: &str) -> Result<Value, String> {
        match obj {
            Value::Instance(inst) => {
                let class = &self.prog.classes[inst.class.0 as usize];
                let i = class.field_index(name).ok_or_else(|| format!("`{}` has no field `{name}`", class.name))?;
                Ok(inst.fields.read()[i].clone())
            }
            Value::Map(m) => m.read().get(&Key::Str(name.into())).cloned().ok_or_else(|| format!("key `{name}` is not in the map")),
            Value::Py(p) => crate::python::get_attr(p, name),
            other => Err(format!("a {} has no field `{name}`", other.type_name())),
        }
    }

    fn set_field(&self, obj: &Value, name: &str, v: Value) -> Result<(), String> {
        match obj {
            Value::Instance(inst) => {
                let class = &self.prog.classes[inst.class.0 as usize];
                let i = class.field_index(name).ok_or_else(|| format!("`{}` has no field `{name}`", class.name))?;
                inst.fields.write()[i] = v;
                Ok(())
            }
            Value::Map(m) => {
                m.write().insert(Key::Str(name.into()), v);
                Ok(())
            }
            Value::Py(p) => crate::python::set_attr(p, name, &v),
            other => Err(format!("a {} has no field `{name}`", other.type_name())),
        }
    }

    fn get_index(&self, obj: &Value, idx: &Value) -> Result<Value, String> {
        match (obj, idx) {
            (Value::List(l), Value::Int(i)) => {
                let l = l.read();
                Ok(l[builtins::index(*i, l.len())?].clone())
            }
            (Value::Str(s), Value::Int(i)) => {
                let len = s.chars().count();
                let at = builtins::index(*i, len)?;
                Ok(Value::Str(s.chars().nth(at).expect("index in range").to_string().into()))
            }
            (Value::Map(m), k) => {
                let key = Key::from_value(k)?;
                m.read().get(&key).cloned().ok_or_else(|| format!("key {} is not in the map", self.display(k)))
            }
            (Value::Py(p), k) => crate::python::get_item(p, k),
            (Value::Tensor(t), Value::Int(i)) => tensor::index(t, *i),
            (o, i) => Err(format!("cannot index a {} with a {}", o.type_name(), i.type_name())),
        }
    }

    fn set_index(&self, obj: &Value, idx: &Value, v: Value) -> Result<(), String> {
        match (obj, idx) {
            (Value::List(l), Value::Int(i)) => {
                let mut l = l.write();
                let at = builtins::index(*i, l.len())?;
                l[at] = v;
                Ok(())
            }
            (Value::Map(m), k) => {
                m.write().insert(Key::from_value(k)?, v);
                Ok(())
            }
            (Value::Py(p), k) => crate::python::set_item(p, k, &v),
            (o, i) => Err(format!("cannot assign into a {} with a {} index", o.type_name(), i.type_name())),
        }
    }

    fn binary_values(&self, op: BinOp, a: &Value, b: &Value) -> Result<Value, String> {
        if matches!(a, Value::Py(_)) || matches!(b, Value::Py(_)) {
            return crate::python::binary_op(a, b, op.symbol());
        }
        if matches!(a, Value::Tensor(_)) || matches!(b, Value::Tensor(_)) {
            return tensor::binary(op, a, b);
        }
        ops::binary(op, a, b)
    }

    // ---- expressions ---------------------------------------------------------------------

    fn eval_all(&self, exprs: &[hir::Expr], frame: &mut Frame) -> Result<Vec<Value>, RtError> {
        exprs.iter().map(|e| self.eval(e, frame)).collect()
    }

    fn eval(&self, e: &hir::Expr, frame: &mut Frame) -> Eval {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Int(i) => Value::Int(*i),
            ExprKind::Float(f) => Value::Float(*f),
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Str(s) => Value::Str(s.clone()),
            ExprKind::None => Value::None,
            ExprKind::FStr(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        hir::FStrPart::Lit(s) => out.push_str(s),
                        hir::FStrPart::Expr { expr, spec } => {
                            let v = self.eval(expr, frame)?;
                            let text = self.display(&v);
                            match spec {
                                Some(spec) => out.push_str(&builtins::format_spec(text, &v, spec).at(expr.span)?),
                                None => out.push_str(&text),
                            }
                        }
                    }
                }
                Value::Str(out.into())
            }
            ExprKind::List(items) => Value::list(self.eval_all(items, frame)?),
            ExprKind::Map(entries) => {
                let mut dict = Dict::default();
                for (k, v) in entries {
                    let key = self.eval(k, frame)?;
                    let key = Key::from_value(&key).at(k.span)?;
                    dict.insert(key, self.eval(v, frame)?);
                }
                Value::map(dict)
            }
            ExprKind::Local(l) => frame.locals[l.0 as usize].clone(),
            ExprKind::Global(g) => self.globals[g.0 as usize].read().clone(),
            ExprKind::Func { func, .. } => Value::Func(Arc::new(Closure { func: *func, captures: Vec::new() })),
            ExprKind::Closure { func, captures } => Value::Func(Arc::new(Closure { func: *func, captures: self.eval_all(captures, frame)? })),
            ExprKind::Recurse => match &frame.closure {
                Some(c) => Value::Func(c.clone()),
                None => return fail("no enclosing function to recurse into", span),
            },
            ExprKind::Unary { op, operand } => {
                let v = self.eval(operand, frame)?;
                match op {
                    UnaryOp::Not => match v {
                        Value::Bool(b) => Value::Bool(!b),
                        other => return fail(format!("`not` needs a Bool, found a {}", other.type_name()), span),
                    },
                    UnaryOp::Neg => match &v {
                        Value::Tensor(t) => tensor::negate(t).at(span)?,
                        _ => ops::negate(&v).at(span)?,
                    },
                }
            }
            ExprKind::Binary { op: BinOp::And, lhs, rhs } => Value::Bool(self.cond(lhs, frame)? && self.cond(rhs, frame)?),
            ExprKind::Binary { op: BinOp::Or, lhs, rhs } => Value::Bool(self.cond(lhs, frame)? || self.cond(rhs, frame)?),
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.eval(lhs, frame)?;
                let b = self.eval(rhs, frame)?;
                self.binary_values(*op, &a, &b).at(span)?
            }
            ExprKind::Call { callee, args } => {
                let f = self.eval(callee, frame)?;
                let args = self.eval_all(args, frame)?;
                self.call_value(&f, args, span)?
            }
            ExprKind::CallFunc { func, args, inst, native } => {
                let args = self.eval_all(args, frame)?;
                let entry = self.native.as_ref().and_then(|n| native.0.get_or_init(|| n.entry(*func, inst)).as_ref());
                match entry {
                    Some(entry) => self.call_native(entry, args, *func, span)?,
                    None => self.call_func(*func, args, None, span)?,
                }
            }
            ExprKind::CallBuiltin { builtin, args } => {
                let args = self.eval_all(args, frame)?;
                self.call_builtin(*builtin, args).at(span)?
            }
            ExprKind::MethodCall { recv, name, args } => {
                let obj = self.eval(recv, frame)?;
                let args = self.eval_all(args, frame)?;
                self.call_method(&obj, name, args, span)?
            }
            ExprKind::New { class, args } => {
                let fields = self.eval_all(args, frame)?;
                Value::Instance(Arc::new(Instance { class: *class, fields: RwLock::new(fields) }))
            }
            ExprKind::Field { target, name } => {
                let obj = self.eval(target, frame)?;
                self.get_field(&obj, name).at(span)?
            }
            ExprKind::Index { target, index } => {
                let obj = self.eval(target, frame)?;
                let idx = self.eval(index, frame)?;
                self.get_index(&obj, &idx).at(index.span)?
            }
            ExprKind::Slice { target, start, end } => {
                let obj = self.eval(target, frame)?;
                let bound = |b: &Option<Box<hir::Expr>>, frame: &mut Frame| -> Result<Option<i64>, RtError> {
                    match b {
                        None => Ok(None),
                        Some(e) => match self.eval(e, frame)? {
                            Value::Int(i) => Ok(Some(i)),
                            other => fail(format!("slice bounds must be Ints, found a {}", other.type_name()), e.span),
                        },
                    }
                };
                let (s, t) = (bound(start, frame)?, bound(end, frame)?);
                match obj {
                    Value::List(l) => {
                        let l = l.read();
                        let (a, b) = builtins::bounds(s, t, l.len());
                        Value::list(l[a..b].to_vec())
                    }
                    Value::Str(text) => {
                        let chars: Vec<char> = text.chars().collect();
                        let (a, b) = builtins::bounds(s, t, chars.len());
                        Value::Str(chars[a..b].iter().collect::<String>().into())
                    }
                    Value::Tensor(tensor) => tensor::slice(&tensor, s, t).at(span)?,
                    other => return fail(format!("cannot slice a {}", other.type_name()), span),
                }
            }
            ExprKind::Query { op, target, item, body, parallel } => {
                let items = self.items(&self.eval(target, frame)?).at(target.span)?;
                self.query(*op, items, *item, body.as_deref(), *parallel, &e.ty, frame, span)?
            }
            ExprKind::Together(closure) => {
                let Value::Func(c) = self.eval(closure, frame)? else { unreachable!("together wraps a closure") };
                let handle = Arc::new(TaskHandle::default());
                let (interp, task) = (self.clone(), handle.clone());
                std::thread::Builder::new()
                    .stack_size(TASK_STACK)
                    .spawn(move || {
                        let result = interp.call_func(c.func, Vec::new(), Some(c.clone()), span);
                        task.complete(result.map_err(|e| e.message));
                    })
                    .map_err(|e| e.to_string())
                    .at(span)?;
                Value::Task(handle)
            }
            ExprKind::Await(task) => match self.eval(task, frame)? {
                Value::Task(t) => t.wait().at(span)?,
                other => return fail(format!("`await` needs a Task, found a {}", other.type_name()), span),
            },
            ExprKind::RunParallel(items) => Value::list(self.parallel(items.len(), frame, |i, f| self.eval(&items[i], f))?),
            ExprKind::IntToFloat(inner) => match self.eval(inner, frame)? {
                Value::Int(i) => Value::Float(i as f64),
                other => other,
            },
            ExprKind::PyImport(module) => crate::python::import_module(module).at(span)?,
        })
    }

    fn call_method(&self, obj: &Value, name: &str, args: Vec<Value>, span: Span) -> Eval {
        match obj {
            Value::Instance(inst) => {
                let class = &self.prog.classes[inst.class.0 as usize];
                if let Some(m) = class.method(name) {
                    let mut all = Vec::with_capacity(args.len() + 1);
                    all.push(obj.clone());
                    all.extend(args);
                    return self.call_func(m, all, None, span);
                }
                if class.field_index(name).is_some() {
                    let f = self.get_field(obj, name).at(span)?;
                    return self.call_value(&f, args, span);
                }
                fail(format!("`{}` has no method `{name}`", class.name), span)
            }
            Value::Py(p) => {
                let method = crate::python::get_attr(p, name).at(span)?;
                self.call_value(&method, args, span)
            }
            Value::Tensor(t) => self.tensors.method(t, name, &args).at(span),
            Value::Optimizer(o) => tensor::optimizer_method(o, name, &args).at(span),
            Value::Map(m) if !is_map_method(name) => {
                let f = m.read().get(&Key::Str(name.into())).cloned();
                match f {
                    Some(f) => self.call_value(&f, args, span),
                    None => fail(format!("Map has no method `{name}`"), span),
                }
            }
            _ => self.call_method_builtin(obj, name, &args).at(span),
        }
    }

    /// Evaluates `n` jobs on scoped threads, each with its own copy of the frame.
    fn parallel(&self, n: usize, frame: &Frame, job: impl Fn(usize, &mut Frame) -> Eval + Sync) -> Result<Vec<Value>, RtError> {
        let workers = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4).min(n.max(1));
        let chunk = n.div_ceil(workers.max(1)).max(1);
        let results: Vec<Result<Vec<Value>, RtError>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n)
                .step_by(chunk)
                .map(|start| {
                    let mut local = frame.clone();
                    let job = &job;
                    std::thread::Builder::new()
                        .stack_size(TASK_STACK)
                        .spawn_scoped(s, move || (start..(start + chunk).min(n)).map(|i| job(i, &mut local)).collect())
                        .expect("spawn a worker thread")
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| fail("a parallel task panicked", Span::default()))).collect()
        });
        let mut out = Vec::with_capacity(n);
        for r in results {
            out.extend(r?);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn query(&self, op: QueryOp, items: Vec<Value>, slot: hir::LocalId, body: Option<&hir::Expr>, parallel: bool, ty: &Ty, frame: &mut Frame, span: Span) -> Eval {
        let apply = |item: &Value, frame: &mut Frame| -> Eval {
            match body {
                Some(b) => {
                    frame.locals[slot.0 as usize] = item.clone();
                    self.eval(b, frame)
                }
                None => Ok(item.clone()),
            }
        };
        if parallel && op == QueryOp::Select {
            return Ok(Value::list(self.parallel(items.len(), frame, |i, f| apply(&items[i], f))?));
        }
        Ok(match op {
            QueryOp::Where => {
                let mut kept = Vec::new();
                for item in items {
                    match apply(&item, frame)? {
                        Value::Bool(true) => kept.push(item),
                        Value::Bool(false) => {}
                        other => return fail(format!("a `where` condition must be a Bool, found a {}", other.type_name()), span),
                    }
                }
                Value::list(kept)
            }
            QueryOp::Select => {
                let mut out = Vec::with_capacity(items.len());
                for item in &items {
                    out.push(apply(item, frame)?);
                }
                Value::list(out)
            }
            QueryOp::Sum | QueryOp::Average => {
                let mut total = if *ty == Ty::Float && op == QueryOp::Sum { Value::Float(0.0) } else { Value::Int(0) };
                let n = items.len();
                for item in &items {
                    let v = apply(item, frame)?;
                    total = self.binary_values(BinOp::Add, &total, &v).at(span)?;
                }
                if op == QueryOp::Average {
                    if n == 0 {
                        return fail("average of an empty list", span);
                    }
                    match total.as_f64() {
                        Some(t) => Value::Float(t / n as f64),
                        None => self.binary_values(BinOp::Div, &total, &Value::Int(n as i64)).at(span)?,
                    }
                } else {
                    total
                }
            }
            QueryOp::Min | QueryOp::Max => {
                let mut mapped = Vec::with_capacity(items.len());
                for item in &items {
                    mapped.push(apply(item, frame)?);
                }
                let b = if op == QueryOp::Max { crate::check::builtins::Builtin::Max } else { crate::check::builtins::Builtin::Min };
                self.call_builtin(b, vec![Value::list(mapped)]).at(span)?
            }
        })
    }
}

fn is_map_method(name: &str) -> bool {
    crate::check::builtins::method_signature(&Ty::map(Ty::Any, Ty::Any), name).is_some()
}

struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
    }
}
