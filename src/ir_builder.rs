
use crate::ast::{Expression, ExpressionNode, Program, Statement, StatementNode};
use crate::ir::*;

pub struct IRBuilder {
    pub functions: Vec<FunctionIR>,
    current_func: usize,
    current_block: BlockId,
    locals: Vec<String>,
    scope_depth: usize,
}

impl IRBuilder {
    pub fn new() -> Self {
        let main_func = FunctionIR::new("main".to_string());
        let mut builder = IRBuilder {
            functions: vec![main_func],
            current_func: 0,
            current_block: BlockId(0),
            locals: Vec::new(),
            scope_depth: 0,
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

        self.optimize();

        for func in &mut self.functions {
            crate::ir_ssa::convert_to_ssa(func);
        }

        self.optimize();

        Ok(self.functions)
    }

    fn optimize(&mut self) {
        // Simple constant folding pass
        for func in &mut self.functions {
            let mut constants = std::collections::HashMap::new();
            
            for (_, block) in func.blocks.iter_mut() {
                for (val_id, inst) in &mut block.instructions {
                    match inst {
                        Instruction::Constant(c) => {
                            constants.insert(*val_id, c.clone());
                        }
                        Instruction::Add(lhs, rhs) => {
                            if let (Some(l), Some(r)) = (constants.get(lhs), constants.get(rhs)) {
                                if let (crate::interpreter::Value::Integer(li), crate::interpreter::Value::Integer(ri)) = (l, r) {
                                    let result = crate::interpreter::Value::Integer(li + ri);
                                    constants.insert(*val_id, result.clone());
                                    *inst = Instruction::Constant(result);
                                }
                            }
                        }
                        Instruction::Sub(lhs, rhs) => {
                            if let (Some(l), Some(r)) = (constants.get(lhs), constants.get(rhs)) {
                                if let (crate::interpreter::Value::Integer(li), crate::interpreter::Value::Integer(ri)) = (l, r) {
                                    let result = crate::interpreter::Value::Integer(li - ri);
                                    constants.insert(*val_id, result.clone());
                                    *inst = Instruction::Constant(result);
                                }
                            }
                        }
                        Instruction::Mul(lhs, rhs) => {
                            if let (Some(l), Some(r)) = (constants.get(lhs), constants.get(rhs)) {
                                if let (crate::interpreter::Value::Integer(li), crate::interpreter::Value::Integer(ri)) = (l, r) {
                                    let result = crate::interpreter::Value::Integer(li * ri);
                                    constants.insert(*val_id, result.clone());
                                    *inst = Instruction::Constant(result);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            
            // Dead Code Elimination
            let mut changed = true;
            while changed {
                changed = false;
                let mut used = std::collections::HashSet::new();
                
                for (_, block) in func.blocks.iter() {
                    for (_, inst) in &block.instructions {
                        match inst {
                            Instruction::Add(l, r) | Instruction::Sub(l, r) | 
                            Instruction::Mul(l, r) | Instruction::Div(l, r) | 
                            Instruction::Mod(l, r) | Instruction::Eq(l, r) | 
                            Instruction::Neq(l, r) | Instruction::Lt(l, r) | 
                            Instruction::Le(l, r) | Instruction::Gt(l, r) | 
                            Instruction::Ge(l, r) | Instruction::IndexGet(l, r) => {
                                used.insert(*l);
                                used.insert(*r);
                            }
                            Instruction::IndexSet(obj, idx, val) => {
                                used.insert(*obj);
                                used.insert(*idx);
                                used.insert(*val);
                            }
                            Instruction::Call(callee, args) => {
                                used.insert(*callee);
                                for a in args { used.insert(*a); }
                            }
                            Instruction::GetProperty(obj, _) | Instruction::Spawn(obj) | 
                            Instruction::Await(obj) | Instruction::StoreLocal(_, obj) | 
                            Instruction::StoreGlobal(_, obj) | Instruction::Cast(obj, _) |
                            Instruction::Print(obj) => {
                                used.insert(*obj);
                            }
                            Instruction::PropertySet(obj, _, val) => {
                                used.insert(*obj);
                                used.insert(*val);
                            }
                            Instruction::BuildList(args) => {
                                for a in args { used.insert(*a); }
                            }
                            Instruction::BuildMap(args) => {
                                for (k, v) in args {
                                    used.insert(*k);
                                    used.insert(*v);
                                }
                            }
                            Instruction::Phi(args) => {
                                for (_, v) in args { used.insert(*v); }
                            }
                            _ => {}
                        }
                    }
                    match &block.terminator {
                        Terminator::CondBranch { condition, .. } => { used.insert(*condition); },
                        Terminator::Return(Some(v)) => { used.insert(*v); },
                        _ => {}
                    }
                }
                
                for (_, block) in func.blocks.iter_mut() {
                    let mut new_insts = Vec::new();
                    for (val_id, inst) in block.instructions.drain(..) {
                        let is_side_effect = matches!(inst, 
                            Instruction::StoreLocal(_, _) | Instruction::StoreGlobal(_, _) |
                            Instruction::Call(_, _) | Instruction::Spawn(_) |
                            Instruction::IndexSet(_, _, _) | Instruction::PropertySet(_, _, _) |
                            Instruction::MakeFunction(_, _) | Instruction::Print(_)
                        );
                        
                        if used.contains(&val_id) || is_side_effect {
                            new_insts.push((val_id, inst));
                        } else {
                            changed = true;
                        }
                    }
                    block.instructions = new_insts;
                }
            }
        }
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
                if self.scope_depth > 0 {
                    if !self.locals.contains(name) {
                        self.locals.push(name.clone());
                    }
                    self.emit(Instruction::StoreLocal(name.clone(), val_id));
                } else {
                    self.emit(Instruction::StoreGlobal(name.clone(), val_id));
                }
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
                self.scope_depth += 1;
                for s in &body.statements {
                    self.build_statement(s)?;
                }
                self.scope_depth -= 1;
                
                let func = &mut self.functions[self.current_func];
                let true_end_block = func.blocks.get_mut(&self.current_block).unwrap();
                if let Terminator::Unreachable = true_end_block.terminator {
                    true_end_block.terminator = Terminator::Branch(merge_block);
                }
                
                if let Some(other_body) = otherwise {
                    self.current_block = false_block;
                    self.scope_depth += 1;
                    self.build_statement(other_body)?;
                    self.scope_depth -= 1;
                    
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
                self.scope_depth += 1;
                for s in &body.statements {
                    self.build_statement(s)?;
                }
                self.scope_depth -= 1;
                
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
                let old_scope = self.scope_depth;
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                self.scope_depth = 1;
                
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
                self.scope_depth = old_scope;
                
                let val_id = self.emit(Instruction::MakeFunction(func_idx, vec![]));
                
                if self.scope_depth > 0 {
                    if !self.locals.contains(name) {
                        self.locals.push(name.clone());
                    }
                    self.emit(Instruction::StoreLocal(name.clone(), val_id));
                } else {
                    self.emit(Instruction::StoreGlobal(name.clone(), val_id));
                }
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
                let old_scope = self.scope_depth;
                
                self.current_func = func_idx;
                self.current_block = entry;
                self.locals.clear();
                self.scope_depth = 1;
                
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
                self.scope_depth = old_scope;
                
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
            _ => Err("Expression not supported in IR yet".to_string()),
        }
    }
}
