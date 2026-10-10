//! Optimizers: SGD with momentum, Adam and AdamW, with gradient clipping by
//! global norm and state that can be saved and restored.

use candle_core::{DType, Tensor, Var};
use std::collections::HashMap;

type R<T> = Result<T, String>;

fn err(e: candle_core::Error) -> String {
    e.to_string()
        .lines()
        .next()
        .unwrap_or("tensor error")
        .to_string()
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Sgd {
        momentum: f64,
    },
    Adam {
        beta1: f64,
        beta2: f64,
        eps: f64,
        weight_decay: f64,
    },
}

pub struct Optimizer {
    kind: Kind,
    vars: Vec<Var>,
    lr: f64,
    /// Gradients are scaled down when their global norm exceeds this.
    clip: Option<f64>,
    grad_norm: f64,
    steps: u64,
    /// Per-parameter state: the momentum buffer for SGD, the two moments for Adam.
    slots: Vec<Vec<Tensor>>,
}

impl std::fmt::Debug for Optimizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.kind {
            Kind::Sgd { .. } => "SGD",
            Kind::Adam { weight_decay, .. } if weight_decay == 0.0 => "Adam",
            Kind::Adam { .. } => "AdamW",
        })
    }
}

impl Optimizer {
    pub fn new(kind: Kind, vars: Vec<Var>, lr: f64) -> R<Optimizer> {
        let per_param = match kind {
            Kind::Sgd { momentum } if momentum == 0.0 => 0,
            Kind::Sgd { .. } => 1,
            Kind::Adam { .. } => 2,
        };
        let slots = vars
            .iter()
            .map(|v| {
                (0..per_param)
                    .map(|_| v.as_tensor().zeros_like())
                    .collect::<candle_core::Result<Vec<_>>>()
            })
            .collect::<candle_core::Result<_>>()
            .map_err(err)?;
        Ok(Optimizer {
            kind,
            vars,
            lr,
            clip: None,
            grad_norm: 0.0,
            steps: 0,
            slots,
        })
    }

    pub fn lr(&self) -> f64 {
        self.lr
    }

    pub fn set_lr(&mut self, lr: f64) {
        self.lr = lr;
    }

    pub fn set_clip(&mut self, max_norm: f64) {
        self.clip = (max_norm > 0.0).then_some(max_norm);
    }

    /// The global gradient norm from the latest step, before clipping.
    pub fn grad_norm(&self) -> f64 {
        self.grad_norm
    }

    /// Runs backward from `loss` and updates every parameter.
    pub fn step(&mut self, loss: &Tensor) -> R<()> {
        let store = loss.backward().map_err(err)?;
        let grads: Vec<Option<Tensor>> = self
            .vars
            .iter()
            .map(|v| store.get(v.as_tensor()).cloned())
            .collect();
        let mut sum = 0.0;
        for g in grads.iter().flatten() {
            sum += g
                .to_dtype(DType::F64)
                .and_then(|g| g.sqr()?.sum_all()?.to_scalar::<f64>())
                .map_err(err)?;
        }
        self.grad_norm = sum.sqrt();
        let scale = match self.clip {
            Some(max) if self.grad_norm > max => max / (self.grad_norm + 1e-6),
            _ => 1.0,
        };
        self.steps += 1;
        for (i, g) in grads.into_iter().enumerate() {
            let Some(g) = g else { continue };
            let g = if scale == 1.0 {
                g
            } else {
                (g * scale).map_err(err)?
            };
            self.update(i, g).map_err(err)?;
        }
        Ok(())
    }

    fn update(&mut self, i: usize, g: Tensor) -> candle_core::Result<()> {
        let p = self.vars[i].as_tensor();
        let next = match self.kind {
            Kind::Sgd { momentum } if momentum == 0.0 => (p - (g * self.lr)?)?,
            Kind::Sgd { momentum } => {
                let buf = ((&self.slots[i][0] * momentum)? + g)?;
                let next = (p - (&buf * self.lr)?)?;
                self.slots[i][0] = buf;
                next
            }
            Kind::Adam {
                beta1,
                beta2,
                eps,
                weight_decay,
            } => {
                let t = self.steps as i32;
                let m = ((&self.slots[i][0] * beta1)? + (&g * (1.0 - beta1))?)?;
                let v = ((&self.slots[i][1] * beta2)? + (g.sqr()? * (1.0 - beta2))?)?;
                let m_hat = (&m / (1.0 - beta1.powi(t)))?;
                let v_hat = (&v / (1.0 - beta2.powi(t)))?;
                let decayed = (p * (1.0 - self.lr * weight_decay))?;
                let next = (decayed - ((m_hat / (v_hat.sqrt()? + eps)?)? * self.lr)?)?;
                self.slots[i] = vec![m, v];
                next
            }
        };
        self.vars[i].set(&next)
    }

    /// Writes the step count, learning rate and per-parameter state to a safetensors file.
    pub fn save(&self, path: &str) -> R<()> {
        let mut named = HashMap::new();
        let meta =
            Tensor::new(&[self.steps as f64, self.lr], &candle_core::Device::Cpu).map_err(err)?;
        named.insert("optimizer.meta".to_string(), meta);
        for (i, slots) in self.slots.iter().enumerate() {
            for (k, t) in slots.iter().enumerate() {
                named.insert(format!("optimizer.{i}.{k}"), t.contiguous().map_err(err)?);
            }
        }
        candle_core::safetensors::save(&named, path).map_err(err)
    }

    /// Restores state written by `save` for an optimizer over the same parameters.
    pub fn load(&mut self, path: &str) -> R<()> {
        let device = self
            .vars
            .first()
            .map(|v| v.device().clone())
            .unwrap_or(candle_core::Device::Cpu);
        let named = candle_core::safetensors::load(path, &device).map_err(err)?;
        let meta = named
            .get("optimizer.meta")
            .ok_or("this file is not an optimizer checkpoint")?;
        let meta: Vec<f64> = meta.to_vec1().map_err(err)?;
        let mut slots = self.slots.clone();
        for (i, param_slots) in slots.iter_mut().enumerate() {
            for (k, slot) in param_slots.iter_mut().enumerate() {
                let saved = named
                    .get(&format!("optimizer.{i}.{k}"))
                    .ok_or("the checkpoint was saved for a different set of parameters")?;
                if saved.dims() != slot.dims() {
                    return Err(format!(
                        "the checkpoint has shape {:?} for parameter {i}, which now has shape {:?}",
                        saved.dims(),
                        slot.dims()
                    ));
                }
                *slot = saved.to_dtype(slot.dtype()).map_err(err)?;
            }
        }
        self.slots = slots;
        self.steps = meta[0] as u64;
        self.lr = meta[1];
        Ok(())
    }
}

/// Linear warmup to `max_lr` over `warmup` steps, then cosine decay to `min_lr` at `total`.
pub fn cosine_lr(step: i64, warmup: i64, total: i64, max_lr: f64, min_lr: f64) -> f64 {
    if step < warmup {
        return max_lr * (step + 1) as f64 / warmup as f64;
    }
    if step >= total {
        return min_lr;
    }
    let progress = (step - warmup) as f64 / (total - warmup).max(1) as f64;
    min_lr + 0.5 * (max_lr - min_lr) * (1.0 + (std::f64::consts::PI * progress).cos())
}
