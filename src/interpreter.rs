use crate::ast::*;
use std::collections::HashMap;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyTuple};

#[derive(Debug, Clone)]
pub enum Value {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Function {
        params: Vec<String>,
        body: Block,
    },
    NativeFunction {
        name: String,
    },
    PythonObject(PyObject),
    Null,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (Value::Null, Value::Null) => true,
            _ => false,
        }
    }
}

fn type_name(val: &Value) -> &'static str {
    match val {
        Value::Integer(_) => "a number",
        Value::Float(_) => "a number",
        Value::String(_) => "text",
        Value::Boolean(_) => "a boolean",
        Value::List(_) => "a list",
        Value::Map(_) => "a map",
        Value::Function { .. } | Value::NativeFunction { .. } => "a function",
        Value::PythonObject(_) => "a python object",
        Value::Null => "null",
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Integer(i) => write!(f, "{}", i),
            Value::Float(fl) => write!(f, "{}", fl),
            Value::String(s) => write!(f, "{}", s),
            Value::Boolean(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Value::List(l) => {
                let items: Vec<String> = l.iter().map(|v| format!("{}", v)).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Value::Map(m) => {
                let items: Vec<String> = m.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
            Value::Function { .. } | Value::NativeFunction { .. } => write!(f, "<function>"),
            Value::PythonObject(_) => write!(f, "<python_object>"),
            Value::Null => write!(f, "null"),
        }
    }
}

fn value_to_py(py: Python, val: &Value) -> Result<PyObject, String> {
    match val {
        Value::Integer(i) => Ok(i.into_py(py)),
        Value::Float(f) => Ok(f.into_py(py)),
        Value::String(s) => Ok(s.into_py(py)),
        Value::Boolean(b) => Ok(b.into_py(py)),
        Value::PythonObject(obj) => Ok(obj.clone_ref(py)),
        Value::Null => Ok(py.None()),
        _ => Err(format!("Cannot convert {} to Python", type_name(val))),
    }
}

fn py_to_value(py: Python, obj: PyObject) -> Result<Value, String> {
    // Attempt basic extraction
    if let Ok(i) = obj.extract::<i64>(py) {
        return Ok(Value::Integer(i));
    }
    if let Ok(f) = obj.extract::<f64>(py) {
        return Ok(Value::Float(f));
    }
    if let Ok(s) = obj.extract::<String>(py) {
        return Ok(Value::String(s));
    }
    if let Ok(b) = obj.extract::<bool>(py) {
        return Ok(Value::Boolean(b));
    }
    if obj.is_none(py) {
        return Ok(Value::Null);
    }
    
    // Otherwise, keep it as a Python object
    Ok(Value::PythonObject(obj))
}

#[derive(Clone)]
pub struct Environment {
    pub scopes: Vec<std::collections::HashMap<String, Value>>,
    pub context_item: Option<Value>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            scopes: vec![std::collections::HashMap::new()],
            context_item: None,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn define(&mut self, name: String, value: Value) {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(&name) {
                scope.insert(name, value);
                return;
            }
        }
        self.scopes.last_mut().unwrap().insert(name, value);
    }

    pub fn get(&self, name: &str) -> Result<Value, String> {
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.get(name) {
                return Ok(val.clone());
            }
        }
        if let Some(ref val) = self.context_item {
            return Ok(val.clone());
        }
        Err(format!("Unknown variable '{}'", name))
    }
}

pub enum ExecResult {
    Ok,
    Return(Value),
}

pub struct Interpreter {
    pub concurrent_mode: bool,
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter { concurrent_mode: false }
    }

    pub fn interpret(&mut self, program: &Program) -> Result<(), String> {
        // Prepare python env early to catch initialization errors
        pyo3::prepare_freethreaded_python();
        
        let mut global_env = Environment::new();
        
        global_env.define("str".to_string(), Value::NativeFunction { 
            name: "str".to_string() 
        });
        
        for stmt in &program.statements {
            if let ExecResult::Return(_) = self.execute(stmt, &mut global_env)? {
                return Err("Return statement outside function".to_string());
            }
        }
        Ok(())
    }

    fn execute(&mut self, stmt: &Statement, env: &mut Environment) -> Result<ExecResult, String> {
        match stmt {
            Statement::Assignment { name, value, .. } => {
                let val = self.evaluate(value, env)?;
                env.define(name.clone(), val);
            }
            Statement::Show { value } => {
                let val = self.evaluate(value, env)?;
                println!("{}", val);
            }
            Statement::If { condition, body, otherwise } => {
                let cond_val = self.evaluate(condition, env)?;
                let is_true = match cond_val {
                    Value::Boolean(b) => b,
                    _ => return Err(format!("Condition must evaluate to a boolean, got {:?}", cond_val)),
                };
                
                if is_true {
                    if let ExecResult::Return(v) = self.execute_block(body, env)? {
                        return Ok(ExecResult::Return(v));
                    }
                } else if let Some(other) = otherwise {
                    if let ExecResult::Return(v) = self.execute(other, env)? {
                        return Ok(ExecResult::Return(v));
                    }
                }
            }
            Statement::Repeat { count, body } => {
                let count_val = self.evaluate(count, env)?;
                let c = match count_val {
                    Value::Integer(i) => i,
                    _ => return Err(format!("Repeat count must be an integer, got {:?}", count_val)),
                };
                for _ in 0..c {
                    if let ExecResult::Return(v) = self.execute_block(body, env)? {
                        return Ok(ExecResult::Return(v));
                    }
                }
            }
            Statement::FunctionDeclaration { name, params, body, .. } => {
                let param_names = params.iter().map(|(n, _)| n.clone()).collect();
                let func_val = Value::Function {
                    params: param_names,
                    body: body.clone(),
                };
                env.define(name.clone(), func_val);
            }
            Statement::UsePython { module, alias } => {
                let py_module = Python::with_gil(|py| -> Result<PyObject, String> {
                    let m = PyModule::import(py, module.as_str())
                        .map_err(|e| format!("Failed to import python module {}: {}", module, e))?;
                    Ok(m.into_py(py))
                })?;
                let name = alias.as_ref().unwrap_or(module).clone();
                env.define(name, Value::PythonObject(py_module));
            }
            Statement::Return { value } => {
                let ret_val = match value {
                    Some(expr) => self.evaluate(expr, env)?,
                    None => Value::Null,
                };
                return Ok(ExecResult::Return(ret_val));
            }
        }
        Ok(ExecResult::Ok)
    }

    fn execute_block(&mut self, block: &Block, env: &mut Environment) -> Result<ExecResult, String> {
        env.push_scope();
        for stmt in &block.statements {
            if let ExecResult::Return(v) = self.execute(stmt, env)? {
                env.pop_scope();
                return Ok(ExecResult::Return(v));
            }
        }
        env.pop_scope();
        Ok(ExecResult::Ok)
    }

    fn evaluate(&mut self, expr: &Expression, env: &Environment) -> Result<Value, String> {
        match expr {
            Expression::Integer(i) => Ok(Value::Integer(*i)),
            Expression::Float(f) => Ok(Value::Float(*f)),
            Expression::String(s) => Ok(Value::String(s.clone())),
            Expression::Boolean(b) => Ok(Value::Boolean(*b)),
            Expression::Identifier(id) => env.get(id),
            Expression::Concurrent(inner) => {
                let prev = self.concurrent_mode;
                self.concurrent_mode = true;
                let result = self.evaluate(inner, env);
                self.concurrent_mode = prev;
                result
            }
            Expression::Cast(expr, target_type) => {
                let val = self.evaluate(expr, env)?;
                let execute_cast = |v: &Value, t: &str| -> Result<Value, String> {
                    match (v, t) {
                        (Value::String(s), "json") => {
                            Python::with_gil(|py| -> Result<Value, String> {
                                let json = PyModule::import(py, "json").map_err(|e| e.to_string())?;
                                let loads = json.getattr("loads").map_err(|e| e.to_string())?;
                                let py_result = loads.call1((s,)).map_err(|e| e.to_string())?;
                                py_to_value(py, py_result.into_py(py))
                            })
                        }
                        (Value::String(s), "int") => {
                            s.parse::<i64>().map(Value::Integer).map_err(|_| "Failed to cast to int".to_string())
                        }
                        _ => Err(format!("Cannot cast to {}", t)),
                    }
                };
                match val {
                    Value::List(items) => {
                        let mut results = Vec::new();
                        for item in items {
                            results.push(execute_cast(&item, target_type)?);
                        }
                        Ok(Value::List(results))
                    }
                    _ => execute_cast(&val, target_type),
                }
            }
            Expression::List(items) => {
                if self.concurrent_mode {
                    let mut handles = Vec::new();
                    for item in items {
                        let env_clone = env.clone();
                        let item_clone = item.clone();
                        let is_concurrent = self.concurrent_mode;
                        handles.push(std::thread::spawn(move || {
                            let mut interp = Interpreter { concurrent_mode: is_concurrent };
                            interp.evaluate(&item_clone, &env_clone)
                        }));
                    }
                    
                    let mut list = Vec::new();
                    for handle in handles {
                        match handle.join() {
                            Ok(Ok(val)) => list.push(val),
                            Ok(Err(e)) => return Err(e),
                            Err(_) => return Err("Thread panicked during parallel execution".to_string()),
                        }
                    }
                    Ok(Value::List(list))
                } else {
                    let mut list = Vec::new();
                    for item in items {
                        list.push(self.evaluate(item, env)?);
                    }
                    Ok(Value::List(list))
                }
            }
            Expression::Read(path_expr) => {
                let path_val = self.evaluate(path_expr, env)?;
                let execute_read = |path: &str| -> Result<Value, String> {
                    let content = std::fs::read_to_string(path)
                        .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
                    Ok(Value::String(content))
                };
                
                match path_val {
                    Value::String(path) => execute_read(&path),
                    Value::List(paths) => {
                        if self.concurrent_mode {
                            let mut handles = Vec::new();
                            for path_item in paths {
                                if let Value::String(path) = path_item {
                                    handles.push(std::thread::spawn(move || execute_read(&path)));
                                } else {
                                    return Err("Expected list of strings for read".to_string());
                                }
                            }
                            let mut results = Vec::new();
                            for handle in handles {
                                match handle.join() {
                                    Ok(Ok(val)) => results.push(val),
                                    Ok(Err(e)) => return Err(e),
                                    Err(_) => return Err("Thread panicked".to_string()),
                                }
                            }
                            Ok(Value::List(results))
                        } else {
                            let mut results = Vec::new();
                            for path_item in paths {
                                if let Value::String(path) = path_item {
                                    results.push(execute_read(&path)?);
                                } else {
                                    return Err("Expected list of strings for read".to_string());
                                }
                            }
                            Ok(Value::List(results))
                        }
                    }
                    _ => Err("Expected string or list of strings for read".to_string()),
                }
            }
            Expression::Get(url_expr) => {
                let url_val = self.evaluate(url_expr, env)?;
                let execute_get = |url: &str| -> Result<Value, String> {
                    Python::with_gil(|py| -> Result<Value, String> {
                        let urllib = PyModule::import(py, "urllib.request").map_err(|e| format!("Failed to import urllib: {}", e))?;
                        let urlopen = urllib.getattr("urlopen").map_err(|e| e.to_string())?;
                        let py_result = urlopen.call1((url,)).map_err(|e| format!("HTTP request failed: {}", e))?;
                        let read_func = py_result.getattr("read").map_err(|e| e.to_string())?;
                        let bytes = read_func.call0().map_err(|e| e.to_string())?;
                        let decode_func = bytes.getattr("decode").map_err(|e| e.to_string())?;
                        let decoded = decode_func.call1(("utf-8",)).map_err(|e| e.to_string())?;
                        py_to_value(py, decoded.into_py(py))
                    })
                };
                
                match url_val {
                    Value::String(url) => execute_get(&url),
                    Value::List(urls) => {
                        if self.concurrent_mode {
                            let mut handles = Vec::new();
                            for url_item in urls {
                                if let Value::String(url) = url_item {
                                    handles.push(std::thread::spawn(move || execute_get(&url)));
                                } else {
                                    return Err("Expected list of strings for get".to_string());
                                }
                            }
                            let mut results = Vec::new();
                            for handle in handles {
                                match handle.join() {
                                    Ok(Ok(val)) => results.push(val),
                                    Ok(Err(e)) => return Err(e),
                                    Err(_) => return Err("Thread panicked".to_string()),
                                }
                            }
                            Ok(Value::List(results))
                        } else {
                            let mut results = Vec::new();
                            for url_item in urls {
                                if let Value::String(url) = url_item {
                                    results.push(execute_get(&url)?);
                                } else {
                                    return Err("Expected list of strings for get".to_string());
                                }
                            }
                            Ok(Value::List(results))
                        }
                    }
                    _ => Err("Expected string or list of strings for get".to_string()),
                }
            }
            Expression::Map(items) => {
                let mut map = HashMap::new();
                for (key, expr) in items {
                    let val = self.evaluate(expr, env)?;
                    map.insert(key.clone(), val);
                }
                Ok(Value::Map(map))
            }
            Expression::PropertyAccess { target, property } => {
                let obj = self.evaluate(target, env)?;
                match obj {
                    Value::Map(map) => {
                        if let Some(val) = map.get(property) {
                            Ok(val.clone())
                        } else {
                            Err(format!("Property '{}' not found", property))
                        }
                    }
                    Value::PythonObject(py_obj) => {
                        Python::with_gil(|py| {
                            if let Ok(attr) = py_obj.getattr(py, property.as_str()) {
                                py_to_value(py, attr)
                            } else if let Ok(item) = py_obj.call_method1(py, "__getitem__", (property.as_str(),)) {
                                py_to_value(py, item)
                            } else {
                                Err(format!("Python object has no attribute or key '{}'", property))
                            }
                        })
                    }
                    _ => Err("Property access only allowed on objects/maps or Python objects".to_string()),
                }
            }
            Expression::IndexAccess { target, index } => {
                let list_val = self.evaluate(target, env)?;
                let idx_val = self.evaluate(index, env)?;
                match list_val {
                    Value::List(items) => {
                        if let Value::Integer(i) = idx_val {
                            if i >= 0 && (i as usize) < items.len() {
                                Ok(items[i as usize].clone())
                            } else {
                                Err(format!("Index {} out of bounds", i))
                            }
                        } else {
                            Err("List index must be an integer".to_string())
                        }
                    }
                    Value::PythonObject(py_obj) => {
                        Python::with_gil(|py| -> Result<Value, String> {
                            let py_idx = value_to_py(py, &idx_val)?;
                            let result = py_obj.call_method1(py, "__getitem__", (py_idx,))
                                .map_err(|e| format!("Python index error: {}", e))?;
                            py_to_value(py, result)
                        })
                    }
                    _ => Err("Can only index into a list or Python object".to_string())
                }
            }
            Expression::Where { target, condition } => {
                let list_val = self.evaluate(target, env)?;
                
                let items = match list_val {
                    Value::List(l) => l,
                    Value::PythonObject(py_obj) => {
                        Python::with_gil(|py| -> Result<Vec<Value>, String> {
                            if let Ok(list) = py_obj.extract::<Vec<PyObject>>(py) {
                                let mut res = Vec::new();
                                for item in list {
                                    res.push(py_to_value(py, item)?);
                                }
                                Ok(res)
                            } else {
                                Err("'where' can only be used on an iterable Python object".to_string())
                            }
                        })?
                    }
                    _ => return Err("'where' can only be used on a list or iterable".to_string())
                };
                
                let mut result = Vec::new();
                for item in items {
                    let mut local_env = env.clone();
                    local_env.context_item = Some(item.clone());
                    let cond_val = self.evaluate(condition, &local_env)?;
                    if let Value::Boolean(true) = cond_val {
                        result.push(item);
                    }
                }
                Ok(Value::List(result))
            }
            Expression::Select { target, mapping } => {
                let list_val = self.evaluate(target, env)?;
                
                let items = match list_val {
                    Value::List(l) => l,
                    Value::PythonObject(py_obj) => {
                        Python::with_gil(|py| -> Result<Vec<Value>, String> {
                            if let Ok(list) = py_obj.extract::<Vec<PyObject>>(py) {
                                let mut res = Vec::new();
                                for item in list {
                                    res.push(py_to_value(py, item)?);
                                }
                                Ok(res)
                            } else {
                                Err("'select' can only be used on an iterable Python object".to_string())
                            }
                        })?
                    }
                    _ => return Err("'select' can only be used on a list or iterable".to_string())
                };
                
                let mut result = Vec::new();
                for item in items {
                    let mut local_env = env.clone();
                    local_env.context_item = Some(item.clone());
                    let map_val = self.evaluate(mapping, &local_env)?;
                    result.push(map_val);
                }
                Ok(Value::List(result))
            }
            Expression::Call { callee, arguments } => {
                let func = self.evaluate(callee, env)?;
                
                // Handle Python objects being called
                if let Value::PythonObject(py_obj) = func {
                    return Python::with_gil(|py| -> Result<Value, String> {
                        let mut py_args = Vec::new();
                        for arg in arguments {
                            let evaluated = self.evaluate(arg, env)?;
                            py_args.push(value_to_py(py, &evaluated)?);
                        }
                        
                        let args_tuple = PyTuple::new(py, py_args);
                        
                        let result = py_obj.call1(py, args_tuple)
                            .map_err(|e| format!("Python error during call: {}", e))?;
                            
                        py_to_value(py, result)
                    });
                }
                
                // Handle native functions
                if let Value::NativeFunction { name } = &func {
                    if name == "str" {
                        if arguments.len() != 1 {
                            return Err("str() takes exactly 1 argument".to_string());
                        }
                        let arg_val = self.evaluate(&arguments[0], env)?;
                        return Ok(Value::String(format!("{}", arg_val)));
                    }
                }
                
                let (params, body) = match func {
                    Value::Function { params, body } => (params, body),
                    _ => return Err(format!("Can only call functions, got {:?}", func)),
                };
                
                if arguments.len() != params.len() {
                    return Err(format!("Expected {} arguments, got {}", params.len(), arguments.len()));
                }
                
                let mut eval_args = Vec::new();
                for arg in arguments {
                    eval_args.push(self.evaluate(arg, env)?);
                }
                
                // Using a closure environment
                let mut func_env = env.clone();
                func_env.push_scope(); // Create new scope for function parameters
                for (param_name, arg_val) in params.into_iter().zip(eval_args) {
                    func_env.define(param_name, arg_val);
                }
                
                let res = match self.execute_block(&body, &mut func_env)? {
                    ExecResult::Return(v) => Ok(v),
                    ExecResult::Ok => Ok(Value::Null),
                };
                func_env.pop_scope();
                res
            }
            Expression::BinaryOp { left, operator, right } => {
                let l = self.evaluate(left, env)?;
                let r = self.evaluate(right, env)?;
                
                match operator.as_str() {
                    "+" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                            (left_val, right_val) => Err(format!(
                                "Cannot add {} to {}.\n\nTry converting one of them first using str().",
                                type_name(&right_val), type_name(&left_val)
                            )),
                        }
                    }
                    "-" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a - b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                            _ => Err("Cannot subtract these types".to_string()),
                        }
                    }
                    "*" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a * b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                            _ => Err("Cannot multiply these types".to_string()),
                        }
                    }
                    "%" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a % b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
                            _ => Err("Cannot modulo these types".to_string()),
                        }
                    }
                    "and" => {
                        let l_bool = match l { Value::Boolean(b) => b, _ => return Err("Expected boolean for 'and'".to_string()) };
                        let r_bool = match r { Value::Boolean(b) => b, _ => return Err("Expected boolean for 'and'".to_string()) };
                        Ok(Value::Boolean(l_bool && r_bool))
                    }
                    "or" => {
                        let l_bool = match l { Value::Boolean(b) => b, _ => return Err("Expected boolean for 'or'".to_string()) };
                        let r_bool = match r { Value::Boolean(b) => b, _ => return Err("Expected boolean for 'or'".to_string()) };
                        Ok(Value::Boolean(l_bool || r_bool))
                    }
                    ">" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Boolean(a > b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Boolean(a > b)),
                            _ => Err("Cannot compare these types".to_string()),
                        }
                    }
                    "<" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Boolean(a < b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Boolean(a < b)),
                            _ => Err("Cannot compare these types".to_string()),
                        }
                    }
                    "<=" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Boolean(a <= b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Boolean(a <= b)),
                            _ => Err("Cannot compare these types".to_string()),
                        }
                    }
                    ">=" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Boolean(a >= b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Boolean(a >= b)),
                            _ => Err("Cannot compare these types".to_string()),
                        }
                    }
                    "==" => Ok(Value::Boolean(l == r)),
                    "!=" => Ok(Value::Boolean(l != r)),
                    _ => Err(format!("Unknown operator {}", operator)),
                }
            }
        }
    }
}
