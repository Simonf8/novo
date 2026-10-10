//! Tensors, gradients and optimizers, backed by candle on the CPU.

use super::optim::{self, Kind, Optimizer};
use super::value::{Dict, Key, Value};
use crate::check::builtins::Builtin;
use crate::syntax::ast::BinOp;
use candle_core::backprop::GradStore;
use candle_core::{D, DType, Device, Tensor, Var};
use parking_lot::Mutex;
use std::sync::Arc;

type R<T> = Result<T, String>;

fn err(e: candle_core::Error) -> String {
    e.to_string()
        .lines()
        .next()
        .unwrap_or("tensor error")
        .to_string()
}

/// A tensor value. Parameters wrap a `Var` so optimizers can update them in place.
#[derive(Debug)]
pub struct NTensor {
    tensor: Tensor,
    var: Option<Var>,
}

impl NTensor {
    pub fn get(&self) -> Tensor {
        match &self.var {
            Some(v) => v.as_tensor().clone(),
            None => self.tensor.clone(),
        }
    }
}

fn wrap(t: Tensor) -> Value {
    Value::Tensor(Arc::new(NTensor {
        tensor: t,
        var: None,
    }))
}

/// Process-wide state: the random generator, the gradients from the latest
/// `backward`, and the device new tensors are created on.
pub struct State {
    rng: Mutex<Rng>,
    grads: Mutex<Option<GradStore>>,
    device: Mutex<Device>,
}

impl Default for State {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        State {
            rng: Mutex::new(Rng::new(seed)),
            grads: Mutex::new(None),
            device: Mutex::new(Device::Cpu),
        }
    }
}

/// SplitMix64, so seeded programs produce the same numbers everywhere.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

fn shape_arg(v: &Value) -> R<Vec<usize>> {
    let Value::List(items) = v else {
        return Err("a shape must be a list of Ints".into());
    };
    items
        .read()
        .iter()
        .map(|d| match d {
            Value::Int(n) if *n >= 0 => Ok(*n as usize),
            _ => Err("shape dimensions must be non-negative Ints".to_string()),
        })
        .collect()
}

/// A target shape where one dimension may be `-1`, inferred from the element count.
fn reshape_arg(v: &Value, from: &[usize]) -> R<Vec<usize>> {
    let Value::List(items) = v else {
        return Err("a shape must be a list of Ints".into());
    };
    let dims: Vec<i64> = items.read().iter().map(int_arg).collect::<R<_>>()?;
    let total: usize = from.iter().product();
    let known: i64 = dims.iter().filter(|d| **d != -1).product();
    let holes = dims.iter().filter(|d| **d == -1).count();
    let bad = || format!("cannot reshape a tensor of shape {from:?} into {dims:?}");
    if holes > 1 || dims.iter().any(|d| *d < -1) {
        return Err(bad());
    }
    let shape: Vec<usize> = dims
        .iter()
        .map(|d| {
            if *d == -1 {
                if known == 0 {
                    0
                } else {
                    total / known as usize
                }
            } else {
                *d as usize
            }
        })
        .collect();
    if shape.iter().product::<usize>() != total {
        return Err(bad());
    }
    Ok(shape)
}

/// The trainable variables in a list of parameters.
fn param_vars(v: &Value, user: &str) -> R<Vec<Var>> {
    let Value::List(items) = v else {
        return Err(format!("{user} take a list of parameters"));
    };
    items
        .read()
        .iter()
        .map(|item| match item {
            Value::Tensor(t) => t.var.clone().ok_or_else(|| {
                format!("{user} can only use parameters; wrap trainable tensors with `nn.param`")
            }),
            other => Err(format!(
                "expected a Tensor parameter, found a {}",
                other.type_name()
            )),
        })
        .collect()
}

fn tensor_arg(v: &Value) -> R<Tensor> {
    match v {
        Value::Tensor(t) => Ok(t.get()),
        other => Err(format!("expected a Tensor, found a {}", other.type_name())),
    }
}

fn int_arg(v: &Value) -> R<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(format!("expected an Int, found a {}", other.type_name())),
    }
}

fn float_arg(v: &Value) -> R<f64> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, found a {}", v.type_name()))
}

fn parse_device(name: &str) -> R<Device> {
    let (kind, index) = match name.split_once(':') {
        Some((k, i)) => (
            k,
            i.parse::<usize>()
                .map_err(|_| format!("invalid device `{name}`"))?,
        ),
        None => (name, 0),
    };
    match kind {
        "cpu" => Ok(Device::Cpu),
        "cuda" => Device::new_cuda(index).map_err(|_| "CUDA is not available; build Novo with `--features cuda` on a machine with an NVIDIA GPU".to_string()),
        "metal" => Device::new_metal(index).map_err(|_| "Metal is not available; build Novo with `--features metal` on a Mac".to_string()),
        _ => Err(format!("unknown device `{name}`; use \"cpu\", \"cuda\" or \"metal\"")),
    }
}

fn device_name(d: &Device) -> String {
    match d.location() {
        candle_core::DeviceLocation::Cpu => "cpu".into(),
        candle_core::DeviceLocation::Cuda { gpu_id } => format!("cuda:{gpu_id}"),
        candle_core::DeviceLocation::Metal { gpu_id } => format!("metal:{gpu_id}"),
    }
}

fn parse_dtype(name: &str) -> R<DType> {
    Ok(match name {
        "f32" | "float32" => DType::F32,
        "f16" | "float16" | "half" => DType::F16,
        "bf16" | "bfloat16" => DType::BF16,
        "f64" | "float64" => DType::F64,
        _ => {
            return Err(format!(
                "unknown dtype `{name}`; use \"f32\", \"f16\", \"bf16\" or \"f64\""
            ));
        }
    })
}

/// Builds a tensor from a number or nested lists of numbers.
fn from_value(v: &Value) -> R<Tensor> {
    let mut shape = Vec::new();
    let mut probe = v.clone();
    loop {
        let next = match &probe {
            Value::List(items) => {
                let items = items.read();
                shape.push(items.len());
                match items.first() {
                    Some(first) => first.clone(),
                    None => break,
                }
            }
            _ => break,
        };
        probe = next;
    }
    let mut data = Vec::new();
    flatten(v, &shape, 0, &mut data)?;
    Tensor::from_vec(data, shape, &Device::Cpu).map_err(err)
}

fn flatten(v: &Value, shape: &[usize], depth: usize, out: &mut Vec<f32>) -> R<()> {
    match v {
        Value::List(items) => {
            let items = items.read();
            if depth >= shape.len() || items.len() != shape[depth] {
                return Err("nested lists must form a rectangular shape".into());
            }
            for item in items.iter() {
                flatten(item, shape, depth + 1, out)?;
            }
            Ok(())
        }
        Value::Tensor(t) if depth == shape.len() => {
            let t = t.get();
            if t.rank() != 0 {
                return Err("nested tensors must be scalars".into());
            }
            out.push(
                t.to_dtype(DType::F32)
                    .and_then(|t| t.to_scalar::<f32>())
                    .map_err(err)?,
            );
            Ok(())
        }
        other if depth == shape.len() => {
            out.push(float_arg(other)? as f32);
            Ok(())
        }
        _ => Err("nested lists must form a rectangular shape".into()),
    }
}

impl State {
    pub fn builtin(&self, b: Builtin, args: &[Value]) -> R<Value> {
        let device = self.device.lock().clone();
        let cpu = &device;
        let t = match b {
            Builtin::NnTensor => from_value(&args[0])?.to_device(cpu).map_err(err)?,
            Builtin::NnDevice => {
                if let Some(Value::Str(name)) = args.first() {
                    *self.device.lock() = parse_device(name)?;
                }
                return Ok(Value::Str(device_name(&self.device.lock()).into()));
            }
            Builtin::NnZeros => {
                Tensor::zeros(shape_arg(&args[0])?, DType::F32, cpu).map_err(err)?
            }
            Builtin::NnOnes => Tensor::ones(shape_arg(&args[0])?, DType::F32, cpu).map_err(err)?,
            Builtin::NnRandn | Builtin::NnRand => {
                let shape = shape_arg(&args[0])?;
                let n: usize = shape.iter().product();
                let mut rng = self.rng.lock();
                let data: Vec<f32> = (0..n).map(|_| if b == Builtin::NnRandn { rng.normal() } else { rng.uniform() } as f32).collect();
                Tensor::from_vec(data, shape, &Device::Cpu)
                    .and_then(|t| t.to_device(cpu))
                    .map_err(err)?
            }
            Builtin::NnArange => {
                Tensor::arange(0f32, int_arg(&args[0])?.max(0) as f32, cpu).map_err(err)?
            }
            Builtin::NnEye => {
                Tensor::eye(int_arg(&args[0])?.max(0) as usize, DType::F32, cpu).map_err(err)?
            }
            Builtin::NnParam => {
                let var = Var::from_tensor(&tensor_arg(&args[0])?).map_err(err)?;
                return Ok(Value::Tensor(Arc::new(NTensor {
                    tensor: var.as_tensor().clone(),
                    var: Some(var),
                })));
            }
            Builtin::NnSeed | Builtin::RandomSeed => {
                *self.rng.lock() = Rng::new(int_arg(&args[0])? as u64);
                return Ok(Value::None);
            }
            Builtin::RandomInt => {
                let (lo, hi) = (int_arg(&args[0])?, int_arg(&args[1])?);
                if lo >= hi {
                    return Err(format!("`random.int` needs low < high, got {lo} and {hi}"));
                }
                let span = hi.wrapping_sub(lo) as u64;
                return Ok(Value::Int(
                    lo.wrapping_add((self.rng.lock().next() % span) as i64),
                ));
            }
            Builtin::RandomFloat => return Ok(Value::Float(self.rng.lock().uniform())),
            Builtin::RandomChoice | Builtin::RandomShuffle => {
                let Value::List(items) = &args[0] else {
                    return Err("expected a list".into());
                };
                let mut rng = self.rng.lock();
                if b == Builtin::RandomChoice {
                    let items = items.read();
                    if items.is_empty() {
                        return Err("`random.choice` of an empty list".into());
                    }
                    return Ok(items[(rng.next() % items.len() as u64) as usize].clone());
                }
                let mut items = items.write();
                for i in (1..items.len()).rev() {
                    let j = (rng.next() % (i as u64 + 1)) as usize;
                    items.swap(i, j);
                }
                return Ok(Value::None);
            }
            Builtin::NnSgd | Builtin::NnAdam | Builtin::NnAdamW => {
                let vars = param_vars(&args[0], "optimizers")?;
                let lr = float_arg(&args[1])?;
                let extra = args.get(2).map(float_arg).transpose()?;
                let kind = match b {
                    Builtin::NnSgd => Kind::Sgd {
                        momentum: extra.unwrap_or(0.0),
                    },
                    _ => Kind::Adam {
                        beta1: 0.9,
                        beta2: 0.999,
                        eps: 1e-8,
                        weight_decay: if b == Builtin::NnAdamW {
                            extra.unwrap_or(0.01)
                        } else {
                            0.0
                        },
                    },
                };
                return Ok(Value::Optimizer(Arc::new(Mutex::new(Optimizer::new(
                    kind, vars, lr,
                )?))));
            }
            Builtin::NnCosineLr => {
                let min = args.get(4).map(float_arg).transpose()?.unwrap_or(0.0);
                let (step, warmup, total) =
                    (int_arg(&args[0])?, int_arg(&args[1])?, int_arg(&args[2])?);
                return Ok(Value::Float(optim::cosine_lr(
                    step,
                    warmup,
                    total,
                    float_arg(&args[3])?,
                    min,
                )));
            }
            Builtin::NnSaveParams => {
                let vars = param_vars(&args[0], "`nn.save_params`")?;
                let named: std::collections::HashMap<String, Tensor> = vars
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Ok((
                            format!("param.{i}"),
                            v.as_tensor().contiguous().map_err(err)?,
                        ))
                    })
                    .collect::<R<_>>()?;
                let Value::Str(path) = &args[1] else {
                    return Err("`nn.save_params` needs a file path".into());
                };
                candle_core::safetensors::save(&named, &**path).map_err(err)?;
                return Ok(Value::None);
            }
            Builtin::NnLoadParams => {
                let vars = param_vars(&args[0], "`nn.load_params`")?;
                let Value::Str(path) = &args[1] else {
                    return Err("`nn.load_params` needs a file path".into());
                };
                let named = candle_core::safetensors::load(&**path, cpu).map_err(err)?;
                if named.len() != vars.len() {
                    return Err(format!(
                        "the file holds {} parameters but {} were given",
                        named.len(),
                        vars.len()
                    ));
                }
                for (i, v) in vars.iter().enumerate() {
                    let saved = named
                        .get(&format!("param.{i}"))
                        .ok_or("this file was not written by `nn.save_params`")?;
                    if saved.dims() != v.dims() {
                        return Err(format!(
                            "parameter {i} has shape {:?} but the file holds {:?}",
                            v.dims(),
                            saved.dims()
                        ));
                    }
                    v.set(&saved.to_dtype(v.dtype()).map_err(err)?)
                        .map_err(err)?;
                }
                return Ok(Value::None);
            }
            Builtin::NnMse => {
                candle_nn::loss::mse(&tensor_arg(&args[0])?, &tensor_arg(&args[1])?).map_err(err)?
            }
            Builtin::NnCrossEntropy => {
                let targets = tensor_arg(&args[1])?.to_dtype(DType::U32).map_err(err)?;
                candle_nn::loss::cross_entropy(&tensor_arg(&args[0])?, &targets).map_err(err)?
            }
            Builtin::NnStack | Builtin::NnCat => {
                let name = if b == Builtin::NnStack {
                    "nn.stack"
                } else {
                    "nn.cat"
                };
                let Value::List(items) = &args[0] else {
                    return Err(format!("`{name}` takes a list of tensors"));
                };
                let tensors = items.read().iter().map(tensor_arg).collect::<R<Vec<_>>>()?;
                if tensors.is_empty() {
                    return Err(format!("`{name}` needs at least one tensor"));
                }
                let rank = tensors[0].rank() + usize::from(b == Builtin::NnStack);
                let dim = match args.get(1) {
                    Some(d) => dim(int_arg(d)?, rank)?,
                    None => 0,
                };
                if b == Builtin::NnStack {
                    Tensor::stack(&tensors, dim)
                } else {
                    Tensor::cat(&tensors, dim)
                }
                .map_err(err)?
            }
            Builtin::NnTril => {
                Tensor::tril2(int_arg(&args[0])?.max(0) as usize, DType::F32, cpu).map_err(err)?
            }
            Builtin::NnSave => {
                let Value::Map(map) = &args[0] else {
                    return Err("`nn.save` takes a map of tensors".into());
                };
                let mut named = std::collections::HashMap::new();
                for (k, v) in map.read().iter() {
                    let Key::Str(name) = k else {
                        return Err("tensor names must be strings".into());
                    };
                    named.insert(name.to_string(), tensor_arg(v)?.contiguous().map_err(err)?);
                }
                let Value::Str(path) = &args[1] else {
                    return Err("`nn.save` needs a file path".into());
                };
                candle_core::safetensors::save(&named, &**path).map_err(err)?;
                return Ok(Value::None);
            }
            Builtin::NnLoad => {
                let Value::Str(path) = &args[0] else {
                    return Err("`nn.load` needs a file path".into());
                };
                let dtype = match args.get(1) {
                    Some(Value::Str(name)) => parse_dtype(name)?,
                    _ => DType::F32,
                };
                let mut named: Vec<(String, Tensor)> = candle_core::safetensors::load(&**path, cpu)
                    .map_err(err)?
                    .into_iter()
                    .collect();
                named.sort_by(|a, b| a.0.cmp(&b.0));
                let mut dict = Dict::default();
                for (name, t) in named {
                    let t = if t.dtype() == dtype || !t.dtype().is_float() {
                        t
                    } else {
                        t.to_dtype(dtype).map_err(err)?
                    };
                    dict.insert(Key::Str(name.into()), wrap(t));
                }
                return Ok(Value::map(dict));
            }
            _ => unreachable!("not a tensor builtin"),
        };
        Ok(wrap(t))
    }

    pub fn method(&self, recv: &NTensor, name: &str, args: &[Value]) -> R<Value> {
        let t = recv.get();
        let arg_dim = |i: usize| -> R<Option<usize>> {
            match args.get(i) {
                Some(v) => Ok(Some(dim(int_arg(v)?, t.rank())?)),
                None => Ok(None),
            }
        };
        let keepdim = matches!(args.get(1), Some(Value::Bool(true)));
        let out = match name {
            "shape" => {
                return Ok(Value::list(
                    t.dims().iter().map(|d| Value::Int(*d as i64)).collect(),
                ));
            }
            "rank" => return Ok(Value::Int(t.rank() as i64)),
            "size" => return Ok(Value::Int(t.elem_count() as i64)),
            "item" => {
                if t.elem_count() != 1 {
                    return Err(format!(
                        "`item` needs a tensor with one element, this one has shape {:?}",
                        t.dims()
                    ));
                }
                let v = t
                    .flatten_all()
                    .and_then(|f| f.to_dtype(DType::F64))
                    .and_then(|f| f.to_vec1::<f64>())
                    .map_err(err)?;
                return Ok(Value::Float(v[0]));
            }
            "to_list" => return to_list(&t),
            "backward" => {
                let grads = t.backward().map_err(err)?;
                *self.grads.lock() = Some(grads);
                return Ok(Value::None);
            }
            "grad" => {
                let grads = self.grads.lock();
                let g = grads.as_ref().and_then(|g| g.get(&t)).ok_or(
                    "this tensor has no gradient; call `backward` on a result that depends on it",
                )?;
                g.clone()
            }
            "sum" => match arg_dim(0)? {
                Some(d) if keepdim => t.sum_keepdim(d),
                Some(d) => t.sum(d),
                None => t.sum_all(),
            }
            .map_err(err)?,
            "mean" => match arg_dim(0)? {
                Some(d) if keepdim => t.mean_keepdim(d),
                Some(d) => t.mean(d),
                None => t.mean_all(),
            }
            .map_err(err)?,
            "max" if keepdim => t.max_keepdim(arg_dim(0)?.unwrap()).map_err(err)?,
            "min" if keepdim => t.min_keepdim(arg_dim(0)?.unwrap()).map_err(err)?,
            "max" => t.max(arg_dim(0)?.unwrap()).map_err(err)?,
            "min" => t.min(arg_dim(0)?.unwrap()).map_err(err)?,
            "narrow" => {
                let d = dim(int_arg(&args[0])?, t.rank())?;
                let (start, len) = (int_arg(&args[1])?, int_arg(&args[2])?);
                let size = t.dims()[d] as i64;
                if start < 0 || len < 0 || start + len > size {
                    return Err(format!(
                        "cannot take {len} items from {start} in a dimension of size {size}"
                    ));
                }
                t.narrow(d, start as usize, len as usize).map_err(err)?
            }
            "chunk" => {
                let n = int_arg(&args[0])?;
                if n <= 0 {
                    return Err("`chunk` needs a positive number of chunks".into());
                }
                let d = match args.get(1) {
                    Some(v) => dim(int_arg(v)?, t.rank())?,
                    None => 0,
                };
                let parts = t.chunk(n as usize, d).map_err(err)?;
                return Ok(Value::list(parts.into_iter().map(wrap).collect()));
            }
            "batches" => {
                let size = int_arg(&args[0])?;
                if size <= 0 || t.rank() == 0 {
                    return Err(
                        "`batches` needs a positive size and a tensor with at least one dimension"
                            .into(),
                    );
                }
                let n = t.dims()[0];
                let parts = (0..n)
                    .step_by(size as usize)
                    .map(|start| t.narrow(0, start, (size as usize).min(n - start)).map(wrap))
                    .collect::<candle_core::Result<Vec<_>>>()
                    .map_err(err)?;
                return Ok(Value::list(parts));
            }
            "take" => {
                let Value::List(items) = &args[0] else {
                    return Err("`take` needs a list of indices".into());
                };
                let n = t.dims().first().copied().unwrap_or(0) as i64;
                let ids = items
                    .read()
                    .iter()
                    .map(|v| match int_arg(v)? {
                        i if (0..n).contains(&i) => Ok(i as u32),
                        i => Err(format!(
                            "index {i} is out of range for a first dimension of size {n}"
                        )),
                    })
                    .collect::<R<Vec<u32>>>()?;
                let ids = Tensor::new(ids, t.device()).map_err(err)?;
                t.index_select(&ids, 0).map_err(err)?
            }
            "embed" => {
                let ids = tensor_arg(&args[0])?;
                let mut shape = ids.dims().to_vec();
                shape.push(
                    *t.dims()
                        .last()
                        .ok_or("`embed` needs a table with at least one dimension")?,
                );
                let flat = ids
                    .flatten_all()
                    .and_then(|i| i.to_dtype(DType::U32))
                    .map_err(err)?;
                t.index_select(&flat, 0)
                    .and_then(|rows| rows.reshape(shape))
                    .map_err(err)?
            }
            "masked_fill" => {
                let mask = tensor_arg(&args[0])?.broadcast_as(t.dims()).map_err(err)?;
                let fill = Tensor::full(float_arg(&args[1])? as f32, t.dims(), t.device())
                    .and_then(|f| f.to_dtype(t.dtype()))
                    .map_err(err)?;
                mask.ne(0f32)
                    .and_then(|m| m.where_cond(&fill, &t))
                    .map_err(err)?
            }
            "layer_norm" => {
                let (weight, bias) = (tensor_arg(&args[0])?, tensor_arg(&args[1])?);
                let eps = match args.get(2) {
                    Some(v) => float_arg(v)?,
                    None => 1e-5,
                };
                layer_norm(&t, &weight, &bias, eps).map_err(err)?
            }
            "device" => return Ok(Value::Str(device_name(t.device()).into())),
            "dtype" => return Ok(Value::Str(t.dtype().as_str().into())),
            "to" => {
                let Value::Str(name) = &args[0] else {
                    return Err("`to` needs a device name".into());
                };
                t.to_device(&parse_device(name)?).map_err(err)?
            }
            "to_dtype" => {
                let Value::Str(name) = &args[0] else {
                    return Err("`to_dtype` needs a dtype name".into());
                };
                t.to_dtype(parse_dtype(name)?).map_err(err)?
            }
            "sample" => {
                let probs = values(&t)?;
                let total: f64 = probs.iter().map(|p| f64::from(*p)).sum();
                if t.rank() != 1 || !(total > 0.0) {
                    return Err(
                        "`sample` needs a one-dimensional tensor of non-negative weights".into(),
                    );
                }
                let mut target = self.rng.lock().uniform() * total;
                let mut pick = probs.len() - 1;
                for (i, p) in probs.iter().enumerate() {
                    target -= f64::from(*p);
                    if target < 0.0 {
                        pick = i;
                        break;
                    }
                }
                return Ok(Value::Int(pick as i64));
            }
            "argmax" => t
                .argmax(arg_dim(0)?.unwrap())
                .and_then(|a| a.to_dtype(DType::F32))
                .map_err(err)?,
            "argmin" => t
                .argmin(arg_dim(0)?.unwrap())
                .and_then(|a| a.to_dtype(DType::F32))
                .map_err(err)?,
            "softmax" => candle_nn::ops::softmax(&t, arg_dim(0)?.unwrap()).map_err(err)?,
            "log_softmax" => candle_nn::ops::log_softmax(&t, arg_dim(0)?.unwrap()).map_err(err)?,
            "squeeze" => t.squeeze(arg_dim(0)?.unwrap()).map_err(err)?,
            "unsqueeze" => t
                .unsqueeze(dim(int_arg(&args[0])?, t.rank() + 1)?)
                .map_err(err)?,
            "relu" => t.relu().map_err(err)?,
            "sigmoid" => candle_nn::ops::sigmoid(&t).map_err(err)?,
            "tanh" => t.tanh().map_err(err)?,
            "exp" => t.exp().map_err(err)?,
            "log" => t.log().map_err(err)?,
            "sqrt" => t.sqrt().map_err(err)?,
            "abs" => t.abs().map_err(err)?,
            "gelu" => t.gelu().map_err(err)?,
            "silu" => t.silu().map_err(err)?,
            "square" => t.sqr().map_err(err)?,
            "t" => {
                if t.rank() < 2 {
                    return Err("`t` needs a tensor with at least two dimensions".into());
                }
                t.t().map_err(err)?
            }
            "transpose" => t
                .transpose(
                    dim(int_arg(&args[0])?, t.rank())?,
                    dim(int_arg(&args[1])?, t.rank())?,
                )
                .map_err(err)?,
            "flatten" => t.flatten_all().map_err(err)?,
            "detach" => t.detach(),
            "copy" => t.copy().map_err(err)?,
            "reshape" => {
                let shape = reshape_arg(&args[0], t.dims())?;
                t.reshape(shape).map_err(err)?
            }
            "matmul" => matmul(&t, &tensor_arg(&args[0])?)?,
            "eq" => t
                .broadcast_eq(&tensor_arg(&args[0])?)
                .and_then(|m| m.to_dtype(DType::F32))
                .map_err(err)?,
            "pow" => t.powf(float_arg(&args[0])?).map_err(err)?,
            "clamp" => t
                .clamp(float_arg(&args[0])? as f32, float_arg(&args[1])? as f32)
                .map_err(err)?,
            _ => return Err(format!("Tensor has no method `{name}`")),
        };
        Ok(wrap(out))
    }
}

pub fn optimizer_method(opt: &Mutex<Optimizer>, name: &str, args: &[Value]) -> R<Value> {
    let mut opt = opt.lock();
    let path = |i: usize| match args.get(i) {
        Some(Value::Str(p)) => Ok(p.to_string()),
        _ => Err("expected a file path".to_string()),
    };
    match name {
        "step" => opt.step(&tensor_arg(&args[0])?)?,
        "lr" => return Ok(Value::Float(opt.lr())),
        "set_lr" => opt.set_lr(float_arg(&args[0])?),
        "clip" => opt.set_clip(float_arg(&args[0])?),
        "grad_norm" => return Ok(Value::Float(opt.grad_norm())),
        "save" => opt.save(&path(0)?)?,
        "load" => opt.load(&path(0)?)?,
        _ => return Err(format!("Optimizer has no method `{name}`")),
    }
    Ok(Value::None)
}

fn layer_norm(x: &Tensor, weight: &Tensor, bias: &Tensor, eps: f64) -> candle_core::Result<Tensor> {
    let last = D::Minus1;
    let mean = x.mean_keepdim(last)?;
    let centered = x.broadcast_sub(&mean)?;
    let var = centered.sqr()?.mean_keepdim(last)?;
    let normed = centered.broadcast_div(&(var + eps)?.sqrt()?)?;
    normed.broadcast_mul(weight)?.broadcast_add(bias)
}

fn dim(d: i64, rank: usize) -> R<usize> {
    let resolved = if d < 0 { d + rank as i64 } else { d };
    if resolved < 0 || resolved >= rank as i64 {
        return Err(format!(
            "dimension {d} is out of range for a tensor with {rank} dimensions"
        ));
    }
    Ok(resolved as usize)
}

fn matmul(a: &Tensor, b: &Tensor) -> R<Tensor> {
    let (ar, br) = (a.rank(), b.rank());
    if ar == 0 || br == 0 {
        return Err("`@` needs tensors with at least one dimension".into());
    }
    let a2 = if ar == 1 {
        a.unsqueeze(0).map_err(err)?
    } else {
        a.clone()
    };
    let b2 = if br == 1 {
        b.unsqueeze(1).map_err(err)?
    } else {
        b.clone()
    };
    let (ka, kb) = (a2.dims()[a2.rank() - 1], b2.dims()[b2.rank() - 2]);
    if ka != kb {
        return Err(format!(
            "cannot multiply shapes {:?} and {:?}: inner dimensions {ka} and {kb} differ",
            a.dims(),
            b.dims()
        ));
    }
    let (a2, b2) = (a2.contiguous().map_err(err)?, b2.contiguous().map_err(err)?);
    let out = if a2.rank() == b2.rank() {
        a2.matmul(&b2)
    } else {
        a2.broadcast_matmul(&b2)
    }
    .map_err(err)?;
    let out = if ar == 1 {
        out.squeeze(out.rank() - 2).map_err(err)?
    } else {
        out
    };
    if br == 1 {
        out.squeeze(D::Minus1).map_err(err)
    } else {
        Ok(out)
    }
}

pub fn binary(op: BinOp, a: &Value, b: &Value) -> R<Value> {
    let scalar = |v: &Value| v.as_f64();
    let out = match (a, b) {
        (Value::Tensor(x), Value::Tensor(y)) => {
            let (x, y) = (x.get(), y.get());
            match op {
                BinOp::Add => x.broadcast_add(&y),
                BinOp::Sub => x.broadcast_sub(&y),
                BinOp::Mul => x.broadcast_mul(&y),
                BinOp::Div => x.broadcast_div(&y),
                BinOp::MatMul => return matmul(&x, &y).map(wrap),
                _ => return Err(format!("cannot apply `{}` to two tensors", op.symbol())),
            }
            .map_err(|e| shape_error(op, &x, &y, e))?
        }
        (Value::Tensor(x), s) => {
            let x = x.get();
            let s = scalar(s).ok_or_else(|| {
                format!(
                    "cannot apply `{}` to a Tensor and a {}",
                    op.symbol(),
                    s.type_name()
                )
            })?;
            match op {
                BinOp::Add => x.affine(1.0, s),
                BinOp::Sub => x.affine(1.0, -s),
                BinOp::Mul => x.affine(s, 0.0),
                BinOp::Div => {
                    if s == 0.0 {
                        return Err("division by zero".into());
                    }
                    x.affine(1.0 / s, 0.0)
                }
                BinOp::Pow => x.powf(s),
                _ => {
                    return Err(format!(
                        "cannot apply `{}` to a Tensor and a number",
                        op.symbol()
                    ));
                }
            }
            .map_err(err)?
        }
        (s, Value::Tensor(y)) => {
            let y = y.get();
            let s = scalar(s).ok_or_else(|| {
                format!(
                    "cannot apply `{}` to a {} and a Tensor",
                    op.symbol(),
                    s.type_name()
                )
            })?;
            match op {
                BinOp::Add => y.affine(1.0, s),
                BinOp::Sub => y.affine(-1.0, s),
                BinOp::Mul => y.affine(s, 0.0),
                BinOp::Div => y.recip().and_then(|r| r.affine(s, 0.0)),
                _ => {
                    return Err(format!(
                        "cannot apply `{}` to a number and a Tensor",
                        op.symbol()
                    ));
                }
            }
            .map_err(err)?
        }
        _ => unreachable!("one operand is a tensor"),
    };
    Ok(wrap(out))
}

fn shape_error(op: BinOp, x: &Tensor, y: &Tensor, e: candle_core::Error) -> String {
    let msg = err(e);
    if msg.contains("shape") {
        format!(
            "cannot apply `{}` to tensors of shapes {:?} and {:?}",
            op.symbol(),
            x.dims(),
            y.dims()
        )
    } else {
        msg
    }
}

pub fn negate(t: &NTensor) -> R<Value> {
    t.get().neg().map(wrap).map_err(err)
}

pub fn index(t: &NTensor, i: i64) -> R<Value> {
    let t = t.get();
    let len = *t
        .dims()
        .first()
        .ok_or("cannot index a tensor with no dimensions")?;
    let at = super::builtins::index(i, len)?;
    t.get(at).map(wrap).map_err(err)
}

pub fn slice(t: &NTensor, start: Option<i64>, end: Option<i64>) -> R<Value> {
    let t = t.get();
    let len = *t
        .dims()
        .first()
        .ok_or("cannot slice a tensor with no dimensions")?;
    let (a, b) = super::builtins::bounds(start, end, len);
    t.narrow(0, a, b - a).map(wrap).map_err(err)
}

pub fn rows(t: &NTensor) -> R<Vec<Value>> {
    let t = t.get();
    let len = *t
        .dims()
        .first()
        .ok_or("cannot iterate over a tensor with no dimensions")?;
    (0..len).map(|i| t.get(i).map(wrap).map_err(err)).collect()
}

pub fn len(t: &NTensor) -> R<i64> {
    t.get()
        .dims()
        .first()
        .map(|d| *d as i64)
        .ok_or_else(|| "a tensor with no dimensions has no length".to_string())
}

fn values(t: &Tensor) -> R<Vec<f32>> {
    t.flatten_all()
        .and_then(|f| f.to_dtype(DType::F32))
        .and_then(|f| f.to_vec1::<f32>())
        .map_err(err)
}

fn to_list(t: &Tensor) -> R<Value> {
    let data = values(t)?;
    fn build(data: &[f32], dims: &[usize]) -> Value {
        match dims.split_first() {
            None => Value::Float(f64::from(data[0])),
            Some((&n, rest)) => {
                let stride: usize = rest.iter().product();
                Value::list((0..n).map(|i| build(&data[i * stride..], rest)).collect())
            }
        }
    }
    Ok(build(&data, t.dims()))
}

fn format_f32(f: f32) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else if f.abs() >= 1e16 || f.abs() < 1e-4 {
        format!("{f:e}")
    } else {
        format!("{f}")
    }
}

/// Formats like `tensor([[1.0, 2.0], [3.0, 4.0]])`, eliding the middle of long dimensions.
pub fn display(t: &NTensor) -> String {
    let t = t.get();
    let Ok(data) = values(&t) else {
        return "tensor(<unprintable>)".into();
    };
    fn write(out: &mut String, data: &[f32], dims: &[usize]) {
        match dims.split_first() {
            None => out.push_str(&format_f32(data[0])),
            Some((&n, rest)) => {
                let stride: usize = rest.iter().product();
                out.push('[');
                let shown: Vec<usize> = if n > 6 {
                    (0..3).chain(n - 3..n).collect()
                } else {
                    (0..n).collect()
                };
                for (k, i) in shown.iter().enumerate() {
                    if k > 0 {
                        out.push_str(", ");
                    }
                    if n > 6 && k == 3 {
                        out.push_str("..., ");
                    }
                    write(out, &data[i * stride..], rest);
                }
                out.push(']');
            }
        }
    }
    let mut out = String::from("tensor(");
    if t.elem_count() == 0 {
        out.push_str(&format!("[], shape={:?}", t.dims()));
    } else {
        write(&mut out, &data, t.dims());
    }
    out.push(')');
    out
}
