//! Native code generation with Cranelift. Function instances whose values are
//! all `Int`, `Float`, `Bool` or `None` compile to machine code on their first
//! call; everything else stays in the interpreter. Compiled code must behave
//! exactly like the interpreter, including its error messages.

mod lower;
mod runtime;

use crate::check::hir::{FuncId, Program};
use crate::check::types::Ty;
use crate::source::Span;
use cranelift_codegen::ir::{self, types, AbiParam, InstBuilder, MemFlagsData, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId as ClifFunc, Linkage, Module};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::{BufWriter, Stdout};
use std::sync::Arc;

pub use runtime::Ctx;

/// The run-time representation of a value in compiled code.
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

/// A compiled function instance as the interpreter calls it.
#[derive(Debug, Clone)]
pub struct Entry {
    trampoline: usize,
    pub params: Vec<Scalar>,
    pub ret: Scalar,
}

type Trampoline = unsafe extern "C" fn(*mut Ctx, *const u64, *mut u64);

impl Entry {
    /// Calls the compiled code with arguments already converted to raw words.
    pub fn call(&self, args: &[u64], depth: usize, out: &Mutex<BufWriter<Stdout>>) -> Result<u64, (String, Span)> {
        debug_assert_eq!(args.len(), self.params.len());
        let mut ctx = Ctx::new(depth as u64, out);
        let mut result = 0u64;
        // SAFETY: the trampoline was generated for exactly this parameter list and
        // reads `args.len()` words; `ctx` and `result` outlive the call.
        unsafe {
            let f: Trampoline = std::mem::transmute(self.trampoline);
            f(&mut ctx, args.as_ptr(), &mut result);
        }
        match ctx.take_error() {
            Some(err) => Err(err),
            None => Ok(result),
        }
    }
}

type Key = (FuncId, Vec<Ty>);

#[derive(Clone)]
enum Instance {
    Compiled { id: ClifFunc, params: Vec<Scalar>, ret: Scalar, trampoline: Option<usize> },
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
}

impl Native {
    /// Sets up a JIT for the host machine, or returns `None` if Cranelift does not support it.
    pub fn new(prog: Arc<Program>) -> Option<Native> {
        let mut flags = settings::builder();
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", if cfg!(target_arch = "x86_64") { "true" } else { "false" }).ok()?;
        flags.set("opt_level", "speed").ok()?;
        let isa = cranelift_native::builder().ok()?.finish(settings::Flags::new(flags)).ok()?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        runtime::register(&mut builder);
        let mut module = JITModule::new(builder);
        let helpers = runtime::Helpers::declare(&mut module).ok()?;
        let log = std::env::var_os("NOVO_NATIVE_LOG").is_some();
        Some(Native { prog, compiler: Mutex::new(Compiler { module, instances: HashMap::new(), helpers, names: 0 }), log })
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
            Some(Instance::Compiled { trampoline: Some(t), params, ret, .. }) => Some(Entry { trampoline: t, params, ret }),
            Some(Instance::Compiled { id, params, ret, trampoline: None }) => {
                let t = c.trampoline(id, &params, ret).ok()?;
                c.instances.insert(key, Instance::Compiled { id, params: params.clone(), ret, trampoline: Some(t) });
                Some(Entry { trampoline: t, params, ret })
            }
            None => c.compile(&self.prog, key, self.log),
        }
    }
}

fn has_vars(ty: &Ty) -> bool {
    matches!(ty, Ty::Var(_)) || ty.children().into_iter().any(has_vars)
}

/// Function instances lowered together; any failure discards the whole batch.
struct Batch {
    queue: Vec<(Key, ClifFunc)>,
    pending: HashMap<Key, (ClifFunc, Vec<Scalar>, Scalar)>,
}

impl Compiler {
    fn fresh_name(&mut self) -> String {
        self.names += 1;
        format!("novo_fn_{}", self.names)
    }

    fn signature(&self, params: &[Scalar], ret: Scalar) -> ir::Signature {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.extend(params.iter().map(|p| AbiParam::new(p.clif())));
        sig.returns.push(AbiParam::new(ret.clif()));
        sig
    }

    fn compile(&mut self, prog: &Program, root: Key, log: bool) -> Option<Entry> {
        let mut batch = Batch { queue: Vec::new(), pending: HashMap::new() };
        let lowered = self.lower_batch(prog, &root, &mut batch);
        let functions = match lowered {
            Ok(fs) => fs,
            Err(reason) => {
                if log {
                    eprintln!("[native] {} stays interpreted: {reason}", prog.funcs[root.0 .0 as usize].name);
                }
                for key in batch.pending.into_keys() {
                    self.instances.insert(key, Instance::Failed);
                }
                self.instances.insert(root, Instance::Failed);
                return None;
            }
        };
        let mut ctx = Context::new();
        for (id, func) in functions {
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
        for (key, (id, params, ret)) in batch.pending {
            self.instances.insert(key, Instance::Compiled { id, params, ret, trampoline: None });
        }
        let Some(Instance::Compiled { id, params, ret, .. }) = self.instances.get(&root).cloned() else { return None };
        let t = self.trampoline(id, &params, ret).ok()?;
        if log {
            eprintln!("[native] compiled {}", prog.funcs[root.0 .0 as usize].name);
        }
        self.instances.insert(root, Instance::Compiled { id, params: params.clone(), ret, trampoline: Some(t) });
        Some(Entry { trampoline: t, params, ret })
    }

    fn lower_batch(&mut self, prog: &Program, root: &Key, batch: &mut Batch) -> Result<Vec<(ClifFunc, ir::Function)>, String> {
        self.request(prog, root, batch)?;
        let mut out = Vec::new();
        let mut fctx = FunctionBuilderContext::new();
        while let Some((key, id)) = batch.queue.pop() {
            let (_, params, ret) = batch.pending[&key].clone();
            let mut func = ir::Function::with_name_signature(UserFuncName::user(0, id.as_u32()), self.signature(&params, ret));
            lower::lower(self, prog, batch, &key, &mut func, &mut fctx)?;
            out.push((id, func));
        }
        Ok(out)
    }

    /// Finds or declares the compiled function for an instance, queueing it for lowering if new.
    fn request(&mut self, prog: &Program, key: &Key, batch: &mut Batch) -> Result<(ClifFunc, Vec<Scalar>, Scalar), String> {
        match self.instances.get(key) {
            Some(Instance::Compiled { id, params, ret, .. }) => return Ok((*id, params.clone(), *ret)),
            Some(Instance::Failed) => return Err(format!("`{}` cannot be compiled", prog.funcs[key.0 .0 as usize].name)),
            None => {}
        }
        if let Some(found) = batch.pending.get(key) {
            return Ok(found.clone());
        }
        let func = &prog.funcs[key.0 .0 as usize];
        let subst: Vec<(u32, Ty)> = func.generics.iter().copied().zip(key.1.iter().cloned()).collect();
        let scalar = |ty: &Ty| Scalar::of(&lower::substitute(ty, &subst)).ok_or_else(|| format!("`{}` uses non-scalar values", func.name));
        let params = func.locals[..func.params].iter().map(|l| scalar(&l.ty)).collect::<Result<Vec<_>, _>>()?;
        let ret = match &func.ty {
            Ty::Func(_, r) => scalar(r)?,
            _ => return Err("not a function".into()),
        };
        let name = self.fresh_name();
        let id = self.module.declare_function(&name, Linkage::Local, &self.signature(&params, ret)).map_err(|e| e.to_string())?;
        batch.pending.insert(key.clone(), (id, params.clone(), ret));
        batch.queue.push((key.clone(), id));
        Ok((id, params, ret))
    }

    /// Builds `extern "C" fn(ctx, args, result)` that unpacks raw words and calls `target`.
    fn trampoline(&mut self, target: ClifFunc, params: &[Scalar], ret: Scalar) -> Result<usize, String> {
        let mut sig = self.module.make_signature();
        sig.params.extend([AbiParam::new(types::I64); 3]);
        let name = self.fresh_name();
        let id = self.module.declare_function(&name, Linkage::Local, &sig).map_err(|e| e.to_string())?;
        let mut func = ir::Function::with_name_signature(UserFuncName::user(0, id.as_u32()), sig);
        let mut fctx = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut func, &mut fctx);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            let (ctx, args, out) = {
                let p = b.block_params(block);
                (p[0], p[1], p[2])
            };
            let mut call_args = vec![ctx];
            for (i, p) in params.iter().enumerate() {
                let word = b.ins().load(types::I64, MemFlagsData::trusted(), args, (i * 8) as i32);
                call_args.push(match p {
                    Scalar::Int | Scalar::None => word,
                    Scalar::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), word),
                    Scalar::Bool => b.ins().ireduce(types::I8, word),
                });
            }
            let callee = self.module.declare_func_in_func(target, b.func);
            let call = b.ins().call(callee, &call_args);
            let value = b.inst_results(call)[0];
            let word = match ret {
                Scalar::Int | Scalar::None => value,
                Scalar::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), value),
                Scalar::Bool => b.ins().uextend(types::I64, value),
            };
            b.ins().store(MemFlagsData::trusted(), word, out, 0);
            b.ins().return_(&[]);
            b.seal_all_blocks();
            b.finalize(self.module.isa().frontend_config());
        }
        let mut ctx = Context::new();
        ctx.func = func;
        self.module.define_function(id, &mut ctx).map_err(|e| format!("{e:?}"))?;
        self.module.finalize_definitions().map_err(|e| e.to_string())?;
        Ok(self.module.get_finalized_function(id) as usize)
    }
}
