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
                    self.locals.push(name.clone());
                    // Local variables live on the stack, no instruction needed to define,
                    // they are just left on the stack after the expression evaluates!
                } else {
                    let idx = self.chunk.add_constant(Value::String(name.clone()));
                    self.chunk.write(OpCode::SetGlobal(idx), stmt.span.line);
                    self.chunk.write(OpCode::Pop, stmt.span.line); // SetGlobal leaves value on stack? Or removes it? Wait, let's keep it popped.
                }
            }
            _ => return Err(format!("Statement type not fully implemented in compiler yet: {:?}", stmt.node)),
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
            _ => return Err(format!("Expression type not fully implemented in compiler yet: {:?}", expr.node)),
        }
        Ok(())
    }
}
