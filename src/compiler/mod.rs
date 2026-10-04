use crate::ast::{Expression, ExpressionNode, Program, Statement, StatementNode};
use crate::vm::chunk::Chunk;
use crate::vm::opcode::OpCode;
use crate::interpreter::Value;
use std::collections::HashMap;

pub struct Compiler {
    chunk: Chunk,
    locals: Vec<String>,
    scope_depth: usize,
}

impl Compiler {
    pub fn new() -> Self {
        Compiler {
            chunk: Chunk::new(),
            locals: Vec::new(),
            scope_depth: 0,
        }
    }

    pub fn compile(mut self, program: &Program) -> Result<Chunk, String> {
        for stmt in &program.statements {
            self.compile_statement(stmt)?;
        }
        Ok(self.chunk)
    }

    fn compile_statement(&mut self, stmt: &Statement) -> Result<(), String> {
        match &stmt.node {
            StatementNode::Show { value } => {
                self.compile_expression(value)?;
                self.chunk.write(OpCode::Print, stmt.span.line);
            }
            StatementNode::Assignment { name, value, .. } => {
                self.compile_expression(value)?;
                
                if self.scope_depth > 0 {
                    if let Some(pos) = self.locals.iter().rposition(|l| l == name) {
                        self.chunk.write(OpCode::SetLocal(pos), stmt.span.line);
                    } else {
                        self.locals.push(name.clone());
                    }
                } else {
                    let idx = self.chunk.add_constant(Value::String(name.clone()));
                    self.chunk.write(OpCode::SetGlobal(idx), stmt.span.line);
                    self.chunk.write(OpCode::Pop, stmt.span.line);
                }
            }
            StatementNode::Return { value } => {
                if let Some(expr) = value {
                    self.compile_expression(expr)?;
                } else {
                    self.chunk.write(OpCode::Null, stmt.span.line);
                }
                self.chunk.write(OpCode::Return, stmt.span.line);
            }
            StatementNode::If { condition, body, otherwise } => {
                self.compile_expression(condition)?;
                let if_jump = self.chunk.emit_jump(OpCode::JumpIfFalse(0), stmt.span.line);
                self.chunk.write(OpCode::Pop, stmt.span.line);
                
                let block_stmt = crate::ast::Block { statements: body.statements.clone() };
                self.compile_block(&block_stmt)?;
                
                let else_jump = self.chunk.emit_jump(OpCode::Jump(0), stmt.span.line);
                
                self.chunk.patch_jump(if_jump);
                self.chunk.write(OpCode::Pop, stmt.span.line);
                
                if let Some(else_branch) = otherwise {
                    self.compile_statement(else_branch)?;
                }
                
                self.chunk.patch_jump(else_jump);
            }
            StatementNode::FunctionDeclaration { name, params, body, .. } => {
                let mut function_compiler = Compiler::new();
                for (param_name, _) in params {
                    function_compiler.locals.push(param_name.clone());
                }
                let block_stmt = crate::ast::Block { statements: body.statements.clone() };
                function_compiler.compile_block(&block_stmt)?;
                function_compiler.chunk.write(OpCode::Null, stmt.span.line);
                function_compiler.chunk.write(OpCode::Return, stmt.span.line);
                
                let chunk = function_compiler.chunk;
                let func_val = Value::CompiledFunction {
                    chunk: std::sync::Arc::new(chunk),
                    arity: params.len(),
                };
                
                let idx = self.chunk.add_constant(func_val);
                self.chunk.write(OpCode::Constant(idx), stmt.span.line);
                let name_idx = self.chunk.add_constant(Value::String(name.clone()));
                
                if self.scope_depth > 0 {
                    self.locals.push(name.clone());
                } else {
                    self.chunk.write(OpCode::SetGlobal(name_idx), stmt.span.line);
                    self.chunk.write(OpCode::Pop, stmt.span.line);
                }
            }
            StatementNode::While { condition, body } => {
                let loop_start = self.chunk.code.len();
                self.compile_expression(condition)?;
                let exit_jump = self.chunk.emit_jump(OpCode::JumpIfFalse(0), stmt.span.line);
                self.chunk.write(OpCode::Pop, stmt.span.line);
                
                let block_stmt = crate::ast::Block { statements: body.statements.clone() };
                self.compile_block(&block_stmt)?;
                
                let loop_jump = self.chunk.emit_jump(OpCode::Loop(0), stmt.span.line);
                let offset = self.chunk.code.len() - loop_start;
                if let OpCode::Loop(ref mut j) = self.chunk.code[loop_jump] {
                    *j = offset;
                }
                
                self.chunk.patch_jump(exit_jump);
                self.chunk.write(OpCode::Pop, stmt.span.line);
            }
            StatementNode::IndexAssignment { target, index, value } => {
                self.compile_expression(target)?;
                self.compile_expression(index)?;
                self.compile_expression(value)?;
                self.chunk.write(OpCode::IndexSet, stmt.span.line);
            }
            StatementNode::PropertyAssignment { target, property, value } => {
                self.compile_expression(target)?;
                self.compile_expression(value)?;
                let prop_idx = self.chunk.add_constant(Value::String(property.clone()));
                self.chunk.write(OpCode::PropertySet(prop_idx), stmt.span.line);
            }
            StatementNode::ClassDef { name, methods } => {
                let mut method_map = HashMap::new();
                for m in methods {
                    if let StatementNode::FunctionDeclaration { name: ref m_name, ref params, ref body, .. } = m.node {
                        let mut function_compiler = Compiler::new();
                        for (param_name, _) in params {
                            function_compiler.locals.push(param_name.clone());
                        }
                        let block_stmt = crate::ast::Block { statements: body.statements.clone() };
                        function_compiler.compile_block(&block_stmt)?;
                        function_compiler.chunk.write(OpCode::Null, m.span.line);
                        function_compiler.chunk.write(OpCode::Return, m.span.line);
                        
                        let func_val = Value::CompiledFunction {
                            chunk: std::sync::Arc::new(function_compiler.chunk),
                            arity: params.len(),
                        };
                        method_map.insert(m_name.clone(), func_val);
                    }
                }
                
                let class_val = Value::Class {
                    name: name.clone(),
                    methods: method_map,
                };
                
                let idx = self.chunk.add_constant(class_val);
                self.chunk.write(OpCode::Constant(idx), stmt.span.line);
                let name_idx = self.chunk.add_constant(Value::String(name.clone()));
                self.chunk.write(OpCode::SetGlobal(name_idx), stmt.span.line);
                self.chunk.write(OpCode::Pop, stmt.span.line);
            }
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                let catch_jump = self.chunk.emit_jump(OpCode::TryStart(0), stmt.span.line);
                
                let t_block = crate::ast::Block { statements: try_block.statements.clone() };
                self.compile_block(&t_block)?;
                
                self.chunk.write(OpCode::TryEnd, stmt.span.line);
                let end_jump = self.chunk.emit_jump(OpCode::Jump(0), stmt.span.line);
                
                self.chunk.patch_jump(catch_jump);
                
                self.scope_depth += 1;
                self.locals.push(catch_var.clone());
                // The exception value is pushed onto the stack by the VM when jumping to catch block.
                let c_block = crate::ast::Block { statements: catch_block.statements.clone() };
                self.compile_block(&c_block)?;
                self.scope_depth -= 1;
                self.locals.pop();
                self.chunk.write(OpCode::Pop, stmt.span.line); // Pop the exception value
                
                self.chunk.patch_jump(end_jump);
            }
            StatementNode::UsePython { module, alias } => {
                let mod_idx = self.chunk.add_constant(Value::String(module.clone()));
                self.chunk.write(OpCode::ImportPython(mod_idx), stmt.span.line);
                let name = alias.as_ref().unwrap_or(module).clone();
                let name_idx = self.chunk.add_constant(Value::String(name));
                self.chunk.write(OpCode::SetGlobal(name_idx), stmt.span.line);
                self.chunk.write(OpCode::Pop, stmt.span.line);
            }
            StatementNode::UseModule { path: _path, alias: _alias } => {
                return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node));
            }
            StatementNode::Save { data: _data, destination: _destination } => {
                return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node));
            }
            StatementNode::Break => {
                return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node));
            }
            StatementNode::Continue => {
                return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node));
            }
            _ => return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node)),
        }
        Ok(())
    }

    fn compile_block(&mut self, block: &crate::ast::Block) -> Result<(), String> {
        self.scope_depth += 1;
        let locals_count = self.locals.len();
        
        for s in &block.statements {
            self.compile_statement(s)?;
        }
        
        self.scope_depth -= 1;
        while self.locals.len() > locals_count {
            self.locals.pop();
            // Assuming block ending doesn't have line number accessible directly without tracking
            self.chunk.write(OpCode::Pop, 0); 
        }
        Ok(())
    }

    fn compile_expression(&mut self, expr: &Expression) -> Result<(), String> {
        match &expr.node {
            ExpressionNode::Integer(i) => {
                let idx = self.chunk.add_constant(Value::Integer(*i));
                self.chunk.write(OpCode::Constant(idx), expr.span.line);
            }
            ExpressionNode::Float(f) => {
                let idx = self.chunk.add_constant(Value::Float(*f));
                self.chunk.write(OpCode::Constant(idx), expr.span.line);
            }
            ExpressionNode::String(s) => {
                let idx = self.chunk.add_constant(Value::String(s.clone()));
                self.chunk.write(OpCode::Constant(idx), expr.span.line);
            }
            ExpressionNode::Boolean(b) => {
                if *b {
                    self.chunk.write(OpCode::True, expr.span.line);
                } else {
                    self.chunk.write(OpCode::False, expr.span.line);
                }
            }
            ExpressionNode::BinaryOp { left, operator, right } => {
                self.compile_expression(left)?;
                self.compile_expression(right)?;
                match operator.as_str() {
                    "+" => self.chunk.write(OpCode::Add, expr.span.line),
                    "-" => self.chunk.write(OpCode::Subtract, expr.span.line),
                    "*" => self.chunk.write(OpCode::Multiply, expr.span.line),
                    "/" => self.chunk.write(OpCode::Divide, expr.span.line),
                    "%" => self.chunk.write(OpCode::Modulo, expr.span.line),
                    "==" => self.chunk.write(OpCode::Equal, expr.span.line),
                    "!=" => self.chunk.write(OpCode::NotEqual, expr.span.line),
                    ">" => self.chunk.write(OpCode::Greater, expr.span.line),
                    "<" => self.chunk.write(OpCode::Less, expr.span.line),
                    ">=" => self.chunk.write(OpCode::GreaterEqual, expr.span.line),
                    "<=" => self.chunk.write(OpCode::LessEqual, expr.span.line),
                    _ => return Err(format!("Operator {} not fully implemented", operator)),
                }
            }
            ExpressionNode::Identifier(name) => {
                if let Some(pos) = self.locals.iter().rposition(|l| l == name) {
                    self.chunk.write(OpCode::GetLocal(pos), expr.span.line);
                } else {
                    let idx = self.chunk.add_constant(Value::String(name.clone()));
                    self.chunk.write(OpCode::GetGlobal(idx), expr.span.line);
                }
            }
            ExpressionNode::Call { callee, arguments } => {
                self.compile_expression(callee)?;
                for arg in arguments {
                    self.compile_expression(arg)?;
                }
                self.chunk.write(OpCode::Call(arguments.len()), expr.span.line);
            }
            ExpressionNode::List(elements) => {
                for el in elements {
                    self.compile_expression(el)?;
                }
                self.chunk.write(OpCode::BuildList(elements.len()), expr.span.line);
            }
            ExpressionNode::Map(elements) => {
                for (k, v) in elements {
                    let k_idx = self.chunk.add_constant(Value::String(k.clone()));
                    self.chunk.write(OpCode::Constant(k_idx), expr.span.line);
                    self.compile_expression(v)?;
                }
                self.chunk.write(OpCode::BuildMap(elements.len()), expr.span.line);
            }
            ExpressionNode::IndexAccess { target, index } => {
                self.compile_expression(target)?;
                self.compile_expression(index)?;
                self.chunk.write(OpCode::IndexGet, expr.span.line);
            }
            ExpressionNode::PropertyAccess { target, property } => {
                self.compile_expression(target)?;
                let prop_idx = self.chunk.add_constant(Value::String(property.clone()));
                self.chunk.write(OpCode::PropertyGet(prop_idx), expr.span.line);
            }
            ExpressionNode::Cast(target, type_name) => {
                self.compile_expression(target)?;
                let type_idx = self.chunk.add_constant(Value::String(type_name.clone()));
                self.chunk.write(OpCode::Cast(type_idx), expr.span.line);
            }
            ExpressionNode::Read(target) => {
                self.compile_expression(target)?;
                self.chunk.write(OpCode::Read, expr.span.line);
            }
            ExpressionNode::Get(target) => {
                self.compile_expression(target)?;
                self.chunk.write(OpCode::Get, expr.span.line);
            }
            ExpressionNode::Aggregate { target, mapping, op } => {
                self.compile_expression(target)?;
                self.chunk.write(OpCode::PushContextItem, expr.span.line);
                self.compile_expression(mapping)?;
                self.chunk.write(OpCode::PopContextItem, expr.span.line);
                
                match op.as_str() {
                    "sum" => self.chunk.write(OpCode::AggregateSum, expr.span.line),
                    "average" => self.chunk.write(OpCode::AggregateAverage, expr.span.line),
                    "max" => self.chunk.write(OpCode::AggregateMax, expr.span.line),
                    "min" => self.chunk.write(OpCode::AggregateMin, expr.span.line),
                    _ => return Err(format!("Unknown aggregate operator: {}", op)),
                }
            }
            ExpressionNode::Where { target: _target, condition: _condition } => {
                // Not fully mapped to real VM OpCodes natively yet, but we'll emit what we can
                return Err(format!("Expression type not fully implemented in compiler yet: {:?}", expr.node));
            }
            ExpressionNode::Select { target: _target, mapping: _mapping } => {
                return Err(format!("Expression type not fully implemented in compiler yet: {:?}", expr.node));
            }
            ExpressionNode::Concurrent(_block) => {
                return Err(format!("Expression type not fully implemented in compiler yet: {:?}", expr.node));
            }
            ExpressionNode::StringInterpolation(parts) => {
                for part in parts {
                    self.compile_expression(part)?;
                }
                self.chunk.write(OpCode::StringInterpolate(parts.len()), expr.span.line);
            }
            ExpressionNode::Closure { params: _params, body: _body } => {
                return Err(format!("Expression type not fully implemented in compiler yet: {:?}", expr.node));
            }
            ExpressionNode::Null => {
                self.chunk.write(OpCode::Null, expr.span.line);
            }
            ExpressionNode::ImportPython(module) => {
                let mod_idx = self.chunk.add_constant(Value::String(module.clone()));
                self.chunk.write(OpCode::ImportPython(mod_idx), expr.span.line);
            }
        }
        Ok(())
    }
}
