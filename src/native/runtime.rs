//! Functions compiled code calls back into, and the context it carries.

use crate::eval::value::format_float;
use crate::source::{FileId, Span};
use cranelift_codegen::ir::{types, AbiParam, Signature};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, ModuleResult};
use parking_lot::Mutex;
use std::io::{BufWriter, Stdout, Write};

pub const MAX_DEPTH: u64 = 10_000;

pub mod code {
    pub const OVERFLOW: u32 = 1;
    pub const DIV_ZERO: u32 = 2;
    pub const DEPTH: u32 = 3;
    pub const NEG_EXP: u32 = 4;
    pub const RANGE_STEP: u32 = 5;
    pub const FLOAT_TO_INT: u32 = 6;
}

fn message(code: u32) -> &'static str {
    match code {
        code::OVERFLOW => "integer overflow",
        code::DIV_ZERO => "division by zero",
        code::DEPTH => "maximum recursion depth exceeded",
        code::NEG_EXP => "negative exponent for an Int power; use a Float base",
        code::RANGE_STEP => "`range` step cannot be zero",
        code::FLOAT_TO_INT => "cannot convert a Float to an Int",
        _ => "internal error in compiled code",
    }
}

/// State shared between compiled code and the runtime. Compiled code reads only
/// `status` (offset 0) and, on entry, the starting `depth` (offset 8).
#[repr(C)]
pub struct Ctx {
    status: u8,
    depth: u64,
    error: Option<(String, Span)>,
    out: *const Mutex<BufWriter<Stdout>>,
    line: String,
}

pub const STATUS_OFFSET: i32 = 0;
pub const DEPTH_OFFSET: i32 = 8;

impl Ctx {
    pub fn new(depth: u64, out: &Mutex<BufWriter<Stdout>>) -> Ctx {
        Ctx { status: 0, depth, error: None, out, line: String::new() }
    }

    pub fn take_error(&mut self) -> Option<(String, Span)> {
        self.error.take()
    }
}

fn span(file: u32, lo: u32, hi: u32) -> Span {
    Span { file: FileId(file), lo, hi }
}

/// # Safety
/// `ctx` must point to the live context passed into compiled code.
unsafe extern "C" fn novo_fail(ctx: *mut Ctx, code: u32, file: u32, lo: u32, hi: u32) {
    let ctx = unsafe { &mut *ctx };
    ctx.status = 1;
    ctx.error = Some((message(code).to_string(), span(file, lo, hi)));
}

/// # Safety
/// `ctx` must be live and `ptr..ptr+len` must be valid UTF-8 owned by the program.
unsafe extern "C" fn novo_fail_msg(ctx: *mut Ctx, ptr: *const u8, len: usize, file: u32, lo: u32, hi: u32) {
    let ctx = unsafe { &mut *ctx };
    let text = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) };
    ctx.status = 1;
    ctx.error = Some((text.to_string(), span(file, lo, hi)));
}

/// # Safety
/// `ctx` must point to the live context passed into compiled code.
unsafe extern "C" fn novo_ipow(ctx: *mut Ctx, base: i64, exp: i64, file: u32, lo: u32, hi: u32) -> i64 {
    if exp < 0 {
        unsafe { novo_fail(ctx, code::NEG_EXP, file, lo, hi) };
        return 0;
    }
    match u32::try_from(exp).ok().and_then(|e| base.checked_pow(e)) {
        Some(v) => v,
        None => {
            unsafe { novo_fail(ctx, code::OVERFLOW, file, lo, hi) };
            0
        }
    }
}

extern "C" fn novo_fmod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r < 0.0) != (b < 0.0)) { r + b } else { r }
}

pub mod math {
    pub const SIN: u32 = 0;
    pub const COS: u32 = 1;
    pub const TAN: u32 = 2;
    pub const LOG: u32 = 3;
    pub const EXP: u32 = 4;
}

extern "C" fn novo_math(op: u32, x: f64) -> f64 {
    match op {
        math::SIN => x.sin(),
        math::COS => x.cos(),
        math::TAN => x.tan(),
        math::LOG => x.ln(),
        _ => x.exp(),
    }
}

extern "C" fn novo_powf(a: f64, b: f64) -> f64 {
    a.powf(b)
}

extern "C" fn novo_round_digits(x: f64, digits: i64) -> f64 {
    let scale = 10f64.powi(digits as i32);
    (x * scale).round_ties_even() / scale
}

extern "C" fn novo_time_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

extern "C" fn novo_time_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// # Safety
/// `ctx` must point to the live context passed into compiled code.
unsafe extern "C" fn novo_show_int(ctx: *mut Ctx, v: i64) {
    let ctx = unsafe { &mut *ctx };
    let _ = std::fmt::Write::write_fmt(&mut ctx.line, format_args!("{v}"));
}

/// # Safety
/// `ctx` must point to the live context passed into compiled code.
unsafe extern "C" fn novo_show_float(ctx: *mut Ctx, v: f64) {
    unsafe { &mut *ctx }.line.push_str(&format_float(v));
}

/// # Safety
/// `ctx` must point to the live context passed into compiled code.
unsafe extern "C" fn novo_show_bool(ctx: *mut Ctx, v: u8) {
    unsafe { &mut *ctx }.line.push_str(if v != 0 { "true" } else { "false" });
}

/// # Safety
/// `ctx` must be live and `ptr..ptr+len` must be valid UTF-8 owned by the program.
unsafe extern "C" fn novo_show_str(ctx: *mut Ctx, ptr: *const u8, len: usize) {
    let text = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) };
    unsafe { &mut *ctx }.line.push_str(text);
}

/// # Safety
/// `ctx` must point to the live context, whose output lock outlives the call.
unsafe extern "C" fn novo_show_end(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    ctx.line.push('\n');
    let out = unsafe { &*ctx.out };
    let _ = out.lock().write_all(ctx.line.as_bytes());
    ctx.line.clear();
}

macro_rules! helpers {
    ($($field:ident = $func:ident ( $($p:ident),* ) $(-> $r:ident)?;)*) => {
        pub struct Helpers {
            $(pub $field: FuncId,)*
        }

        impl Helpers {
            pub fn declare(module: &mut JITModule) -> ModuleResult<Helpers> {
                Ok(Helpers {
                    $($field: {
                        let mut sig: Signature = module.make_signature();
                        $(sig.params.push(AbiParam::new(types::$p));)*
                        $(sig.returns.push(AbiParam::new(types::$r));)?
                        module.declare_function(stringify!($func), Linkage::Import, &sig)?
                    },)*
                })
            }
        }

        pub fn register(builder: &mut JITBuilder) {
            $(builder.symbol(stringify!($func), $func as *const u8);)*
        }
    };
}

helpers! {
    fail = novo_fail(I64, I32, I32, I32, I32);
    fail_msg = novo_fail_msg(I64, I64, I64, I32, I32, I32);
    ipow = novo_ipow(I64, I64, I64, I32, I32, I32) -> I64;
    fmod = novo_fmod(F64, F64) -> F64;
    math = novo_math(I32, F64) -> F64;
    powf = novo_powf(F64, F64) -> F64;
    round_digits = novo_round_digits(F64, I64) -> F64;
    time_now = novo_time_now() -> F64;
    time_now_ms = novo_time_now_ms() -> I64;
    show_int = novo_show_int(I64, I64);
    show_float = novo_show_float(I64, F64);
    show_bool = novo_show_bool(I64, I8);
    show_str = novo_show_str(I64, I64, I64);
    show_end = novo_show_end(I64);
}
