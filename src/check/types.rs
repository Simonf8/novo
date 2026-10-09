use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClassId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    None,
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Func(Vec<Ty>, Box<Ty>),
    Class(ClassId),
    Task(Box<Ty>),
    Any,
    Var(u32),
}

impl Ty {
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }

    pub fn map(key: Ty, value: Ty) -> Ty {
        Ty::Map(Box::new(key), Box::new(value))
    }

    pub fn task(inner: Ty) -> Ty {
        Ty::Task(Box::new(inner))
    }

    pub fn func(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::Func(params, Box::new(ret))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    pub fn children(&self) -> Vec<&Ty> {
        match self {
            Ty::List(t) | Ty::Task(t) => vec![t],
            Ty::Map(k, v) => vec![k, v],
            Ty::Func(ps, r) => ps.iter().chain(std::iter::once(&**r)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn display<'a>(&'a self, classes: &'a [String]) -> TyDisplay<'a> {
        TyDisplay { ty: self, classes }
    }
}

pub struct TyDisplay<'a> {
    ty: &'a Ty,
    classes: &'a [String],
}

impl<'a> fmt::Display for TyDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sub = |ty: &'a Ty| TyDisplay { ty, classes: self.classes };
        match self.ty {
            Ty::Int => f.write_str("Int"),
            Ty::Float => f.write_str("Float"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Str => f.write_str("String"),
            Ty::None => f.write_str("None"),
            Ty::List(t) => write!(f, "List[{}]", sub(t)),
            Ty::Map(k, v) => write!(f, "Map[{}, {}]", sub(k), sub(v)),
            Ty::Func(ps, r) => {
                f.write_str("(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", sub(p))?;
                }
                write!(f, ") -> {}", sub(r))
            }
            Ty::Class(id) => f.write_str(self.classes.get(id.0 as usize).map(String::as_str).unwrap_or("<class>")),
            Ty::Task(t) => write!(f, "Task[{}]", sub(t)),
            Ty::Any => f.write_str("Any"),
            Ty::Var(_) => f.write_str("_"),
        }
    }
}

/// Union-find style substitution for type variables.
#[derive(Default)]
pub struct Infer {
    vars: Vec<Option<Ty>>,
}

impl Infer {
    pub fn fresh(&mut self) -> Ty {
        self.vars.push(None);
        Ty::Var(self.vars.len() as u32 - 1)
    }

    pub fn shallow(&self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        while let Ty::Var(v) = ty {
            match &self.vars[v as usize] {
                Some(t) => ty = t.clone(),
                None => break,
            }
        }
        ty
    }

    pub fn zonk(&self, ty: &Ty) -> Ty {
        match self.shallow(ty) {
            Ty::List(t) => Ty::list(self.zonk(&t)),
            Ty::Task(t) => Ty::task(self.zonk(&t)),
            Ty::Map(k, v) => Ty::map(self.zonk(&k), self.zonk(&v)),
            Ty::Func(ps, r) => Ty::func(ps.iter().map(|p| self.zonk(p)).collect(), self.zonk(&r)),
            other => other,
        }
    }

    pub fn free_vars(&self, ty: &Ty, out: &mut Vec<u32>) {
        match self.shallow(ty) {
            Ty::Var(v) => {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            t => {
                for c in t.children() {
                    self.free_vars(c, out);
                }
            }
        }
    }

    fn occurs(&self, var: u32, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Var(v) => v == var,
            t => t.children().into_iter().any(|c| self.occurs(var, c)),
        }
    }

    pub fn bind(&mut self, var: u32, ty: Ty) {
        self.vars[var as usize] = Some(ty);
    }

    /// Makes two types equal. `Any` is compatible with every type without binding it.
    pub fn unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => true,
            (Ty::Var(x), _) => {
                if self.occurs(*x, &b) {
                    return false;
                }
                self.bind(*x, b);
                true
            }
            (_, Ty::Var(y)) => {
                if self.occurs(*y, &a) {
                    return false;
                }
                self.bind(*y, a);
                true
            }
            (Ty::Any, _) | (_, Ty::Any) => true,
            (Ty::List(x), Ty::List(y)) | (Ty::Task(x), Ty::Task(y)) => self.unify(x, y),
            (Ty::Map(k1, v1), Ty::Map(k2, v2)) => self.unify(k1, k2) && self.unify(v1, v2),
            (Ty::Func(p1, r1), Ty::Func(p2, r2)) => {
                p1.len() == p2.len() && p1.iter().zip(p2).all(|(x, y)| self.unify(x, y)) && self.unify(r1, r2)
            }
            _ => a == b,
        }
    }

    /// Replaces the given variables with fresh ones throughout `ty`.
    pub fn substitute(&self, ty: &Ty, map: &[(u32, Ty)]) -> Ty {
        match self.shallow(ty) {
            Ty::Var(v) => map.iter().find(|(k, _)| *k == v).map(|(_, t)| t.clone()).unwrap_or(Ty::Var(v)),
            Ty::List(t) => Ty::list(self.substitute(&t, map)),
            Ty::Task(t) => Ty::task(self.substitute(&t, map)),
            Ty::Map(k, v) => Ty::map(self.substitute(&k, map), self.substitute(&v, map)),
            Ty::Func(ps, r) => Ty::func(ps.iter().map(|p| self.substitute(p, map)).collect(), self.substitute(&r, map)),
            other => other,
        }
    }
}
