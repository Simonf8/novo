use crate::ast::*;
use std::collections::HashMap;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

#[derive(Debug, Clone)]
pub enum Value {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    List(std::sync::Arc<parking_lot::RwLock<Vec<Value>>>),
    Map(std::sync::Arc<parking_lot::RwLock<HashMap<String, Value>>>),
    Function {
        params: Vec<String>,
        body: Block,
    },
    CompiledFunction {
        chunk: std::sync::Arc<crate::vm::chunk::Chunk>,
        arity: usize,
    },
    NativeFunction {
        name: String,
        bound_target: Option<Box<Value>>,
    },
    PythonObject(PyObject),
    Class {
        name: String,
        methods: HashMap<String, Value>,
    },
    Instance {
        class_name: String,
        properties: std::sync::Arc<parking_lot::RwLock<HashMap<String, Value>>>,
    },
    Null,
    Promise(std::sync::Arc<parking_lot::RwLock<Option<Result<Value, String>>>>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::List(a), Value::List(b)) => std::sync::Arc::ptr_eq(a, b),
            (Value::Map(a), Value::Map(b)) => std::sync::Arc::ptr_eq(a, b),
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
        Value::CompiledFunction { .. } => "function",
        Value::Class { .. } => "class",
        Value::Instance { .. } => "an instance",
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
                let items: Vec<String> = l.read().iter().map(|v| format!("{}", v)).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Value::Map(m) => {
                let items: Vec<String> = m.read().iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
            Value::Function { .. } | Value::NativeFunction { .. } => write!(f, "<function>"),
            Value::PythonObject(_) => write!(f, "<python_object>"),
            Value::Null => write!(f, "null"),
            Value::CompiledFunction { .. } => write!(f, "<compiled function>"),
            Value::Class { name, .. } => write!(f, "<class {}>", name),
            Value::Instance { class_name, .. } => write!(f, "<{} instance>", class_name),
        }
    }
}

pub fn value_to_py(py: Python, val: &Value) -> Result<PyObject, String> {
    match val {
        Value::Integer(i) => Ok(i.into_py(py)),
        Value::Float(f) => Ok(f.into_py(py)),
        Value::String(s) => Ok(s.into_py(py)),
        Value::Boolean(b) => Ok(b.into_py(py)),
        Value::List(list) => {
            let mut py_list = Vec::new();
            for item in list.read().iter() {
                py_list.push(value_to_py(py, item)?);
            }
            Ok(py_list.into_py(py))
        }
        Value::PythonObject(obj) => Ok(obj.clone_ref(py)),
        Value::Null => Ok(py.None()),
        _ => Err(format!("Cannot convert {} to Python", type_name(val))),
    }
}

pub fn py_to_value(py: Python, obj: PyObject) -> Result<Value, String> {
    // Attempt basic extraction
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
    if obj.is_none(py) {
        return Ok(Value::Null);
    }
    
    if obj.downcast::<pyo3::types::PyBytes>(py).is_ok() {
        return Ok(Value::PythonObject(obj));
    }
    
    if let Ok(dict) = obj.downcast::<pyo3::types::PyDict>(py) {
        let mut map = std::collections::HashMap::new();
        for (k, v) in dict {
            if let Ok(key_str) = k.extract::<String>() {
                map.insert(key_str, py_to_value(py, v.into())?);
            }
        }
        return Ok(Value::Map(std::sync::Arc::new(parking_lot::RwLock::new(map))));
    }
    
    if let Ok(list) = obj.downcast::<pyo3::types::PyList>(py) {
        let mut vec = Vec::new();
        for item in list {
            vec.push(py_to_value(py, item.into())?);
        }
        return Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(vec))));
    }

    if let Ok(tuple) = obj.downcast::<pyo3::types::PyTuple>(py) {
        let mut vec = Vec::new();
        for item in tuple {
            vec.push(py_to_value(py, item.into())?);
        }
        return Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(vec))));
    }
    
    // Otherwise, keep it as a Python object
    Ok(Value::PythonObject(obj))
}

pub fn python_operator_fallback(l: &Value, r: &Value, op: &str) -> Result<Value, String> {
    Python::with_gil(|py| -> Result<Value, String> {
        let py_l = value_to_py(py, l)?;
        let py_r = value_to_py(py, r)?;
        let op_module = PyModule::import(py, "operator").map_err(|e| e.to_string())?;
        
        let py_func_name = match op {
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
            _ => return Err(format!("Unsupported operator for Python fallback: {}", op))
        };
        
        let func = op_module.getattr(py_func_name).map_err(|e| e.to_string())?;
        let py_result = func.call1((py_l, py_r)).map_err(|e| e.to_string())?;
        py_to_value(py, py_result.into_py(py))
    })
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
    Break,
    Continue,
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
            name: "str".to_string(),
            bound_target: None,
        });
        
        for stmt in &program.statements {
            if let ExecResult::Return(_) = self.execute(stmt, &mut global_env)? {
                return Err("Return statement outside function".to_string());
            }
        }
        Ok(())
    }

    fn execute(&mut self, stmt: &Statement, env: &mut Environment) -> Result<ExecResult, String> {
        self.execute_inner(stmt, env).map_err(|e| {
            if e.contains(" at line ") {
                e
            } else {
                format!("{} at line {}, col {}, len {}", e, stmt.span.line, stmt.span.column, stmt.span.length)
            }
        })
    }

    fn execute_inner(&mut self, stmt: &Statement, env: &mut Environment) -> Result<ExecResult, String> {
        match &stmt.node {
            StatementNode::Assignment { name, value, .. } => {
                let val = self.evaluate(value, env)?;
                env.define(name.clone(), val);
            }
            StatementNode::IndexAssignment { .. } | StatementNode::PropertyAssignment { .. } => {
                return Err("Index and Property assignment are only supported in VM mode.".to_string());
            }
            StatementNode::Show { value } => {
                let val = self.evaluate(value, env)?;
                println!("{}", val);
            }
            StatementNode::If { condition, body, otherwise } => {
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
            StatementNode::Repeat { count, body } => {
                let count_val = self.evaluate(count, env)?;
                let c = match count_val {
                    Value::Integer(i) => i,
                    _ => return Err(format!("Repeat count must be an integer, got {:?}", count_val)),
                };
                for i in 0..c {
                    env.push_scope();
                    env.define("it".to_string(), Value::Integer(i));
                    let res = self.execute_block_no_scope(body, env)?;
                    env.pop_scope();
                    match res {
                        ExecResult::Return(v) => return Ok(ExecResult::Return(v)),
                        ExecResult::Break => break,
                        ExecResult::Continue => continue,
                        ExecResult::Ok => {}
                    }
                }
            }
            StatementNode::While { condition, body } => {
                loop {
                    let cond_val = self.evaluate(condition, env)?;
                    let is_true = match cond_val {
                        Value::Boolean(b) => b,
                        _ => return Err(format!("While condition must evaluate to a boolean, got {:?}", cond_val)),
                    };
                    if !is_true { break; }
                    
                    env.push_scope();
                    let res = self.execute_block_no_scope(body, env)?;
                    env.pop_scope();
                    
                    match res {
                        ExecResult::Return(v) => return Ok(ExecResult::Return(v)),
                        ExecResult::Break => break,
                        ExecResult::Continue => continue,
                        ExecResult::Ok => {}
                    }
                }
            }
            StatementNode::Break => return Ok(ExecResult::Break),
            StatementNode::Continue => return Ok(ExecResult::Continue),
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                env.push_scope();
                let res = self.execute_block_no_scope(try_block, env);
                env.pop_scope();
                match res {
                    Ok(r) => match r {
                        ExecResult::Return(v) => return Ok(ExecResult::Return(v)),
                        ExecResult::Break => return Ok(ExecResult::Break),
                        ExecResult::Continue => return Ok(ExecResult::Continue),
                        ExecResult::Ok => {}
                    },
                    Err(e) => {
                        env.push_scope();
                        env.define(catch_var.clone(), Value::String(e));
                        let catch_res = self.execute_block_no_scope(catch_block, env)?;
                        env.pop_scope();
                        match catch_res {
                            ExecResult::Return(v) => return Ok(ExecResult::Return(v)),
                            ExecResult::Break => return Ok(ExecResult::Break),
                            ExecResult::Continue => return Ok(ExecResult::Continue),
                            ExecResult::Ok => {}
                        }
                    }
                }
            }
            StatementNode::ClassDef { name, methods } => {
                // A very simple class implementation: we define a function that returns a Map
                // containing closures for methods, binding `this` manually (or just capturing env).
                // Actually, let's just make it return an empty map for now or a map of methods.
                // We'll leave it as a dummy class or construct it as a function that returns a Map.
                let mut method_map = std::collections::HashMap::new();
                for m in methods {
                    if let StatementNode::FunctionDeclaration { name: method_name, params, body, .. } = &m.node {
                        let param_names = params.iter().map(|(n, _)| n.clone()).collect();
                        method_map.insert(method_name.clone(), Value::Function {
                            params: param_names,
                            body: body.clone(),
                        });
                    }
                }
                // Construct a constructor function
                let constructor = Value::Function {
                    params: vec![],
                    body: Block { statements: vec![
                        StatementNode::Return { value: Some(ExpressionNode::Map(vec![]).into()) }.into()
                    ]} // Ideally we'd return method_map, but returning empty map for basic class
                };
                env.define(name.clone(), constructor);
            }
            StatementNode::FunctionDeclaration { name, params, body, .. } => {
                let param_names = params.iter().map(|(n, _)| n.clone()).collect();
                let func_val = Value::Function {
                    params: param_names,
                    body: body.clone(),
                };
                env.define(name.clone(), func_val);
            }
            StatementNode::UsePython { module, alias } => {
                let py_module = Python::with_gil(|py| -> Result<PyObject, String> {
                    let m = PyModule::import(py, module.as_str())
                        .map_err(|e| format!("Failed to import python module {}: {}", module, e))?;
                    Ok(m.into_py(py))
                })?;
                let name = alias.as_ref().unwrap_or(module).clone();
                env.define(name, Value::PythonObject(py_module));
            }
            StatementNode::UseModule { path, alias } => {
                let code = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read module '{}': {}", path, e))?;
                
                let mut lexer = crate::lexer::Lexer::new(&code);
                let tokens = lexer.tokenize()?;
                let mut parser = crate::parser::Parser::new(tokens);
                let program = parser.parse()?;
                
                let mut module_env = Environment::new();
                for stmt in &program.statements {
                    self.execute(stmt, &mut module_env)?;
                }
                
                let module_map = module_env.scopes.last().unwrap().clone();
                // If path is "math.nv", alias or path is used. If path has ".nv", strip it?
                // Let's just use alias, or the path string itself. 
                // We'll strip ".nv" or path segments for default name.
                let default_name = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&path)
                    .to_string();
                
                let name = alias.as_ref().unwrap_or(&default_name).clone();
                env.define(name, Value::Map(std::sync::Arc::new(parking_lot::RwLock::new(module_map))));
            }
            StatementNode::Return { value } => {
                let ret_val = match value {
                    Some(expr) => self.evaluate(expr, env)?,
                    None => Value::Null,
                };
                return Ok(ExecResult::Return(ret_val));
            }
            StatementNode::Save { data, destination } => {
                let data_val = self.evaluate(data, env)?;
                let dest_val = self.evaluate(destination, env)?;
                
                let dest_str = match dest_val {
                    Value::String(s) => s,
                    _ => return Err(format!("Destination for save must be a string, got {:?}", dest_val)),
                };
                
                let content = match data_val {
                    Value::String(s) => s,
                    Value::List(l) => {
                        let mut result = String::new();
                        for (i, val) in l.read().iter().enumerate() {
                            if i > 0 {
                                result.push('\n');
                            }
                            result.push_str(&val.to_string());
                        }
                        result
                    },
                    _ => data_val.to_string(),
                };
                
                std::fs::write(&dest_str, content).map_err(|e| format!("Failed to write to {}: {}", dest_str, e))?;
            }
        }
        Ok(ExecResult::Ok)
    }

    fn execute_block(&mut self, block: &Block, env: &mut Environment) -> Result<ExecResult, String> {
        env.push_scope();
        let res = self.execute_block_no_scope(block, env);
        env.pop_scope();
        res
    }

    fn execute_block_no_scope(&mut self, block: &Block, env: &mut Environment) -> Result<ExecResult, String> {
        for stmt in &block.statements {
            let res = self.execute(stmt, env)?;
            match res {
                ExecResult::Return(v) => return Ok(ExecResult::Return(v)),
                ExecResult::Break => return Ok(ExecResult::Break),
                ExecResult::Continue => return Ok(ExecResult::Continue),
                ExecResult::Ok => {}
            }
        }
        Ok(ExecResult::Ok)
    }

    fn evaluate(&mut self, expr: &Expression, env: &Environment) -> Result<Value, String> {
        self.evaluate_inner(expr, env).map_err(|e| {
            if e.contains(" at line ") {
                e
            } else {
                format!("{} at line {}, col {}, len {}", e, expr.span.line, expr.span.column, expr.span.length)
            }
        })
    }

    fn evaluate_inner(&mut self, expr: &Expression, env: &Environment) -> Result<Value, String> {
        match &expr.node {
            ExpressionNode::Integer(i) => Ok(Value::Integer(*i)),
            ExpressionNode::Float(f) => Ok(Value::Float(*f)),
            ExpressionNode::String(s) => Ok(Value::String(s.clone())),
            ExpressionNode::Boolean(b) => Ok(Value::Boolean(*b)),
            ExpressionNode::Identifier(id) => env.get(id),
            ExpressionNode::Concurrent(inner) => {
                let prev = self.concurrent_mode;
                self.concurrent_mode = true;
                let result = self.evaluate(inner, env);
                self.concurrent_mode = prev;
                result
            }
            ExpressionNode::Cast(expr, target_type) => {
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
                        (Value::String(s), "csv") => {
                            Python::with_gil(|py| -> Result<Value, String> {
                                let csv = PyModule::import(py, "csv").map_err(|e| e.to_string())?;
                                let io = PyModule::import(py, "io").map_err(|e| e.to_string())?;
                                let string_io = io.getattr("StringIO").map_err(|e| e.to_string())?.call1((s,)).map_err(|e| e.to_string())?;
                                let dict_reader = csv.getattr("DictReader").map_err(|e| e.to_string())?.call1((string_io,)).map_err(|e| e.to_string())?;
                                let list_func = py.eval("list", None, None).map_err(|e| e.to_string())?;
                                let py_result = list_func.call1((dict_reader,)).map_err(|e| e.to_string())?;
                                py_to_value(py, py_result.into_py(py))
                            })
                        }
                        (Value::String(s), "int") => {
                            s.parse::<i64>().map(Value::Integer).map_err(|_| "Failed to cast to int".to_string())
                        }
                        (Value::String(s), "float") => {
                            s.parse::<f64>().map(Value::Float).map_err(|_| "Failed to cast to float".to_string())
                        }
                        _ => Err(format!("Cannot cast to {}", t)),
                    }
                };
                match val {
                    Value::List(items) => {
                        let mut results = Vec::new();
                        for item in items.read().iter() {
                            results.push(execute_cast(&item, target_type)?);
                        }
                        Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))))
                    }
                    _ => execute_cast(&val, target_type),
                }
            }
            ExpressionNode::List(items) => {
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
                    Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(list))))
                } else {
                    let mut list = Vec::new();
                    for item in items {
                        list.push(self.evaluate(item, env)?);
                    }
                    Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(list))))
                }
            }
            ExpressionNode::Read(path_expr) => {
                let path_val = self.evaluate(path_expr, env)?;
                let execute_read = |path: &str| -> Result<Value, String> {
                    let content = std::fs::read_to_string(path)
                        .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
                    Ok(Value::String(content))
                };
                
                match path_val {
                    Value::String(path) => execute_read(&path),
                    Value::List(paths_arc) => {
                        let paths = paths_arc.read().clone();
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
                            Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))))
                        } else {
                            let mut results = Vec::new();
                            for path_item in paths {
                                if let Value::String(path) = path_item {
                                    results.push(execute_read(&path)?);
                                } else {
                                    return Err("Expected list of strings for read".to_string());
                                }
                            }
                            Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))))
                        }
                    }
                    _ => Err("Expected string or list of strings for read".to_string()),
                }
            }
            ExpressionNode::Get(url_expr) => {
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
                    Value::List(urls_arc) => {
                        let urls = urls_arc.read().clone();
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
                            Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))))
                        } else {
                            let mut results = Vec::new();
                            for url_item in urls {
                                if let Value::String(url) = url_item {
                                    results.push(execute_get(&url)?);
                                } else {
                                    return Err("Expected list of strings for get".to_string());
                                }
                            }
                            Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))))
                        }
                    }
                    _ => Err("Expected string or list of strings for get".to_string()),
                }
            }
            ExpressionNode::Map(items) => {
                let mut map = HashMap::new();
                for (key, expr) in items {
                    let val = self.evaluate(expr, env)?;
                    map.insert(key.clone(), val);
                }
                Ok(Value::Map(std::sync::Arc::new(parking_lot::RwLock::new(map))))
            }
            ExpressionNode::PropertyAccess { target, property } => {
                let obj = self.evaluate(target, env)?;
                match obj {
                    Value::Map(map_arc) => {
                        let map = map_arc.read();
                        if let Some(val) = map.get(property) {
                            Ok(val.clone())
                        } else {
                            Err(format!("Property '{}' not found", property))
                        }
                    }
                    Value::String(s) => {
                        if property == "split" || property == "contains" {
                            Ok(Value::NativeFunction {
                                name: property.clone(),
                                bound_target: Some(Box::new(Value::String(s.clone())))
                            })
                        } else {
                            Err(format!("String has no property '{}'", property))
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
                    _ => Err("Property access only allowed on objects/maps, strings, or Python objects".to_string()),
                }
            }
            ExpressionNode::IndexAccess { target, index } => {
                let list_val = self.evaluate(target, env)?;
                let idx_val = self.evaluate(index, env)?;
                match list_val {
                    Value::List(items_arc) => {
                        let items = items_arc.read();
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
            ExpressionNode::Where { target, condition } => {
                let list_val = self.evaluate(target, env)?;
                
                let items = match list_val {
                    Value::List(l) => l.read().clone(),
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
                let mut local_env = env.clone();
                for item in items {
                    local_env.context_item = Some(item.clone());
                    let cond_val = self.evaluate(condition, &local_env)?;
                    if let Value::Boolean(true) = cond_val {
                        result.push(item);
                    }
                }
                Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(result))))
            }
            ExpressionNode::Select { target, mapping } => {
                let list_val = self.evaluate(target, env)?;
                
                let items = match list_val {
                    Value::List(l) => l.read().clone(),
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
                let mut local_env = env.clone();
                for item in items {
                    local_env.context_item = Some(item.clone());
                    let map_val = self.evaluate(mapping, &local_env)?;
                    result.push(map_val);
                }
                Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(result))))
            }
            ExpressionNode::Aggregate { target, mapping, op } => {
                let list_val = self.evaluate(target, env)?;
                let items = match list_val {
                    Value::List(l) => l.read().clone(),
                    Value::PythonObject(py_obj) => {
                        Python::with_gil(|py| -> Result<Vec<Value>, String> {
                            if let Ok(list) = py_obj.extract::<Vec<PyObject>>(py) {
                                let mut res = Vec::new();
                                for item in list {
                                    res.push(py_to_value(py, item)?);
                                }
                                Ok(res)
                            } else {
                                Err(format!("'{}' can only be used on an iterable Python object", op))
                            }
                        })?
                    }
                    _ => return Err(format!("'{}' can only be used on a list", op))
                };
                
                if items.is_empty() { return Ok(Value::Null); }

                let mut mapped = Vec::new();
                let mut local_env = env.clone();
                for item in items {
                    local_env.context_item = Some(item.clone());
                    mapped.push(self.evaluate(mapping, &local_env)?);
                }

                if op == "sum" || op == "average" {
                    let mut running_int = 0;
                    let mut running_float = 0.0;
                    let mut is_float = false;
                    for val in &mapped {
                        match val {
                            Value::Integer(i) => if is_float { running_float += *i as f64; } else { running_int += *i; },
                            Value::Float(f) => {
                                if !is_float { is_float = true; running_float = running_int as f64; }
                                running_float += *f;
                            },
                            _ => return Err(format!("'{}' requires numeric values", op))
                        }
                    }
                    if op == "sum" {
                        if is_float { Ok(Value::Float(running_float)) } else { Ok(Value::Integer(running_int)) }
                    } else {
                        let count = mapped.len() as f64;
                        if is_float { Ok(Value::Float(running_float / count)) } else { Ok(Value::Float(running_int as f64 / count)) }
                    }
                } else if op == "max" || op == "min" {
                    let mut current_max_int = None;
                    let mut current_min_int = None;
                    let mut current_max_float = None;
                    let mut current_min_float = None;
                    let mut is_float = false;

                    for val in &mapped {
                        match val {
                            Value::Integer(i) => {
                                let f = *i as f64;
                                if !is_float {
                                    if current_max_int.map_or(true, |m| *i > m) { current_max_int = Some(*i); }
                                    if current_min_int.map_or(true, |m| *i < m) { current_min_int = Some(*i); }
                                }
                                if current_max_float.map_or(true, |m| f > m) { current_max_float = Some(f); }
                                if current_min_float.map_or(true, |m| f < m) { current_min_float = Some(f); }
                            }
                            Value::Float(f) => {
                                is_float = true;
                                if current_max_float.map_or(true, |m| *f > m) { current_max_float = Some(*f); }
                                if current_min_float.map_or(true, |m| *f < m) { current_min_float = Some(*f); }
                            }
                            _ => return Err(format!("'{}' requires numeric values", op))
                        }
                    }
                    if op == "max" {
                        if is_float { Ok(Value::Float(current_max_float.unwrap())) } else { Ok(Value::Integer(current_max_int.unwrap())) }
                    } else {
                        if is_float { Ok(Value::Float(current_min_float.unwrap())) } else { Ok(Value::Integer(current_min_int.unwrap())) }
                    }
                } else {
                    return Err(format!("Unknown aggregate op: {}", op));
                }
            }
            ExpressionNode::Call { callee, arguments } => {
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
                if let Value::NativeFunction { name, bound_target } = &func {
                    if name == "str" {
                        if arguments.len() != 1 {
                            return Err("str() takes exactly 1 argument".to_string());
                        }
                        let arg_val = self.evaluate(&arguments[0], env)?;
                        return Ok(Value::String(format!("{}", arg_val)));
                    } else if name == "split" {
                        if let Some(target) = bound_target {
                            if let Value::String(s) = &**target {
                                let sep = if arguments.len() == 1 {
                                    if let Value::String(sep_str) = self.evaluate(&arguments[0], env)? {
                                        sep_str
                                    } else {
                                        return Err("split() argument must be a string".to_string());
                                    }
                                } else if arguments.len() == 0 {
                                    " ".to_string()
                                } else {
                                    return Err("split() takes 0 or 1 arguments".to_string());
                                };
                                
                                let parts: Vec<Value> = if arguments.len() == 0 {
                                    s.split_whitespace().map(|p| Value::String(p.to_string())).collect()
                                } else {
                                    s.split(&sep).map(|p| Value::String(p.to_string())).collect()
                                };
                                
                                return Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(parts))));
                            }
                        }
                        return Err("split() can only be called on strings".to_string());
                    } else if name == "contains" {
                        if let Some(target) = bound_target {
                            if let Value::String(s) = &**target {
                                if arguments.len() != 1 {
                                    return Err("contains() takes exactly 1 argument".to_string());
                                }
                                if let Value::String(sub) = self.evaluate(&arguments[0], env)? {
                                    return Ok(Value::Boolean(s.contains(&sub)));
                                } else {
                                    return Err("contains() argument must be a string".to_string());
                                }
                            }
                        }
                        return Err("contains() can only be called on strings".to_string());
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
                    ExecResult::Break | ExecResult::Continue => Err("Break or continue outside of loop".to_string()),
                };
                func_env.pop_scope();
                res
            }
            ExpressionNode::BinaryOp { left, operator, right } => {
                let l = self.evaluate(left, env)?;
                let r = self.evaluate(right, env)?;
                
                if matches!(l, Value::PythonObject(_)) || matches!(r, Value::PythonObject(_)) {
                    if matches!(operator.as_str(), "+" | "-" | "*" | "/" | "%" | "==" | "!=" | ">" | "<" | ">=" | "<=") {
                        return python_operator_fallback(&l, &r, operator.as_str());
                    }
                }
                
                match operator.as_str() {
                    "+" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                            (Value::List(a), Value::List(b)) => {
                                let mut new_list_vec = a.read().clone();
                                new_list_vec.extend(b.read().clone());
                                Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(new_list_vec))))
                            },
                            (Value::List(a), b) => {
                                let mut new_list_vec = a.read().clone();
                                new_list_vec.push(b.clone());
                                Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(new_list_vec))))
                            },
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
                    "/" => {
                        match (l, r) {
                            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Float((a as f64) / (b as f64))),
                            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                            _ => Err("Cannot divide these types".to_string()),
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
                    "starts_with" => {
                        match (l, r) {
                            (Value::String(a), Value::String(b)) => Ok(Value::Boolean(a.starts_with(&b))),
                            (Value::PythonObject(py_obj), Value::String(b)) => {
                                Python::with_gil(|py| -> Result<Value, String> {
                                    if let Ok(method) = py_obj.getattr(py, "startswith") {
                                        let result = method.call1(py, (b,)).map_err(|e| e.to_string())?;
                                        py_to_value(py, result.into_py(py))
                                    } else {
                                        Err("startswith method not found on object".to_string())
                                    }
                                })
                            }
                            _ => Err("starts_with requires strings or a python string".to_string()),
                        }
                    }
                    "ends_with" => {
                        match (l, r) {
                            (Value::String(a), Value::String(b)) => Ok(Value::Boolean(a.ends_with(&b))),
                            (Value::PythonObject(py_obj), Value::String(b)) => {
                                Python::with_gil(|py| -> Result<Value, String> {
                                    if let Ok(method) = py_obj.getattr(py, "endswith") {
                                        let result = method.call1(py, (b,)).map_err(|e| e.to_string())?;
                                        py_to_value(py, result.into_py(py))
                                    } else {
                                        Err("endswith method not found on object".to_string())
                                    }
                                })
                            }
                            _ => Err("ends_with requires strings or a python string".to_string()),
                        }
                    }
                    _ => Err(format!("Unknown operator {}", operator)),
                }
            }
            ExpressionNode::StringInterpolation(parts) => {
                let mut result = String::new();
                for part in parts {
                    let val = self.evaluate(part, env)?;
                    result.push_str(&val.to_string());
                }
                Ok(Value::String(result))
            }
            ExpressionNode::Closure { params, body } => {
                let block = Block {
                    statements: vec![StatementNode::Return { value: Some(*body.clone()) }.into()]
                };
                Ok(Value::Function {
                    params: params.clone(),
                    body: block,
                })
            }
            ExpressionNode::ImportPython(m) => {
                let py_module = Python::with_gil(|py| -> PyResult<Py<PyAny>> {
                    let module = PyModule::import(py, m.as_str())?;
                    Ok(module.into())
                }).map_err(|e| e.to_string())?;
                Ok(Value::PythonObject(py_module))
            }
            ExpressionNode::Null => Ok(Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn test_error_span() {
        let source = "area = 5 * \"hello\"";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        
        let mut interpreter = Interpreter::new();
        let err = interpreter.interpret(&ast).unwrap_err();
        assert!(err.contains("len 11"), "{}", err);
    }
}
