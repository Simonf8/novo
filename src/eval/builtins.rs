use super::Interp;
use super::value::{Key, Value, compare, equal, format_float};
use crate::check::builtins::Builtin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type R = Result<Value, String>;

fn int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(format!("expected an Int, found a {}", other.type_name())),
    }
}

fn float(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, found a {}", v.type_name()))
}

fn list(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::List(l) => Ok(l.read().clone()),
        other => Err(format!("expected a List, found a {}", other.type_name())),
    }
}

fn string_list(v: &Value) -> Result<Vec<String>, String> {
    let Value::List(items) = v else {
        return Err(format!("expected a List, found a {}", v.type_name()));
    };
    items
        .read()
        .iter()
        .map(|s| string(s).map(|s| s.to_string()))
        .collect()
}

/// Up to `max` lines starting at byte `offset`, and the offset just after them,
/// so a large file can be read a piece at a time.
fn read_lines_from(path: &str, offset: i64, max: i64) -> Result<Value, String> {
    use std::io::{BufRead, Seek};
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    file.seek(std::io::SeekFrom::Start(offset.max(0) as u64))
        .map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(file);
    let mut lines = Vec::new();
    let mut next = offset.max(0);
    let mut line = String::new();
    while (lines.len() as i64) < max {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("cannot read `{path}`: {e}"))?;
        if n == 0 {
            break;
        }
        next += n as i64;
        lines.push(Value::str(line.trim_end_matches(['\n', '\r'])));
    }
    Ok(Value::Tuple(
        vec![Value::list(lines), Value::Int(next)].into(),
    ))
}

fn csv_rows(text: &str) -> Result<Value, String> {
    let rows = super::csv::parse(text)?;
    Ok(Value::list(
        rows.into_iter()
            .map(|r| Value::list(r.into_iter().map(|f| Value::Str(f.into())).collect()))
            .collect(),
    ))
}

fn string_rows(v: &Value) -> Result<Vec<Vec<String>>, String> {
    let Value::List(rows) = v else {
        return Err(format!("expected a List, found a {}", v.type_name()));
    };
    rows.read()
        .iter()
        .map(|row| match row {
            Value::List(fields) => fields
                .read()
                .iter()
                .map(|f| string(f).map(|s| s.to_string()))
                .collect(),
            other => Err(format!("expected a List, found a {}", other.type_name())),
        })
        .collect()
}

fn string(v: &Value) -> Result<Arc<str>, String> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        other => Err(format!("expected a String, found a {}", other.type_name())),
    }
}

/// Resolves a possibly negative index against `len`.
pub fn index(i: i64, len: usize) -> Result<usize, String> {
    let resolved = if i < 0 { i + len as i64 } else { i };
    if resolved < 0 || resolved >= len as i64 {
        return Err(format!("index {i} is out of range for length {len}"));
    }
    Ok(resolved as usize)
}

/// Resolves slice bounds, clamping them to `0..=len`.
pub fn bounds(start: Option<i64>, end: Option<i64>, len: usize) -> (usize, usize) {
    let fix = |i: i64| -> usize {
        let i = if i < 0 { i + len as i64 } else { i };
        i.clamp(0, len as i64) as usize
    };
    let s = start.map(fix).unwrap_or(0);
    let e = end.map(fix).unwrap_or(len);
    (s, e.max(s))
}

fn char_slice(s: &str, start: Option<i64>, end: Option<i64>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let (a, b) = bounds(start, end, chars.len());
    chars[a..b].iter().collect()
}

impl Interp {
    pub(crate) fn call_builtin(&self, b: Builtin, args: Vec<Value>) -> R {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::None);
        Ok(match b {
            Builtin::Len => Value::Int(match &args[0] {
                Value::Str(s) => s.chars().count() as i64,
                Value::List(l) => l.read().len() as i64,
                Value::Map(m) => m.read().len() as i64,
                Value::Tensor(t) => super::tensor::len(t)?,
                Value::Tuple(items) => items.len() as i64,
                other => {
                    return Err(format!(
                        "`len` needs a list, string, map or tensor, found a {}",
                        other.type_name()
                    ));
                }
            }),
            Builtin::Range => {
                let (start, end, step) = match args.len() {
                    1 => (0, int(&args[0])?, 1),
                    2 => (int(&args[0])?, int(&args[1])?, 1),
                    _ => (int(&args[0])?, int(&args[1])?, int(&args[2])?),
                };
                if step == 0 {
                    return Err("`range` step cannot be zero".into());
                }
                let mut items = Vec::new();
                let mut i = start;
                while (step > 0 && i < end) || (step < 0 && i > end) {
                    items.push(Value::Int(i));
                    match i.checked_add(step) {
                        Some(next) => i = next,
                        None => break,
                    }
                }
                Value::list(items)
            }
            Builtin::ToStr => Value::Str(self.display(&args[0]).into()),
            Builtin::ToInt => Value::Int(match &args[0] {
                Value::Int(i) => *i,
                Value::Float(f) if f.is_finite() => f.trunc() as i64,
                Value::Bool(b) => i64::from(*b),
                Value::Str(s) => s
                    .trim()
                    .parse()
                    .map_err(|_| format!("cannot convert \"{s}\" to an Int"))?,
                other => return Err(format!("cannot convert a {} to an Int", other.type_name())),
            }),
            Builtin::ToFloat => Value::Float(match &args[0] {
                Value::Int(i) => *i as f64,
                Value::Float(f) => *f,
                Value::Bool(b) => f64::from(u8::from(*b)),
                Value::Str(s) => s
                    .trim()
                    .parse()
                    .map_err(|_| format!("cannot convert \"{s}\" to a Float"))?,
                other => return Err(format!("cannot convert a {} to a Float", other.type_name())),
            }),
            Builtin::ToBool => Value::Bool(match &args[0] {
                Value::Bool(b) => *b,
                Value::Int(i) => *i != 0,
                Value::Float(f) => *f != 0.0,
                Value::Str(s) => !s.is_empty(),
                Value::None => false,
                Value::List(l) => !l.read().is_empty(),
                Value::Map(m) => m.read().len() > 0,
                _ => true,
            }),
            Builtin::Abs => match &args[0] {
                Value::Int(i) => Value::Int(i.checked_abs().ok_or("integer overflow")?),
                other => Value::Float(float(other)?.abs()),
            },
            Builtin::Min | Builtin::Max => {
                let items = if args.len() == 1 {
                    match &args[0] {
                        Value::List(l) => l.read().clone(),
                        other => {
                            return Err(format!(
                                "`{}` needs a list, found a {}",
                                b.name(),
                                other.type_name()
                            ));
                        }
                    }
                } else {
                    args
                };
                extreme(&items, b == Builtin::Max)?
            }
            Builtin::Round => match args.len() {
                1 => Value::Int(float(&args[0])?.round_ties_even() as i64),
                _ => {
                    let scale = 10f64.powi(int(&args[1])? as i32);
                    Value::Float((float(&args[0])? * scale).round_ties_even() / scale)
                }
            },
            Builtin::Assert => {
                if !matches!(args[0], Value::Bool(true)) {
                    return Err(match args.get(1) {
                        Some(Value::Str(m)) => format!("assertion failed: {m}"),
                        _ => "assertion failed".into(),
                    });
                }
                Value::None
            }
            Builtin::AssertEq => {
                if !equal(&args[0], &args[1]) {
                    let detail =
                        format!("{} != {}", self.display(&args[0]), self.display(&args[1]));
                    return Err(match args.get(2) {
                        Some(Value::Str(m)) => format!("assertion failed: {m} ({detail})"),
                        _ => format!("assertion failed: {detail}"),
                    });
                }
                Value::None
            }
            Builtin::Fail => return Err(string(&args[0])?.to_string()),
            Builtin::Chr => {
                let code = int(&args[0])?;
                let c = u32::try_from(code)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("{code} is not a valid character code"))?;
                Value::Str(c.to_string().into())
            }
            Builtin::Ord => {
                let s = string(&args[0])?;
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Value::Int(i64::from(u32::from(c))),
                    _ => return Err(format!("`ord` needs a single character, found \"{s}\"")),
                }
            }
            Builtin::FromBytes => {
                let bytes = list(&args[0])?
                    .iter()
                    .map(|v| match v {
                        Value::Int(b) if (0..=255).contains(b) => Ok(*b as u8),
                        _ => Err("`from_bytes` needs a list of Ints from 0 to 255".to_string()),
                    })
                    .collect::<Result<Vec<u8>, _>>()?;
                Value::Str(String::from_utf8_lossy(&bytes).into_owned().into())
            }
            Builtin::Enumerate => Value::list(
                list(&args[0])?
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Value::Tuple(vec![Value::Int(i as i64), v].into()))
                    .collect(),
            ),
            Builtin::Zip => {
                let (a, b) = (list(&args[0])?, list(&args[1])?);
                Value::list(
                    a.into_iter()
                        .zip(b)
                        .map(|(x, y)| Value::Tuple(vec![x, y].into()))
                        .collect(),
                )
            }
            Builtin::Reversed => Value::list(list(&args[0])?.into_iter().rev().collect()),
            Builtin::Sum => {
                let mut total = Value::Int(0);
                for (i, v) in list(&args[0])?.into_iter().enumerate() {
                    total = if i == 0 {
                        v
                    } else {
                        self.binary_values(crate::syntax::ast::BinOp::Add, &total, &v)?
                    };
                }
                total
            }
            Builtin::Any | Builtin::All => {
                let items = list(&args[0])?;
                let truthy = |v: &Value| matches!(v, Value::Bool(true));
                Value::Bool(if b == Builtin::Any {
                    items.iter().any(truthy)
                } else {
                    items.iter().all(truthy)
                })
            }
            Builtin::Sorted => {
                let items = list(&args[0])?;
                let mut keyed: Vec<(Value, Value)> = match args.get(1) {
                    Some(key) => items
                        .into_iter()
                        .map(|v| {
                            Ok((
                                self.call_value(
                                    key,
                                    vec![v.clone()],
                                    crate::source::Span::default(),
                                )
                                .map_err(|e| e.message)?,
                                v,
                            ))
                        })
                        .collect::<Result<_, String>>()?,
                    None => items.into_iter().map(|v| (v.clone(), v)).collect(),
                };
                let mut failure = None;
                keyed.sort_by(|a, b| {
                    compare(&a.0, &b.0).unwrap_or_else(|e| {
                        failure.get_or_insert(e);
                        std::cmp::Ordering::Equal
                    })
                });
                if let Some(e) = failure {
                    return Err(e);
                }
                Value::list(keyed.into_iter().map(|(_, v)| v).collect())
            }
            Builtin::TypeOf => Value::str(match &args[0] {
                Value::Instance(i) => &self.prog.classes[i.class.0 as usize].name,
                other => other.type_name(),
            }),
            Builtin::Input => {
                if let Value::Str(prompt) = arg(0) {
                    self.write_out(&prompt);
                }
                self.flush();
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                Value::str(line.trim_end_matches(['\n', '\r']))
            }
            Builtin::MathPi => Value::Float(std::f64::consts::PI),
            Builtin::MathE => Value::Float(std::f64::consts::E),
            Builtin::MathSqrt => Value::Float(float(&args[0])?.sqrt()),
            Builtin::MathPow => Value::Float(float(&args[0])?.powf(float(&args[1])?)),
            Builtin::MathFloor => Value::Int(float(&args[0])?.floor() as i64),
            Builtin::MathCeil => Value::Int(float(&args[0])?.ceil() as i64),
            Builtin::MathSin => Value::Float(float(&args[0])?.sin()),
            Builtin::MathCos => Value::Float(float(&args[0])?.cos()),
            Builtin::MathTan => Value::Float(float(&args[0])?.tan()),
            Builtin::MathLog => Value::Float(float(&args[0])?.ln()),
            Builtin::MathExp => Value::Float(float(&args[0])?.exp()),
            Builtin::TimeNow => Value::Float(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_secs_f64(),
            ),
            Builtin::TimeNowMs => Value::Int(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_millis() as i64,
            ),
            Builtin::TimeSleep => {
                std::thread::sleep(std::time::Duration::from_millis(
                    int(&args[0])?.max(0) as u64
                ));
                Value::None
            }
            Builtin::JsonParse => json::parse(&string(&args[0])?)?,
            Builtin::CsvParse => csv_rows(&string(&args[0])?)?,
            Builtin::CsvRead => {
                csv_rows(&std::fs::read_to_string(&*string(&args[0])?).map_err(|e| e.to_string())?)?
            }
            Builtin::CsvFormat => Value::Str(super::csv::format(&string_rows(&args[0])?).into()),
            Builtin::CsvWrite => {
                std::fs::write(
                    &*string(&args[0])?,
                    super::csv::format(&string_rows(&args[1])?),
                )
                .map_err(|e| e.to_string())?;
                Value::None
            }
            Builtin::JsonStringify => Value::Str(json::stringify(&args[0])?.into()),
            Builtin::TokLoad => super::tokenize::load(&string(&args[0])?)?,
            Builtin::TokTrain => {
                let texts = string_list(&args[0])?;
                let special = match args.get(2) {
                    Some(v) => string_list(v)?,
                    None => Vec::new(),
                };
                let size = int(&args[1])?;
                if size <= 0 {
                    return Err("the vocabulary size must be positive".into());
                }
                super::tokenize::train(&texts, size as usize, &special)?
            }
            Builtin::FsReadLinesFrom => {
                read_lines_from(&string(&args[0])?, int(&args[1])?, int(&args[2])?)?
            }
            Builtin::FsRead => Value::Str(
                std::fs::read_to_string(&*string(&args[0])?)
                    .map_err(|e| e.to_string())?
                    .into(),
            ),
            Builtin::FsWrite => {
                std::fs::write(&*string(&args[0])?, string(&args[1])?.as_bytes())
                    .map_err(|e| e.to_string())?;
                Value::None
            }
            Builtin::FsAppend => {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&*string(&args[0])?)
                    .map_err(|e| e.to_string())?;
                f.write_all(string(&args[1])?.as_bytes())
                    .map_err(|e| e.to_string())?;
                Value::None
            }
            Builtin::FsExists => Value::Bool(std::path::Path::new(&*string(&args[0])?).exists()),
            Builtin::FsListDir => {
                let mut names: Vec<String> = std::fs::read_dir(&*string(&args[0])?)
                    .map_err(|e| e.to_string())?
                    .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect();
                names.sort();
                Value::list(names.into_iter().map(|n| Value::Str(n.into())).collect())
            }
            Builtin::FsDelete => {
                std::fs::remove_file(&*string(&args[0])?).map_err(|e| e.to_string())?;
                Value::None
            }
            Builtin::FsCreateDir => {
                std::fs::create_dir_all(&*string(&args[0])?).map_err(|e| e.to_string())?;
                Value::None
            }
            Builtin::EnvGet => {
                let name = string(&args[0])?;
                match (std::env::var(&*name), args.get(1)) {
                    (Ok(v), _) => Value::Str(v.into()),
                    (Err(_), Some(default)) => default.clone(),
                    (Err(_), None) => {
                        return Err(format!("environment variable `{name}` is not set"));
                    }
                }
            }
            Builtin::EnvSet => {
                // SAFETY: Novo programs set variables from one thread at a time through this call.
                unsafe { std::env::set_var(&*string(&args[0])?, &*string(&args[1])?) };
                Value::None
            }
            Builtin::EnvArgs => Value::list(
                self.args
                    .iter()
                    .map(|a| Value::Str(a.as_str().into()))
                    .collect(),
            ),
            Builtin::NnTensor
            | Builtin::NnZeros
            | Builtin::NnOnes
            | Builtin::NnRandn
            | Builtin::NnRand
            | Builtin::NnArange
            | Builtin::NnEye
            | Builtin::NnParam
            | Builtin::NnSeed
            | Builtin::NnSgd
            | Builtin::NnAdam
            | Builtin::NnMse
            | Builtin::NnCrossEntropy
            | Builtin::NnStack
            | Builtin::NnCat
            | Builtin::NnTril
            | Builtin::NnSave
            | Builtin::NnLoad
            | Builtin::NnDevice
            | Builtin::NnAdamW
            | Builtin::NnCosineLr
            | Builtin::NnSaveParams
            | Builtin::NnLoadParams
            | Builtin::NnSaveTokens
            | Builtin::NnLoadTokens
            | Builtin::NnSampleWindows
            | Builtin::RandomSeed
            | Builtin::RandomInt
            | Builtin::RandomFloat
            | Builtin::RandomChoice
            | Builtin::RandomShuffle => self.tensors.builtin(b, &args)?,
        })
    }

    pub(super) fn call_method_builtin(&self, recv: &Value, name: &str, args: &[Value]) -> R {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::None);
        match recv {
            Value::Str(s) => Ok(match name {
                "length" | "len" => Value::Int(s.chars().count() as i64),
                "to_upper" | "upper" => Value::Str(s.to_uppercase().into()),
                "to_lower" | "lower" => Value::Str(s.to_lowercase().into()),
                "trim" => Value::str(s.trim()),
                "trim_start" => Value::str(s.trim_start()),
                "trim_end" => Value::str(s.trim_end()),
                "split" => {
                    let parts: Vec<Value> = match args.first() {
                        Some(sep) => {
                            let sep = string(sep)?;
                            if sep.is_empty() {
                                return Err("cannot split on an empty separator".into());
                            }
                            s.split(&*sep).map(Value::str).collect()
                        }
                        None => s.split_whitespace().map(Value::str).collect(),
                    };
                    Value::list(parts)
                }
                "lines" => Value::list(s.lines().map(Value::str).collect()),
                "chars" => Value::list(
                    s.chars()
                        .map(|c| Value::Str(c.to_string().into()))
                        .collect(),
                ),
                "replace" => Value::Str(s.replace(&*string(&args[0])?, &string(&args[1])?).into()),
                "contains" => Value::Bool(s.contains(&*string(&args[0])?)),
                "starts_with" => Value::Bool(s.starts_with(&*string(&args[0])?)),
                "ends_with" => Value::Bool(s.ends_with(&*string(&args[0])?)),
                "index_of" => {
                    let needle = string(&args[0])?;
                    Value::Int(
                        s.find(&*needle)
                            .map(|b| s[..b].chars().count() as i64)
                            .unwrap_or(-1),
                    )
                }
                "slice" => {
                    let end = match args.get(1) {
                        Some(e) => Some(int(e)?),
                        None => None,
                    };
                    Value::Str(char_slice(s, Some(int(&args[0])?), end).into())
                }
                "repeat" => Value::Str(s.repeat(int(&args[0])?.max(0) as usize).into()),
                "pad_start" | "pad_end" => {
                    let width = int(&args[0])?.max(0) as usize;
                    let fill = match args.get(1) {
                        Some(f) => string(f)?.chars().next().unwrap_or(' '),
                        None => ' ',
                    };
                    let len = s.chars().count();
                    let pad: String =
                        std::iter::repeat_n(fill, width.saturating_sub(len)).collect();
                    Value::Str(
                        if name == "pad_start" {
                            format!("{pad}{s}")
                        } else {
                            format!("{s}{pad}")
                        }
                        .into(),
                    )
                }
                "is_empty" => Value::Bool(s.is_empty()),
                "join" => {
                    let parts: Vec<String> =
                        list(&args[0])?.iter().map(|v| self.display(v)).collect();
                    Value::Str(parts.join(s).into())
                }
                "bytes" => Value::list(s.bytes().map(|b| Value::Int(i64::from(b))).collect()),
                "is_alpha" | "is_digit" | "is_space" | "is_alnum" | "is_upper" | "is_lower" => {
                    let test: fn(char) -> bool = match name {
                        "is_alpha" => char::is_alphabetic,
                        "is_digit" => char::is_numeric,
                        "is_space" => char::is_whitespace,
                        "is_alnum" => char::is_alphanumeric,
                        "is_upper" => char::is_uppercase,
                        _ => char::is_lowercase,
                    };
                    Value::Bool(!s.is_empty() && s.chars().all(test))
                }
                _ => return Err(format!("String has no method `{name}`")),
            }),
            Value::List(list) => Ok(match name {
                "length" | "len" => Value::Int(list.read().len() as i64),
                "push" | "append" => {
                    list.write().push(arg(0));
                    Value::None
                }
                "pop" => list.write().pop().ok_or("pop from an empty list")?,
                "insert" => {
                    let mut l = list.write();
                    let len = l.len();
                    let i = int(&args[0])?;
                    let at = if i < 0 {
                        (i + len as i64).max(0) as usize
                    } else {
                        (i as usize).min(len)
                    };
                    l.insert(at, arg(1));
                    Value::None
                }
                "remove" => {
                    let mut l = list.write();
                    let i = index(int(&args[0])?, l.len())?;
                    l.remove(i)
                }
                "contains" => Value::Bool(list.read().iter().any(|x| equal(x, &args[0]))),
                "index_of" => Value::Int(
                    list.read()
                        .iter()
                        .position(|x| equal(x, &args[0]))
                        .map(|i| i as i64)
                        .unwrap_or(-1),
                ),
                "slice" => {
                    let l = list.read();
                    let end = match args.get(1) {
                        Some(e) => Some(int(e)?),
                        None => None,
                    };
                    let (a, b) = bounds(Some(int(&args[0])?), end, l.len());
                    Value::list(l[a..b].to_vec())
                }
                "reverse" => {
                    list.write().reverse();
                    Value::None
                }
                "sort" => {
                    let mut items = list.read().clone();
                    let mut err = None;
                    items.sort_by(|a, b| {
                        compare(a, b).unwrap_or_else(|e| {
                            err.get_or_insert(e);
                            std::cmp::Ordering::Equal
                        })
                    });
                    if let Some(e) = err {
                        return Err(e);
                    }
                    *list.write() = items;
                    Value::None
                }
                "clear" => {
                    list.write().clear();
                    Value::None
                }
                "join" => {
                    let sep = match args.first() {
                        Some(s) => string(s)?,
                        None => "".into(),
                    };
                    let parts: Vec<String> = list.read().iter().map(|v| self.display(v)).collect();
                    Value::Str(parts.join(&sep).into())
                }
                "extend" => {
                    let other = match &args[0] {
                        Value::List(o) => o.read().clone(),
                        other => {
                            return Err(format!(
                                "`extend` needs a list, found a {}",
                                other.type_name()
                            ));
                        }
                    };
                    list.write().extend(other);
                    Value::None
                }
                "copy" | "clone" => Value::list(list.read().clone()),
                "first" => list
                    .read()
                    .first()
                    .cloned()
                    .ok_or("`first` of an empty list")?,
                "last" => list
                    .read()
                    .last()
                    .cloned()
                    .ok_or("`last` of an empty list")?,
                "is_empty" => Value::Bool(list.read().is_empty()),
                _ => return Err(format!("List has no method `{name}`")),
            }),
            Value::Map(map) => Ok(match name {
                "length" | "len" => Value::Int(map.read().len() as i64),
                "has" | "contains" => {
                    Value::Bool(map.read().get(&Key::from_value(&args[0])?).is_some())
                }
                "keys" => Value::list(map.read().iter().map(|(k, _)| k.to_value()).collect()),
                "values" => Value::list(map.read().iter().map(|(_, v)| v.clone()).collect()),
                "items" => Value::list(
                    map.read()
                        .iter()
                        .map(|(k, v)| Value::Tuple(vec![k.to_value(), v.clone()].into()))
                        .collect(),
                ),
                "get" => map
                    .read()
                    .get(&Key::from_value(&args[0])?)
                    .cloned()
                    .unwrap_or_else(|| arg(1)),
                "remove" => {
                    let key = Key::from_value(&args[0])?;
                    map.write().remove(&key).ok_or_else(|| {
                        format!("key {} is not in the map", self.display(&args[0]))
                    })?
                }
                "clear" => {
                    map.write().clear();
                    Value::None
                }
                "copy" | "clone" => Value::map(map.read().clone()),
                "is_empty" => Value::Bool(map.read().len() == 0),
                _ => return Err(format!("Map has no method `{name}`")),
            }),
            other => Err(format!("a {} has no method `{name}`", other.type_name())),
        }
    }
}

fn extreme(items: &[Value], max: bool) -> R {
    let mut best: Option<&Value> = None;
    for item in items {
        best = Some(match best {
            None => item,
            Some(b) => {
                // As in Python, an item replaces the best only if it compares
                // strictly beyond it, so NaN never wins against a number already chosen.
                let beyond = match (item.as_f64(), b.as_f64()) {
                    (Some(x), Some(y))
                        if matches!((item, b), (Value::Float(_), _) | (_, Value::Float(_))) =>
                    {
                        if max {
                            x > y
                        } else {
                            x < y
                        }
                    }
                    _ => {
                        let ord = compare(item, b)?;
                        (max && ord.is_gt()) || (!max && ord.is_lt())
                    }
                };
                if beyond { item } else { b }
            }
        });
    }
    best.cloned()
        .ok_or_else(|| format!("`{}` of an empty list", if max { "max" } else { "min" }))
}

/// Formats a value according to a Python-style format spec such as `>8`, `.2f` or `,d`.
pub fn format_spec(text: String, value: &Value, spec: &str) -> Result<String, String> {
    let bad = || format!("invalid format spec `{spec}`");
    let mut chars: Vec<char> = spec.chars().collect();
    let kind = match chars.last() {
        Some(c @ ('f' | 'd' | 'e' | '%' | 's')) => {
            let c = *c;
            chars.pop();
            Some(c)
        }
        _ => None,
    };
    let mut i = 0;
    let (mut fill, mut align) = (' ', None);
    if chars.len() >= 2 && matches!(chars[1], '<' | '>' | '^') {
        fill = chars[0];
        align = Some(chars[1]);
        i = 2;
    } else if !chars.is_empty() && matches!(chars[0], '<' | '>' | '^') {
        align = Some(chars[0]);
        i = 1;
    }
    if chars.get(i) == Some(&'0') && align.is_none() {
        fill = '0';
        align = Some('=');
        i += 1;
    }
    let mut width = String::new();
    while let Some(c) = chars.get(i).filter(|c| c.is_ascii_digit()) {
        width.push(*c);
        i += 1;
    }
    let comma = chars.get(i) == Some(&',');
    if comma {
        i += 1;
    }
    let mut precision = None;
    if chars.get(i) == Some(&'.') {
        let digits: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        i += 1 + digits.len();
        precision = Some(digits.parse::<usize>().map_err(|_| bad())?);
    }
    if i != chars.len() {
        return Err(bad());
    }

    let mut body = match (kind, value) {
        (Some('f'), v) => format!("{:.*}", precision.unwrap_or(6), v.as_f64().ok_or_else(bad)?),
        (Some('e'), v) => format!(
            "{:.*e}",
            precision.unwrap_or(6),
            v.as_f64().ok_or_else(bad)?
        ),
        (Some('%'), v) => format!(
            "{:.*}%",
            precision.unwrap_or(6),
            v.as_f64().ok_or_else(bad)? * 100.0
        ),
        (Some('d'), Value::Int(n)) => n.to_string(),
        (Some('d'), _) => return Err(bad()),
        (_, Value::Float(f)) if precision.is_some() => format!("{:.*}", precision.unwrap(), f),
        (_, Value::Float(f)) => format_float(*f),
        (_, Value::Str(_)) if precision.is_some() => {
            text.chars().take(precision.unwrap()).collect()
        }
        _ => text,
    };
    if comma {
        body = group_thousands(&body);
    }
    let width: usize = if width.is_empty() {
        0
    } else {
        width.parse().map_err(|_| bad())?
    };
    let len = body.chars().count();
    if len >= width {
        return Ok(body);
    }
    let pad = width - len;
    let numeric = matches!(value, Value::Int(_) | Value::Float(_));
    let fills = |n: usize| std::iter::repeat_n(fill, n).collect::<String>();
    Ok(match align.unwrap_or(if numeric { '>' } else { '<' }) {
        '<' => format!("{body}{}", fills(pad)),
        '^' => format!("{}{body}{}", fills(pad / 2), fills(pad - pad / 2)),
        '=' if body.starts_with('-') => format!("-{}{}", fills(pad), &body[1..]),
        _ => format!("{}{body}", fills(pad)),
    })
}

fn group_thousands(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map(|r| ("-", r)).unwrap_or(("", s));
    let (int_part, frac) = rest
        .split_once('.')
        .map(|(a, b)| (a, Some(b)))
        .unwrap_or((rest, None));
    let mut grouped = String::new();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

pub mod json {
    use super::super::value::{Dict, Key, Value, format_float};

    pub fn parse(text: &str) -> Result<Value, String> {
        let mut p = Parser {
            s: text.as_bytes(),
            pos: 0,
        };
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.pos != p.s.len() {
            return Err(p.err("unexpected trailing characters"));
        }
        Ok(v)
    }

    struct Parser<'a> {
        s: &'a [u8],
        pos: usize,
    }

    impl Parser<'_> {
        fn err(&self, msg: &str) -> String {
            format!("invalid JSON at byte {}: {msg}", self.pos)
        }

        fn ws(&mut self) {
            while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
        }

        fn eat(&mut self, lit: &str) -> bool {
            if self.s[self.pos..].starts_with(lit.as_bytes()) {
                self.pos += lit.len();
                true
            } else {
                false
            }
        }

        fn value(&mut self, depth: usize) -> Result<Value, String> {
            if depth > 512 {
                return Err(self.err("nesting is too deep"));
            }
            match self.s.get(self.pos) {
                Some(b'{') => {
                    self.pos += 1;
                    let mut dict = Dict::default();
                    self.ws();
                    if self.eat("}") {
                        return Ok(Value::map(dict));
                    }
                    loop {
                        self.ws();
                        let key = self.string()?;
                        self.ws();
                        if !self.eat(":") {
                            return Err(self.err("expected `:`"));
                        }
                        self.ws();
                        let v = self.value(depth + 1)?;
                        dict.insert(Key::Str(key.into()), v);
                        self.ws();
                        if self.eat(",") {
                            continue;
                        }
                        if self.eat("}") {
                            return Ok(Value::map(dict));
                        }
                        return Err(self.err("expected `,` or `}`"));
                    }
                }
                Some(b'[') => {
                    self.pos += 1;
                    let mut items = Vec::new();
                    self.ws();
                    if self.eat("]") {
                        return Ok(Value::list(items));
                    }
                    loop {
                        self.ws();
                        items.push(self.value(depth + 1)?);
                        self.ws();
                        if self.eat(",") {
                            continue;
                        }
                        if self.eat("]") {
                            return Ok(Value::list(items));
                        }
                        return Err(self.err("expected `,` or `]`"));
                    }
                }
                Some(b'"') => Ok(Value::Str(self.string()?.into())),
                Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
                Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
                Some(b'n') if self.eat("null") => Ok(Value::None),
                Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
                _ => Err(self.err("expected a value")),
            }
        }

        fn number(&mut self) -> Result<Value, String> {
            let start = self.pos;
            let mut float = false;
            while let Some(&c) = self.s.get(self.pos) {
                match c {
                    b'0'..=b'9' | b'-' | b'+' => {}
                    b'.' | b'e' | b'E' => float = true,
                    _ => break,
                }
                self.pos += 1;
            }
            let text = std::str::from_utf8(&self.s[start..self.pos]).expect("ascii");
            if !float && let Ok(i) = text.parse::<i64>() {
                return Ok(Value::Int(i));
            }
            text.parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.err("invalid number"))
        }

        fn string(&mut self) -> Result<String, String> {
            if !self.eat("\"") {
                return Err(self.err("expected a string"));
            }
            let mut out = Vec::new();
            loop {
                let Some(&c) = self.s.get(self.pos) else {
                    return Err(self.err("unterminated string"));
                };
                self.pos += 1;
                match c {
                    b'"' => return String::from_utf8(out).map_err(|_| self.err("invalid UTF-8")),
                    b'\\' => {
                        let Some(&e) = self.s.get(self.pos) else {
                            return Err(self.err("unterminated escape"));
                        };
                        self.pos += 1;
                        let ch = match e {
                            b'"' => '"',
                            b'\\' => '\\',
                            b'/' => '/',
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            b'u' => {
                                let mut code = self.hex4()?;
                                if (0xD800..0xDC00).contains(&code) && self.eat("\\u") {
                                    let low = self.hex4()?;
                                    code = 0x10000
                                        + ((code - 0xD800) << 10)
                                        + (low.wrapping_sub(0xDC00) & 0x3FF);
                                }
                                char::from_u32(code)
                                    .ok_or_else(|| self.err("invalid unicode escape"))?
                            }
                            _ => return Err(self.err("invalid escape")),
                        };
                        let mut buf = [0; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    c => out.push(c),
                }
            }
        }

        fn hex4(&mut self) -> Result<u32, String> {
            let digits = self
                .s
                .get(self.pos..self.pos + 4)
                .ok_or_else(|| self.err("short unicode escape"))?;
            self.pos += 4;
            u32::from_str_radix(
                std::str::from_utf8(digits).map_err(|_| self.err("invalid escape"))?,
                16,
            )
            .map_err(|_| self.err("invalid escape"))
        }
    }

    pub fn stringify(v: &Value) -> Result<String, String> {
        let mut out = String::new();
        write(&mut out, v, 0)?;
        Ok(out)
    }

    fn write(out: &mut String, v: &Value, depth: usize) -> Result<(), String> {
        if depth > 512 {
            return Err("value is nested too deeply to convert to JSON".into());
        }
        match v {
            Value::None => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => out.push_str(&i.to_string()),
            Value::Float(f) if f.is_finite() => out.push_str(&format_float(*f)),
            Value::Float(_) => return Err("NaN and infinity cannot be converted to JSON".into()),
            Value::Tuple(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(out, item, depth + 1)?;
                }
                out.push(']');
            }
            Value::Str(s) => out.push_str(&crate::diagnostic::json_string(s)),
            Value::List(items) => {
                out.push('[');
                for (i, item) in items.read().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(out, item, depth + 1)?;
                }
                out.push(']');
            }
            Value::Map(dict) => {
                out.push('{');
                for (i, (k, val)) in dict.read().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let key = match k {
                        Key::Str(s) => s.to_string(),
                        Key::Int(n) => n.to_string(),
                        Key::Bool(b) => b.to_string(),
                        Key::None => "null".into(),
                    };
                    out.push_str(&crate::diagnostic::json_string(&key));
                    out.push(':');
                    write(out, val, depth + 1)?;
                }
                out.push('}');
            }
            other => {
                return Err(format!(
                    "a {} cannot be converted to JSON",
                    other.type_name()
                ));
            }
        }
        Ok(())
    }
}
