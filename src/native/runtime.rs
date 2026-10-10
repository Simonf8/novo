//! Functions compiled code calls back into, and the context it carries.
//!
//! Values that are not `Int`, `Float`, `Bool` or `None` live in slots of the
//! native frame. Each slot owns a `Value`; helpers that write a slot drop what
//! was there. Operations on such values reuse the interpreter's own code, so
//! both engines behave identically.

use crate::check::builtins::Builtin;
use crate::check::hir::{self, FuncId, GlobalId, ModuleId};
use crate::check::types::ClassId;
use crate::eval::value::{Closure, Dict, Instance, Items, Key, Value, format_float};
use crate::eval::{Interp, RtError};
use crate::source::{FileId, Span};
use crate::syntax::ast::BinOp;
use cranelift_codegen::ir::{AbiParam, Signature, types};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId as ClifFunc, Linkage, Module, ModuleResult};
use parking_lot::RwLock;
use std::io::Write;
use std::sync::Arc;

pub const MAX_DEPTH: u64 = 10_000;
pub const VALUE_SIZE: u32 = std::mem::size_of::<Value>() as u32;

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

/// State shared between compiled code and the runtime. Compiled code reads
/// `status` (offset 0) and writes its current recursion `depth` (offset 8)
/// before calls that may run interpreted code.
#[repr(C)]
pub struct Ctx {
    status: u8,
    depth: u64,
    interp: *const Interp,
    error: Option<RtError>,
    line: String,
    fstrings: Vec<String>,
}

pub const STATUS_OFFSET: i32 = 0;
pub const DEPTH_OFFSET: i32 = 8;

impl Ctx {
    pub fn new(depth: u64, interp: &Interp) -> Ctx {
        Ctx {
            status: 0,
            depth,
            interp,
            error: None,
            line: String::new(),
            fstrings: Vec::new(),
        }
    }

    pub fn take_error(&mut self) -> Option<RtError> {
        self.error.take()
    }

    fn interp(&self) -> &Interp {
        // SAFETY: the interpreter outlives every call into compiled code.
        unsafe { &*self.interp }
    }

    fn raise(&mut self, err: RtError) {
        self.status = 1;
        self.error = Some(err);
    }

    fn check<T>(&mut self, result: Result<T, String>, span: u64) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(message) => {
                self.raise(RtError {
                    message,
                    span: unpack(span),
                    trace: Vec::new(),
                    value: None,
                });
                None
            }
        }
    }

    fn check_rt<T>(&mut self, result: Result<T, RtError>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.raise(e);
                None
            }
        }
    }
}

/// Packs a span into one word: 16 bits of file and 24 bits each of offsets.
pub fn pack(span: Span) -> i64 {
    let clamp = |x: u32| u64::from(x.min((1 << 24) - 1));
    ((u64::from(span.file.0.min(0xFFFF)) << 48) | (clamp(span.lo) << 24) | clamp(span.hi)) as i64
}

fn unpack(word: u64) -> Span {
    Span {
        file: FileId((word >> 48) as u32),
        lo: ((word >> 24) & 0xFF_FFFF) as u32,
        hi: (word & 0xFF_FFFF) as u32,
    }
}

// The helpers below are called only by generated code, which passes a live
// context, valid slot pointers and in-range counts.

unsafe fn ctx<'a>(c: *mut Ctx) -> &'a mut Ctx {
    unsafe { &mut *c }
}

unsafe fn slot<'a>(p: *mut Value) -> &'a mut Value {
    unsafe { &mut *p }
}

unsafe fn values(args: *const *const Value, n: usize) -> Vec<Value> {
    (0..n).map(|i| unsafe { (**args.add(i)).clone() }).collect()
}

unsafe fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
}

unsafe extern "C" fn novo_fail(c: *mut Ctx, code: u32, span: u64) {
    let c = unsafe { ctx(c) };
    c.raise(RtError {
        message: message(code).to_string(),
        span: unpack(span),
        trace: Vec::new(),
        value: None,
    });
}

unsafe extern "C" fn novo_fail_msg(c: *mut Ctx, ptr: *const u8, len: usize, span: u64) {
    let c = unsafe { ctx(c) };
    let message = unsafe { text(ptr, len) }.to_string();
    c.raise(RtError {
        message,
        span: unpack(span),
        trace: Vec::new(),
        value: None,
    });
}

unsafe extern "C" fn novo_ipow(c: *mut Ctx, base: i64, exp: i64, span: u64) -> i64 {
    if exp < 0 {
        unsafe { novo_fail(c, code::NEG_EXP, span) };
        return 0;
    }
    match u32::try_from(exp).ok().and_then(|e| base.checked_pow(e)) {
        Some(v) => v,
        None => {
            unsafe { novo_fail(c, code::OVERFLOW, span) };
            0
        }
    }
}

extern "C" fn novo_fmod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r < 0.0) != (b < 0.0)) {
        r + b
    } else {
        r
    }
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

extern "C" fn novo_time_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- output ------------------------------------------------------------------------------

unsafe extern "C" fn novo_show_int(c: *mut Ctx, v: i64) {
    let _ = std::fmt::Write::write_fmt(&mut unsafe { ctx(c) }.line, format_args!("{v}"));
}

unsafe extern "C" fn novo_show_float(c: *mut Ctx, v: f64) {
    unsafe { ctx(c) }.line.push_str(&format_float(v));
}

unsafe extern "C" fn novo_show_bool(c: *mut Ctx, v: u8) {
    unsafe { ctx(c) }
        .line
        .push_str(if v != 0 { "true" } else { "false" });
}

unsafe extern "C" fn novo_show_str(c: *mut Ctx, ptr: *const u8, len: usize) {
    let s = unsafe { text(ptr, len) };
    unsafe { ctx(c) }.line.push_str(s);
}

unsafe extern "C" fn novo_show_value(c: *mut Ctx, v: *mut Value) {
    let c = unsafe { ctx(c) };
    let shown = c.interp().display(unsafe { slot(v) });
    c.line.push_str(&shown);
}

unsafe extern "C" fn novo_show_end(c: *mut Ctx) {
    let c = unsafe { ctx(c) };
    c.line.push('\n');
    let _ = c.interp().output().lock().write_all(c.line.as_bytes());
    c.line.clear();
}

// ---- slots -------------------------------------------------------------------------------

unsafe extern "C" fn novo_slots_init(base: *mut Value, n: usize) {
    for i in 0..n {
        unsafe { base.add(i).write(Value::None) };
    }
}

unsafe extern "C" fn novo_slots_drop(base: *mut Value, n: usize) {
    for i in 0..n {
        unsafe { base.add(i).drop_in_place() };
    }
}

unsafe extern "C" fn novo_clone(dst: *mut Value, src: *mut Value) {
    if dst != src {
        let v = unsafe { slot(src) }.clone();
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_take(dst: *mut Value, src: *mut Value) {
    if dst != src {
        let v = std::mem::replace(unsafe { slot(src) }, Value::None);
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_set_int(dst: *mut Value, v: i64) {
    *unsafe { slot(dst) } = Value::Int(v);
}

unsafe extern "C" fn novo_set_float(dst: *mut Value, v: f64) {
    *unsafe { slot(dst) } = Value::Float(v);
}

unsafe extern "C" fn novo_set_bool(dst: *mut Value, v: u8) {
    *unsafe { slot(dst) } = Value::Bool(v != 0);
}

unsafe extern "C" fn novo_set_none(dst: *mut Value) {
    *unsafe { slot(dst) } = Value::None;
}

unsafe extern "C" fn novo_set_str(dst: *mut Value, s: *const Arc<str>) {
    *unsafe { slot(dst) } = Value::Str(unsafe { (*s).clone() });
}

unsafe extern "C" fn novo_get_int(src: *mut Value) -> i64 {
    match unsafe { slot(src) } {
        Value::Int(i) => *i,
        _ => 0,
    }
}

unsafe extern "C" fn novo_get_float(src: *mut Value) -> f64 {
    match unsafe { slot(src) } {
        Value::Float(f) => *f,
        Value::Int(i) => *i as f64,
        _ => 0.0,
    }
}

unsafe extern "C" fn novo_get_bool(src: *mut Value) -> u8 {
    u8::from(matches!(unsafe { slot(src) }, Value::Bool(true)))
}

// ---- operations --------------------------------------------------------------------------

unsafe extern "C" fn novo_binary(
    c: *mut Ctx,
    dst: *mut Value,
    op: u32,
    a: *mut Value,
    b: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let result = c
        .interp()
        .binary_values(BinOp::ALL[op as usize], unsafe { slot(a) }, unsafe {
            slot(b)
        });
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_negate(c: *mut Ctx, dst: *mut Value, a: *mut Value, span: u64) {
    let c = unsafe { ctx(c) };
    let result = crate::eval::negate_value(unsafe { slot(a) });
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_index(
    c: *mut Ctx,
    dst: *mut Value,
    obj: *mut Value,
    idx: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let result = c
        .interp()
        .get_index(unsafe { slot(obj) }, unsafe { slot(idx) });
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_set_index(
    c: *mut Ctx,
    obj: *mut Value,
    idx: *mut Value,
    v: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let value = unsafe { slot(v) }.clone();
    let result = c
        .interp()
        .set_index(unsafe { slot(obj) }, unsafe { slot(idx) }, value);
    c.check(result, span);
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn novo_slice(
    c: *mut Ctx,
    dst: *mut Value,
    obj: *mut Value,
    has_start: u8,
    start: i64,
    has_end: u8,
    end: i64,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let start = (has_start != 0).then_some(start);
    let end = (has_end != 0).then_some(end);
    let result = crate::eval::slice_value(unsafe { slot(obj) }, start, end);
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_field(
    c: *mut Ctx,
    dst: *mut Value,
    obj: *mut Value,
    name: *const u8,
    len: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let result = c
        .interp()
        .get_field(unsafe { slot(obj) }, unsafe { text(name, len) });
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_set_field(
    c: *mut Ctx,
    obj: *mut Value,
    name: *const u8,
    len: usize,
    v: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let value = unsafe { slot(v) }.clone();
    let result = c
        .interp()
        .set_field(unsafe { slot(obj) }, unsafe { text(name, len) }, value);
    c.check(result, span);
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn novo_method(
    c: *mut Ctx,
    dst: *mut Value,
    recv: *mut Value,
    name: *const u8,
    len: usize,
    args: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let args = unsafe { values(args, n) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.call_method(
            unsafe { slot(recv) },
            unsafe { text(name, len) },
            args,
            unpack(span),
        )
    });
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_builtin(
    c: *mut Ctx,
    dst: *mut Value,
    id: u32,
    args: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let args = unsafe { values(args, n) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.builtin_at(Builtin::ALL[id as usize], args, unpack(span))
    });
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_call_value(
    c: *mut Ctx,
    dst: *mut Value,
    callee: *mut Value,
    args: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let args = unsafe { values(args, n) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.call_value(unsafe { slot(callee) }, args, unpack(span))
    });
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_call_interp(
    c: *mut Ctx,
    dst: *mut Value,
    func: u32,
    args: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let args = unsafe { values(args, n) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.call_func(FuncId(func), args, None, unpack(span))
    });
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_new(
    dst: *mut Value,
    class: u32,
    args: *const *const Value,
    n: usize,
    variant: u8,
) {
    let fields = unsafe { values(args, n) };
    *unsafe { slot(dst) } = Value::Instance(Arc::new(Instance {
        class: ClassId(class),
        fields: RwLock::new(fields),
        variant: variant != 0,
    }));
}

unsafe extern "C" fn novo_make_list(dst: *mut Value, args: *const *const Value, n: usize) {
    *unsafe { slot(dst) } = Value::list(unsafe { values(args, n) });
}

unsafe extern "C" fn novo_make_tuple(dst: *mut Value, args: *const *const Value, n: usize) {
    *unsafe { slot(dst) } = Value::Tuple(unsafe { values(args, n) }.into());
}

unsafe extern "C" fn novo_make_map(
    c: *mut Ctx,
    dst: *mut Value,
    keys: *const *const Value,
    vals: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let (keys, vals) = unsafe { (values(keys, n), values(vals, n)) };
    let mut dict = Dict::default();
    for (k, v) in keys.iter().zip(vals) {
        match c.check(Key::from_value(k), span) {
            Some(key) => dict.insert(key, v),
            None => return,
        }
    }
    *unsafe { slot(dst) } = Value::map(dict);
}

unsafe extern "C" fn novo_make_closure(
    dst: *mut Value,
    func: u32,
    args: *const *const Value,
    n: usize,
) {
    let captures = unsafe { values(args, n) };
    *unsafe { slot(dst) } = Value::Func(Arc::new(Closure {
        func: FuncId(func),
        captures,
    }));
}

unsafe extern "C" fn novo_global_get(c: *mut Ctx, dst: *mut Value, g: u32, span: u64) {
    let c = unsafe { ctx(c) };
    let result = c.interp().global(GlobalId(g));
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_global_set(c: *mut Ctx, g: u32, src: *mut Value) {
    let v = unsafe { slot(src) }.clone();
    unsafe { ctx(c) }.interp().set_global(GlobalId(g), v);
}

unsafe extern "C" fn novo_items(c: *mut Ctx, dst: *mut Value, iterable: *mut Value, span: u64) {
    let c = unsafe { ctx(c) };
    if let Value::List(items) = unsafe { slot(iterable) } {
        let copy = items.read().clone();
        *unsafe { slot(dst) } = Value::from_items(copy);
        return;
    }
    let result = c.interp().items(unsafe { slot(iterable) });
    if let Some(items) = c.check(result, span) {
        *unsafe { slot(dst) } = Value::list(items);
    }
}

/// The distinct lists among `n` slots, in address order, each with whether any
/// use writes to it. Bit 2 of a kind marks a write.
unsafe fn distinct_lists<'a>(
    n: usize,
    slots: *const *mut Value,
    kinds: *const u8,
) -> Option<Vec<(usize, &'a RwLock<Items>, bool)>> {
    let mut lists: Vec<(usize, &RwLock<Items>, bool)> = Vec::with_capacity(n);
    for i in 0..n {
        let Value::List(list) = (unsafe { slot(*slots.add(i)) }) else {
            return None;
        };
        let write = unsafe { *kinds.add(i) } & 4 != 0;
        let lock: &RwLock<Items> = list;
        lists.push((lock as *const _ as usize, lock, write));
    }
    lists.sort_by_key(|l| l.0);
    lists.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.2 |= later.2;
        }
        same
    });
    Some(lists)
}

unsafe fn release(lists: &[(usize, &RwLock<Items>, bool)]) {
    use parking_lot::lock_api::RawRwLock as _;
    for (_, lock, write) in lists {
        // SAFETY: `novo_lock_lists` locked each list in this mode.
        unsafe {
            if *write {
                lock.raw().unlock_exclusive();
            } else {
                lock.raw().unlock_shared();
            }
        }
    }
}

/// Locks the lists a loop reads and writes directly, in address order so that
/// loops on different threads cannot deadlock, and reports where each list's
/// unboxed elements are. Returns 0, holding no locks, if a list is not stored
/// unboxed with the element kind the loop expects.
unsafe extern "C" fn novo_lock_lists(
    n: usize,
    slots: *const *mut Value,
    kinds: *const u8,
    ptrs: *mut u64,
    lens: *mut u64,
) -> u8 {
    use parking_lot::lock_api::RawRwLock as _;
    let Some(lists) = (unsafe { distinct_lists(n, slots, kinds) }) else {
        return 0;
    };
    for (_, lock, write) in &lists {
        // SAFETY: each lock is released by `release`, after the loop or on failure.
        unsafe {
            if *write {
                lock.raw().lock_exclusive();
            } else {
                lock.raw().lock_shared();
            }
        }
    }
    for i in 0..n {
        let Value::List(list) = (unsafe { slot(*slots.add(i)) }) else {
            unreachable!("checked above")
        };
        let kind = unsafe { *kinds.add(i) };
        // SAFETY: the list is locked, exclusively if this loop writes to it.
        let items = unsafe { &mut *list.data_ptr() };
        if kind & 4 != 0 && items.is_empty() && matches!(items, Items::Values(_)) {
            *items = match kind & 3 {
                0 => Items::Ints(Vec::new()),
                1 => Items::Floats(Vec::new()),
                _ => Items::Bools(Vec::new()),
            };
        }
        let found = match (kind & 3, items) {
            (0, Items::Ints(v)) => Some((v.as_mut_ptr() as u64, v.len())),
            (1, Items::Floats(v)) => Some((v.as_mut_ptr() as u64, v.len())),
            (2, Items::Bools(v)) => Some((v.as_mut_ptr() as u64, v.len())),
            _ => None,
        };
        let Some((ptr, len)) = found else {
            unsafe { release(&lists) };
            return 0;
        };
        unsafe {
            *ptrs.add(i) = ptr;
            *lens.add(i) = len as u64;
        }
    }
    1
}

unsafe extern "C" fn novo_unlock_lists(n: usize, slots: *const *mut Value, kinds: *const u8) {
    if let Some(lists) = unsafe { distinct_lists(n, slots, kinds) } {
        unsafe { release(&lists) };
    }
}

unsafe extern "C" fn novo_raise_index(c: *mut Ctx, i: i64, len: i64, span: u64) {
    let c = unsafe { ctx(c) };
    c.check::<usize>(crate::eval::builtins::index(i, len as usize), span);
}

unsafe extern "C" fn novo_is_variant(value: *mut Value, class: u32) -> u8 {
    u8::from(matches!(unsafe { slot(value) }, Value::Instance(i) if i.class.0 == class))
}

unsafe extern "C" fn novo_list_push_int(list: *mut Value, v: i64) {
    if let Value::List(items) = unsafe { slot(list) } {
        items.write().push(Value::Int(v));
    }
}

unsafe extern "C" fn novo_list_push_float(list: *mut Value, v: f64) {
    if let Value::List(items) = unsafe { slot(list) } {
        items.write().push(Value::Float(v));
    }
}

unsafe extern "C" fn novo_list_push_bool(list: *mut Value, v: u8) {
    if let Value::List(items) = unsafe { slot(list) } {
        items.write().push(Value::Bool(v != 0));
    }
}

unsafe extern "C" fn novo_list_len(list: *mut Value) -> i64 {
    match unsafe { slot(list) } {
        Value::List(items) => items.read().len() as i64,
        _ => 0,
    }
}

unsafe extern "C" fn novo_list_get(dst: *mut Value, list: *mut Value, i: i64) {
    let v = match unsafe { slot(list) } {
        Value::List(items) => items.read().get(i as usize).unwrap_or(Value::None),
        _ => Value::None,
    };
    *unsafe { slot(dst) } = v;
}

unsafe extern "C" fn novo_list_push(list: *mut Value, item: *mut Value) {
    if let Value::List(items) = unsafe { slot(list) } {
        items.write().push(unsafe { slot(item) }.clone());
    }
}

unsafe extern "C" fn novo_len(c: *mut Ctx, v: *mut Value, span: u64) -> i64 {
    let n = match unsafe { slot(v) } {
        Value::Str(s) => Ok(s.chars().count() as i64),
        Value::List(items) => Ok(items.read().len() as i64),
        Value::Map(m) => Ok(m.read().len() as i64),
        Value::Tuple(items) => Ok(items.len() as i64),
        other => Err(format!(
            "`len` needs a list, string, map or tensor, found a {}",
            other.type_name()
        )),
    };
    unsafe { ctx(c) }.check(n, span).unwrap_or(0)
}

unsafe extern "C" fn novo_map_get(
    c: *mut Ctx,
    dst: *mut Value,
    map: *mut Value,
    key: *mut Value,
    default: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let Value::Map(m) = (unsafe { slot(map) }) else {
        return;
    };
    let Some(key) = c.check(Key::from_value(unsafe { slot(key) }), span) else {
        return;
    };
    let found = m.read().get(&key).cloned();
    *unsafe { slot(dst) } = found.unwrap_or_else(|| unsafe { slot(default) }.clone());
}

unsafe extern "C" fn novo_map_has(c: *mut Ctx, map: *mut Value, key: *mut Value, span: u64) -> u8 {
    let c = unsafe { ctx(c) };
    let Value::Map(m) = (unsafe { slot(map) }) else {
        return 0;
    };
    let Some(key) = c.check(Key::from_value(unsafe { slot(key) }), span) else {
        return 0;
    };
    u8::from(m.read().get(&key).is_some())
}

unsafe extern "C" fn novo_list_index(
    c: *mut Ctx,
    dst: *mut Value,
    list: *mut Value,
    i: i64,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let Value::List(items) = (unsafe { slot(list) }) else {
        return;
    };
    let items = items.read();
    if let Some(at) = c.check(crate::eval::builtins::index(i, items.len()), span) {
        let v = items.get(at).expect("index in range");
        drop(items);
        *unsafe { slot(dst) } = v;
    }
}

/// List element access for compiled code working with raw Ints, Floats and
/// Bools (passed as bytes): unboxed lists are read and written directly, and
/// other lists convert, raising an error if an element has the wrong kind.
macro_rules! typed_list_access {
    ($get:ident, $set:ident, $raw:ty, $items:ident, $value:ident, $name:literal, $to_raw:expr, $from_raw:expr) => {
        unsafe extern "C" fn $get(c: *mut Ctx, list: *mut Value, i: i64, span: u64) -> $raw {
            let c = unsafe { ctx(c) };
            let Value::List(items) = (unsafe { slot(list) }) else {
                return Default::default();
            };
            let items = items.read();
            let Some(at) = c.check(crate::eval::builtins::index(i, items.len()), span) else {
                return Default::default();
            };
            let found = match &*items {
                Items::$items(xs) => Some(xs[at]),
                other => match other.get(at) {
                    Some(Value::$value(x)) => Some(x),
                    _ => None,
                },
            };
            match found {
                Some(x) => $to_raw(x),
                None => {
                    c.check::<()>(Err(format!("expected {} in the list", $name)), span);
                    Default::default()
                }
            }
        }

        unsafe extern "C" fn $set(c: *mut Ctx, list: *mut Value, i: i64, v: $raw, span: u64) {
            let c = unsafe { ctx(c) };
            let Value::List(items) = (unsafe { slot(list) }) else {
                return;
            };
            let mut items = items.write();
            let Some(at) = c.check(crate::eval::builtins::index(i, items.len()), span) else {
                return;
            };
            match &mut *items {
                Items::$items(xs) => xs[at] = $from_raw(v),
                other => other.set(at, Value::$value($from_raw(v))),
            }
        }
    };
}

typed_list_access!(
    novo_list_get_int,
    novo_list_set_int,
    i64,
    Ints,
    Int,
    "an Int",
    |x| x,
    |x| x
);
typed_list_access!(
    novo_list_get_float,
    novo_list_set_float,
    f64,
    Floats,
    Float,
    "a Float",
    |x| x,
    |x| x
);
typed_list_access!(
    novo_list_get_bool,
    novo_list_set_bool,
    u8,
    Bools,
    Bool,
    "a Bool",
    u8::from,
    |x: u8| x != 0
);

unsafe extern "C" fn novo_list_set(
    c: *mut Ctx,
    list: *mut Value,
    i: i64,
    v: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let Value::List(items) = (unsafe { slot(list) }) else {
        return;
    };
    let mut items = items.write();
    if let Some(at) = c.check(crate::eval::builtins::index(i, items.len()), span) {
        items.set(at, unsafe { slot(v) }.clone());
    }
}

unsafe extern "C" fn novo_run_parallel(
    c: *mut Ctx,
    dst: *mut Value,
    funcs: *const *const Value,
    n: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let jobs = unsafe { values(funcs, n) }
        .into_iter()
        .map(|f| (f, Vec::new()))
        .collect();
    let result = c.interp().run_parallel(jobs, unpack(span));
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = Value::list(v);
    }
}

unsafe extern "C" fn novo_parallel_map(
    c: *mut Ctx,
    dst: *mut Value,
    f: *mut Value,
    list: *mut Value,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let f = unsafe { slot(f) }.clone();
    let items = match unsafe { slot(list) } {
        Value::List(items) => items.read().to_vec(),
        _ => Vec::new(),
    };
    let jobs = items
        .into_iter()
        .map(|item| (f.clone(), vec![item]))
        .collect();
    let result = c.interp().run_parallel(jobs, unpack(span));
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = Value::list(v);
    }
}

/// How much output and f-string text is pending when a `try` starts, so a
/// `catch` discards only what its own body left unfinished.
unsafe extern "C" fn novo_try_mark(c: *mut Ctx) -> u64 {
    let c = unsafe { ctx(c) };
    c.line.len() as u64 | ((c.fstrings.len() as u64) << 32)
}

/// Records that the pending error passed through a call to `func` at `span`.
unsafe extern "C" fn novo_through(c: *mut Ctx, func: u32, span: u64) {
    let c = unsafe { ctx(c) };
    if let Some(e) = c.error.take() {
        let name = c.interp().func_name(FuncId(func)).to_string();
        c.error = Some(e.through(&name, unpack(span)));
    }
}

/// Clears the pending error and stores its message, or with `typed` set the
/// value it was raised with.
unsafe extern "C" fn novo_take_error(c: *mut Ctx, dst: *mut Value, mark: u64, typed: u8) {
    let c = unsafe { ctx(c) };
    c.status = 0;
    c.line.truncate((mark & 0xFFFF_FFFF) as usize);
    c.fstrings.truncate((mark >> 32) as usize);
    let caught = match c.error.take() {
        Some(RtError { value: Some(v), .. }) if typed != 0 => v,
        e => Value::Str(e.map(|e| e.message).unwrap_or_default().into()),
    };
    *unsafe { slot(dst) } = caught;
}

/// Whether the pending error was raised with an instance of `class`.
unsafe extern "C" fn novo_error_is(c: *mut Ctx, class: u32) -> u8 {
    let c = unsafe { ctx(c) };
    u8::from(matches!(
        &c.error,
        Some(RtError { value: Some(Value::Instance(i)), .. }) if i.class.0 == class
    ))
}

unsafe extern "C" fn novo_init_module(c: *mut Ctx, m: u32, span: u64) {
    let c = unsafe { ctx(c) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.init_module(ModuleId(m), unpack(span))
    });
    c.check_rt(result);
}

unsafe extern "C" fn novo_eval_closed(c: *mut Ctx, dst: *mut Value, expr: *const hir::Expr) {
    let c = unsafe { ctx(c) };
    let interp = c.interp();
    let result = Interp::at_depth(c.depth as usize, || {
        interp.eval(unsafe { &*expr }, &mut crate::eval::Frame::empty())
    });
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_together(c: *mut Ctx, dst: *mut Value, closure: *mut Value, span: u64) {
    let c = unsafe { ctx(c) };
    let result = c
        .interp()
        .spawn_task(unsafe { slot(closure) }, unpack(span));
    if let Some(v) = c.check_rt(result) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_await(c: *mut Ctx, dst: *mut Value, task: *mut Value, span: u64) {
    let c = unsafe { ctx(c) };
    let result = match unsafe { slot(task) } {
        Value::Task(t) => t.wait(),
        other => Err(format!(
            "`await` needs a Task, found a {}",
            other.type_name()
        )),
    };
    if let Some(v) = c.check(result, span) {
        *unsafe { slot(dst) } = v;
    }
}

unsafe extern "C" fn novo_fstr_begin(c: *mut Ctx) {
    unsafe { ctx(c) }.fstrings.push(String::new());
}

unsafe extern "C" fn novo_fstr_lit(c: *mut Ctx, ptr: *const u8, len: usize) {
    let s = unsafe { text(ptr, len) };
    if let Some(top) = unsafe { ctx(c) }.fstrings.last_mut() {
        top.push_str(s);
    }
}

unsafe extern "C" fn novo_fstr_value(
    c: *mut Ctx,
    v: *mut Value,
    spec: *const u8,
    spec_len: usize,
    span: u64,
) {
    let c = unsafe { ctx(c) };
    let value = unsafe { slot(v) };
    let shown = c.interp().display(value);
    let piece = if spec.is_null() {
        Ok(shown)
    } else {
        crate::eval::builtins::format_spec(shown, value, unsafe { text(spec, spec_len) })
    };
    if let Some(piece) = c.check(piece, span)
        && let Some(top) = c.fstrings.last_mut()
    {
        top.push_str(&piece);
    }
}

unsafe extern "C" fn novo_fstr_end(c: *mut Ctx, dst: *mut Value) {
    let s = unsafe { ctx(c) }.fstrings.pop().unwrap_or_default();
    *unsafe { slot(dst) } = Value::Str(s.into());
}

macro_rules! helpers {
    ($($field:ident = $func:ident ( $($p:ident),* ) $(-> $r:ident)?;)*) => {
        pub struct Helpers {
            $(pub $field: ClifFunc,)*
        }

        impl Helpers {
            pub fn declare(module: &mut JITModule) -> ModuleResult<Helpers> {
                Ok(Helpers {
                    $($field: {
                        let mut sig: Signature = module.make_signature();
                        // Rust expects callers to zero-extend narrow integer arguments.
                        $(sig.params.push(match types::$p {
                            types::I8 => AbiParam::new(types::I8).uext(),
                            t => AbiParam::new(t),
                        });)*
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
    fail = novo_fail(I64, I32, I64);
    fail_msg = novo_fail_msg(I64, I64, I64, I64);
    ipow = novo_ipow(I64, I64, I64, I64) -> I64;
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
    show_value = novo_show_value(I64, I64);
    show_end = novo_show_end(I64);
    slots_init = novo_slots_init(I64, I64);
    slots_drop = novo_slots_drop(I64, I64);
    clone = novo_clone(I64, I64);
    take = novo_take(I64, I64);
    set_int = novo_set_int(I64, I64);
    set_float = novo_set_float(I64, F64);
    set_bool = novo_set_bool(I64, I8);
    set_none = novo_set_none(I64);
    set_str = novo_set_str(I64, I64);
    get_int = novo_get_int(I64) -> I64;
    get_float = novo_get_float(I64) -> F64;
    get_bool = novo_get_bool(I64) -> I8;
    binary = novo_binary(I64, I64, I32, I64, I64, I64);
    negate = novo_negate(I64, I64, I64, I64);
    index = novo_index(I64, I64, I64, I64, I64);
    set_index = novo_set_index(I64, I64, I64, I64, I64);
    slice = novo_slice(I64, I64, I64, I8, I64, I8, I64, I64);
    field = novo_field(I64, I64, I64, I64, I64, I64);
    set_field = novo_set_field(I64, I64, I64, I64, I64, I64);
    method = novo_method(I64, I64, I64, I64, I64, I64, I64, I64);
    builtin = novo_builtin(I64, I64, I32, I64, I64, I64);
    call_value = novo_call_value(I64, I64, I64, I64, I64, I64);
    call_interp = novo_call_interp(I64, I64, I32, I64, I64, I64);
    new = novo_new(I64, I32, I64, I64, I8);
    make_list = novo_make_list(I64, I64, I64);
    make_tuple = novo_make_tuple(I64, I64, I64);
    make_map = novo_make_map(I64, I64, I64, I64, I64, I64);
    make_closure = novo_make_closure(I64, I32, I64, I64);
    global_get = novo_global_get(I64, I64, I32, I64);
    global_set = novo_global_set(I64, I32, I64);
    items = novo_items(I64, I64, I64, I64);
    list_len = novo_list_len(I64) -> I64;
    list_get = novo_list_get(I64, I64, I64);
    list_push = novo_list_push(I64, I64);
    len = novo_len(I64, I64, I64) -> I64;
    map_get = novo_map_get(I64, I64, I64, I64, I64, I64);
    map_has = novo_map_has(I64, I64, I64, I64) -> I8;
    list_index = novo_list_index(I64, I64, I64, I64, I64);
    list_set = novo_list_set(I64, I64, I64, I64, I64);
    list_get_int = novo_list_get_int(I64, I64, I64, I64) -> I64;
    list_push_int = novo_list_push_int(I64, I64);
    is_variant = novo_is_variant(I64, I32) -> I8;
    lock_lists = novo_lock_lists(I64, I64, I64, I64, I64) -> I8;
    unlock_lists = novo_unlock_lists(I64, I64, I64);
    raise_index = novo_raise_index(I64, I64, I64, I64);
    list_push_float = novo_list_push_float(I64, F64);
    list_push_bool = novo_list_push_bool(I64, I8);
    list_get_float = novo_list_get_float(I64, I64, I64, I64) -> F64;
    list_get_bool = novo_list_get_bool(I64, I64, I64, I64) -> I8;
    list_set_int = novo_list_set_int(I64, I64, I64, I64, I64);
    list_set_float = novo_list_set_float(I64, I64, I64, F64, I64);
    list_set_bool = novo_list_set_bool(I64, I64, I64, I8, I64);
    run_parallel = novo_run_parallel(I64, I64, I64, I64, I64);
    parallel_map = novo_parallel_map(I64, I64, I64, I64, I64);
    try_mark = novo_try_mark(I64) -> I64;
    through = novo_through(I64, I32, I64);
    take_error = novo_take_error(I64, I64, I64, I8);
    error_is = novo_error_is(I64, I32) -> I8;
    init_module = novo_init_module(I64, I32, I64);
    eval_closed = novo_eval_closed(I64, I64, I64);
    together = novo_together(I64, I64, I64, I64);
    await_task = novo_await(I64, I64, I64, I64);
    fstr_begin = novo_fstr_begin(I64);
    fstr_lit = novo_fstr_lit(I64, I64, I64);
    fstr_value = novo_fstr_value(I64, I64, I64, I64, I64);
    fstr_end = novo_fstr_end(I64, I64);
}
