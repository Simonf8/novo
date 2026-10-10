//! Native code generation with Cranelift. Every function instance compiles to
//! machine code on its first call. `Int`, `Float` and `Bool` stay in registers;
//! other values live in slots of the native frame and are handled by runtime
//! helpers that share the interpreter's implementation. Functions using the
//! few constructs not yet supported stay on the interpreter.

mod globals;
mod lower;
mod runtime;

use crate::check::hir::{FuncId, Program};
use crate::check::types::Ty;
use crate::eval::value::Value;
use crate::eval::{Interp, RtError};
use crate::source::Span;
use cranelift_codegen::Context;
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId as ClifFunc, Linkage, Module, default_libcall_names};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

use runtime::Ctx;

/// The run-time representation of a scalar in compiled code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scalar {
    Int,
    Float,
    Bool,
    None,
}

impl Scalar {
    pub fn of(ty: &Ty) -> Option<Scalar> {
        Some(match ty {
            Ty::Int => Scalar::Int,
            Ty::Float => Scalar::Float,
            Ty::Bool => Scalar::Bool,
            Ty::None => Scalar::None,
            _ => return None,
        })
    }

    fn clif(self) -> ir::Type {
        match self {
            Scalar::Int | Scalar::None => types::I64,
            Scalar::Float => types::F64,
            Scalar::Bool => types::I8,
        }
    }
}

/// How a value is passed: a scalar in a register, or a pointer to a `Value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rep {
    Scalar(Scalar),
    Obj,
}

impl Rep {
    pub fn of(ty: &Ty) -> Rep {
        Scalar::of(ty).map(Rep::Scalar).unwrap_or(Rep::Obj)
    }

    fn clif(self) -> ir::Type {
        match self {
            Rep::Scalar(s) => s.clif(),
            Rep::Obj => types::I64,
        }
    }
}

/// A compiled function instance as the interpreter calls it.
#[derive(Debug, Clone)]
pub struct Entry {
    trampoline: usize,
    params: Vec<Rep>,
    ret: Rep,
}

type Trampoline = unsafe extern "C" fn(*mut Ctx, *const u64, *mut u64, *mut Value);

impl Entry {
    pub fn call(
        &self,
        interp: &Interp,
        args: &[Value],
        depth: usize,
        span: Span,
    ) -> Result<Value, RtError> {
        let mut words = Vec::with_capacity(args.len());
        for (v, rep) in args.iter().zip(&self.params) {
            words.push(match (rep, v) {
                (Rep::Scalar(Scalar::Int), Value::Int(i)) => *i as u64,
                (Rep::Scalar(Scalar::Float), Value::Float(f)) => f.to_bits(),
                (Rep::Scalar(Scalar::Float), Value::Int(i)) => (*i as f64).to_bits(),
                (Rep::Scalar(Scalar::Bool), Value::Bool(b)) => u64::from(*b),
                (Rep::Scalar(Scalar::None), _) => 0,
                (Rep::Obj, v) => v as *const Value as u64,
                (rep, v) => {
                    let message = format!(
                        "internal error: a {} was passed where compiled code expected {rep:?}",
                        v.type_name()
                    );
                    return Err(RtError {
                        message,
                        span,
                        trace: Vec::new(),
                        value: None,
                    });
                }
            });
        }
        let mut ctx = Ctx::new(depth as u64, interp);
        let mut word = 0u64;
        let mut obj = Value::None;
        // SAFETY: the trampoline was generated for exactly this parameter list;
        // every pointer passed outlives the call.
        unsafe {
            let f: Trampoline = std::mem::transmute(self.trampoline);
            f(&mut ctx, words.as_ptr(), &mut word, &mut obj);
        }
        if let Some(err) = ctx.take_error() {
            return Err(err);
        }
        Ok(match self.ret {
            Rep::Scalar(Scalar::Int) => Value::Int(word as i64),
            Rep::Scalar(Scalar::Float) => Value::Float(f64::from_bits(word)),
            Rep::Scalar(Scalar::Bool) => Value::Bool(word != 0),
            Rep::Scalar(Scalar::None) => Value::None,
            Rep::Obj => obj,
        })
    }
}

type Key = (FuncId, Vec<Ty>);

#[derive(Clone)]
struct Compiled {
    id: ClifFunc,
    params: Vec<Rep>,
    ret: Rep,
}

#[derive(Clone)]
enum Instance {
    Compiled(Compiled, Option<usize>),
    Failed,
}

pub struct Native {
    prog: Arc<Program>,
    compiler: Mutex<Compiler>,
    log: bool,
}

struct Compiler {
    module: JITModule,
    instances: HashMap<Key, Instance>,
    helpers: runtime::Helpers,
    names: u32,
    private_globals: HashMap<FuncId, std::collections::HashSet<crate::check::hir::GlobalId>>,
}

impl Native {
    /// Sets up a JIT for the host machine, or returns `None` if Cranelift does not support it.
    pub fn new(prog: Arc<Program>) -> Option<Native> {
        let mut flags = settings::builder();
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags
            .set(
                "is_pic",
                if cfg!(target_arch = "x86_64") {
                    "true"
                } else {
                    "false"
                },
            )
            .ok()?;
        flags.set("opt_level", "speed").ok()?;
        let isa = cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flags))
            .ok()?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        runtime::register(&mut builder);
        let mut module = JITModule::new(builder);
        let helpers = runtime::Helpers::declare(&mut module).ok()?;
        let log = std::env::var_os("NOVO_NATIVE_LOG").is_some();
        let private_globals = globals::private_globals(&prog);
        let compiler = Compiler {
            module,
            instances: HashMap::new(),
            helpers,
            names: 0,
            private_globals,
        };
        Some(Native {
            prog,
            compiler: Mutex::new(compiler),
            log,
        })
    }

    /// Returns compiled code for `func` instantiated at `inst`, compiling it on first request.
    pub fn entry(&self, func: FuncId, inst: &[Ty]) -> Option<Entry> {
        if inst.iter().any(has_vars) {
            return None;
        }
        let key = (func, inst.to_vec());
        let mut c = self.compiler.lock();
        match c.instances.get(&key).cloned() {
            Some(Instance::Failed) => None,
            Some(Instance::Compiled(f, Some(t))) => Some(Entry {
                trampoline: t,
                params: f.params,
                ret: f.ret,
            }),
            Some(Instance::Compiled(f, None)) => {
                let t = c.trampoline(&f).ok()?;
                c.instances
                    .insert(key, Instance::Compiled(f.clone(), Some(t)));
                Some(Entry {
                    trampoline: t,
                    params: f.params,
                    ret: f.ret,
                })
            }
            None => c.compile(&self.prog, key, self.log),
        }
    }
}

fn has_vars(ty: &Ty) -> bool {
    matches!(ty, Ty::Var(_)) || ty.children().into_iter().any(has_vars)
}

/// Function instances lowered together.
struct Batch {
    queue: Vec<Key>,
    pending: HashMap<Key, Compiled>,
}

impl Compiler {
    fn fresh_name(&mut self) -> String {
        self.names += 1;
        format!("novo_fn_{}", self.names)
    }

    /// `(ctx, depth, [result slot], params...) -> scalar result`
    fn signature(&self, params: &[Rep], ret: Rep) -> ir::Signature {
        let mut sig = self.module.make_signature();
        sig.params.extend([AbiParam::new(types::I64); 2]);
        if ret == Rep::Obj {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.params
            .extend(params.iter().map(|p| AbiParam::new(p.clif())));
        sig.returns.push(AbiParam::new(ret.clif()));
        sig
    }

    /// Compiles `root` and the instances it calls. When one cannot be compiled it
    /// is marked as interpreted and the batch is lowered again, so its callers
    /// reach it through the interpreter.
    fn compile(&mut self, prog: &Program, root: Key, log: bool) -> Option<Entry> {
        loop {
            let mut batch = Batch {
                queue: Vec::new(),
                pending: HashMap::new(),
            };
            match self.lower_batch(prog, &root, &mut batch) {
                Ok(functions) => {
                    let mut ctx = Context::new();
                    for (id, func) in functions {
                        if std::env::var_os("NOVO_CLIF").is_some() {
                            eprintln!("{}", func.display());
                        }
                        ctx.func = func;
                        if let Err(e) = self.module.define_function(id, &mut ctx) {
                            if log {
                                eprintln!("[native] code generation failed: {e:?}");
                            }
                            for key in batch.pending.into_keys() {
                                self.instances.insert(key, Instance::Failed);
                            }
                            return None;
                        }
                        ctx.clear();
                    }
                    if log {
                        for key in batch.pending.keys() {
                            eprintln!("[native] compiled {}", prog.funcs[key.0.0 as usize].name);
                        }
                    }
                    for (key, f) in batch.pending {
                        self.instances.insert(key, Instance::Compiled(f, None));
                    }
                    let Some(Instance::Compiled(f, _)) = self.instances.get(&root).cloned() else {
                        return None;
                    };
                    let t = self.trampoline(&f).ok()?;
                    self.instances
                        .insert(root, Instance::Compiled(f.clone(), Some(t)));
                    return Some(Entry {
                        trampoline: t,
                        params: f.params,
                        ret: f.ret,
                    });
                }
                Err((key, reason)) => {
                    if log {
                        eprintln!(
                            "[native] {} stays interpreted: {reason}",
                            prog.funcs[key.0.0 as usize].name
                        );
                    }
                    let failed_root = key == root;
                    self.instances.insert(key, Instance::Failed);
                    if failed_root {
                        return None;
                    }
                }
            }
        }
    }

    fn lower_batch(
        &mut self,
        prog: &Program,
        root: &Key,
        batch: &mut Batch,
    ) -> Result<Vec<(ClifFunc, ir::Function)>, (Key, String)> {
        self.request(prog, root, batch)
            .map_err(|e| (root.clone(), e))?;
        let mut out = Vec::new();
        let mut fctx = FunctionBuilderContext::new();
        while let Some(key) = batch.queue.pop() {
            let f = batch.pending[&key].clone();
            let mut func = ir::Function::with_name_signature(
                UserFuncName::user(0, f.id.as_u32()),
                self.signature(&f.params, f.ret),
            );
            lower::lower(self, prog, batch, &key, &mut func, &mut fctx)
                .map_err(|e| (key.clone(), e))?;
            out.push((f.id, func));
        }
        Ok(out)
    }

    /// Finds or declares the compiled function for an instance, queueing it for
    /// lowering if new. `None` means the instance runs on the interpreter.
    fn request(
        &mut self,
        prog: &Program,
        key: &Key,
        batch: &mut Batch,
    ) -> Result<Option<Compiled>, String> {
        match self.instances.get(key) {
            Some(Instance::Compiled(f, _)) => return Ok(Some(f.clone())),
            Some(Instance::Failed) => return Ok(None),
            None => {}
        }
        if let Some(found) = batch.pending.get(key) {
            return Ok(Some(found.clone()));
        }
        let func = &prog.funcs[key.0.0 as usize];
        let subst: Vec<(u32, Ty)> = func
            .generics
            .iter()
            .copied()
            .zip(key.1.iter().cloned())
            .collect();
        let rep = |ty: &Ty| Rep::of(&lower::substitute(ty, &subst));
        let params: Vec<Rep> = lower::param_locals(func)
            .iter()
            .map(|&l| rep(&func.locals[l].ty))
            .collect();
        let ret = match &func.ty {
            Ty::Func(_, r) => rep(r),
            _ => return Err("not a function".into()),
        };
        let name = self.fresh_name();
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &self.signature(&params, ret))
            .map_err(|e| e.to_string())?;
        let compiled = Compiled { id, params, ret };
        batch.pending.insert(key.clone(), compiled.clone());
        batch.queue.push(key.clone());
        Ok(Some(compiled))
    }

    /// Builds `extern "C" fn(ctx, args, result, result_slot)` that unpacks raw words and calls the function.
    fn trampoline(&mut self, f: &Compiled) -> Result<usize, String> {
        let mut sig = self.module.make_signature();
        sig.params.extend([AbiParam::new(types::I64); 4]);
        let name = self.fresh_name();
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &sig)
            .map_err(|e| e.to_string())?;
        let mut func = ir::Function::with_name_signature(UserFuncName::user(0, id.as_u32()), sig);
        let mut fctx = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut func, &mut fctx);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            let (ctx, args, out, slot) = {
                let p = b.block_params(block);
                (p[0], p[1], p[2], p[3])
            };
            let depth = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                ctx,
                runtime::DEPTH_OFFSET,
            );
            let mut call_args = vec![ctx, depth];
            if f.ret == Rep::Obj {
                call_args.push(slot);
            }
            for (i, p) in f.params.iter().enumerate() {
                let word = b
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), args, (i * 8) as i32);
                call_args.push(match p {
                    Rep::Scalar(Scalar::Int | Scalar::None) | Rep::Obj => word,
                    Rep::Scalar(Scalar::Float) => {
                        b.ins().bitcast(types::F64, MemFlagsData::new(), word)
                    }
                    Rep::Scalar(Scalar::Bool) => b.ins().ireduce(types::I8, word),
                });
            }
            let callee = self.module.declare_func_in_func(f.id, b.func);
            let call = b.ins().call(callee, &call_args);
            let value = b.inst_results(call)[0];
            let word = match f.ret {
                Rep::Scalar(Scalar::Int | Scalar::None) | Rep::Obj => value,
                Rep::Scalar(Scalar::Float) => {
                    b.ins().bitcast(types::I64, MemFlagsData::new(), value)
                }
                Rep::Scalar(Scalar::Bool) => b.ins().uextend(types::I64, value),
            };
            b.ins().store(MemFlagsData::trusted(), word, out, 0);
            b.ins().return_(&[]);
            b.seal_all_blocks();
            b.finalize(self.module.isa().frontend_config());
        }
        let mut ctx = Context::new();
        ctx.func = func;
        self.module
            .define_function(id, &mut ctx)
            .map_err(|e| format!("{e:?}"))?;
        self.module
            .finalize_definitions()
            .map_err(|e| e.to_string())?;
        Ok(self.module.get_finalized_function(id) as usize)
    }
}
