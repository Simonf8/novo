use crate::check::hir::{FuncId, Program};
use crate::check::types::ClassId;
use parking_lot::{Condvar, Mutex, RwLock};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    List(Arc<RwLock<Vec<Value>>>),
    Map(Arc<RwLock<Dict>>),
    Func(Arc<Closure>),
    Instance(Arc<Instance>),
    Task(Arc<TaskHandle>),
    Tensor(Arc<super::tensor::NTensor>),
    Optimizer(Arc<Mutex<super::tensor::Opt>>),
    #[allow(dead_code)]
    Py(crate::python::PyHandle),
}

#[derive(Debug)]
pub struct Closure {
    pub func: FuncId,
    pub captures: Vec<Value>,
}

#[derive(Debug)]
pub struct Instance {
    pub class: ClassId,
    pub fields: RwLock<Vec<Value>>,
}

#[derive(Debug, Default)]
pub struct TaskHandle {
    result: Mutex<Option<Result<Value, String>>>,
    ready: Condvar,
}

impl TaskHandle {
    pub fn complete(&self, result: Result<Value, String>) {
        *self.result.lock() = Some(result);
        self.ready.notify_all();
    }

    pub fn wait(&self) -> Result<Value, String> {
        let mut slot = self.result.lock();
        while slot.is_none() {
            self.ready.wait(&mut slot);
        }
        slot.clone().expect("task finished")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    None,
    Bool(bool),
    Int(i64),
    Str(Arc<str>),
}

impl Key {
    pub fn from_value(v: &Value) -> Result<Key, String> {
        Ok(match v {
            Value::None => Key::None,
            Value::Bool(b) => Key::Bool(*b),
            Value::Int(i) => Key::Int(*i),
            Value::Str(s) => Key::Str(s.clone()),
            other => return Err(format!("a {} cannot be a map key", other.type_name())),
        })
    }

    pub fn to_value(&self) -> Value {
        match self {
            Key::None => Value::None,
            Key::Bool(b) => Value::Bool(*b),
            Key::Int(i) => Value::Int(*i),
            Key::Str(s) => Value::Str(s.clone()),
        }
    }
}

/// An insertion-ordered map.
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: Vec<(Key, Value)>,
    index: HashMap<Key, usize>,
}

impl Dict {
    pub fn get(&self, key: &Key) -> Option<&Value> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn insert(&mut self, key: Key, value: Value) {
        match self.index.get(&key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    pub fn remove(&mut self, key: &Key) -> Option<Value> {
        let i = self.index.remove(key)?;
        let (_, value) = self.entries.remove(i);
        for slot in self.index.values_mut() {
            if *slot > i {
                *slot -= 1;
            }
        }
        Some(value)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Key, Value)> {
        self.entries.iter()
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Arc::new(RwLock::new(items)))
    }

    pub fn map(dict: Dict) -> Value {
        Value::Map(Arc::new(RwLock::new(dict)))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::None => "None",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Func(_) => "Function",
            Value::Instance(_) => "instance",
            Value::Task(_) => "Task",
            Value::Tensor(_) => "Tensor",
            Value::Optimizer(_) => "Optimizer",
            Value::Py(_) => "Python object",
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn display(&self, prog: &Program) -> String {
        let mut out = String::new();
        write_value(&mut out, self, prog, 0);
        out
    }
}

pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else if f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else if f.abs() >= 1e16 || f.abs() < 1e-4 {
        let s = format!("{f:e}");
        match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e:0>2}"),
            Some((m, e)) => format!("{m}e-{:0>2}", &e[1..]),
            None => s,
        }
    } else {
        format!("{f}")
    }
}

fn write_value(out: &mut String, v: &Value, prog: &Program, depth: usize) {
    if depth > 64 {
        out.push_str("...");
        return;
    }
    match v {
        Value::None => out.push_str("none"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => out.push_str(&format_float(*f)),
        Value::Str(s) => out.push_str(s),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.read().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, prog, depth + 1);
            }
            out.push(']');
        }
        Value::Map(dict) => {
            out.push('{');
            for (i, (k, val)) in dict.read().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, &k.to_value(), prog, depth + 1);
                out.push_str(": ");
                write_value(out, val, prog, depth + 1);
            }
            out.push('}');
        }
        Value::Func(c) => {
            let _ = write!(out, "<function {}>", prog.funcs[c.func.0 as usize].name);
        }
        Value::Instance(inst) => {
            let class = &prog.classes[inst.class.0 as usize];
            out.push_str(&class.name);
            out.push('(');
            for (i, (field, val)) in class.fields.iter().zip(inst.fields.read().iter()).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&field.name);
                out.push_str(": ");
                write_value(out, val, prog, depth + 1);
            }
            out.push(')');
        }
        Value::Task(_) => out.push_str("<task>"),
        Value::Tensor(t) => out.push_str(&super::tensor::display(t)),
        Value::Optimizer(o) => {
            let _ = write!(out, "<{:?} optimizer>", o.lock());
        }
        Value::Py(_) => out.push_str("<python object>"),
    }
}

pub fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => a.as_f64() == b.as_f64(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            if Arc::ptr_eq(x, y) {
                return true;
            }
            let (x, y) = (x.read().clone(), y.read().clone());
            x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| equal(p, q))
        }
        (Value::Map(x), Value::Map(y)) => {
            if Arc::ptr_eq(x, y) {
                return true;
            }
            let (x, y) = (x.read().clone(), y.read().clone());
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
        }
        (Value::Func(x), Value::Func(y)) => Arc::ptr_eq(x, y),
        (Value::Instance(x), Value::Instance(y)) => Arc::ptr_eq(x, y),
        (Value::Task(x), Value::Task(y)) => Arc::ptr_eq(x, y),
        (Value::Tensor(x), Value::Tensor(y)) => Arc::ptr_eq(x, y),
        (Value::Optimizer(x), Value::Optimizer(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

pub fn compare(a: &Value, b: &Value) -> Result<Ordering, String> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            a.as_f64().unwrap().partial_cmp(&b.as_f64().unwrap()).ok_or_else(|| "cannot order NaN".to_string())
        }
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        (Value::List(x), Value::List(y)) => {
            let (x, y) = (x.read().clone(), y.read().clone());
            for (p, q) in x.iter().zip(&y) {
                match compare(p, q)? {
                    Ordering::Equal => continue,
                    other => return Ok(other),
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => Err(format!("cannot compare a {} with a {}", a.type_name(), b.type_name())),
    }
}
