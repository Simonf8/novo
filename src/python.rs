//! Bridge to CPython. Compiled only with `--features python`; otherwise every
//! entry point reports that interop is disabled.

pub use imp::{init, PyHandle};
pub use modern::*;

/// The bridge for the old engines' value type.
pub mod legacy {
    pub use super::imp::*;
}

#[cfg(feature = "python")]
mod imp {
    use crate::interpreter::Value;
    use parking_lot::RwLock;
    use pyo3::prelude::*;
    use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug)]
    pub struct PyHandle(pub(crate) PyObject);

    impl Clone for PyHandle {
        fn clone(&self) -> Self {
            Python::with_gil(|py| PyHandle(self.0.clone_ref(py)))
        }
    }

    pub fn init() {
        pyo3::prepare_freethreaded_python();
    }

    fn to_py(py: Python, val: &Value) -> Result<PyObject, String> {
        match val {
            Value::Integer(i) => Ok(i.into_py(py)),
            Value::Float(f) => Ok(f.into_py(py)),
            Value::String(s) => Ok(s.into_py(py)),
            Value::Boolean(b) => Ok(b.into_py(py)),
            Value::List(list) => {
                let items = list.read().iter().map(|v| to_py(py, v)).collect::<Result<Vec<_>, _>>()?;
                Ok(items.into_py(py))
            }
            Value::Map(map) => {
                let dict = PyDict::new(py);
                for (k, v) in map.read().iter() {
                    dict.set_item(k, to_py(py, v)?).map_err(|e| e.to_string())?;
                }
                Ok(dict.into_py(py))
            }
            Value::PythonObject(obj) => Ok(obj.0.clone_ref(py)),
            Value::Null => Ok(py.None()),
            _ => Err(format!("cannot pass {} to Python", crate::interpreter::type_name(val))),
        }
    }

    fn from_py(py: Python, obj: PyObject) -> Result<Value, String> {
        if obj.is_none(py) {
            return Ok(Value::Null);
        }
        if let Ok(b) = obj.extract::<bool>(py) {
            return Ok(Value::Boolean(b));
        }
        if let Ok(i) = obj.extract::<i64>(py) {
            return Ok(Value::Integer(i));
        }
        if let Ok(f) = obj.extract::<f64>(py) {
            return Ok(Value::Float(f));
        }
        if let Ok(s) = obj.extract::<String>(py) {
            return Ok(Value::String(s));
        }
        if obj.downcast::<PyBytes>(py).is_ok() {
            return Ok(Value::PythonObject(PyHandle(obj)));
        }
        if let Ok(dict) = obj.downcast::<PyDict>(py) {
            let mut map = HashMap::new();
            for (k, v) in dict {
                if let Ok(key) = k.extract::<String>() {
                    map.insert(key, from_py(py, v.into())?);
                }
            }
            return Ok(Value::Map(Arc::new(RwLock::new(map))));
        }
        let seq: Option<Vec<PyObject>> = if let Ok(list) = obj.downcast::<PyList>(py) {
            Some(list.iter().map(Into::into).collect())
        } else if let Ok(tuple) = obj.downcast::<PyTuple>(py) {
            Some(tuple.iter().map(Into::into).collect())
        } else {
            None
        };
        if let Some(items) = seq {
            let values = items.into_iter().map(|o| from_py(py, o)).collect::<Result<Vec<_>, _>>()?;
            return Ok(Value::List(Arc::new(RwLock::new(values))));
        }
        Ok(Value::PythonObject(PyHandle(obj)))
    }

    pub fn import(module: &str) -> Result<Value, String> {
        Python::with_gil(|py| {
            let m = PyModule::import(py, module).map_err(|e| format!("failed to import Python module {module}: {e}"))?;
            Ok(Value::PythonObject(PyHandle(m.into_py(py))))
        })
    }

    pub fn call(obj: &PyHandle, args: &[Value]) -> Result<Value, String> {
        Python::with_gil(|py| {
            let args = args.iter().map(|a| to_py(py, a)).collect::<Result<Vec<_>, _>>()?;
            let result = obj.0.call1(py, PyTuple::new(py, args)).map_err(|e| format!("Python error during call: {e}"))?;
            from_py(py, result)
        })
    }

    pub fn get_attr(obj: &PyHandle, name: &str) -> Result<Value, String> {
        Python::with_gil(|py| {
            if let Ok(attr) = obj.0.getattr(py, name) {
                from_py(py, attr)
            } else if let Ok(item) = obj.0.call_method1(py, "__getitem__", (name,)) {
                from_py(py, item)
            } else {
                Err(format!("Python object has no attribute or key '{name}'"))
            }
        })
    }

    pub fn set_attr(obj: &PyHandle, name: &str, value: &Value) -> Result<(), String> {
        Python::with_gil(|py| {
            let value = to_py(py, value)?;
            obj.0.setattr(py, name, value).map_err(|e| e.to_string())
        })
    }

    pub fn get_item(obj: &PyHandle, key: &Value) -> Result<Value, String> {
        Python::with_gil(|py| {
            let key = to_py(py, key)?;
            let result = obj.0.call_method1(py, "__getitem__", (key,)).map_err(|e| format!("Python index error: {e}"))?;
            from_py(py, result)
        })
    }

    pub fn set_item(obj: &PyHandle, key: &Value, value: &Value) -> Result<(), String> {
        Python::with_gil(|py| {
            let key = to_py(py, key)?;
            let value = to_py(py, value)?;
            obj.0.call_method1(py, "__setitem__", (key, value)).map(|_| ()).map_err(|e| e.to_string())
        })
    }

    pub fn to_list(obj: &PyHandle) -> Result<Vec<Value>, String> {
        Python::with_gil(|py| {
            let items = obj.0.extract::<Vec<PyObject>>(py).map_err(|_| "Python object is not iterable".to_string())?;
            items.into_iter().map(|o| from_py(py, o)).collect()
        })
    }

    pub fn binary_op(l: &Value, r: &Value, op: &str) -> Result<Value, String> {
        let name = match op {
            "+" => "add",
            "-" => "sub",
            "*" => "mul",
            "/" => "truediv",
            "%" => "mod",
            "==" => "eq",
            "!=" => "ne",
            ">" => "gt",
            "<" => "lt",
            ">=" => "ge",
            "<=" => "le",
            _ => return Err(format!("operator `{op}` is not supported on Python objects")),
        };
        Python::with_gil(|py| {
            let (l, r) = (to_py(py, l)?, to_py(py, r)?);
            let func = PyModule::import(py, "operator").and_then(|m| m.getattr(name)).map_err(|e| e.to_string())?;
            let result = func.call1((l, r)).map_err(|e| e.to_string())?;
            from_py(py, result.into_py(py))
        })
    }

    pub fn http_get(url: &str) -> Result<Value, String> {
        Python::with_gil(|py| {
            let body: String = PyModule::import(py, "urllib.request")
                .and_then(|m| m.getattr("urlopen"))
                .and_then(|open| open.call1((url,)))
                .and_then(|resp| resp.call_method0("read"))
                .and_then(|bytes| bytes.call_method1("decode", ("utf-8",)))
                .and_then(|s| s.extract())
                .map_err(|e| format!("HTTP request failed: {e}"))?;
            Ok(Value::String(body))
        })
    }
}

#[cfg(not(feature = "python"))]
mod imp {
    use crate::interpreter::Value;

    #[derive(Debug, Clone)]
    pub enum PyHandle {}

    const DISABLED: &str = "Python interop is not enabled; rebuild with `cargo build --features python`";

    pub fn init() {}

    pub fn import(_module: &str) -> Result<Value, String> {
        Err(DISABLED.to_string())
    }

    pub fn call(obj: &PyHandle, _args: &[Value]) -> Result<Value, String> {
        match *obj {}
    }

    pub fn get_attr(obj: &PyHandle, _name: &str) -> Result<Value, String> {
        match *obj {}
    }

    pub fn set_attr(obj: &PyHandle, _name: &str, _value: &Value) -> Result<(), String> {
        match *obj {}
    }

    pub fn get_item(obj: &PyHandle, _key: &Value) -> Result<Value, String> {
        match *obj {}
    }

    pub fn set_item(obj: &PyHandle, _key: &Value, _value: &Value) -> Result<(), String> {
        match *obj {}
    }

    pub fn to_list(obj: &PyHandle) -> Result<Vec<Value>, String> {
        match *obj {}
    }

    pub fn binary_op(_l: &Value, _r: &Value, op: &str) -> Result<Value, String> {
        Err(format!("operator `{op}` needs Python interop; {DISABLED}"))
    }

    pub fn http_get(_url: &str) -> Result<Value, String> {
        Err(format!("`get` needs Python interop for now; {DISABLED}"))
    }
}

#[cfg(feature = "python")]
mod modern {
    use super::PyHandle;
    use crate::eval::value::{Dict, Key, Value};
    use pyo3::prelude::*;
    use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};

    fn to_py(py: Python, val: &Value) -> Result<PyObject, String> {
        match val {
            Value::None => Ok(py.None()),
            Value::Bool(b) => Ok(b.into_py(py)),
            Value::Int(i) => Ok(i.into_py(py)),
            Value::Float(f) => Ok(f.into_py(py)),
            Value::Str(s) => Ok(s.as_ref().into_py(py)),
            Value::List(list) => {
                let items = list.read().iter().map(|v| to_py(py, v)).collect::<Result<Vec<_>, _>>()?;
                Ok(items.into_py(py))
            }
            Value::Map(map) => {
                let dict = PyDict::new(py);
                for (k, v) in map.read().iter() {
                    dict.set_item(to_py(py, &k.to_value())?, to_py(py, v)?).map_err(|e| e.to_string())?;
                }
                Ok(dict.into_py(py))
            }
            Value::Py(obj) => Ok(obj.0.clone_ref(py)),
            other => Err(format!("a {} cannot be passed to Python", other.type_name())),
        }
    }

    fn from_py(py: Python, obj: PyObject) -> Result<Value, String> {
        if obj.is_none(py) {
            return Ok(Value::None);
        }
        if let Ok(b) = obj.extract::<bool>(py) {
            return Ok(Value::Bool(b));
        }
        if let Ok(i) = obj.extract::<i64>(py) {
            return Ok(Value::Int(i));
        }
        if let Ok(f) = obj.extract::<f64>(py) {
            return Ok(Value::Float(f));
        }
        if let Ok(s) = obj.extract::<String>(py) {
            return Ok(Value::Str(s.into()));
        }
        if obj.downcast::<PyBytes>(py).is_ok() {
            return Ok(Value::Py(PyHandle(obj)));
        }
        if let Ok(dict) = obj.downcast::<PyDict>(py) {
            let mut out = Dict::default();
            for (k, v) in dict {
                let key = Key::from_value(&from_py(py, k.into())?)?;
                out.insert(key, from_py(py, v.into())?);
            }
            return Ok(Value::map(out));
        }
        let seq: Option<Vec<PyObject>> = if let Ok(list) = obj.downcast::<PyList>(py) {
            Some(list.iter().map(Into::into).collect())
        } else if let Ok(tuple) = obj.downcast::<PyTuple>(py) {
            Some(tuple.iter().map(Into::into).collect())
        } else {
            None
        };
        if let Some(items) = seq {
            let values = items.into_iter().map(|o| from_py(py, o)).collect::<Result<Vec<_>, _>>()?;
            return Ok(Value::list(values));
        }
        Ok(Value::Py(PyHandle(obj)))
    }

    pub fn import_module(module: &str) -> Result<Value, String> {
        super::init();
        Python::with_gil(|py| {
            let m = PyModule::import(py, module).map_err(|e| format!("failed to import Python module `{module}`: {e}"))?;
            Ok(Value::Py(PyHandle(m.into_py(py))))
        })
    }

    pub fn call(obj: &PyHandle, args: &[Value]) -> Result<Value, String> {
        Python::with_gil(|py| {
            let args = args.iter().map(|a| to_py(py, a)).collect::<Result<Vec<_>, _>>()?;
            let result = obj.0.call1(py, PyTuple::new(py, args)).map_err(|e| format!("Python error: {e}"))?;
            from_py(py, result)
        })
    }

    pub fn get_attr(obj: &PyHandle, name: &str) -> Result<Value, String> {
        Python::with_gil(|py| match obj.0.getattr(py, name) {
            Ok(attr) => from_py(py, attr),
            Err(_) => match obj.0.call_method1(py, "__getitem__", (name,)) {
                Ok(item) => from_py(py, item),
                Err(_) => Err(format!("Python object has no attribute or key `{name}`")),
            },
        })
    }

    pub fn set_attr(obj: &PyHandle, name: &str, value: &Value) -> Result<(), String> {
        Python::with_gil(|py| {
            let value = to_py(py, value)?;
            obj.0.setattr(py, name, value).map_err(|e| e.to_string())
        })
    }

    pub fn get_item(obj: &PyHandle, key: &Value) -> Result<Value, String> {
        Python::with_gil(|py| {
            let key = to_py(py, key)?;
            let item = obj.0.call_method1(py, "__getitem__", (key,)).map_err(|e| format!("Python index error: {e}"))?;
            from_py(py, item)
        })
    }

    pub fn set_item(obj: &PyHandle, key: &Value, value: &Value) -> Result<(), String> {
        Python::with_gil(|py| {
            let (key, value) = (to_py(py, key)?, to_py(py, value)?);
            obj.0.call_method1(py, "__setitem__", (key, value)).map(|_| ()).map_err(|e| e.to_string())
        })
    }

    pub fn to_list(obj: &PyHandle) -> Result<Vec<Value>, String> {
        Python::with_gil(|py| {
            let items = obj.0.extract::<Vec<PyObject>>(py).map_err(|_| "Python object is not iterable".to_string())?;
            items.into_iter().map(|o| from_py(py, o)).collect()
        })
    }

    pub fn binary_op(l: &Value, r: &Value, op: &str) -> Result<Value, String> {
        let name = match op {
            "+" => "add",
            "-" => "sub",
            "*" => "mul",
            "/" => "truediv",
            "//" => "floordiv",
            "%" => "mod",
            "**" => "pow",
            "@" => "matmul",
            "==" => "eq",
            "!=" => "ne",
            ">" => "gt",
            "<" => "lt",
            ">=" => "ge",
            "<=" => "le",
            "in" => "contains",
            _ => return Err(format!("operator `{op}` is not supported on Python objects")),
        };
        Python::with_gil(|py| {
            let (a, b) = if op == "in" { (to_py(py, r)?, to_py(py, l)?) } else { (to_py(py, l)?, to_py(py, r)?) };
            let func = PyModule::import(py, "operator").and_then(|m| m.getattr(name)).map_err(|e| e.to_string())?;
            let result = func.call1((a, b)).map_err(|e| format!("Python error: {e}"))?;
            from_py(py, result.into_py(py))
        })
    }
}

#[cfg(not(feature = "python"))]
mod modern {
    use super::PyHandle;
    use crate::eval::value::Value;

    const DISABLED: &str = "Python interop is not enabled; rebuild with `cargo build --features python`";

    pub fn import_module(_module: &str) -> Result<Value, String> {
        Err(DISABLED.to_string())
    }

    pub fn call(obj: &PyHandle, _args: &[Value]) -> Result<Value, String> {
        match *obj {}
    }

    pub fn get_attr(obj: &PyHandle, _name: &str) -> Result<Value, String> {
        match *obj {}
    }

    pub fn set_attr(obj: &PyHandle, _name: &str, _value: &Value) -> Result<(), String> {
        match *obj {}
    }

    pub fn get_item(obj: &PyHandle, _key: &Value) -> Result<Value, String> {
        match *obj {}
    }

    pub fn set_item(obj: &PyHandle, _key: &Value, _value: &Value) -> Result<(), String> {
        match *obj {}
    }

    pub fn to_list(obj: &PyHandle) -> Result<Vec<Value>, String> {
        match *obj {}
    }

    pub fn binary_op(_l: &Value, _r: &Value, op: &str) -> Result<Value, String> {
        Err(format!("operator `{op}` on a Python object needs Python interop; {DISABLED}"))
    }
}
