//! List storage. Lists whose elements are all Ints, Floats or Bools store
//! them unboxed, which compiled code reads and writes directly; storing any
//! other kind of value switches the list to general storage.

use super::value::Value;

#[derive(Debug, Clone)]
pub enum Items {
    Values(Vec<Value>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
    Bools(Vec<bool>),
}

impl Default for Items {
    fn default() -> Self {
        Items::Values(Vec::new())
    }
}

impl From<Vec<Value>> for Items {
    fn from(values: Vec<Value>) -> Items {
        let unboxed = match values.first() {
            Some(Value::Int(_)) => values
                .iter()
                .map(|v| {
                    if let Value::Int(i) = v {
                        Some(*i)
                    } else {
                        None
                    }
                })
                .collect::<Option<_>>()
                .map(Items::Ints),
            Some(Value::Float(_)) => values
                .iter()
                .map(|v| {
                    if let Value::Float(f) = v {
                        Some(*f)
                    } else {
                        None
                    }
                })
                .collect::<Option<_>>()
                .map(Items::Floats),
            Some(Value::Bool(_)) => values
                .iter()
                .map(|v| {
                    if let Value::Bool(b) = v {
                        Some(*b)
                    } else {
                        None
                    }
                })
                .collect::<Option<_>>()
                .map(Items::Bools),
            _ => None,
        };
        unboxed.unwrap_or(Items::Values(values))
    }
}

impl Items {
    pub fn len(&self) -> usize {
        match self {
            Items::Values(v) => v.len(),
            Items::Ints(v) => v.len(),
            Items::Floats(v) => v.len(),
            Items::Bools(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<Value> {
        match self {
            Items::Values(v) => v.get(i).cloned(),
            Items::Ints(v) => v.get(i).map(|x| Value::Int(*x)),
            Items::Floats(v) => v.get(i).map(|x| Value::Float(*x)),
            Items::Bools(v) => v.get(i).map(|x| Value::Bool(*x)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Value> + '_ {
        (0..self.len()).map(|i| self.get(i).expect("index below the length"))
    }

    pub fn to_vec(&self) -> Vec<Value> {
        match self {
            Items::Values(v) => v.clone(),
            _ => self.iter().collect(),
        }
    }

    /// The elements as general values, converting the storage if needed.
    pub fn values_mut(&mut self) -> &mut Vec<Value> {
        if !matches!(self, Items::Values(_)) {
            *self = Items::Values(self.to_vec());
        }
        match self {
            Items::Values(v) => v,
            _ => unreachable!("converted above"),
        }
    }

    /// Whether `v` can be stored without leaving the current storage.
    fn fits(&self, v: &Value) -> bool {
        matches!(
            (self, v),
            (Items::Values(_), _)
                | (Items::Ints(_), Value::Int(_))
                | (Items::Floats(_), Value::Float(_))
                | (Items::Bools(_), Value::Bool(_))
        )
    }

    pub fn set(&mut self, i: usize, v: Value) {
        if !self.fits(&v) {
            self.values_mut()[i] = v;
            return;
        }
        match (self, v) {
            (Items::Values(xs), v) => xs[i] = v,
            (Items::Ints(xs), Value::Int(x)) => xs[i] = x,
            (Items::Floats(xs), Value::Float(x)) => xs[i] = x,
            (Items::Bools(xs), Value::Bool(x)) => xs[i] = x,
            _ => unreachable!("checked by fits"),
        }
    }

    pub fn push(&mut self, v: Value) {
        if self.is_empty() {
            *self = Items::from(vec![v]);
            return;
        }
        if !self.fits(&v) {
            self.values_mut().push(v);
            return;
        }
        match (self, v) {
            (Items::Values(xs), v) => xs.push(v),
            (Items::Ints(xs), Value::Int(x)) => xs.push(x),
            (Items::Floats(xs), Value::Float(x)) => xs.push(x),
            (Items::Bools(xs), Value::Bool(x)) => xs.push(x),
            _ => unreachable!("checked by fits"),
        }
    }

    pub fn pop(&mut self) -> Option<Value> {
        match self {
            Items::Values(v) => v.pop(),
            Items::Ints(v) => v.pop().map(Value::Int),
            Items::Floats(v) => v.pop().map(Value::Float),
            Items::Bools(v) => v.pop().map(Value::Bool),
        }
    }

    pub fn insert(&mut self, i: usize, v: Value) {
        if !self.fits(&v) {
            self.values_mut().insert(i, v);
            return;
        }
        match (self, v) {
            (Items::Values(xs), v) => xs.insert(i, v),
            (Items::Ints(xs), Value::Int(x)) => xs.insert(i, x),
            (Items::Floats(xs), Value::Float(x)) => xs.insert(i, x),
            (Items::Bools(xs), Value::Bool(x)) => xs.insert(i, x),
            _ => unreachable!("checked by fits"),
        }
    }

    pub fn remove(&mut self, i: usize) -> Value {
        match self {
            Items::Values(v) => v.remove(i),
            Items::Ints(v) => Value::Int(v.remove(i)),
            Items::Floats(v) => Value::Float(v.remove(i)),
            Items::Bools(v) => Value::Bool(v.remove(i)),
        }
    }

    pub fn clear(&mut self) {
        *self = Items::default();
    }

    pub fn reverse(&mut self) {
        match self {
            Items::Values(v) => v.reverse(),
            Items::Ints(v) => v.reverse(),
            Items::Floats(v) => v.reverse(),
            Items::Bools(v) => v.reverse(),
        }
    }

    pub fn extend(&mut self, values: impl IntoIterator<Item = Value>) {
        for v in values {
            self.push(v);
        }
    }

    /// A new list holding elements `start..end`.
    pub fn slice(&self, start: usize, end: usize) -> Items {
        match self {
            Items::Values(v) => Items::Values(v[start..end].to_vec()),
            Items::Ints(v) => Items::Ints(v[start..end].to_vec()),
            Items::Floats(v) => Items::Floats(v[start..end].to_vec()),
            Items::Bools(v) => Items::Bools(v[start..end].to_vec()),
        }
    }
}
