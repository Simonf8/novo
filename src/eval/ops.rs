use super::value::{compare, equal, Key, Value};
use crate::syntax::ast::BinOp;
use std::cmp::Ordering;

type R = Result<Value, String>;

fn overflow() -> String {
    "integer overflow".into()
}

fn zero() -> String {
    "division by zero".into()
}

fn floor_div(a: i64, b: i64) -> Option<i64> {
    let q = a.checked_div(b)?;
    if a % b != 0 && ((a < 0) != (b < 0)) { q.checked_sub(1) } else { Some(q) }
}

fn floor_mod(a: i64, b: i64) -> Option<i64> {
    let r = a.checked_rem(b)?;
    if r != 0 && ((r < 0) != (b < 0)) { Some(r + b) } else { Some(r) }
}

fn float_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && ((r < 0.0) != (b < 0.0)) { r + b } else { r }
}

pub fn binary(op: BinOp, a: &Value, b: &Value) -> R {
    use Value::*;
    let bad = || Err(format!("cannot apply `{}` to a {} and a {}", op.symbol(), a.type_name(), b.type_name()));
    match op {
        BinOp::Add => match (a, b) {
            (Int(x), Int(y)) => x.checked_add(*y).map(Int).ok_or_else(overflow),
            (Str(x), Str(y)) => Ok(Value::Str(format!("{x}{y}").into())),
            (List(x), List(y)) => {
                let mut items = x.read().clone();
                items.extend(y.read().iter().cloned());
                Ok(Value::list(items))
            }
            _ => float_op(a, b, |x, y| x + y).map_or_else(bad, Ok),
        },
        BinOp::Sub => match (a, b) {
            (Int(x), Int(y)) => x.checked_sub(*y).map(Int).ok_or_else(overflow),
            _ => float_op(a, b, |x, y| x - y).map_or_else(bad, Ok),
        },
        BinOp::Mul => match (a, b) {
            (Int(x), Int(y)) => x.checked_mul(*y).map(Int).ok_or_else(overflow),
            (Str(s), Int(n)) | (Int(n), Str(s)) => Ok(Value::Str(s.repeat((*n).max(0) as usize).into())),
            _ => float_op(a, b, |x, y| x * y).map_or_else(bad, Ok),
        },
        BinOp::Div => match (a.as_f64(), b.as_f64()) {
            (Some(_), Some(y)) if y == 0.0 => Err(zero()),
            (Some(x), Some(y)) => Ok(Float(x / y)),
            _ => bad(),
        },
        BinOp::FloorDiv => match (a, b) {
            (Int(_), Int(0)) => Err(zero()),
            (Int(x), Int(y)) => floor_div(*x, *y).map(Int).ok_or_else(overflow),
            _ => match (a.as_f64(), b.as_f64()) {
                (Some(_), Some(y)) if y == 0.0 => Err(zero()),
                (Some(x), Some(y)) => Ok(Float((x / y).floor())),
                _ => bad(),
            },
        },
        BinOp::Mod => match (a, b) {
            (Int(_), Int(0)) => Err(zero()),
            (Int(x), Int(y)) => floor_mod(*x, *y).map(Int).ok_or_else(overflow),
            _ => match (a.as_f64(), b.as_f64()) {
                (Some(_), Some(y)) if y == 0.0 => Err(zero()),
                (Some(x), Some(y)) => Ok(Float(float_mod(x, y))),
                _ => bad(),
            },
        },
        BinOp::Pow => match (a, b) {
            (Int(_), Int(e)) if *e < 0 => Err("negative exponent for an Int power; use a Float base".into()),
            (Int(x), Int(e)) => u32::try_from(*e).ok().and_then(|e| x.checked_pow(e)).map(Int).ok_or_else(overflow),
            _ => float_op(a, b, f64::powf).map_or_else(bad, Ok),
        },
        BinOp::Eq => Ok(Bool(equal(a, b))),
        BinOp::Ne => Ok(Bool(!equal(a, b))),
        BinOp::Lt => compare(a, b).map(|o| Bool(o == Ordering::Less)),
        BinOp::Le => compare(a, b).map(|o| Bool(o != Ordering::Greater)),
        BinOp::Gt => compare(a, b).map(|o| Bool(o == Ordering::Greater)),
        BinOp::Ge => compare(a, b).map(|o| Bool(o != Ordering::Less)),
        BinOp::In => contains(b, a).map(Bool),
        BinOp::And | BinOp::Or => unreachable!("short-circuit operators are evaluated lazily"),
        BinOp::MatMul => bad(),
    }
}

fn float_op(a: &Value, b: &Value, f: impl Fn(f64, f64) -> f64) -> Option<Value> {
    Some(Value::Float(f(a.as_f64()?, b.as_f64()?)))
}

pub fn contains(container: &Value, item: &Value) -> Result<bool, String> {
    match (container, item) {
        (Value::List(items), _) => Ok(items.read().iter().any(|x| equal(x, item))),
        (Value::Map(dict), _) => Ok(dict.read().get(&Key::from_value(item)?).is_some()),
        (Value::Str(s), Value::Str(sub)) => Ok(s.contains(&**sub)),
        _ => Err(format!("cannot test membership in a {}", container.type_name())),
    }
}

pub fn negate(v: &Value) -> R {
    match v {
        Value::Int(i) => i.checked_neg().map(Value::Int).ok_or_else(overflow),
        Value::Float(f) => Ok(Value::Float(-f)),
        other => Err(format!("cannot negate a {}", other.type_name())),
    }
}
