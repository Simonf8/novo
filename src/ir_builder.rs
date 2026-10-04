
use crate::ast::{Expression, ExpressionNode, Program, Statement, StatementNode};
use crate::ir::*;

pub struct IRBuilder {
    pub functions: Vec<FunctionIR>,
    current_func: usize,
    current_block: BlockId,
    locals: Vec<String>,
}

impl IRBuilder {
    pub fn new() -> Self {
        let main_func = FunctionIR::new("main".to_string());
        let mut builder = IRBuilder {
            functions: vec![main_func],
            current_func: 0,
            current_block: BlockId(0),
            locals: Vec::new(),
        };
        builder.current_block = builder.functions[0].new_block();
        builder.functions[0].entry_block = builder.current_block;
        builder
    }

    pub fn build(mut self, program: &Program) -> Result<Vec<FunctionIR>, String> {
        for stmt in &program.statements {
            self.build_statement(stmt)?;
        }
        
        let cur_func = &mut self.functions[self.current_func];
        let cur_block = &mut cur_func.blocks.get_mut(&self.current_block).unwrap();
        if let Terminator::Unreachable = cur_block.terminator {
            cur_block.terminator = Terminator::Return(None);
        }

        for func in &mut self.functions {
            crate::ir_ssa::convert_to_ssa(func);
            crate::ir_opt::optimize(func);
        }

        Ok(self.functions)
    }

    fn emit(&mut self, inst: Instruction) -> ValueId {
        let func = &mut self.functions[self.current_func];
        let val = func.new_value();
        let block = func.blocks.get_mut(&self.current_block).unwrap();
        block.instructions.push((val, inst));
        val
    }

    fn build_statement(&mut self, stmt: &Statement) -> Result<(), String> {
        match &stmt.node {
            StatementNode::Assignment { name, value, .. } => {
                let val_id = self.build_expression(value)?;
                if !self.locals.contains(name) {
                    self.locals.push(name.clone());
                }
                self.emit(Instruction::StoreLocal(name.clone(), val_id));
            }
            StatementNode::Return { value } => {
                let val_id = if let Some(expr) = value {
                    Some(self.build_expression(expr)?)
                } else {
                    None
                };
                let func = &mut self.functions[self.current_func];
                let block = func.blocks.get_mut(&self.current_block).unwrap();
                block.terminator = Terminator::Return(val_id);
                // Create a new unreachable block for any subsequent statements
                self.current_block = func.new_block();
            }
            StatementNode::If { condition, body, otherwise } => {
                let cond_id = self.build_expression(condition)?;
                
                let func = &mut self.functions[self.current_func];
                let true_block = func.new_block();
                let false_block = func.new_block();
                let merge_block = func.new_block();
                
                let current_block_ref = func.blocks.get_mut(&self.current_block).unwrap();
                current_block_ref.terminator = Terminator::CondBranch {
                    condition: cond_id,
                    true_block,
                    false_block,
                };
                
                self.current_block = true_block;
                for s in &body.statements {
                    self.build_statement(s)?;
                }
                
                let func = &mut self.functions[self.current_func];
                let true_end_block = func.blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = true_end_block.terminator {
                    true_end_block.terminator = Terminator::Branch(merge_block);
                }
                
                if let Some(other_body) = otherwise {
                    self.current_block = false_block;
                    self.build_statement(other_body)?;
                    
                    let func = &mut self.functions[self.current_func];
                    let false_end_block = func.blocks.get_mut(&self.current_block).unwrap();
                    if let Terminator::Unreachable = false_end_block.terminator {
                        false_end_block.terminator = Terminator::Branch(merge_block);
                    }
                } else {
                    let func = &mut self.functions[self.current_func];
                    let false_block_ref = func.blocks.get_mut(&false_block).unwrap();
                    false_block_ref.terminator = Terminator::Branch(merge_block);
                }
                
                self.current_block = merge_block;
            }
            StatementNode::While { condition, body } => {
                let func = &mut self.functions[self.current_func];
                let cond_block = func.new_block();
                let body_block = func.new_block();
                let end_block = func.new_block();
                
                let current_block_ref = func.blocks.get_mut(&self.current_block).unwrap();
                current_block_ref.terminator = Terminator::Branch(cond_block);
                
                self.current_block = cond_block;
                let cond_id = self.build_expression(condition)?;
                
                let func = &mut self.functions[self.current_func];
                let cond_block_ref = func.blocks.get_mut(&self.current_block).unwrap();
                cond_block_ref.terminator = Terminator::CondBranch {
                    condition: cond_id,
                    true_block: body_block,
                    false_block: end_block,
                };
                
                self.current_block = body_block;
                for s in &body.statements {
                    self.build_statement(s)?;
                }
                
                let func = &mut self.functions[self.current_func];
                let body_end_block = func.blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = body_end_block.terminator {
                    body_end_block.terminator = Terminator::Branch(cond_block);
                }
                
                self.current_block = end_block;
            }
            StatementNode::Show { value } => {
                let val_id = self.build_expression(value)?;
                self.emit(Instruction::Print(val_id));
            }
            StatementNode::FunctionDeclaration { name, params, body, .. } => {
                let func_idx = self.functions.len();
                let mut new_func = FunctionIR::new(name.clone());
                for (_p, _) in params {
                    let p_id = new_func.new_value();
                    new_func.params.push(p_id);
                }
                let entry = new_func.new_block();
                new_func.entry_block = entry;
                self.functions.push(new_func);
                
                // Switch context to compile the function body
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                for (i, (p, _)) in params.iter().enumerate() {
                    self.locals.push(p.clone());
                    let p_id = self.functions[func_idx].params[i];
                    self.emit(Instruction::StoreLocal(p.clone(), p_id));
                }
                
                for s in &body.statements {
                    self.build_statement(s)?;
                }
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = cur_block.terminator {
                    cur_block.terminator = Terminator::Return(None);
                }
                
                // Restore context
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                let val_id = self.emit(Instruction::MakeFunction(func_idx, vec![]));
                
                if self.current_func == 0 {
                    self.emit(Instruction::StoreGlobal(name.clone(), val_id));
                } else {
                    if !self.locals.contains(name) {
                        self.locals.push(name.clone());
                    }
                    self.emit(Instruction::StoreLocal(name.clone(), val_id));
                }
            }
            StatementNode::UsePython { module, alias } => {
                let val_id = self.emit(Instruction::ImportPython(module.clone()));
                let name = alias.as_ref().unwrap_or(module).clone();
                self.emit(Instruction::StoreGlobal(name, val_id));
            }
            StatementNode::IndexAssignment { target, index, value } => {
                let t_id = self.build_expression(target)?;
                let i_id = self.build_expression(index)?;
                let v_id = self.build_expression(value)?;
                self.emit(Instruction::IndexSet(t_id, i_id, v_id));
            }
            StatementNode::PropertyAssignment { target, property, value } => {
                let t_id = self.build_expression(target)?;
                let v_id = self.build_expression(value)?;
                self.emit(Instruction::PropertySet(t_id, property.clone(), v_id));
            }
            StatementNode::Repeat { count, body } => {
                let count_val = self.build_expression(count)?;
                let zero = self.emit(Instruction::Constant(crate::interpreter::Value::Integer(0)));
                let one = self.emit(Instruction::Constant(crate::interpreter::Value::Integer(1)));
                let counter_name = format!("__repeat_i_{}", self.functions[self.current_func].new_value().0);

                self.locals.push(counter_name.clone());
                self.emit(Instruction::StoreLocal(counter_name.clone(), zero));

                let func = &mut self.functions[self.current_func];
                let cond_block = func.new_block();
                let body_block = func.new_block();
                let end_block = func.new_block();

                let cur = func.blocks.get_mut(&self.current_block).unwrap();
                cur.terminator = Terminator::Branch(cond_block);

                self.current_block = cond_block;
                let i_val = self.emit(Instruction::LoadLocal(counter_name.clone()));
                let cond_id = self.emit(Instruction::Lt(i_val, count_val));
                let cond_block_ref = self.functions[self.current_func].blocks.get_mut(&self.current_block).unwrap();
                cond_block_ref.terminator = Terminator::CondBranch {
                    condition: cond_id,
                    true_block: body_block,
                    false_block: end_block,
                };

                self.current_block = body_block;
                for s in &body.statements {
                    self.build_statement(s)?;
                }

                let cur_i = self.emit(Instruction::LoadLocal(counter_name.clone()));
                let next_i = self.emit(Instruction::Add(cur_i, one));
                self.emit(Instruction::StoreLocal(counter_name.clone(), next_i));

                let body_end = self.functions[self.current_func].blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = body_end.terminator {
                    body_end.terminator = Terminator::Branch(cond_block);
                }

                self.current_block = end_block;
            }
            StatementNode::ClassDef { name, methods } => {
                let mut method_indices = Vec::new();
                for m in methods {
                    if let StatementNode::FunctionDeclaration { name: ref m_name, ref params, ref body, .. } = m.node {
                        let func_idx = self.functions.len();
                        let mut new_func = FunctionIR::new(format!("{}::{}", name, m_name));
                        for (_p, _) in params {
                            let p_id = new_func.new_value();
                            new_func.params.push(p_id);
                        }
                        let entry = new_func.new_block();
                        new_func.entry_block = entry;
                        self.functions.push(new_func);

                        let old_func = self.current_func;
                        let old_block = self.current_block;
                        let old_locals = self.locals.clone();

                        self.current_func = func_idx;
                        self.current_block = entry;
                        self.locals.clear();

                        for (i, (p, _)) in params.iter().enumerate() {
                            self.locals.push(p.clone());
                            let p_id = self.functions[func_idx].params[i];
                            self.emit(Instruction::StoreLocal(p.clone(), p_id));
                        }

                        for s in &body.statements {
                            self.build_statement(s)?;
                        }

                        let cur_func = &mut self.functions[self.current_func];
                        let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                        if let Terminator::Unreachable = cur_block.terminator {
                            cur_block.terminator = Terminator::Return(None);
                        }

                        self.current_func = old_func;
                        self.current_block = old_block;
                        self.locals = old_locals;

                        method_indices.push((m_name.clone(), func_idx));
                    }
                }
                let class_id = self.emit(Instruction::MakeClass(name.clone(), method_indices));
                self.emit(Instruction::StoreGlobal(name.clone(), class_id));
            }
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                let func = &mut self.functions[self.current_func];
                let try_body_block = func.new_block();
                let catch_body_block = func.new_block();
                let merge_block = func.new_block();

                // Branch to try_body_block
                let cur = func.blocks.get_mut(&self.current_block).unwrap();
                cur.terminator = Terminator::Branch(try_body_block);

                // Build try block
                self.current_block = try_body_block;
                self.emit(Instruction::TryStart(catch_body_block));

                for s in &try_block.statements {
                    self.build_statement(s)?;
                }

                self.emit(Instruction::TryEnd);
                let try_end = self.functions[self.current_func].blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = try_end.terminator {
                    try_end.terminator = Terminator::Branch(merge_block);
                }

                // Build catch block
                self.current_block = catch_body_block;
                let err_id = self.emit(Instruction::CatchException);
                if !self.locals.contains(catch_var) {
                    self.locals.push(catch_var.clone());
                }
                self.emit(Instruction::StoreLocal(catch_var.clone(), err_id));

                for s in &catch_block.statements {
                    self.build_statement(s)?;
                }

                let catch_end = self.functions[self.current_func].blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = catch_end.terminator {
                    catch_end.terminator = Terminator::Branch(merge_block);
                }

                self.current_block = merge_block;
            }
            _ => {
                // Ignore unsupported statements for now
            }
        }
        Ok(())
    }

    fn build_expression(&mut self, expr: &Expression) -> Result<ValueId, String> {
        match &expr.node {
            ExpressionNode::Integer(i) => {
                Ok(self.emit(Instruction::Constant(crate::interpreter::Value::Integer(*i))))
            }
            ExpressionNode::Float(f) => {
                Ok(self.emit(Instruction::Constant(crate::interpreter::Value::Float(*f))))
            }
            ExpressionNode::String(s) => {
                Ok(self.emit(Instruction::Constant(crate::interpreter::Value::String(s.clone()))))
            }
            ExpressionNode::Boolean(b) => {
                Ok(self.emit(Instruction::Constant(crate::interpreter::Value::Boolean(*b))))
            }
            ExpressionNode::Identifier(name) => {
                if self.locals.contains(name) {
                    Ok(self.emit(Instruction::LoadLocal(name.clone())))
                } else {
                    Ok(self.emit(Instruction::LoadGlobal(name.clone())))
                }
            }
            ExpressionNode::Call { callee, arguments } => {
                let callee_id = self.build_expression(callee)?;
                let mut arg_ids = Vec::new();
                for arg in arguments {
                    arg_ids.push(self.build_expression(arg)?);
                }
                Ok(self.emit(Instruction::Call(callee_id, arg_ids)))
            }
            ExpressionNode::PropertyAccess { target, property } => {
                let target_id = self.build_expression(target)?;
                Ok(self.emit(Instruction::GetProperty(target_id, property.clone())))
            }
            ExpressionNode::IndexAccess { target, index } => {
                let target_id = self.build_expression(target)?;
                let index_id = self.build_expression(index)?;
                Ok(self.emit(Instruction::IndexGet(target_id, index_id)))
            }
            ExpressionNode::List(elements) => {
                let mut el_ids = Vec::new();
                for el in elements {
                    el_ids.push(self.build_expression(el)?);
                }
                Ok(self.emit(Instruction::BuildList(el_ids)))
            }
            ExpressionNode::Map(elements) => {
                let mut el_ids = Vec::new();
                for (k, v) in elements {
                    let k_id = self.emit(Instruction::Constant(crate::interpreter::Value::String(k.clone())));
                    let v_id = self.build_expression(v)?;
                    el_ids.push((k_id, v_id));
                }
                Ok(self.emit(Instruction::BuildMap(el_ids)))
            }
            ExpressionNode::Closure { params, body } => {
                let func_idx = self.functions.len();
                let mut new_func = FunctionIR::new("closure".to_string());
                for _ in params {
                    let p_id = new_func.new_value();
                    new_func.params.push(p_id);
                }
                let entry = new_func.new_block();
                new_func.entry_block = entry;
                self.functions.push(new_func);
                
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                for (i, p) in params.iter().enumerate() {
                    self.locals.push(p.clone());
                    let p_id = self.functions[func_idx].params[i];
                    self.emit(Instruction::StoreLocal(p.clone(), p_id));
                }
                
                let body_id = self.build_expression(body)?;
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                cur_block.terminator = Terminator::Return(Some(body_id));
                
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                Ok(self.emit(Instruction::MakeFunction(func_idx, vec![]))) // empty captures for now
            }
            ExpressionNode::BinaryOp { left, operator, right } => {
                let lhs = self.build_expression(left)?;
                let rhs = self.build_expression(right)?;
                match operator.as_str() {
                    "+" => Ok(self.emit(Instruction::Add(lhs, rhs))),
                    "-" => Ok(self.emit(Instruction::Sub(lhs, rhs))),
                    "*" => Ok(self.emit(Instruction::Mul(lhs, rhs))),
                    "/" => Ok(self.emit(Instruction::Div(lhs, rhs))),
                    "%" => Ok(self.emit(Instruction::Mod(lhs, rhs))),
                    "==" => Ok(self.emit(Instruction::Eq(lhs, rhs))),
                    "!=" => Ok(self.emit(Instruction::Neq(lhs, rhs))),
                    "<" => Ok(self.emit(Instruction::Lt(lhs, rhs))),
                    "<=" => Ok(self.emit(Instruction::Le(lhs, rhs))),
                    ">" => Ok(self.emit(Instruction::Gt(lhs, rhs))),
                    ">=" => Ok(self.emit(Instruction::Ge(lhs, rhs))),
                    _ => Err(format!("Unsupported operator: {}", operator)),
                }
            }
            ExpressionNode::Cast(target, type_name) => {
                let target_id = self.build_expression(target)?;
                Ok(self.emit(Instruction::Cast(target_id, type_name.clone())))
            }
            ExpressionNode::Read(target) => {
                let target_id = self.build_expression(target)?;
                Ok(self.emit(Instruction::Read(target_id)))
            }
            ExpressionNode::Get(target) => {
                let target_id = self.build_expression(target)?;
                Ok(self.emit(Instruction::Get(target_id)))
            }
            ExpressionNode::Where { target, condition } => {
                let target_id = self.build_expression(target)?;
                let func_idx = self.functions.len();
                let mut helper_func = FunctionIR::new(format!("where_pred_{}", func_idx));
                let entry = helper_func.new_block();
                helper_func.entry_block = entry;
                self.functions.push(helper_func);
                
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                let cond_id = self.build_expression(condition)?;
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                cur_block.terminator = Terminator::Return(Some(cond_id));
                
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                Ok(self.emit(Instruction::Where(target_id, func_idx)))
            }
            ExpressionNode::Select { target, mapping } => {
                let target_id = self.build_expression(target)?;
                let func_idx = self.functions.len();
                let mut helper_func = FunctionIR::new(format!("select_map_{}", func_idx));
                let entry = helper_func.new_block();
                helper_func.entry_block = entry;
                self.functions.push(helper_func);
                
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                let map_id = self.build_expression(mapping)?;
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                cur_block.terminator = Terminator::Return(Some(map_id));
                
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                Ok(self.emit(Instruction::Select(target_id, func_idx)))
            }
            ExpressionNode::Aggregate { target, mapping, op } => {
                let target_id = self.build_expression(target)?;
                let func_idx = self.functions.len();
                let mut helper_func = FunctionIR::new(format!("aggregate_map_{}", func_idx));
                let entry = helper_func.new_block();
                helper_func.entry_block = entry;
                self.functions.push(helper_func);
                
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                let map_id = self.build_expression(mapping)?;
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                cur_block.terminator = Terminator::Return(Some(map_id));
                
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                Ok(self.emit(Instruction::Aggregate(target_id, func_idx, op.clone())))
            }
            ExpressionNode::StringInterpolation(parts) => {
                let mut part_ids = Vec::new();
                for p in parts {
                    part_ids.push(self.build_expression(p)?);
                }
                Ok(self.emit(Instruction::StringInterpolate(part_ids)))
            }
            ExpressionNode::Concurrent(expr_in) => {
                let func_idx = self.functions.len();
                let mut helper_func = FunctionIR::new(format!("task_{}", func_idx));
                let entry = helper_func.new_block();
                helper_func.entry_block = entry;
                self.functions.push(helper_func);
                
                let old_func = self.current_func;
                let old_block = self.current_block;
                let old_locals = self.locals.clone();
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                
                let task_id = self.build_expression(expr_in)?;
                
                let cur_func = &mut self.functions[self.current_func];
                let cur_block = cur_func.blocks.get_mut(&self.current_block).unwrap();
                cur_block.terminator = Terminator::Return(Some(task_id));
                
                self.current_func = old_func;
                self.current_block = old_block;
                self.locals = old_locals;
                
                Ok(self.emit(Instruction::Spawn(func_idx)))
            }
            ExpressionNode::Await(expr) => {
                let promise_id = self.build_expression(expr)?;
                Ok(self.emit(Instruction::Await(promise_id)))
            }
        }
    }
}
