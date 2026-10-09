use crate::interpreter::Value;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn is_string_method(name: &str) -> bool {
    matches!(
        name,
        "split"
            | "trim"
            | "trim_start"
            | "trim_left"
            | "trim_end"
            | "trim_right"
            | "to_upper"
            | "upper"
            | "to_lower"
            | "lower"
            | "replace"
            | "starts_with"
            | "ends_with"
            | "contains"
            | "index_of"
            | "length"
            | "len"
            | "chars"
            | "repeat"
            | "slice"
            | "pad_start"
            | "pad_end"
    )
}

pub fn is_list_method(name: &str) -> bool {
    matches!(
        name,
        "push"
            | "append"
            | "pop"
            | "insert"
            | "remove"
            | "length"
            | "len"
            | "contains"
            | "index_of"
            | "slice"
            | "reverse"
            | "sort"
            | "join"
            | "clone"
            | "clear"
    )
}

pub fn is_map_method(name: &str) -> bool {
    matches!(
        name,
        "keys"
            | "values"
            | "entries"
            | "has"
            | "contains"
            | "remove"
            | "length"
            | "len"
            | "clone"
            | "clear"
    )
}

pub fn register_globals(globals: &mut HashMap<String, Value>) {
    globals.insert("math".to_string(), build_math_module());
    globals.insert("time".to_string(), build_time_module());
    globals.insert("fs".to_string(), build_fs_module());
    globals.insert("json".to_string(), build_json_module());
    globals.insert("env".to_string(), build_env_module());

    let global_funcs = [
        "len",
        "range",
        "int",
        "float",
        "str",
        "bool",
        "abs",
        "min",
        "max",
        "assert",
        "assert_eq",
        "parse_json",
    ];

    for name in global_funcs {
        globals.insert(
            name.to_string(),
            Value::NativeFunction {
                name: name.to_string(),
                bound_target: None,
            },
        );
    }
}

pub fn get_standard_module(name: &str) -> Option<Value> {
    match name {
        "math" => Some(build_math_module()),
        "time" => Some(build_time_module()),
        "fs" => Some(build_fs_module()),
        "json" => Some(build_json_module()),
        "env" => Some(build_env_module()),
        _ => None,
    }
}

fn build_math_module() -> Value {
    let mut map = HashMap::new();
    map.insert("pi".to_string(), Value::Float(std::f64::consts::PI));
    map.insert("e".to_string(), Value::Float(std::f64::consts::E));

    let math_funcs = [
        "sqrt", "pow", "abs", "floor", "ceil", "round", "sin", "cos", "tan", "log", "log10",
        "log2", "min", "max",
    ];

    for func in math_funcs {
        map.insert(
            func.to_string(),
            Value::NativeFunction {
                name: format!("math.{}", func),
                bound_target: None,
            },
        );
    }

    Value::Map(Arc::new(RwLock::new(map)))
}

fn build_time_module() -> Value {
    let mut map = HashMap::new();
    let time_funcs = ["now", "now_ms", "sleep"];

    for func in time_funcs {
        map.insert(
            func.to_string(),
            Value::NativeFunction {
                name: format!("time.{}", func),
                bound_target: None,
            },
        );
    }

    Value::Map(Arc::new(RwLock::new(map)))
}

fn build_fs_module() -> Value {
    let mut map = HashMap::new();
    let fs_funcs = [
        "read_file",
        "write_file",
        "append_file",
        "exists",
        "list_dir",
        "delete_file",
        "create_dir",
    ];

    for func in fs_funcs {
        map.insert(
            func.to_string(),
            Value::NativeFunction {
                name: format!("fs.{}", func),
                bound_target: None,
            },
        );
    }

    Value::Map(Arc::new(RwLock::new(map)))
}

fn build_json_module() -> Value {
    let mut map = HashMap::new();
    let json_funcs = ["parse", "stringify"];

    for func in json_funcs {
        map.insert(
            func.to_string(),
            Value::NativeFunction {
                name: format!("json.{}", func),
                bound_target: None,
            },
        );
    }

    Value::Map(Arc::new(RwLock::new(map)))
}

fn build_env_module() -> Value {
    let mut map = HashMap::new();
    let env_funcs = ["get", "set", "args"];

    for func in env_funcs {
        map.insert(
            func.to_string(),
            Value::NativeFunction {
                name: format!("env.{}", func),
                bound_target: None,
            },
        );
    }

    Value::Map(Arc::new(RwLock::new(map)))
}

pub fn call_native_function(
    name: &str,
    bound_target: Option<&Value>,
    args: &[Value],
) -> Result<Value, String> {
    if let Some(target) = bound_target {
        match target {
            Value::String(s) => return call_string_method(name, s, args),
            Value::List(l) => return call_list_method(name, l, args),
            Value::Map(m) => return call_map_method(name, m, args),
            _ => {}
        }
    }

    if let Some(stripped) = name.strip_prefix("math.") {
        return call_math_func(stripped, args);
    }
    if let Some(stripped) = name.strip_prefix("time.") {
        return call_time_func(stripped, args);
    }
    if let Some(stripped) = name.strip_prefix("fs.") {
        return call_fs_func(stripped, args);
    }
    if let Some(stripped) = name.strip_prefix("json.") {
        return call_json_func(stripped, args);
    }
    if let Some(stripped) = name.strip_prefix("env.") {
        return call_env_func(stripped, args);
    }

    match name {
        "len" => {
            if args.len() != 1 {
                return Err("len() requires exactly 1 argument".to_string());
            }
            match &args[0] {
                Value::String(s) => Ok(Value::Integer(s.chars().count() as i64)),
                Value::List(l) => Ok(Value::Integer(l.read().len() as i64)),
                Value::Map(m) => Ok(Value::Integer(m.read().len() as i64)),
                other => Err(format!("len() not supported for {:?}", other)),
            }
        }
        "range" => {
            let (start, end, step) = match args.len() {
                1 => match &args[0] {
                    Value::Integer(e) => (0, *e, 1),
                    _ => return Err("range() arguments must be integers".to_string()),
                },
                2 => match (&args[0], &args[1]) {
                    (Value::Integer(s), Value::Integer(e)) => (*s, *e, 1),
                    _ => return Err("range() arguments must be integers".to_string()),
                },
                3 => match (&args[0], &args[1], &args[2]) {
                    (Value::Integer(s), Value::Integer(e), Value::Integer(st)) => {
                        if *st == 0 {
                            return Err("range() step cannot be 0".to_string());
                        }
                        (*s, *e, *st)
                    }
                    _ => return Err("range() arguments must be integers".to_string()),
                },
                _ => return Err("range() takes 1, 2, or 3 arguments".to_string()),
            };

            let mut values = Vec::new();
            if step > 0 {
                let mut curr = start;
                while curr < end {
                    values.push(Value::Integer(curr));
                    curr += step;
                }
            } else {
                let mut curr = start;
                while curr > end {
                    values.push(Value::Integer(curr));
                    curr += step;
                }
            }
            Ok(Value::List(Arc::new(RwLock::new(values))))
        }
        "int" => {
            if args.len() != 1 {
                return Err("int() requires exactly 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(*i)),
                Value::Float(f) => Ok(Value::Integer(*f as i64)),
                Value::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map(Value::Integer)
                    .map_err(|_| format!("Cannot parse integer from '{}'", s)),
                Value::Boolean(b) => Ok(Value::Integer(if *b { 1 } else { 0 })),
                other => Err(format!("Cannot convert {:?} to int", other)),
            }
        }
        "float" => {
            if args.len() != 1 {
                return Err("float() requires exactly 1 argument".to_string());
            }
            match &args[0] {
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Integer(i) => Ok(Value::Float(*i as f64)),
                Value::String(s) => s
                    .trim()
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| format!("Cannot parse float from '{}'", s)),
                Value::Boolean(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                other => Err(format!("Cannot convert {:?} to float", other)),
            }
        }
        "str" => {
            if args.len() != 1 {
                return Err("str() requires exactly 1 argument".to_string());
            }
            Ok(Value::String(format!("{}", args[0])))
        }
        "bool" => {
            if args.len() != 1 {
                return Err("bool() requires exactly 1 argument".to_string());
            }
            Ok(Value::Boolean(is_truthy(&args[0])))
        }
        "abs" => {
            if args.len() != 1 {
                return Err("abs() requires exactly 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(i.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("abs() requires numeric argument".to_string()),
            }
        }
        "min" => {
            if args.len() != 2 {
                return Err("min() requires exactly 2 arguments".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer((*a).min(*b))),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float((*a).min(*b))),
                (Value::Integer(a), Value::Float(b)) => Ok(Value::Float((*a as f64).min(*b))),
                (Value::Float(a), Value::Integer(b)) => Ok(Value::Float((*a).min(*b as f64))),
                _ => Err("min() requires numeric arguments".to_string()),
            }
        }
        "max" => {
            if args.len() != 2 {
                return Err("max() requires exactly 2 arguments".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer((*a).max(*b))),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float((*a).max(*b))),
                (Value::Integer(a), Value::Float(b)) => Ok(Value::Float((*a as f64).max(*b))),
                (Value::Float(a), Value::Integer(b)) => Ok(Value::Float((*a).max(*b as f64))),
                _ => Err("max() requires numeric arguments".to_string()),
            }
        }
        "assert" => {
            if args.is_empty() || args.len() > 2 {
                return Err("assert() takes 1 or 2 arguments".to_string());
            }
            if !is_truthy(&args[0]) {
                let msg = if args.len() == 2 {
                    format!("{}", args[1])
                } else {
                    "Assertion failed".to_string()
                };
                return Err(msg);
            }
            Ok(Value::Null)
        }
        "assert_eq" => {
            if args.len() < 2 || args.len() > 3 {
                return Err("assert_eq() takes 2 or 3 arguments".to_string());
            }
            if args[0] != args[1] {
                let msg = if args.len() == 3 {
                    format!("{}", args[2])
                } else {
                    format!("Assertion failed: expected '{}', got '{}'", args[1], args[0])
                };
                return Err(msg);
            }
            Ok(Value::Null)
        }
        "parse_json" => {
            if args.len() != 1 {
                return Err("parse_json() requires exactly 1 argument".to_string());
            }
            if let Value::String(s) = &args[0] {
                parse_json_str(s)
            } else {
                Err("parse_json() requires string argument".to_string())
            }
        }
        _ => Err(format!("Unknown native function: {}", name)),
    }
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Boolean(b) => *b,
        Value::Null => false,
        Value::Integer(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::List(l) => !l.read().is_empty(),
        Value::Map(m) => !m.read().is_empty(),
        _ => true,
    }
}

fn call_string_method(name: &str, s: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        "split" => {
            if args.is_empty() {
                let parts: Vec<Value> = s
                    .split_whitespace()
                    .map(|p| Value::String(p.to_string()))
                    .collect();
                Ok(Value::List(Arc::new(RwLock::new(parts))))
            } else if args.len() == 1 {
                if let Value::String(sep) = &args[0] {
                    let parts: Vec<Value> =
                        s.split(sep).map(|p| Value::String(p.to_string())).collect();
                    Ok(Value::List(Arc::new(RwLock::new(parts))))
                } else {
                    Err("split() requires string separator".to_string())
                }
            } else {
                Err("split() takes 0 or 1 argument".to_string())
            }
        }
        "trim" => {
            if !args.is_empty() {
                return Err("trim() takes 0 arguments".to_string());
            }
            Ok(Value::String(s.trim().to_string()))
        }
        "trim_start" | "trim_left" => {
            if !args.is_empty() {
                return Err("trim_start() takes 0 arguments".to_string());
            }
            Ok(Value::String(s.trim_start().to_string()))
        }
        "trim_end" | "trim_right" => {
            if !args.is_empty() {
                return Err("trim_end() takes 0 arguments".to_string());
            }
            Ok(Value::String(s.trim_end().to_string()))
        }
        "to_upper" | "upper" => {
            if !args.is_empty() {
                return Err("to_upper() takes 0 arguments".to_string());
            }
            Ok(Value::String(s.to_uppercase()))
        }
        "to_lower" | "lower" => {
            if !args.is_empty() {
                return Err("to_lower() takes 0 arguments".to_string());
            }
            Ok(Value::String(s.to_lowercase()))
        }
        "replace" => {
            if args.len() != 2 {
                return Err("replace() requires exactly 2 arguments (from, to)".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::String(from), Value::String(to)) => {
                    Ok(Value::String(s.replace(from, to)))
                }
                _ => Err("replace() requires string arguments".to_string()),
            }
        }
        "starts_with" => {
            if args.len() != 1 {
                return Err("starts_with() requires 1 argument".to_string());
            }
            if let Value::String(prefix) = &args[0] {
                Ok(Value::Boolean(s.starts_with(prefix)))
            } else {
                Err("starts_with() requires string prefix".to_string())
            }
        }
        "ends_with" => {
            if args.len() != 1 {
                return Err("ends_with() requires 1 argument".to_string());
            }
            if let Value::String(suffix) = &args[0] {
                Ok(Value::Boolean(s.ends_with(suffix)))
            } else {
                Err("ends_with() requires string suffix".to_string())
            }
        }
        "contains" => {
            if args.len() != 1 {
                return Err("contains() requires 1 argument".to_string());
            }
            if let Value::String(sub) = &args[0] {
                Ok(Value::Boolean(s.contains(sub)))
            } else {
                Err("contains() requires string argument".to_string())
            }
        }
        "index_of" => {
            if args.len() != 1 {
                return Err("index_of() requires 1 argument".to_string());
            }
            if let Value::String(sub) = &args[0] {
                Ok(Value::Integer(
                    s.find(sub).map(|idx| idx as i64).unwrap_or(-1),
                ))
            } else {
                Err("index_of() requires string argument".to_string())
            }
        }
        "length" | "len" => {
            if !args.is_empty() {
                return Err("length() takes 0 arguments".to_string());
            }
            Ok(Value::Integer(s.chars().count() as i64))
        }
        "chars" => {
            if !args.is_empty() {
                return Err("chars() takes 0 arguments".to_string());
            }
            let list: Vec<Value> = s.chars().map(|c| Value::String(c.to_string())).collect();
            Ok(Value::List(Arc::new(RwLock::new(list))))
        }
        "repeat" => {
            if args.len() != 1 {
                return Err("repeat() requires count argument".to_string());
            }
            if let Value::Integer(n) = args[0] {
                if n < 0 {
                    return Err("repeat() count cannot be negative".to_string());
                }
                Ok(Value::String(s.repeat(n as usize)))
            } else {
                Err("repeat() requires integer count".to_string())
            }
        }
        "slice" => {
            let len = s.chars().count() as i64;
            let start = if !args.is_empty() {
                match args[0] {
                    Value::Integer(i) => {
                        if i < 0 {
                            (len + i).max(0) as usize
                        } else {
                            (i as usize).min(len as usize)
                        }
                    }
                    _ => return Err("slice() start must be integer".to_string()),
                }
            } else {
                0
            };

            let end = if args.len() > 1 {
                match args[1] {
                    Value::Integer(i) => {
                        if i < 0 {
                            (len + i).max(0) as usize
                        } else {
                            (i as usize).min(len as usize)
                        }
                    }
                    _ => return Err("slice() end must be integer".to_string()),
                }
            } else {
                len as usize
            };

            let chars: Vec<char> = s.chars().collect();
            let slice_str: String = if start < end {
                chars[start..end].iter().collect()
            } else {
                String::new()
            };
            Ok(Value::String(slice_str))
        }
        "pad_start" => {
            if args.is_empty() || args.len() > 2 {
                return Err("pad_start() requires target length and optional pad char".to_string());
            }
            let target_len = match args[0] {
                Value::Integer(i) if i >= 0 => i as usize,
                _ => return Err("pad_start() length must be non-negative integer".to_string()),
            };
            let pad_char = if args.len() == 2 {
                match &args[1] {
                    Value::String(p) if !p.is_empty() => p.chars().next().unwrap(),
                    _ => return Err("pad_start() pad character must be non-empty string".to_string()),
                }
            } else {
                ' '
            };
            let char_count = s.chars().count();
            if char_count >= target_len {
                Ok(Value::String(s.to_string()))
            } else {
                let padding: String = std::iter::repeat(pad_char).take(target_len - char_count).collect();
                Ok(Value::String(format!("{}{}", padding, s)))
            }
        }
        "pad_end" => {
            if args.is_empty() || args.len() > 2 {
                return Err("pad_end() requires target length and optional pad char".to_string());
            }
            let target_len = match args[0] {
                Value::Integer(i) if i >= 0 => i as usize,
                _ => return Err("pad_end() length must be non-negative integer".to_string()),
            };
            let pad_char = if args.len() == 2 {
                match &args[1] {
                    Value::String(p) if !p.is_empty() => p.chars().next().unwrap(),
                    _ => return Err("pad_end() pad character must be non-empty string".to_string()),
                }
            } else {
                ' '
            };
            let char_count = s.chars().count();
            if char_count >= target_len {
                Ok(Value::String(s.to_string()))
            } else {
                let padding: String = std::iter::repeat(pad_char).take(target_len - char_count).collect();
                Ok(Value::String(format!("{}{}", s, padding)))
            }
        }
        _ => Err(format!("Unknown string method: {}", name)),
    }
}

fn call_list_method(
    name: &str,
    l: &Arc<RwLock<Vec<Value>>>,
    args: &[Value],
) -> Result<Value, String> {
    match name {
        "push" | "append" => {
            if args.len() != 1 {
                return Err(format!("{}() requires exactly 1 argument", name));
            }
            l.write().push(args[0].clone());
            Ok(Value::Null)
        }
        "pop" => {
            if !args.is_empty() {
                return Err("pop() takes 0 arguments".to_string());
            }
            Ok(l.write().pop().unwrap_or(Value::Null))
        }
        "insert" => {
            if args.len() != 2 {
                return Err("insert() requires index and item arguments".to_string());
            }
            let idx = match args[0] {
                Value::Integer(i) if i >= 0 => i as usize,
                _ => return Err("insert() index must be non-negative integer".to_string()),
            };
            let mut list = l.write();
            if idx <= list.len() {
                list.insert(idx, args[1].clone());
                Ok(Value::Null)
            } else {
                Err(format!("insert() index {} out of range (length {})", idx, list.len()))
            }
        }
        "remove" => {
            if args.len() != 1 {
                return Err("remove() requires index argument".to_string());
            }
            let idx = match args[0] {
                Value::Integer(i) if i >= 0 => i as usize,
                _ => return Err("remove() index must be non-negative integer".to_string()),
            };
            let mut list = l.write();
            if idx < list.len() {
                Ok(list.remove(idx))
            } else {
                Err(format!("remove() index {} out of bounds (length {})", idx, list.len()))
            }
        }
        "length" | "len" => {
            if !args.is_empty() {
                return Err("length() takes 0 arguments".to_string());
            }
            Ok(Value::Integer(l.read().len() as i64))
        }
        "contains" => {
            if args.len() != 1 {
                return Err("contains() requires 1 argument".to_string());
            }
            let target = &args[0];
            let list = l.read();
            Ok(Value::Boolean(list.iter().any(|item| item == target)))
        }
        "index_of" => {
            if args.len() != 1 {
                return Err("index_of() requires 1 argument".to_string());
            }
            let target = &args[0];
            let list = l.read();
            for (idx, item) in list.iter().enumerate() {
                if item == target {
                    return Ok(Value::Integer(idx as i64));
                }
            }
            Ok(Value::Integer(-1))
        }
        "slice" => {
            let list = l.read();
            let len = list.len() as i64;
            let start = if !args.is_empty() {
                match args[0] {
                    Value::Integer(i) => {
                        if i < 0 {
                            (len + i).max(0) as usize
                        } else {
                            (i as usize).min(len as usize)
                        }
                    }
                    _ => return Err("slice() start must be integer".to_string()),
                }
            } else {
                0
            };

            let end = if args.len() > 1 {
                match args[1] {
                    Value::Integer(i) => {
                        if i < 0 {
                            (len + i).max(0) as usize
                        } else {
                            (i as usize).min(len as usize)
                        }
                    }
                    _ => return Err("slice() end must be integer".to_string()),
                }
            } else {
                len as usize
            };

            let slice_items: Vec<Value> = if start < end {
                list[start..end].to_vec()
            } else {
                Vec::new()
            };
            Ok(Value::List(Arc::new(RwLock::new(slice_items))))
        }
        "reverse" => {
            if !args.is_empty() {
                return Err("reverse() takes 0 arguments".to_string());
            }
            l.write().reverse();
            Ok(Value::Null)
        }
        "sort" => {
            if !args.is_empty() {
                return Err("sort() takes 0 arguments".to_string());
            }
            let mut list = l.write();
            list.sort_by(|a, b| match (a, b) {
                (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
                (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                (Value::Integer(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                (Value::Float(x), Value::Integer(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(std::cmp::Ordering::Equal),
                (Value::String(x), Value::String(y)) => x.cmp(y),
                _ => std::cmp::Ordering::Equal,
            });
            Ok(Value::Null)
        }
        "join" => {
            let sep = if args.is_empty() {
                "".to_string()
            } else if args.len() == 1 {
                if let Value::String(s) = &args[0] {
                    s.clone()
                } else {
                    return Err("join() separator must be string".to_string());
                }
            } else {
                return Err("join() takes 0 or 1 argument".to_string());
            };
            let list = l.read();
            let joined: Vec<String> = list.iter().map(|item| format!("{}", item)).collect();
            Ok(Value::String(joined.join(&sep)))
        }
        "clone" => {
            if !args.is_empty() {
                return Err("clone() takes 0 arguments".to_string());
            }
            let cloned = l.read().clone();
            Ok(Value::List(Arc::new(RwLock::new(cloned))))
        }
        "clear" => {
            if !args.is_empty() {
                return Err("clear() takes 0 arguments".to_string());
            }
            l.write().clear();
            Ok(Value::Null)
        }
        _ => Err(format!("Unknown list method: {}", name)),
    }
}

fn call_map_method(
    name: &str,
    m: &Arc<RwLock<HashMap<String, Value>>>,
    args: &[Value],
) -> Result<Value, String> {
    match name {
        "keys" => {
            if !args.is_empty() {
                return Err("keys() takes 0 arguments".to_string());
            }
            let keys: Vec<Value> = m
                .read()
                .keys()
                .cloned()
                .map(Value::String)
                .collect();
            Ok(Value::List(Arc::new(RwLock::new(keys))))
        }
        "values" => {
            if !args.is_empty() {
                return Err("values() takes 0 arguments".to_string());
            }
            let values: Vec<Value> = m.read().values().cloned().collect();
            Ok(Value::List(Arc::new(RwLock::new(values))))
        }
        "entries" => {
            if !args.is_empty() {
                return Err("entries() takes 0 arguments".to_string());
            }
            let entries: Vec<Value> = m
                .read()
                .iter()
                .map(|(k, v)| {
                    Value::List(Arc::new(RwLock::new(vec![
                        Value::String(k.clone()),
                        v.clone(),
                    ])))
                })
                .collect();
            Ok(Value::List(Arc::new(RwLock::new(entries))))
        }
        "has" | "contains" => {
            if args.len() != 1 {
                return Err(format!("{}() requires key argument", name));
            }
            if let Value::String(k) = &args[0] {
                Ok(Value::Boolean(m.read().contains_key(k)))
            } else {
                Err(format!("{}() key must be string", name))
            }
        }
        "remove" => {
            if args.len() != 1 {
                return Err("remove() requires key argument".to_string());
            }
            if let Value::String(k) = &args[0] {
                Ok(m.write().remove(k).unwrap_or(Value::Null))
            } else {
                Err("remove() key must be string".to_string())
            }
        }
        "length" | "len" => {
            if !args.is_empty() {
                return Err("length() takes 0 arguments".to_string());
            }
            Ok(Value::Integer(m.read().len() as i64))
        }
        "clone" => {
            if !args.is_empty() {
                return Err("clone() takes 0 arguments".to_string());
            }
            let cloned = m.read().clone();
            Ok(Value::Map(Arc::new(RwLock::new(cloned))))
        }
        "clear" => {
            if !args.is_empty() {
                return Err("clear() takes 0 arguments".to_string());
            }
            m.write().clear();
            Ok(Value::Null)
        }
        _ => Err(format!("Unknown map method: {}", name)),
    }
}

fn call_math_func(func: &str, args: &[Value]) -> Result<Value, String> {
    match func {
        "sqrt" => {
            if args.len() != 1 {
                return Err("math.sqrt() requires exactly 1 argument".to_string());
            }
            let val = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.sqrt() requires numeric argument".to_string()),
            };
            if val < 0.0 {
                return Err("math.sqrt() of negative number".to_string());
            }
            Ok(Value::Float(val.sqrt()))
        }
        "pow" => {
            if args.len() != 2 {
                return Err("math.pow() requires 2 arguments (base, exponent)".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::Integer(b), Value::Integer(e)) if *e >= 0 => {
                    Ok(Value::Integer(b.pow(*e as u32)))
                }
                (b, e) => {
                    let base = match b {
                        Value::Integer(i) => *i as f64,
                        Value::Float(f) => *f,
                        _ => return Err("math.pow() requires numeric arguments".to_string()),
                    };
                    let exp = match e {
                        Value::Integer(i) => *i as f64,
                        Value::Float(f) => *f,
                        _ => return Err("math.pow() requires numeric arguments".to_string()),
                    };
                    Ok(Value::Float(base.powf(exp)))
                }
            }
        }
        "abs" => {
            if args.len() != 1 {
                return Err("math.abs() requires 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(i.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("math.abs() requires numeric argument".to_string()),
            }
        }
        "floor" => {
            if args.len() != 1 {
                return Err("math.floor() requires 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(*i)),
                Value::Float(f) => Ok(Value::Integer(f.floor() as i64)),
                _ => Err("math.floor() requires numeric argument".to_string()),
            }
        }
        "ceil" => {
            if args.len() != 1 {
                return Err("math.ceil() requires 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(*i)),
                Value::Float(f) => Ok(Value::Integer(f.ceil() as i64)),
                _ => Err("math.ceil() requires numeric argument".to_string()),
            }
        }
        "round" => {
            if args.len() != 1 {
                return Err("math.round() requires 1 argument".to_string());
            }
            match &args[0] {
                Value::Integer(i) => Ok(Value::Integer(*i)),
                Value::Float(f) => Ok(Value::Integer(f.round() as i64)),
                _ => Err("math.round() requires numeric argument".to_string()),
            }
        }
        "sin" => {
            if args.len() != 1 {
                return Err("math.sin() requires 1 argument".to_string());
            }
            let rad = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.sin() requires numeric argument".to_string()),
            };
            Ok(Value::Float(rad.sin()))
        }
        "cos" => {
            if args.len() != 1 {
                return Err("math.cos() requires 1 argument".to_string());
            }
            let rad = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.cos() requires numeric argument".to_string()),
            };
            Ok(Value::Float(rad.cos()))
        }
        "tan" => {
            if args.len() != 1 {
                return Err("math.tan() requires 1 argument".to_string());
            }
            let rad = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.tan() requires numeric argument".to_string()),
            };
            Ok(Value::Float(rad.tan()))
        }
        "log" => {
            if args.len() != 1 {
                return Err("math.log() requires 1 argument".to_string());
            }
            let val = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.log() requires numeric argument".to_string()),
            };
            if val <= 0.0 {
                return Err("math.log() of non-positive number".to_string());
            }
            Ok(Value::Float(val.ln()))
        }
        "log10" => {
            if args.len() != 1 {
                return Err("math.log10() requires 1 argument".to_string());
            }
            let val = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.log10() requires numeric argument".to_string()),
            };
            if val <= 0.0 {
                return Err("math.log10() of non-positive number".to_string());
            }
            Ok(Value::Float(val.log10()))
        }
        "log2" => {
            if args.len() != 1 {
                return Err("math.log2() requires 1 argument".to_string());
            }
            let val = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("math.log2() requires numeric argument".to_string()),
            };
            if val <= 0.0 {
                return Err("math.log2() of non-positive number".to_string());
            }
            Ok(Value::Float(val.log2()))
        }
        "min" => call_native_function("min", None, args),
        "max" => call_native_function("max", None, args),
        _ => Err(format!("Unknown math function: math.{}", func)),
    }
}

fn call_time_func(func: &str, args: &[Value]) -> Result<Value, String> {
    match func {
        "now" => {
            if !args.is_empty() {
                return Err("time.now() takes 0 arguments".to_string());
            }
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs_f64();
            Ok(Value::Float(secs))
        }
        "now_ms" => {
            if !args.is_empty() {
                return Err("time.now_ms() takes 0 arguments".to_string());
            }
            let ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis() as i64;
            Ok(Value::Integer(ms))
        }
        "sleep" => {
            if args.len() != 1 {
                return Err("time.sleep() requires 1 duration argument".to_string());
            }
            let secs = match args[0] {
                Value::Integer(i) => i as f64,
                Value::Float(f) => f,
                _ => return Err("time.sleep() duration must be a number".to_string()),
            };
            if secs > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(secs));
            }
            Ok(Value::Null)
        }
        _ => Err(format!("Unknown time function: time.{}", func)),
    }
}

fn call_fs_func(func: &str, args: &[Value]) -> Result<Value, String> {
    match func {
        "read_file" => {
            if args.len() != 1 {
                return Err("fs.read_file() requires path argument".to_string());
            }
            if let Value::String(path) = &args[0] {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| format!("fs.read_file('{}') failed: {}", path, e))?;
                Ok(Value::String(content))
            } else {
                Err("fs.read_file() path must be string".to_string())
            }
        }
        "write_file" => {
            if args.len() != 2 {
                return Err("fs.write_file() requires path and content arguments".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::String(path), Value::String(content)) => {
                    std::fs::write(path, content)
                        .map_err(|e| format!("fs.write_file('{}') failed: {}", path, e))?;
                    Ok(Value::Null)
                }
                _ => Err("fs.write_file() arguments must be strings".to_string()),
            }
        }
        "append_file" => {
            if args.len() != 2 {
                return Err("fs.append_file() requires path and content arguments".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::String(path), Value::String(content)) => {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| format!("fs.append_file('{}') failed: {}", path, e))?;
                    file.write_all(content.as_bytes())
                        .map_err(|e| format!("fs.append_file('{}') failed: {}", path, e))?;
                    Ok(Value::Null)
                }
                _ => Err("fs.append_file() arguments must be strings".to_string()),
            }
        }
        "exists" => {
            if args.len() != 1 {
                return Err("fs.exists() requires path argument".to_string());
            }
            if let Value::String(path) = &args[0] {
                Ok(Value::Boolean(std::path::Path::new(path).exists()))
            } else {
                Err("fs.exists() path must be string".to_string())
            }
        }
        "list_dir" => {
            if args.len() != 1 {
                return Err("fs.list_dir() requires path argument".to_string());
            }
            if let Value::String(path) = &args[0] {
                let read_dir = std::fs::read_dir(path)
                    .map_err(|e| format!("fs.list_dir('{}') failed: {}", path, e))?;
                let mut entries = Vec::new();
                for entry in read_dir {
                    if let Ok(entry) = entry {
                        if let Some(name) = entry.file_name().to_str() {
                            entries.push(Value::String(name.to_string()));
                        }
                    }
                }
                Ok(Value::List(Arc::new(RwLock::new(entries))))
            } else {
                Err("fs.list_dir() path must be string".to_string())
            }
        }
        "delete_file" => {
            if args.len() != 1 {
                return Err("fs.delete_file() requires path argument".to_string());
            }
            if let Value::String(path) = &args[0] {
                std::fs::remove_file(path)
                    .map_err(|e| format!("fs.delete_file('{}') failed: {}", path, e))?;
                Ok(Value::Null)
            } else {
                Err("fs.delete_file() path must be string".to_string())
            }
        }
        "create_dir" => {
            if args.len() != 1 {
                return Err("fs.create_dir() requires path argument".to_string());
            }
            if let Value::String(path) = &args[0] {
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("fs.create_dir('{}') failed: {}", path, e))?;
                Ok(Value::Null)
            } else {
                Err("fs.create_dir() path must be string".to_string())
            }
        }
        _ => Err(format!("Unknown fs function: fs.{}", func)),
    }
}

fn call_json_func(func: &str, args: &[Value]) -> Result<Value, String> {
    match func {
        "parse" => {
            if args.len() != 1 {
                return Err("json.parse() requires exactly 1 string argument".to_string());
            }
            if let Value::String(s) = &args[0] {
                parse_json_str(s)
            } else {
                Err("json.parse() argument must be string".to_string())
            }
        }
        "stringify" => {
            if args.len() != 1 {
                return Err("json.stringify() requires exactly 1 argument".to_string());
            }
            Ok(Value::String(stringify_json_value(&args[0])))
        }
        _ => Err(format!("Unknown json function: json.{}", func)),
    }
}

fn call_env_func(func: &str, args: &[Value]) -> Result<Value, String> {
    match func {
        "get" => {
            if args.is_empty() || args.len() > 2 {
                return Err("env.get() requires key and optional default".to_string());
            }
            if let Value::String(k) = &args[0] {
                match std::env::var(k) {
                    Ok(v) => Ok(Value::String(v)),
                    Err(_) => {
                        if args.len() == 2 {
                            Ok(args[1].clone())
                        } else {
                            Ok(Value::Null)
                        }
                    }
                }
            } else {
                Err("env.get() key must be string".to_string())
            }
        }
        "set" => {
            if args.len() != 2 {
                return Err("env.set() requires key and value arguments".to_string());
            }
            match (&args[0], &args[1]) {
                (Value::String(k), Value::String(v)) => {
                    unsafe {
                        std::env::set_var(k, v);
                    }
                    Ok(Value::Null)
                }
                _ => Err("env.set() arguments must be strings".to_string()),
            }
        }
        "args" => {
            if !args.is_empty() {
                return Err("env.args() takes 0 arguments".to_string());
            }
            let args_list: Vec<Value> = std::env::args().map(Value::String).collect();
            Ok(Value::List(Arc::new(RwLock::new(args_list))))
        }
        _ => Err(format!("Unknown env function: env.{}", func)),
    }
}

/// Parses CSV text with a header row into a list of maps from column name to field text.
pub fn parse_csv(input: &str) -> Value {
    let mut rows = csv_records(input).into_iter();
    let header = rows.next().unwrap_or_default();
    let records = rows
        .filter(|r| !(r.len() == 1 && r[0].is_empty()))
        .map(|r| {
            let map = header.iter().cloned().zip(r.into_iter().map(Value::String)).collect();
            Value::Map(Arc::new(RwLock::new(map)))
        })
        .collect();
    Value::List(Arc::new(RwLock::new(records)))
}

fn csv_records(input: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' if quoted => quoted = false,
            '"' if field.is_empty() => quoted = true,
            ',' if !quoted => record.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

pub fn parse_json_str(input: &str) -> Result<Value, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    skip_whitespace(&chars, &mut pos);
    let val = parse_json_val(&chars, &mut pos)?;
    skip_whitespace(&chars, &mut pos);
    if pos < chars.len() {
        return Err(format!("Unexpected character '{}' at index {}", chars[pos], pos));
    }
    Ok(val)
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn parse_json_val(chars: &[char], pos: &mut usize) -> Result<Value, String> {
    skip_whitespace(chars, pos);
    if *pos >= chars.len() {
        return Err("Unexpected end of JSON input".to_string());
    }

    match chars[*pos] {
        'n' => {
            if *pos + 4 <= chars.len() && &chars[*pos..*pos + 4] == ['n', 'u', 'l', 'l'] {
                *pos += 4;
                Ok(Value::Null)
            } else {
                Err(format!("Invalid token at index {}", pos))
            }
        }
        't' => {
            if *pos + 4 <= chars.len() && &chars[*pos..*pos + 4] == ['t', 'r', 'u', 'e'] {
                *pos += 4;
                Ok(Value::Boolean(true))
            } else {
                Err(format!("Invalid token at index {}", pos))
            }
        }
        'f' => {
            if *pos + 5 <= chars.len() && &chars[*pos..*pos + 5] == ['f', 'a', 'l', 's', 'e'] {
                *pos += 5;
                Ok(Value::Boolean(false))
            } else {
                Err(format!("Invalid token at index {}", pos))
            }
        }
        '"' => parse_json_string(chars, pos).map(Value::String),
        '[' => parse_json_array(chars, pos),
        '{' => parse_json_object(chars, pos),
        '-' | '0'..='9' => parse_json_number(chars, pos),
        c => Err(format!("Unexpected token '{}' at index {}", c, pos)),
    }
}

fn parse_json_string(chars: &[char], pos: &mut usize) -> Result<String, String> {
    *pos += 1; // skip opening "
    let mut s = String::new();
    while *pos < chars.len() {
        match chars[*pos] {
            '"' => {
                *pos += 1;
                return Ok(s);
            }
            '\\' => {
                *pos += 1;
                if *pos >= chars.len() {
                    return Err("Unterminated escape sequence in string".to_string());
                }
                match chars[*pos] {
                    '"' => s.push('"'),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'b' => s.push('\x08'),
                    'f' => s.push('\x0c'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    'u' => {
                        *pos += 1;
                        if *pos + 4 > chars.len() {
                            return Err("Invalid unicode escape sequence".to_string());
                        }
                        let hex: String = chars[*pos..*pos + 4].iter().collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| "Invalid hex in unicode escape".to_string())?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| "Invalid unicode codepoint".to_string())?;
                        s.push(ch);
                        *pos += 3; // +1 at loop end
                    }
                    other => return Err(format!("Unknown escape '\\{}'", other)),
                }
                *pos += 1;
            }
            c => {
                s.push(c);
                *pos += 1;
            }
        }
    }
    Err("Unterminated string in JSON".to_string())
}

fn parse_json_number(chars: &[char], pos: &mut usize) -> Result<Value, String> {
    let start = *pos;
    if chars[*pos] == '-' {
        *pos += 1;
    }
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let mut is_float = false;
    if *pos < chars.len() && chars[*pos] == '.' {
        is_float = true;
        *pos += 1;
        while *pos < chars.len() && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }
    if *pos < chars.len() && (chars[*pos] == 'e' || chars[*pos] == 'E') {
        is_float = true;
        *pos += 1;
        if *pos < chars.len() && (chars[*pos] == '+' || chars[*pos] == '-') {
            *pos += 1;
        }
        while *pos < chars.len() && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }
    let num_str: String = chars[start..*pos].iter().collect();
    if is_float {
        num_str
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("Invalid float literal '{}'", num_str))
    } else {
        num_str
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| format!("Invalid integer literal '{}'", num_str))
    }
}

fn parse_json_array(chars: &[char], pos: &mut usize) -> Result<Value, String> {
    *pos += 1; // skip [
    skip_whitespace(chars, pos);
    let mut items = Vec::new();
    if *pos < chars.len() && chars[*pos] == ']' {
        *pos += 1;
        return Ok(Value::List(Arc::new(RwLock::new(items))));
    }

    loop {
        let item = parse_json_val(chars, pos)?;
        items.push(item);
        skip_whitespace(chars, pos);
        if *pos >= chars.len() {
            return Err("Unterminated array in JSON".to_string());
        }
        if chars[*pos] == ',' {
            *pos += 1;
            skip_whitespace(chars, pos);
        } else if chars[*pos] == ']' {
            *pos += 1;
            break;
        } else {
            return Err(format!("Expected ',' or ']' in array, got '{}'", chars[*pos]));
        }
    }
    Ok(Value::List(Arc::new(RwLock::new(items))))
}

fn parse_json_object(chars: &[char], pos: &mut usize) -> Result<Value, String> {
    *pos += 1; // skip {
    skip_whitespace(chars, pos);
    let mut map = HashMap::new();
    if *pos < chars.len() && chars[*pos] == '}' {
        *pos += 1;
        return Ok(Value::Map(Arc::new(RwLock::new(map))));
    }

    loop {
        skip_whitespace(chars, pos);
        if *pos >= chars.len() || chars[*pos] != '"' {
            return Err("Expected string key in object".to_string());
        }
        let key = parse_json_string(chars, pos)?;
        skip_whitespace(chars, pos);
        if *pos >= chars.len() || chars[*pos] != ':' {
            return Err("Expected ':' after key in object".to_string());
        }
        *pos += 1;
        let val = parse_json_val(chars, pos)?;
        map.insert(key, val);
        skip_whitespace(chars, pos);
        if *pos >= chars.len() {
            return Err("Unterminated object in JSON".to_string());
        }
        if chars[*pos] == ',' {
            *pos += 1;
            skip_whitespace(chars, pos);
        } else if chars[*pos] == '}' {
            *pos += 1;
            break;
        } else {
            return Err(format!("Expected ',' or '}}' in object, got '{}'", chars[*pos]));
        }
    }
    Ok(Value::Map(Arc::new(RwLock::new(map))))
}

pub fn stringify_json_value(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => {
            if f.is_nan() || f.is_infinite() {
                "null".to_string()
            } else {
                f.to_string()
            }
        }
        Value::String(s) => escape_json_str(s),
        Value::List(l) => {
            let list = l.read();
            let items: Vec<String> = list.iter().map(stringify_json_value).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Map(m) => {
            let map = m.read();
            let mut entries: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", escape_json_str(k), stringify_json_value(v)))
                .collect();
            entries.sort();
            format!("{{{}}}", entries.join(", "))
        }
        _ => "null".to_string(),
    }
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_methods() {
        let s = Value::String("  hello world  ".to_string());
        
        // trim
        let trimmed = call_native_function("trim", Some(&s), &[]).unwrap();
        assert_eq!(trimmed, Value::String("hello world".to_string()));

        // to_upper
        let upper = call_native_function("to_upper", Some(&trimmed), &[]).unwrap();
        assert_eq!(upper, Value::String("HELLO WORLD".to_string()));

        // split
        let parts = call_native_function("split", Some(&trimmed), &[Value::String(" ".to_string())]).unwrap();
        if let Value::List(l) = parts {
            let list = l.read();
            assert_eq!(list.len(), 2);
            assert_eq!(list[0], Value::String("hello".to_string()));
            assert_eq!(list[1], Value::String("world".to_string()));
        } else {
            panic!("Expected list from split");
        }

        // replace
        let replaced = call_native_function("replace", Some(&trimmed), &[Value::String("world".to_string()), Value::String("novo".to_string())]).unwrap();
        assert_eq!(replaced, Value::String("hello novo".to_string()));

        // starts_with & ends_with
        let starts = call_native_function("starts_with", Some(&trimmed), &[Value::String("hello".to_string())]).unwrap();
        assert_eq!(starts, Value::Boolean(true));
        let ends = call_native_function("ends_with", Some(&trimmed), &[Value::String("world".to_string())]).unwrap();
        assert_eq!(ends, Value::Boolean(true));

        // slice
        let sliced = call_native_function("slice", Some(&trimmed), &[Value::Integer(0), Value::Integer(5)]).unwrap();
        assert_eq!(sliced, Value::String("hello".to_string()));
    }

    #[test]
    fn test_list_and_map_methods() {
        // List
        let list_val = Value::List(Arc::new(RwLock::new(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)])));
        call_native_function("push", Some(&list_val), &[Value::Integer(4)]).unwrap();
        
        let len = call_native_function("len", None, &[list_val.clone()]).unwrap();
        assert_eq!(len, Value::Integer(4));

        call_native_function("sort", Some(&list_val), &[]).unwrap();
        let joined = call_native_function("join", Some(&list_val), &[Value::String(",".to_string())]).unwrap();
        assert_eq!(joined, Value::String("1,2,3,4".to_string()));

        let popped = call_native_function("pop", Some(&list_val), &[]).unwrap();
        assert_eq!(popped, Value::Integer(4));

        // Map
        let mut map_raw = HashMap::new();
        map_raw.insert("a".to_string(), Value::Integer(10));
        map_raw.insert("b".to_string(), Value::Integer(20));
        let map_val = Value::Map(Arc::new(RwLock::new(map_raw)));

        let has_a = call_native_function("has", Some(&map_val), &[Value::String("a".to_string())]).unwrap();
        assert_eq!(has_a, Value::Boolean(true));

        let keys = call_native_function("keys", Some(&map_val), &[]).unwrap();
        if let Value::List(k) = keys {
            assert_eq!(k.read().len(), 2);
        } else {
            panic!("Expected list of keys");
        }
    }

    #[test]
    fn test_json_parse_and_stringify() {
        let json_text = r#"{"name": "Novo", "version": 1, "fast": true, "items": [10, 20]}"#;
        let parsed = parse_json_str(json_text).unwrap();
        if let Value::Map(m) = &parsed {
            let map = m.read();
            assert_eq!(map.get("name"), Some(&Value::String("Novo".to_string())));
            assert_eq!(map.get("version"), Some(&Value::Integer(1)));
            assert_eq!(map.get("fast"), Some(&Value::Boolean(true)));
        } else {
            panic!("Expected map from json parse");
        }

        let serialized = stringify_json_value(&parsed);
        assert!(serialized.contains(r#""name": "Novo""#));
        assert!(serialized.contains(r#""fast": true"#));
    }

    #[test]
    fn test_math_and_globals() {
        let sqrt = call_native_function("math.sqrt", None, &[Value::Integer(25)]).unwrap();
        assert_eq!(sqrt, Value::Float(5.0));

        let pow = call_native_function("math.pow", None, &[Value::Integer(2), Value::Integer(8)]).unwrap();
        assert_eq!(pow, Value::Integer(256));

        let range_val = call_native_function("range", None, &[Value::Integer(1), Value::Integer(5)]).unwrap();
        if let Value::List(l) = range_val {
            let list = l.read();
            assert_eq!(list.len(), 4);
            assert_eq!(list[0], Value::Integer(1));
            assert_eq!(list[3], Value::Integer(4));
        } else {
            panic!("Expected list from range");
        }
    }
}
