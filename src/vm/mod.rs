pub mod opcode;
pub mod chunk;

use std::collections::HashMap;
use crate::interpreter::Value;
use opcode::OpCode;
use chunk::Chunk;

pub struct CallFrame {
    pub chunk: Chunk,
    pub ip: usize,
    pub base_pointer: usize,
}

pub struct VM {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub globals: HashMap<String, Value>,
}

pub enum InterpretResult {
    Ok,
    CompileError,
    RuntimeError(String),
}

impl VM {
    pub fn new() -> Self {
        VM {
            frames: Vec::new(),
            stack: Vec::new(),
            globals: HashMap::new(),
        }
    }

    pub fn interpret(&mut self, chunk: Chunk) -> InterpretResult {
        let frame = CallFrame {
            chunk,
            ip: 0,
            base_pointer: 0,
        };
        self.frames.push(frame);
        self.run()
    }

    fn run(&mut self) -> InterpretResult {
        loop {
            // Need to borrow frame mutably to update IP, but also need to access stack which is part of VM.
            // In Rust, we can't borrow self.frames.last_mut() and self.stack simultaneously easily if both are on `self`.
            // Let's pop the frame, execute, and push it back, or just use indices.
            let mut frame = self.frames.pop().unwrap();
            
            if frame.ip >= frame.chunk.code.len() {
                // If it's the last frame, we are done
                if self.frames.is_empty() {
                    return InterpretResult::Ok;
                }
                // Implicit return
                let result = self.stack.pop().unwrap_or(Value::Null);
                self.stack.truncate(frame.base_pointer);
                self.stack.push(result);
                continue;
            }
            
            let instruction = frame.chunk.code[frame.ip];
            frame.ip += 1;
            
            match instruction {
                OpCode::Return => {
                    let result = self.stack.pop().unwrap_or(Value::Null);
                    if self.frames.is_empty() {
                        return InterpretResult::Ok;
                    }
                    self.stack.truncate(frame.base_pointer);
                    self.stack.push(result);
                    continue; // Do not push this frame back!
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
                    let name_val = &frame.chunk.constants[idx];
                    if let Value::String(name) = name_val {
                        if let Some(val) = self.globals.get(name) {
                            self.stack.push(val.clone());
                        } else {
                            return InterpretResult::RuntimeError(format!("Undefined global variable '{}'", name));
                        }
                    } else {
                        return InterpretResult::RuntimeError("Global name must be a string".to_string());
                    }
                }
                OpCode::SetGlobal(idx) => {
                    let name_val = &frame.chunk.constants[idx];
                    if let Value::String(name) = name_val {
                        let val = self.stack.last().unwrap().clone();
                        self.globals.insert(name.clone(), val);
                    } else {
                        return InterpretResult::RuntimeError("Global name must be a string".to_string());
                    }
                }
                OpCode::True => self.stack.push(Value::Boolean(true)),
                OpCode::False => self.stack.push(Value::Boolean(false)),
                OpCode::Null => self.stack.push(Value::Null),
                OpCode::Add => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Integer(x + y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x + y)),
                        (Value::String(x), Value::String(y)) => self.stack.push(Value::String(format!("{}{}", x, y))),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers or strings".to_string()),
                    }
                }
                OpCode::Subtract => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Integer(x - y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x - y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Multiply => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Integer(x * y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x * y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Divide => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Float((x as f64) / (y as f64))),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x / y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Modulo => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Integer(x % y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x % y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Negate => {
                    let a = self.stack.pop().unwrap();
                    match a {
                        Value::Integer(x) => self.stack.push(Value::Integer(-x)),
                        Value::Float(x) => self.stack.push(Value::Float(-x)),
                        _ => return InterpretResult::RuntimeError("Operand must be a number".to_string()),
                    }
                }
                OpCode::Not => {
                    let a = self.stack.pop().unwrap();
                    match a {
                        Value::Boolean(x) => self.stack.push(Value::Boolean(!x)),
                        _ => return InterpretResult::RuntimeError("Operand must be a boolean".to_string()),
                    }
                }
                OpCode::Equal => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    self.stack.push(Value::Boolean(a == b));
                }
                OpCode::NotEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    self.stack.push(Value::Boolean(a != b));
                }
                OpCode::Greater => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Boolean(x > y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Boolean(x > y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Less => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Boolean(x < y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Boolean(x < y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::GreaterEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Boolean(x >= y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Boolean(x >= y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::LessEqual => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    match (a, b) {
                        (Value::Integer(x), Value::Integer(y)) => self.stack.push(Value::Boolean(x <= y)),
                        (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Boolean(x <= y)),
                        _ => return InterpretResult::RuntimeError("Operands must be numbers".to_string()),
                    }
                }
                OpCode::Jump(offset) => {
                    frame.ip += offset;
                }
                OpCode::JumpIfFalse(offset) => {
                    let val = self.stack.last().unwrap();
                    let condition = match val {
                        Value::Boolean(b) => *b,
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
                    return InterpretResult::RuntimeError("Call not fully implemented".to_string());
                }
            }
            
            // Push frame back
            self.frames.push(frame);
        }
    }
}
