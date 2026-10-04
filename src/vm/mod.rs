pub mod opcode;
pub mod chunk;

use std::collections::HashMap;
use crate::interpreter::Value;
use opcode::OpCode;
use chunk::Chunk;

pub struct CallFrame {
    pub chunk: std::sync::Arc<Chunk>,
    pub ip: usize,
    pub base_pointer: usize,
}

pub struct CatchHandler {
    pub frame_index: usize,
    pub catch_ip: usize,
    pub stack_depth: usize,
}

pub struct VM {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub globals: HashMap<String, Value>,
    pub context_item_stack: Vec<Value>,
    pub try_stack: Vec<CatchHandler>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InterpretResult {
    Ok,
    RuntimeError(String),
}

impl VM {
    pub fn new() -> Self {
        let mut globals = HashMap::new();
        globals.insert("parse_json".to_string(), Value::NativeFunction { name: "parse_json".to_string(), bound_target: None });
        VM {
            frames: Vec::new(),
            stack: Vec::new(),
            globals,
            context_item_stack: Vec::new(),
            try_stack: Vec::new(),
        }
    }

    pub fn interpret(&mut self, chunk: Chunk) -> InterpretResult {
        let frame = CallFrame {
            chunk: std::sync::Arc::new(chunk),
            ip: 0,
            base_pointer: 0,
        };
        self.frames.push(frame);
        self.run()
    }

    fn run(&mut self) -> InterpretResult {
        macro_rules! throw {
            ($frame:expr, $label:lifetime, $err:expr) => {{
                self.frames.push($frame);
                if let Some(handler) = self.try_stack.pop() {
                    self.frames.truncate(handler.frame_index + 1);
                    self.stack.truncate(handler.stack_depth);
                    self.stack.push(Value::String($err.to_string()));
                    let mut f = self.frames.pop().unwrap();
                    f.ip = handler.catch_ip;
                    self.frames.push(f);
                    continue $label;
                } else {
                    return InterpretResult::RuntimeError($err.to_string());
                }
            }}
        }
        
        'vm_loop: loop {
            let mut frame = self.frames.pop().unwrap();
            
            while frame.ip < frame.chunk.code.len() {
                let instruction = frame.chunk.code[frame.ip];
                frame.ip += 1;
                
                match instruction {
                OpCode::Return => {
                    let result = self.stack.pop().unwrap_or(Value::Null);
                    while let Some(top_handler) = self.try_stack.last() {
                        if top_handler.frame_index >= self.frames.len() {
                            self.try_stack.pop();
                        } else {
                            break;
                        }
                    }
                    if self.frames.is_empty() {
                        self.stack.push(result);
                        return InterpretResult::Ok;
                    }
                    self.stack.truncate(frame.base_pointer - 1);
                    self.stack.push(result);
                    continue 'vm_loop;
                }
                OpCode::Constant(idx) => {
                    let constant = frame.chunk.constants[idx].clone();
                    self.stack.push(constant);
                }
                OpCode::Pop => {
                    self.stack.pop();
                }
                OpCode::GetLocal(slot) => {
                    let val = self.stack[frame.base_pointer + slot].clone();
                    self.stack.push(val);
                }
                OpCode::SetLocal(slot) => {
                    let val = self.stack.last().unwrap().clone();
                    self.stack[frame.base_pointer + slot] = val;
                }
                OpCode::GetGlobal(idx) => {
                    let name_val = frame.chunk.constants[idx].clone();
                    if let Value::String(name) = name_val {
                        if let Some(val) = self.globals.get(&name) {
                            self.stack.push(val.clone());
                        } else if let Some(ctx_val) = self.context_item_stack.last() {
                            self.stack.push(ctx_val.clone());
                        } else {
                            throw!(frame, 'vm_loop, format!("Unknown variable '{}'", name));
                        }
                    } else {
                        throw!(frame, 'vm_loop, "Global name must be a string".to_string());
                    }
                }
                OpCode::SetGlobal(idx) => {
                    let name_val = frame.chunk.constants[idx].clone();
                    if let Value::String(name) = name_val {
                        let val = self.stack.last().unwrap().clone();
                        self.globals.insert(name.clone(), val);
                    } else {
                        throw!(frame, 'vm_loop, "Global name must be a string".to_string());
                    }
                }
                OpCode::True => self.stack.push(Value::Boolean(true)),
                OpCode::False => self.stack.push(Value::Boolean(false)),
                OpCode::Null => self.stack.push(Value::Null),
                OpCode::Add => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&mut *a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *x += *y,
                        (Value::Float(x), Value::Float(y)) => *x += *y,
                        (Value::Integer(x), Value::Float(y)) => *a = Value::Float((*x as f64) + *y),
                        (Value::Float(x), Value::Integer(y)) => *x += *y as f64,
                        (Value::String(x), Value::String(y)) => *a = Value::String(format!("{}{}", x, y)),
                        (Value::List(x), Value::List(y)) => {
                            let mut new_vec = x.read().clone();
                            new_vec.extend(y.read().clone());
                            *a = Value::List(std::sync::Arc::new(parking_lot::RwLock::new(new_vec)));
                        }
                        (Value::PythonObject(_), _) | (_, Value::PythonObject(_)) => {
                            match crate::interpreter::python_operator_fallback(&*a, &b, "+") {
                                Ok(v) => *a = v,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers, strings, or lists".to_string()),
                    }
                }
                OpCode::Subtract => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&mut *a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *x -= *y,
                        (Value::Float(x), Value::Float(y)) => *x -= *y,
                        (Value::Integer(x), Value::Float(y)) => *a = Value::Float((*x as f64) - *y),
                        (Value::Float(x), Value::Integer(y)) => *x -= *y as f64,
                        (Value::PythonObject(_), _) | (_, Value::PythonObject(_)) => {
                            match crate::interpreter::python_operator_fallback(&*a, &b, "-") {
                                Ok(v) => *a = v,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::Multiply => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&mut *a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *x *= *y,
                        (Value::Float(x), Value::Float(y)) => *x *= *y,
                        (Value::Integer(x), Value::Float(y)) => *a = Value::Float((*x as f64) * *y),
                        (Value::Float(x), Value::Integer(y)) => *x *= *y as f64,
                        (Value::PythonObject(_), _) | (_, Value::PythonObject(_)) => {
                            match crate::interpreter::python_operator_fallback(&*a, &b, "*") {
                                Ok(v) => *a = v,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::Divide => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&mut *a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *a = Value::Float((*x as f64) / (*y as f64)),
                        (Value::Float(x), Value::Float(y)) => *x /= *y,
                        (Value::Integer(x), Value::Float(y)) => *a = Value::Float((*x as f64) / *y),
                        (Value::Float(x), Value::Integer(y)) => *x /= *y as f64,
                        (Value::PythonObject(_), _) | (_, Value::PythonObject(_)) => {
                            match crate::interpreter::python_operator_fallback(&*a, &b, "/") {
                                Ok(v) => *a = v,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::Modulo => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&mut *a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *x %= *y,
                        (Value::Float(x), Value::Float(y)) => *x %= *y,
                        (Value::Integer(x), Value::Float(y)) => *a = Value::Float((*x as f64) % *y),
                        (Value::Float(x), Value::Integer(y)) => *x %= *y as f64,
                        (Value::PythonObject(_), _) | (_, Value::PythonObject(_)) => {
                            match crate::interpreter::python_operator_fallback(&*a, &b, "%") {
                                Ok(v) => *a = v,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::Equal => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    *a = Value::Boolean(*a == b);
                }
                OpCode::NotEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    *a = Value::Boolean(*a != b);
                }
                OpCode::Greater => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&*a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *a = Value::Boolean(x > y),
                        (Value::Float(x), Value::Float(y)) => *a = Value::Boolean(x > y),
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::Less => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&*a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *a = Value::Boolean(x < y),
                        (Value::Float(x), Value::Float(y)) => *a = Value::Boolean(x < y),
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::GreaterEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&*a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *a = Value::Boolean(x >= y),
                        (Value::Float(x), Value::Float(y)) => *a = Value::Boolean(x >= y),
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::LessEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    match (&*a, &b) {
                        (Value::Integer(x), Value::Integer(y)) => *a = Value::Boolean(x <= y),
                        (Value::Float(x), Value::Float(y)) => *a = Value::Boolean(x <= y),
                        _ => throw!(frame, 'vm_loop, "Operands must be numbers".to_string()),
                    }
                }
                OpCode::AddInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&mut *a, &b) {
                        *x += *y;
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::SubtractInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&mut *a, &b) {
                        *x -= *y;
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::MultiplyInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&mut *a, &b) {
                        *x *= *y;
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::EqualInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x == y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::NotEqualInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x != y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::LessInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x < y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::LessEqualInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x <= y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::GreaterInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x > y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::GreaterEqualInt => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.last_mut().unwrap();
                    if let (Value::Integer(x), Value::Integer(y)) = (&*a, &b) {
                        *a = Value::Boolean(x >= y);
                    } else {
                        throw!(frame, 'vm_loop, "Operands must be integers".to_string());
                    }
                }
                OpCode::Jump(offset) => {
                    frame.ip += offset;
                }
                OpCode::JumpIfFalse(offset) => {
                    let val = self.stack.pop().unwrap();
                    let condition = match val {
                        Value::Boolean(b) => b,
                        Value::Null => false,
                        _ => true,
                    };
                    if !condition {
                        frame.ip += offset;
                    }
                }
                OpCode::Loop(offset) => {
                    frame.ip -= offset;
                }
                OpCode::Print => {
                    let val = self.stack.pop().unwrap();
                    println!("{}", val);
                }
                OpCode::Call(arg_count) => {
                    let callee = self.stack[self.stack.len() - 1 - arg_count].clone();
                    match callee {
                        Value::CompiledFunction { chunk, arity } => {
                            if arg_count != arity {
                                throw!(frame, 'vm_loop, format!("Expected {} arguments but got {}", arity, arg_count));
                            }
                            let new_frame = CallFrame {
                                chunk: chunk.clone(),
                                ip: 0,
                                base_pointer: self.stack.len() - arg_count,
                            };
                            self.frames.push(frame);
                            self.frames.push(new_frame);
                            continue 'vm_loop;
                        }
                        Value::NativeFunction { name, bound_target } => {
                            let target_val = bound_target.clone().map(|b| *b);
                            let mut args = Vec::new();
                            let start = self.stack.len() - arg_count;
                            for i in 0..arg_count {
                                args.push(self.stack[start + i].clone());
                            }
                            
                            let result = match name.as_str() {
                                "split" => {
                                    if let Some(Value::String(s)) = target_val {
                                        if args.len() == 1 {
                                            if let Value::String(sep) = &args[0] {
                                                let parts: Vec<Value> = s.split(sep).map(|part| Value::String(part.to_string())).collect();
                                                Ok(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(parts))))
                                            } else {
                                                Err("split() requires a string argument".to_string())
                                            }
                                        } else {
                                            Err("split() requires exactly 1 argument".to_string())
                                        }
                                    } else {
                                        Err("split() requires a string target".to_string())
                                    }
                                }
                                "contains" => {
                                    if let Some(Value::String(s)) = target_val {
                                        if args.len() == 1 {
                                            if let Value::String(search) = &args[0] {
                                                Ok(Value::Boolean(s.contains(search)))
                                            } else {
                                                Err("contains() requires a string argument".to_string())
                                            }
                                        } else {
                                            Err("contains() requires exactly 1 argument".to_string())
                                        }
                                    } else {
                                        Err("contains() requires a string target".to_string())
                                    }
                                }
                                "length" => {
                                    if let Some(Value::String(s)) = target_val {
                                        Ok(Value::Integer(s.len() as i64))
                                    } else if let Some(Value::List(l)) = target_val {
                                        Ok(Value::Integer(l.read().len() as i64))
                                    } else {
                                        Err("length() requires a string or list target".to_string())
                                    }
                                }
                                "push" => {
                                    if let Some(Value::List(l)) = target_val {
                                        if args.len() == 1 {
                                            l.write().push(args[0].clone());
                                            Ok(Value::Null)
                                        } else {
                                            Err("push() requires exactly 1 argument".to_string())
                                        }
                                    } else {
                                        Err("push() requires a list target".to_string())
                                    }
                                }
                                "pop" => {
                                    if let Some(Value::List(l)) = target_val {
                                        if args.is_empty() {
                                            Ok(l.write().pop().unwrap_or(Value::Null))
                                        } else {
                                            Err("pop() requires exactly 0 arguments".to_string())
                                        }
                                    } else {
                                        Err("pop() requires a list target".to_string())
                                    }
                                }
                                "parse_json" => {
                                    if args.len() == 1 {
                                        if let Value::String(s) = &args[0] {
                                            pyo3::Python::with_gil(|py| -> Result<Value, String> {
                                                let json = pyo3::types::PyModule::import(py, "json").map_err(|e| e.to_string())?;
                                                let loads = json.getattr("loads").map_err(|e| e.to_string())?;
                                                let py_result = loads.call1((s.as_str(),)).map_err(|e| e.to_string())?;
                                                crate::interpreter::py_to_value(py, py_result.into())
                                            })
                                        } else {
                                            Err("parse_json() requires a string argument".to_string())
                                        }
                                    } else {
                                        Err("parse_json() requires exactly 1 argument".to_string())
                                    }
                                }
                                _ => Err(format!("Unknown native function: {}", name)),
                            };
                            
                            match result {
                                Ok(v) => {
                                    self.stack.truncate(self.stack.len() - arg_count - 1);
                                    self.stack.push(v);
                                }
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        Value::Class { name, methods } => {
                            let mut props = std::collections::HashMap::new();
                            for (k, v) in methods.iter() {
                                props.insert(k.clone(), v.clone());
                            }
                            let instance = Value::Instance {
                                class_name: name.clone(),
                                properties: std::sync::Arc::new(parking_lot::RwLock::new(props)),
                            };
                            
                            if let Some(init_val) = methods.get("init") {
                                if let Value::CompiledFunction { chunk: init_chunk, arity } = init_val {
                                    if arg_count != *arity {
                                        throw!(frame, 'vm_loop, format!("Expected {} arguments for init but got {}", arity, arg_count));
                                    }
                                    let mut sub_vm = VM::new();
                                    sub_vm.globals = self.globals.clone();
                                    let start = self.stack.len() - arg_count;
                                    for i in 0..arg_count {
                                        sub_vm.stack.push(self.stack[start + i].clone());
                                    }
                                    sub_vm.frames.push(CallFrame {
                                        chunk: init_chunk.clone(),
                                        ip: 0,
                                        base_pointer: 0,
                                    });
                                    match sub_vm.run() {
                                        InterpretResult::Ok => {
                                            self.globals = sub_vm.globals;
                                        }
                                        InterpretResult::RuntimeError(e) => throw!(frame, 'vm_loop, e),
                                    }
                                }
                            }
                            
                            self.stack.truncate(self.stack.len() - arg_count - 1);
                            self.stack.push(instance);
                        }
                        Value::PythonObject(py_obj) => {
                            let result = pyo3::Python::with_gil(|py| -> Result<Value, String> {
                                let mut py_args = Vec::new();
                                // Arguments are on the stack, but they need to be popped in the right order (they were pushed left-to-right)
                                // Let's just read them
                                let start = self.stack.len() - arg_count;
                                for i in 0..arg_count {
                                    py_args.push(crate::interpreter::value_to_py(py, &self.stack[start + i])?);
                                }
                                
                                let args_tuple = pyo3::types::PyTuple::new(py, py_args);
                                
                                let result = py_obj.call1(py, args_tuple)
                                    .map_err(|e| format!("Python error during call: {}", e))?;
                                    
                                crate::interpreter::py_to_value(py, result)
                            });
                            
                            match result {
                                Ok(v) => {
                                    // Pop arguments and callee
                                    self.stack.truncate(self.stack.len() - arg_count - 1);
                                    self.stack.push(v);
                                }
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        _ => {
                            let callee = self.stack[self.stack.len() - arg_count - 1].clone();
                            throw!(frame, 'vm_loop, format!("Can only call functions and classes, tried to call: {:?}", callee));
                        }
                    }
                }
                OpCode::ImportPython(idx) => {
                    let name = match &frame.chunk.constants[idx] {
                        Value::String(s) => s.clone(),
                        _ => throw!(frame, 'vm_loop, "Import name must be string".to_string()),
                    };
                    let result = pyo3::Python::with_gil(|py| -> Result<Value, String> {
                        let m = pyo3::types::PyModule::import(py, name.as_str())
                            .map_err(|e| format!("Failed to import python module {}: {}", name, e))?;
                        Ok(Value::PythonObject(m.into()))
                    });
                    match result {
                        Ok(v) => self.stack.push(v),
                        Err(e) => throw!(frame, 'vm_loop, e),
                    }
                }
                OpCode::BuildList(count) => {
                    let mut list = Vec::with_capacity(count);
                    for _ in 0..count {
                        list.push(self.stack.pop().unwrap());
                    }
                    list.reverse();
                    self.stack.push(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(list))));
                }
                OpCode::BuildMap(count) => {
                    let mut map = std::collections::HashMap::new();
                    for _ in 0..count {
                        let val = self.stack.pop().unwrap();
                        let key_val = self.stack.pop().unwrap();
                        if let Value::String(k) = key_val {
                            map.insert(k, val);
                        } else {
                            throw!(frame, 'vm_loop, "Map keys must be strings".to_string());
                        }
                    }
                    self.stack.push(Value::Map(std::sync::Arc::new(parking_lot::RwLock::new(map))));
                }
                OpCode::IndexGet => {
                    let index = self.stack.pop().unwrap();
                    let target = self.stack.pop().unwrap();
                    match (target, index) {
                        (Value::List(l), Value::Integer(i)) => {
                            if i >= 0 && (i as usize) < l.read().len() {
                                self.stack.push(l.read()[i as usize].clone());
                            } else {
                                throw!(frame, 'vm_loop, "Index out of bounds".to_string());
                            }
                        }
                        (Value::Map(m), Value::String(k)) => {
                            if let Some(v) = m.read().get(&k) {
                                self.stack.push(v.clone());
                            } else {
                                self.stack.push(Value::Null);
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Invalid index access".to_string()),
                    }
                }
                OpCode::IndexSet => {
                    let value = self.stack.pop().unwrap();
                    let index = self.stack.pop().unwrap();
                    let mut target = self.stack.pop().unwrap();
                    
                    // In this value-semantic implementation, IndexSet on a stack value
                    // will only mutate the clone on the stack, which is immediately discarded!
                    // This is expected given the lack of Rc<RefCell>.
                    match &mut target {
                        Value::List(l) => {
                            if let Value::Integer(i) = index {
                                if i >= 0 && (i as usize) < l.read().len() {
                                    l.write()[i as usize] = value;
                                } else {
                                    throw!(frame, 'vm_loop, "Index out of bounds".to_string());
                                }
                            }
                        }
                        Value::Map(m) => {
                            if let Value::String(k) = index {
                                m.write().insert(k, value);
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Invalid index set".to_string()),
                    }
                    // Since it's an expression/statement, usually a value is left on stack for the result,
                    // but Novo statements don't leave results. However, if this is used in an expression...
                    // Wait, `value` could be pushed back, or we leave it. Let's push Null.
                    self.stack.push(Value::Null);
                }
                OpCode::PropertyGet(idx) => {
                    let name = match &frame.chunk.constants[idx] {
                        Value::String(s) => s.clone(),
                        _ => throw!(frame, 'vm_loop, "Property name must be string".to_string()),
                    };
                    let target = self.stack.pop().unwrap();
                    match target {
                        Value::Map(m) => {
                            if let Some(v) = m.read().get(&name) {
                                self.stack.push(v.clone());
                            } else {
                                self.stack.push(Value::Null);
                            }
                        }
                        Value::Instance { class_name: _, properties } => {
                            if let Some(v) = properties.read().get(&name) {
                                self.stack.push(v.clone());
                            } else {
                                self.stack.push(Value::Null);
                            }
                        }
                        Value::List(l) => {
                            if name == "length" || name == "push" || name == "pop" {
                                self.stack.push(Value::NativeFunction {
                                    name,
                                    bound_target: Some(Box::new(Value::List(l))),
                                });
                            } else {
                                throw!(frame, 'vm_loop, format!("List has no property '{}'", name));
                            }
                        }
                        Value::PythonObject(py_obj) => {
                            let result = pyo3::Python::with_gil(|py| -> Result<Value, String> {
                                let attr = py_obj.getattr(py, name.as_str()).map_err(|e| e.to_string())?;
                                crate::interpreter::py_to_value(py, attr)
                            });
                            match result {
                                Ok(v) => self.stack.push(v),
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        Value::String(s) => {
                            if name == "split" || name == "contains" || name == "length" {
                                self.stack.push(Value::NativeFunction {
                                    name,
                                    bound_target: Some(Box::new(Value::String(s))),
                                });
                            } else {
                                throw!(frame, 'vm_loop, format!("String has no property '{}'", name));
                            }
                        }
                        _ => throw!(frame, 'vm_loop, "Invalid property access".to_string()),
                    }
                }
                OpCode::PropertySet(idx) => {
                    let name = match &frame.chunk.constants[idx] {
                        Value::String(s) => s.clone(),
                        _ => throw!(frame, 'vm_loop, "Property name must be string".to_string()),
                    };
                    let value = self.stack.pop().unwrap();
                    let mut target = self.stack.pop().unwrap();
                    match &mut target {
                        Value::Map(m) => {
                            m.write().insert(name, value);
                        }
                        Value::Instance { properties, .. } => {
                            properties.write().insert(name, value);
                        }
                        _ => throw!(frame, 'vm_loop, "Invalid property set".to_string()),
                    }
                    self.stack.push(Value::Null);
                }
                OpCode::Select(closure_idx) => {
                    let target = self.stack.pop().unwrap();
                    let closure = match &frame.chunk.constants[closure_idx] {
                        Value::CompiledFunction { chunk, .. } => chunk.clone(),
                        _ => throw!(frame, 'vm_loop, "Expected CompiledFunction".to_string()),
                    };
                    if let Value::List(l) = target {
                        let items = l.read().clone();
                        let mut result = Vec::with_capacity(items.len());
                        for item in items {
                            self.context_item_stack.push(item);
                            match self.execute_closure(closure.clone()) {
                                Ok(val) => result.push(val),
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                            self.context_item_stack.pop();
                        }
                        self.stack.push(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(result))));
                    } else {
                        throw!(frame, 'vm_loop, "'select' requires a list".to_string());
                    }
                }
                OpCode::Where(closure_idx) => {
                    let target = self.stack.pop().unwrap();
                    let closure = match &frame.chunk.constants[closure_idx] {
                        Value::CompiledFunction { chunk, .. } => chunk.clone(),
                        _ => throw!(frame, 'vm_loop, "Expected CompiledFunction".to_string()),
                    };
                    if let Value::List(l) = target {
                        let items = l.read().clone();
                        let mut result = Vec::new();
                        for item in items {
                            self.context_item_stack.push(item.clone());
                            let cond = match self.execute_closure(closure.clone()) {
                                Ok(val) => val,
                                Err(e) => throw!(frame, 'vm_loop, e),
                            };
                            self.context_item_stack.pop();
                            
                            if let Value::Boolean(true) = cond {
                                result.push(item);
                            }
                        }
                        self.stack.push(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(result))));
                    } else {
                        throw!(frame, 'vm_loop, "'where' requires a list".to_string());
                    }
                }
                OpCode::AggregateSum(closure_idx) | OpCode::AggregateAverage(closure_idx) | OpCode::AggregateMax(closure_idx) | OpCode::AggregateMin(closure_idx) => {
                    let target = self.stack.pop().unwrap();
                    let closure = match &frame.chunk.constants[closure_idx] {
                        Value::CompiledFunction { chunk, .. } => chunk.clone(),
                        _ => throw!(frame, 'vm_loop, "Expected CompiledFunction".to_string()),
                    };
                    if let Value::List(l) = target {
                        let items = l.read().clone();
                        if items.is_empty() {
                            self.stack.push(Value::Null);
                        } else {
                            let mut is_float = false;
                            let mut running_int = 0;
                            let mut running_float = 0.0;
                            let mut max_int = i64::MIN;
                            let mut max_float = f64::NEG_INFINITY;
                            let mut min_int = i64::MAX;
                            let mut min_float = f64::INFINITY;
                            
                            for item in items.iter() {
                                self.context_item_stack.push(item.clone());
                                let val = match self.execute_closure(closure.clone()) {
                                    Ok(v) => v,
                                    Err(e) => throw!(frame, 'vm_loop, e),
                                };
                                self.context_item_stack.pop();
                                
                                match val {
                                    Value::Integer(i) => {
                                        let f = i as f64;
                                        if is_float { running_float += f; } else { running_int += i; }
                                        if !is_float {
                                            if i > max_int { max_int = i; }
                                            if i < min_int { min_int = i; }
                                        }
                                        if f > max_float { max_float = f; }
                                        if f < min_float { min_float = f; }
                                    }
                                    Value::Float(f) => {
                                        if !is_float { is_float = true; running_float = running_int as f64; }
                                        running_float += f;
                                        if f > max_float { max_float = f; }
                                        if f < min_float { min_float = f; }
                                    }
                                    _ => throw!(frame, 'vm_loop, "Aggregate requires numeric values".to_string()),
                                }
                            }
                            
                            match instruction {
                                OpCode::AggregateSum(_) => {
                                    if is_float { self.stack.push(Value::Float(running_float)); }
                                    else { self.stack.push(Value::Integer(running_int)); }
                                }
                                OpCode::AggregateAverage(_) => {
                                    let count = items.len() as f64;
                                    if is_float { self.stack.push(Value::Float(running_float / count)); }
                                    else { self.stack.push(Value::Float((running_int as f64) / count)); }
                                }
                                OpCode::AggregateMax(_) => {
                                    if is_float { self.stack.push(Value::Float(max_float)); }
                                    else { self.stack.push(Value::Integer(max_int)); }
                                }
                                OpCode::AggregateMin(_) => {
                                    if is_float { self.stack.push(Value::Float(min_float)); }
                                    else { self.stack.push(Value::Integer(min_int)); }
                                }
                                _ => unreachable!(),
                            }
                        }
                    } else {
                        throw!(frame, 'vm_loop, "Aggregate requires a list");
                    }
                }
                OpCode::StringInterpolate(count) => {
                    let mut s = String::new();
                    let start = self.stack.len() - count;
                    for i in start..self.stack.len() {
                        let val = &self.stack[i];
                        let string_val = match val {
                            Value::String(st) => st.clone(),
                            Value::Integer(n) => n.to_string(),
                            Value::Float(f) => f.to_string(),
                            Value::Boolean(b) => b.to_string(),
                            Value::Null => "null".to_string(),
                            _ => "Object".to_string(),
                        };
                        s.push_str(&string_val);
                    }
                    self.stack.truncate(start);
                    self.stack.push(Value::String(s));
                }
                OpCode::Read => {
                    let path_val = self.stack.pop().unwrap();
                    let execute_read = |path: &str| -> Result<Value, String> {
                        let content = std::fs::read_to_string(path)
                            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
                        Ok(Value::String(content))
                    };
                    match path_val {
                        Value::String(path) => {
                            match execute_read(&path) {
                                Ok(v) => self.stack.push(v),
                                Err(e) => throw!(frame, 'vm_loop, e),
                            }
                        }
                        Value::List(paths) => {
                            let mut results = Vec::new();
                            for path_item in paths.read().iter() {
                                if let Value::String(path) = path_item {
                                    match execute_read(path) {
                                        Ok(v) => results.push(v),
                                        Err(e) => throw!(frame, 'vm_loop, e),
                                    }
                                } else {
                                    throw!(frame, 'vm_loop, "Expected list of strings for read".to_string());
                                }
                            }
                            self.stack.push(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))));
                        }
                        _ => throw!(frame, 'vm_loop, "Expected string or list of strings for read path".to_string()),
                    }
                }
                OpCode::Get => {
                    let url_val = self.stack.pop().unwrap();
                    let url = if let Value::String(s) = url_val { s } else { throw!(frame, 'vm_loop, "Expected string for get url".to_string()) };
                    let execute_get = |url: &str| -> Result<Value, String> {
                        pyo3::Python::with_gil(|py| -> Result<Value, String> {
                            let urllib = pyo3::types::PyModule::import(py, "urllib.request").map_err(|e| format!("Failed to import urllib: {}", e))?;
                            let urlopen = urllib.getattr("urlopen").map_err(|e| e.to_string())?;
                            let py_result = urlopen.call1((url,)).map_err(|e| format!("HTTP request failed: {}", e))?;
                            let read_func = py_result.getattr("read").map_err(|e| e.to_string())?;
                            let bytes = read_func.call0().map_err(|e| e.to_string())?;
                            let decode_func = bytes.getattr("decode").map_err(|e| e.to_string())?;
                            let decoded = decode_func.call1(("utf-8",)).map_err(|e| e.to_string())?;
                            crate::interpreter::py_to_value(py, pyo3::IntoPy::into_py(decoded, py))
                        })
                    };
                    match execute_get(&url) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => throw!(frame, 'vm_loop, e),
                    }
                }
                OpCode::Cast(type_idx) => {
                    let target_type_val = frame.chunk.constants[type_idx].clone();
                    let target_type = if let Value::String(s) = target_type_val { s } else { throw!(frame, 'vm_loop, "Invalid cast type".to_string()) };
                    let val = self.stack.pop().unwrap();
                    let execute_cast = |v: &Value, t: &str| -> Result<Value, String> {
                        match (v, t) {
                            (Value::String(s), "json") => {
                                pyo3::Python::with_gil(|py| -> Result<Value, String> {
                                    let json = pyo3::types::PyModule::import(py, "json").map_err(|e| e.to_string())?;
                                    let loads = json.getattr("loads").map_err(|e| e.to_string())?;
                                    let py_result = loads.call1((s,)).map_err(|e| e.to_string())?;
                                    crate::interpreter::py_to_value(py, pyo3::IntoPy::into_py(py_result, py))
                                })
                            }
                            (Value::String(s), "csv") => {
                                pyo3::Python::with_gil(|py| -> Result<Value, String> {
                                    let csv = pyo3::types::PyModule::import(py, "csv").map_err(|e| e.to_string())?;
                                    let io = pyo3::types::PyModule::import(py, "io").map_err(|e| e.to_string())?;
                                    let string_io = io.getattr("StringIO").map_err(|e| e.to_string())?.call1((s,)).map_err(|e| e.to_string())?;
                                    let dict_reader = csv.getattr("DictReader").map_err(|e| e.to_string())?.call1((string_io,)).map_err(|e| e.to_string())?;
                                    let list_func = py.eval("list", None, None).map_err(|e| e.to_string())?;
                                    let py_result = list_func.call1((dict_reader,)).map_err(|e| e.to_string())?;
                                    crate::interpreter::py_to_value(py, pyo3::IntoPy::into_py(py_result, py))
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
                                match execute_cast(item, &target_type) {
                                    Ok(v) => results.push(v),
                                    Err(e) => throw!(frame, 'vm_loop, e),
                                }
                            }
                            self.stack.push(Value::List(std::sync::Arc::new(parking_lot::RwLock::new(results))));
                        }
                        _ => match execute_cast(&val, &target_type) {
                            Ok(v) => self.stack.push(v),
                            Err(e) => throw!(frame, 'vm_loop, e),
                        }
                    }
                }
                OpCode::TryStart(offset) => {
                    let catch_ip = frame.ip + offset;
                    self.try_stack.push(CatchHandler {
                        frame_index: self.frames.len(),
                        catch_ip,
                        stack_depth: self.stack.len(),
                    });
                }
                OpCode::TryEnd => {
                    self.try_stack.pop();
                }
                OpCode::SpawnConcurrent(idx) => {
                    let func = frame.chunk.constants[idx].clone();
                    if let Value::CompiledFunction { chunk, arity: _ } = func {
                        let mut new_vm = VM::new();
                        new_vm.globals = self.globals.clone();
                        
                        let result_arc = std::sync::Arc::new(parking_lot::RwLock::new(None));
                        let ret_arc = result_arc.clone();
                        
                        std::thread::spawn(move || {
                            let call_frame = CallFrame {
                                chunk,
                                ip: 0,
                                base_pointer: 0,
                            };
                            new_vm.frames.push(call_frame);
                            match new_vm.run() {
                                InterpretResult::Ok => {
                                    let ret = new_vm.stack.pop().unwrap_or(Value::Null);
                                    *result_arc.write() = Some(Ok(ret));
                                }
                                InterpretResult::RuntimeError(e) => {
                                    *result_arc.write() = Some(Err(e));
                                }
                            }
                        });
                        
                        self.stack.push(Value::Promise(ret_arc));
                    } else {
                        throw!(frame, 'vm_loop, "SpawnConcurrent requires a CompiledFunction".to_string());
                    }
                }
                OpCode::Await => {
                    let promise_val = self.stack.pop().unwrap();
                    if let Value::Promise(arc) = promise_val {
                        // Busy-wait for simplicity, or we can use a condvar.
                        // For a simple implementation, loop and sleep until it's Some.
                        loop {
                            let val_opt = arc.read().clone();
                            if let Some(res) = val_opt {
                                match res {
                                    Ok(v) => {
                                        self.stack.push(v);
                                        break;
                                    }
                                    Err(e) => {
                                        throw!(frame, 'vm_loop, e);
                                    }
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    } else {
                        throw!(frame, 'vm_loop, "Await requires a Promise".to_string());
                    }
                }
            }
            }
            
            // Implicit return at end of function
            if self.frames.is_empty() {
                return InterpretResult::Ok;
            }
            let result = self.stack.pop().unwrap_or(Value::Null);
            if frame.base_pointer > 0 {
                self.stack.truncate(frame.base_pointer - 1);
            }
            self.stack.push(result);
        }
    }

    fn execute_closure(&mut self, chunk: std::sync::Arc<crate::vm::Chunk>) -> Result<Value, String> {
        let mut sub_vm = VM::new();
        sub_vm.globals = self.globals.clone();
        sub_vm.context_item_stack = self.context_item_stack.clone();
        
        sub_vm.frames.push(CallFrame {
            chunk,
            ip: 0,
            base_pointer: 0,
        });
        
        match sub_vm.run() {
            InterpretResult::Ok => {
                let ret = sub_vm.stack.pop().unwrap_or(Value::Null);
                self.globals = sub_vm.globals;
                Ok(ret)
            }
            InterpretResult::RuntimeError(e) => Err(e),
        }
    }
}
