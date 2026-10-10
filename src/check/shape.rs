//! Tensor shapes at check time. A shape is a list of dimensions, each a known
//! size (`Ty::Dim`), a variable, or `Ty::Any`. Operations on tensors of unknown
//! shape produce tensors of unknown shape; these rules only reject what would
//! certainly fail at run time.

use super::hir;
use super::types::{Infer, Ty};

pub type Dims = Vec<Ty>;

/// A method argument written as a literal, which shape rules can use.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Int(i64),
    Ints(Vec<i64>),
    Bool(bool),
    Other,
}

impl Lit {
    pub fn of(e: &hir::Expr) -> Lit {
        match &e.kind {
            hir::ExprKind::Bool(b) => Lit::Bool(*b),
            hir::ExprKind::List(items) => {
                let ints: Option<Vec<i64>> = items.iter().map(int_literal).collect();
                ints.map_or(Lit::Other, Lit::Ints)
            }
            _ => int_literal(e).map_or(Lit::Other, Lit::Int),
        }
    }
}

fn int_literal(e: &hir::Expr) -> Option<i64> {
    match &e.kind {
        hir::ExprKind::Int(n) => Some(*n),
        hir::ExprKind::Unary {
            op: crate::syntax::ast::UnaryOp::Neg,
            operand,
        } => int_literal(operand).map(|n| -n),
        _ => None,
    }
}

pub fn show(dims: &[Ty]) -> String {
    Ty::Tensor(Some(dims.to_vec())).display(&[]).to_string()
}

fn size(infer: &Infer, d: &Ty) -> Option<u64> {
    match infer.shallow(d) {
        Ty::Dim(n) => Some(n),
        _ => None,
    }
}

/// The shape of a nested list literal of numbers, as `nn.tensor` builds.
pub fn of_literal(e: &hir::Expr) -> Option<Dims> {
    match &e.kind {
        hir::ExprKind::List(items) => {
            let inner = match items.first() {
                Some(first) => of_literal(first)?,
                None => Vec::new(),
            };
            if items
                .iter()
                .skip(1)
                .any(|i| of_literal(i).as_ref() != Some(&inner))
            {
                return None;
            }
            Some(
                std::iter::once(Ty::Dim(items.len() as u64))
                    .chain(inner)
                    .collect(),
            )
        }
        hir::ExprKind::Int(_) | hir::ExprKind::Float(_) | hir::ExprKind::IntToFloat(_) => {
            Some(Vec::new())
        }
        hir::ExprKind::Unary { operand, .. } => of_literal(operand).filter(Vec::is_empty),
        _ => None,
    }
}

/// Element-wise operations broadcast: dimensions are aligned from the right
/// and each pair must match or one of them must be 1.
pub fn broadcast(infer: &Infer, a: &[Ty], b: &[Ty]) -> Result<Dims, String> {
    let n = a.len().max(b.len());
    let at = |s: &[Ty], i: usize| (i + s.len()).checked_sub(n).map(|j| s[j].clone());
    (0..n)
        .map(|i| match (at(a, i), at(b, i)) {
            (Some(x), None) | (None, Some(x)) => Ok(x),
            (Some(x), Some(y)) => match (size(infer, &x), size(infer, &y)) {
                (Some(p), Some(q)) if p == q || q == 1 => Ok(x),
                (Some(1), Some(_)) => Ok(y),
                (Some(_), Some(_)) => Err(format!(
                    "cannot broadcast shapes {} and {}",
                    show(a),
                    show(b)
                )),
                (Some(1), None) => Ok(y),
                (None, Some(1)) => Ok(x),
                (Some(_), None) => Ok(x),
                (None, Some(_)) => Ok(y),
                (None, None) if infer.shallow(&x) == infer.shallow(&y) => Ok(x),
                (None, None) => Ok(Ty::Any),
            },
            (None, None) => unreachable!("index below the longer rank"),
        })
        .collect()
}

/// Matrix multiplication over the last two dimensions, broadcasting the rest.
/// A vector on either side is treated as a row or column and its dimension dropped.
pub fn matmul(infer: &mut Infer, a: &[Ty], b: &[Ty]) -> Result<Dims, String> {
    if a.is_empty() || b.is_empty() {
        return Err("`@` needs tensors with at least one dimension".into());
    }
    let ka = &a[a.len() - 1];
    let kb = &b[b.len().saturating_sub(2)];
    if let (Some(p), Some(q)) = (size(infer, ka), size(infer, kb))
        && p != q
    {
        return Err(format!(
            "cannot multiply shapes {} and {}: inner dimensions {p} and {q} differ",
            show(a),
            show(b)
        ));
    }
    infer.unify(ka, kb);
    let batch_a = if a.len() > 2 { &a[..a.len() - 2] } else { &[] };
    let batch_b = if b.len() > 2 { &b[..b.len() - 2] } else { &[] };
    let mut out = broadcast(infer, batch_a, batch_b).map_err(|_| {
        format!(
            "cannot multiply shapes {} and {}: batch dimensions differ",
            show(a),
            show(b)
        )
    })?;
    if a.len() >= 2 {
        out.push(a[a.len() - 2].clone());
    }
    if b.len() >= 2 {
        out.push(b[b.len() - 1].clone());
    }
    Ok(out)
}

fn axis(d: i64, rank: usize) -> Result<usize, String> {
    let resolved = if d < 0 { d + rank as i64 } else { d };
    if resolved < 0 || resolved >= rank as i64 {
        return Err(format!(
            "dimension {d} is out of range for a tensor with {rank} dimensions"
        ));
    }
    Ok(resolved as usize)
}

/// The shape a tensor method returns, when it can be told from the receiver's
/// shape and literal arguments. `Ok(None)` means unknown.
pub fn method(infer: &Infer, name: &str, d: &[Ty], lits: &[Lit]) -> Result<Option<Dims>, String> {
    let lit_int = |i: usize| match lits.get(i) {
        Some(Lit::Int(n)) => Some(*n),
        _ => None,
    };
    let keepdim = |i: usize| match lits.get(i) {
        None => Some(false),
        Some(Lit::Bool(b)) => Some(*b),
        _ => None,
    };
    let reduce = |dim: Option<i64>, keep: Option<bool>| -> Result<Option<Dims>, String> {
        let (Some(dim), Some(keep)) = (dim, keep) else {
            return Ok(None);
        };
        let a = axis(dim, d.len())?;
        let mut out = d.to_vec();
        if keep {
            out[a] = Ty::Dim(1);
        } else {
            out.remove(a);
        }
        Ok(Some(out))
    };
    Ok(match name {
        "relu" | "sigmoid" | "tanh" | "exp" | "log" | "sqrt" | "abs" | "gelu" | "silu"
        | "square" | "detach" | "copy" | "grad" | "pow" | "clamp" | "masked_fill"
        | "layer_norm" | "rms_norm" | "rope" | "sin" | "cos" | "to" | "to_dtype" | "softmax"
        | "log_softmax" => Some(d.to_vec()),
        "t" if d.len() >= 2 => {
            let mut out = d.to_vec();
            out.swap(d.len() - 1, d.len() - 2);
            Some(out)
        }
        "t" => return Err("`t` needs a tensor with at least two dimensions".into()),
        "transpose" => match (lit_int(0), lit_int(1)) {
            (Some(i), Some(j)) => {
                let mut out = d.to_vec();
                out.swap(axis(i, d.len())?, axis(j, d.len())?);
                Some(out)
            }
            _ => None,
        },
        "sum" | "mean" if lits.is_empty() => Some(Vec::new()),
        "sum" | "mean" | "max" | "min" => reduce(lit_int(0), keepdim(1))?,
        "argmax" | "argmin" => reduce(lit_int(0), Some(false))?,
        "squeeze" => match lit_int(0) {
            Some(i) => {
                let a = axis(i, d.len())?;
                match size(infer, &d[a]) {
                    Some(1) => Some(
                        d.iter()
                            .enumerate()
                            .filter(|(j, _)| *j != a)
                            .map(|(_, x)| x.clone())
                            .collect(),
                    ),
                    Some(_) => Some(d.to_vec()),
                    None => None,
                }
            }
            None => None,
        },
        "unsqueeze" => match lit_int(0) {
            Some(i) => {
                let a = axis(i, d.len() + 1)?;
                let mut out = d.to_vec();
                out.insert(a, Ty::Dim(1));
                Some(out)
            }
            None => None,
        },
        "narrow" => match (lit_int(0), lit_int(2)) {
            (Some(i), Some(len)) if len >= 0 => {
                let mut out = d.to_vec();
                out[axis(i, d.len())?] = Ty::Dim(len as u64);
                Some(out)
            }
            _ => None,
        },
        "flatten" => {
            let sizes: Option<Vec<u64>> = d.iter().map(|x| size(infer, x)).collect();
            Some(vec![sizes.map_or(Ty::Any, |s| Ty::Dim(s.iter().product()))])
        }
        "reshape" => match lits.first() {
            Some(Lit::Ints(dims)) => reshape(infer, d, dims)?,
            _ => None,
        },
        "take" => match lits.first() {
            Some(Lit::Ints(ids)) if !d.is_empty() => Some(
                std::iter::once(Ty::Dim(ids.len() as u64))
                    .chain(d[1..].iter().cloned())
                    .collect(),
            ),
            _ if !d.is_empty() => Some(
                std::iter::once(Ty::Any)
                    .chain(d[1..].iter().cloned())
                    .collect(),
            ),
            _ => None,
        },
        _ => None,
    })
}

fn reshape(infer: &Infer, d: &[Ty], dims: &[i64]) -> Result<Option<Dims>, String> {
    if dims.iter().any(|&x| x < -1) || dims.iter().filter(|&&x| x == -1).count() > 1 {
        return Ok(None);
    }
    let total: Option<u64> = d.iter().map(|x| size(infer, x)).product();
    let known: u64 = dims
        .iter()
        .filter(|&&x| x >= 0)
        .map(|&x| x as u64)
        .product();
    if let Some(total) = total {
        let fits = if dims.contains(&-1) {
            known != 0 && total % known == 0
        } else {
            known == total
        };
        if !fits {
            return Err(format!(
                "cannot reshape a tensor of shape {} into {dims:?}",
                show(d)
            ));
        }
    }
    Ok(Some(
        dims.iter()
            .map(|&x| match (x, total) {
                (-1, Some(total)) => Ty::Dim(total / known),
                (-1, None) => Ty::Any,
                (x, _) => Ty::Dim(x as u64),
            })
            .collect(),
    ))
}
