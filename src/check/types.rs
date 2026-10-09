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
    Tensor,
    Model,
    Optimizer,
    Any,
    Var(u32),
}

/// How a value of a type is represented at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr {
    Int,
    Float,
    Bool,
    Obj,
    Unit,
}

impl Ty {
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }

    pub fn map(key: Ty, value: Ty) -> Ty {
        Ty::Map(Box::new(key), Box::new(value))
    }

    pub fn repr(&self) -> Repr {
        match self {
            Ty::Int => Repr::Int,
            Ty::Float => Repr::Float,
            Ty::Bool => Repr::Bool,
            Ty::None => Repr::Unit,
            Ty::Var(_) => Repr::Int,
            _ => Repr::Obj,
        }
    }

    pub fn is_obj(&self) -> bool {
        self.repr() == Repr::Obj
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    pub fn has_vars(&self) -> bool {
        match self {
            Ty::Var(_) => true,
            Ty::List(t) | Ty::Task(t) => t.has_vars(),
            Ty::Map(k, v) => k.has_vars() || v.has_vars(),
            Ty::Func(ps, r) => ps.iter().any(Ty::has_vars) || r.has_vars(),
            _ => false,
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
                f.write_str("Function(")?;
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
            Ty::Tensor => f.write_str("Tensor"),
            Ty::Model => f.write_str("Model"),
            Ty::Optimizer => f.write_str("Optimizer"),
            Ty::Any => f.write_str("Any"),
            Ty::Var(_) => f.write_str("_"),
        }
    }
}

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
            Ty::Task(t) => Ty::Task(Box::new(self.zonk(&t))),
            Ty::Map(k, v) => Ty::map(self.zonk(&k), self.zonk(&v)),
            Ty::Func(ps, r) => Ty::Func(ps.iter().map(|p| self.zonk(p)).collect(), Box::new(self.zonk(&r))),
            other => other,
        }
    }

    /// Zonks a type and replaces unresolved variables with `None`.
    pub fn finalize(&self, ty: &Ty) -> Ty {
        match self.shallow(ty) {
            Ty::Var(_) => Ty::None,
            Ty::List(t) => Ty::list(self.finalize_elem(&t)),
            Ty::Task(t) => Ty::Task(Box::new(self.finalize(&t))),
            Ty::Map(k, v) => Ty::map(self.finalize_elem(&k), self.finalize_elem(&v)),
            Ty::Func(ps, r) => Ty::Func(ps.iter().map(|p| self.finalize(p)).collect(), Box::new(self.finalize(&r))),
            other => other,
        }
    }

    fn finalize_elem(&self, ty: &Ty) -> Ty {
        match self.shallow(ty) {
            Ty::Var(_) => Ty::Int,
            _ => self.finalize(ty),
        }
    }

    fn occurs(&self, var: u32, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Var(v) => v == var,
            Ty::List(t) | Ty::Task(t) => self.occurs(var, &t),
            Ty::Map(k, v) => self.occurs(var, &k) || self.occurs(var, &v),
            Ty::Func(ps, r) => ps.iter().any(|p| self.occurs(var, p)) || self.occurs(var, &r),
            _ => false,
        }
    }

    pub fn unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => true,
            (Ty::Var(x), _) => {
                if self.occurs(*x, &b) {
                    return false;
                }
                self.vars[*x as usize] = Some(b);
                true
            }
            (_, Ty::Var(y)) => {
                if self.occurs(*y, &a) {
                    return false;
                }
                self.vars[*y as usize] = Some(a);
                true
            }
            (Ty::List(x), Ty::List(y)) | (Ty::Task(x), Ty::Task(y)) => self.unify(x, y),
            (Ty::Map(k1, v1), Ty::Map(k2, v2)) => self.unify(k1, k2) && self.unify(v1, v2),
            (Ty::Func(p1, r1), Ty::Func(p2, r2)) => {
                p1.len() == p2.len() && p1.iter().zip(p2).all(|(x, y)| self.unify(x, y)) && self.unify(r1, r2)
            }
            _ => a == b,
        }
    }
}
