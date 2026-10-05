use std::collections::HashMap;
use crate::ast::{Program, Statement, StatementNode, Expression, ExpressionNode};

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Function(Vec<Type>, Box<Type>),
    Promise(Box<Type>),
    Any,
    None,
}

impl Type {
    pub fn from_string(name: &str) -> Type {
        let trimmed = name.trim();
        match trimmed.to_lowercase().as_str() {
            "int" | "integer" => Type::Int,
            "float" => Type::Float,
            "string" | "str" => Type::String,
            "bool" | "boolean" => Type::Bool,
            "list" => Type::List(Box::new(Type::Any)),
            "map" => Type::Map(Box::new(Type::String), Box::new(Type::Any)),
            "any" => Type::Any,
            "none" | "void" => Type::None,
            _ => {
                let lower = trimmed.to_lowercase();
                if lower.starts_with("list[") && lower.ends_with(']') {
                    let inner = &trimmed[5..trimmed.len() - 1];
                    Type::List(Box::new(Type::from_string(inner)))
                } else if lower.starts_with("map[") && lower.ends_with(']') {
                    let inner = &trimmed[4..trimmed.len() - 1];
                    let parts: Vec<&str> = inner.split(',').collect();
                    if parts.len() == 2 {
                        Type::Map(Box::new(Type::from_string(parts[0].trim())), Box::new(Type::from_string(parts[1].trim())))
                    } else {
                        Type::Map(Box::new(Type::String), Box::new(Type::Any))
                    }
                } else {
                    Type::Any
                }
            },
        }
    }

    pub fn is_assignable_to(&self, other: &Type) -> bool {
        if other == &Type::Any || self == &Type::Any {
            return true;
        }
        match (self, other) {
            (Type::List(a), Type::List(b)) => a.is_assignable_to(b),
            (Type::Map(ka, va), Type::Map(kb, vb)) => ka.is_assignable_to(kb) && va.is_assignable_to(vb),
            (Type::Promise(a), Type::Promise(b)) => a.is_assignable_to(b),
            (Type::Function(pa, ra), Type::Function(pb, rb)) => {
                if pa.len() != pb.len() {
                    return false;
                }
                for (a, b) in pa.iter().zip(pb.iter()) {
                    if !a.is_assignable_to(b) {
                        return false;
                    }
                }
                ra.is_assignable_to(rb)
            },
            (a, b) => a == b,
        }
    }
}

pub struct TypeChecker {
    scopes: Vec<HashMap<String, Type>>,
    current_return_type: Option<Type>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut globals = HashMap::new();
        globals.insert("parse_json".to_string(), Type::Any);
        TypeChecker {
            scopes: vec![globals],
            current_return_type: None,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_variable(&mut self, name: String, ty: Type) {
        self.scopes.last_mut().unwrap().insert(name, ty);
    }

    fn lookup_variable(&self, name: &str) -> Type {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return ty.clone();
            }
        }
        Type::Any
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.statements {
            self.check_statement(stmt)?;
        }
        Ok(())
    }

    fn check_statement(&mut self, stmt: &Statement) -> Result<(), String> {
        match &stmt.node {
            StatementNode::Assignment { name, value, type_annotation } => {
                let mut existing_ty = None;
                for scope in self.scopes.iter().rev() {
                    if let Some(ty) = scope.get(name) {
                        existing_ty = Some(ty.clone());
                        break;
                    }
                }
                
                let val_ty = self.check_expression(value)?;
                
                if let Some(annot) = type_annotation {
                    let annot_ty = Type::from_string(annot);
                    if !val_ty.is_assignable_to(&annot_ty) && annot_ty != Type::Any {
                        return Err(format!("Type mismatch at line {}: cannot assign {:?} to {:?}", stmt.span.line, val_ty, annot_ty));
                    }
                    if let Some(exty) = existing_ty {
                        if !annot_ty.is_assignable_to(&exty) {
                            return Err(format!("Type mismatch at line {}: cannot redefine {} with type {:?}, expected {:?}", stmt.span.line, name, annot_ty, exty));
                        }
                    }
                    self.define_variable(name.clone(), annot_ty);
                } else {
                    if let Some(exty) = existing_ty {
                        if !val_ty.is_assignable_to(&exty) && exty != Type::Any {
                            return Err(format!("Type mismatch at line {}: cannot assign {:?} to {:?}", stmt.span.line, val_ty, exty));
                        }
                        self.define_variable(name.clone(), exty);
                    } else {
                        self.define_variable(name.clone(), val_ty);
                    }
                }
            }
            StatementNode::IndexAssignment { target, index, value } => {
                self.check_expression(target)?;
                self.check_expression(index)?;
                self.check_expression(value)?;
            }
            StatementNode::PropertyAssignment { target, value, .. } => {
                self.check_expression(target)?;
                self.check_expression(value)?;
            }
            StatementNode::Show { value } => {
                self.check_expression(value)?;
            }
            StatementNode::If { condition, body, otherwise } => {
                let cond_ty = self.check_expression(condition)?;
                if !cond_ty.is_assignable_to(&Type::Bool) {
                    return Err(format!("If condition must be a boolean at line {}", stmt.span.line));
                }
                self.push_scope();
                for s in &body.statements {
                    self.check_statement(s)?;
                }
                self.pop_scope();
                if let Some(other) = otherwise {
                    self.check_statement(other)?;
                }
            }
            StatementNode::Repeat { count, body } => {
                let count_ty = self.check_expression(count)?;
                if !count_ty.is_assignable_to(&Type::Int) {
                    return Err(format!("Repeat count must be an integer at line {}", stmt.span.line));
                }
                self.push_scope();
                for s in &body.statements {
                    self.check_statement(s)?;
                }
                self.pop_scope();
            }
            StatementNode::While { condition, body } => {
                let cond_ty = self.check_expression(condition)?;
                if !cond_ty.is_assignable_to(&Type::Bool) {
                    return Err(format!("While condition must be a boolean at line {}", stmt.span.line));
                }
                self.push_scope();
                for s in &body.statements {
                    self.check_statement(s)?;
                }
                self.pop_scope();
            }
            StatementNode::FunctionDeclaration { name, params, return_type, body, .. } => {
                let ret_ty = return_type.as_ref().map(|t| Type::from_string(t)).unwrap_or(Type::Any);
                let mut p_types = Vec::new();
                for (_, p_ty) in params.iter() {
                    let ty = p_ty.as_ref().map(|t| Type::from_string(t)).unwrap_or(Type::Any);
                    p_types.push(ty);
                }
                self.define_variable(name.clone(), Type::Function(p_types.clone(), Box::new(ret_ty.clone())));
                
                self.push_scope();
                let prev_ret_ty = self.current_return_type.clone();
                self.current_return_type = Some(ret_ty.clone());
                
                for (p_name, p_ty) in params {
                    let ty = p_ty.as_ref().map(|t| Type::from_string(t)).unwrap_or(Type::Any);
                    self.define_variable(p_name.clone(), ty);
                }
                
                for s in &body.statements {
                    self.check_statement(s)?;
                }
                
                self.current_return_type = prev_ret_ty;
                self.pop_scope();
            }
            StatementNode::Return { value } => {
                let val_ty = if let Some(v) = value {
                    self.check_expression(v)?
                } else {
                    Type::None
                };
                
                if let Some(expected_ret_ty) = &self.current_return_type {
                    if !val_ty.is_assignable_to(expected_ret_ty) {
                        return Err(format!("Return type mismatch at line {}: expected {:?}, got {:?}", stmt.span.line, expected_ret_ty, val_ty));
                    }
                }
            }
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                self.push_scope();
                for s in &try_block.statements {
                    self.check_statement(s)?;
                }
                self.pop_scope();
                
                self.push_scope();
                self.define_variable(catch_var.clone(), Type::Any);
                for s in &catch_block.statements {
                    self.check_statement(s)?;
                }
                self.pop_scope();
            }
            StatementNode::ClassDef { name, methods } => {
                self.define_variable(name.clone(), Type::Any);
                for m in methods {
                    self.check_statement(m)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn check_expression(&mut self, expr: &Expression) -> Result<Type, String> {
        match &expr.node {
            ExpressionNode::Integer(_) => Ok(Type::Int),
            ExpressionNode::Float(_) => Ok(Type::Float),
            ExpressionNode::String(_) => Ok(Type::String),
            ExpressionNode::Boolean(_) => Ok(Type::Bool),
            ExpressionNode::Identifier(name) => Ok(self.lookup_variable(name)),
            ExpressionNode::List(items) => {
                let mut list_ty = Type::Any;
                for item in items {
                    let ty = self.check_expression(item)?;
                    if list_ty == Type::Any {
                        list_ty = ty.clone();
                    }
                }
                Ok(Type::List(Box::new(list_ty)))
            }
            ExpressionNode::Concurrent(inner) => {
                let inner_ty = self.check_expression(inner)?;
                Ok(Type::Promise(Box::new(inner_ty)))
            }
            ExpressionNode::RunInParallel(inner) => {
                let inner_ty = self.check_expression(inner)?;
                Ok(inner_ty)
            }
            ExpressionNode::Map(items) => {
                let mut key_ty = Type::Any;
                let mut val_ty = Type::Any;
                for (_key, item) in items {
                    // Keys are usually strings in ast, but let's assume they are parsed as such
                    key_ty = Type::String;
                    let ty = self.check_expression(item)?;
                    if val_ty == Type::Any {
                        val_ty = ty.clone();
                    }
                }
                Ok(Type::Map(Box::new(key_ty), Box::new(val_ty)))
            }
            ExpressionNode::BinaryOp { left, operator, right } => {
                let l_ty = self.check_expression(left)?;
                let r_ty = self.check_expression(right)?;
                
                if operator == "==" || operator == "!=" {
                    return Ok(Type::Bool);
                }
                if operator == "+" || operator == "-" || operator == "*" || operator == "/" || operator == "%" {
                    if l_ty == Type::Int && r_ty == Type::Int {
                        return Ok(Type::Int);
                    }
                    if (l_ty == Type::Float || l_ty == Type::Int) && (r_ty == Type::Float || r_ty == Type::Int) {
                        return Ok(Type::Float);
                    }
                    if operator == "+" && (l_ty == Type::String || r_ty == Type::String) {
                        return Ok(Type::String);
                    }
                    if l_ty == Type::Any || r_ty == Type::Any {
                        return Ok(Type::Any);
                    }
                    return Err(format!("Invalid operator '{}' for types {:?} and {:?} at line {}", operator, l_ty, r_ty, expr.span.line));
                }
                if operator == ">" || operator == "<" || operator == ">=" || operator == "<=" {
                    if (l_ty == Type::Int || l_ty == Type::Float || l_ty == Type::Any) && (r_ty == Type::Int || r_ty == Type::Float || r_ty == Type::Any) {
                        return Ok(Type::Bool);
                    }
                    return Err(format!("Invalid operator '{}' for types {:?} and {:?} at line {}", operator, l_ty, r_ty, expr.span.line));
                }
                if operator == "and" || operator == "or" {
                    if (l_ty == Type::Bool || l_ty == Type::Any) && (r_ty == Type::Bool || r_ty == Type::Any) {
                        return Ok(Type::Bool);
                    }
                    return Err(format!("Logical operator '{}' requires booleans at line {}", operator, expr.span.line));
                }
                Ok(Type::Any)
            }
            ExpressionNode::Call { callee, arguments } => {
                let callee_ty = self.check_expression(callee)?;
                let mut arg_tys = Vec::new();
                for arg in arguments {
                    arg_tys.push(self.check_expression(arg)?);
                }
                match callee_ty {
                    Type::Function(param_tys, ret_ty) => {
                        if param_tys.len() != arg_tys.len() {
                            return Err(format!("Arity mismatch at line {}: expected {} arguments, got {}", expr.span.line, param_tys.len(), arg_tys.len()));
                        }
                        for (i, (p_ty, a_ty)) in param_tys.iter().zip(arg_tys.iter()).enumerate() {
                            if !a_ty.is_assignable_to(p_ty) {
                                return Err(format!("Type mismatch in argument {} at line {}: expected {:?}, got {:?}", i+1, expr.span.line, p_ty, a_ty));
                            }
                        }
                        Ok(*ret_ty)
                    }
                    Type::Any => Ok(Type::Any),
                    _ => Err(format!("Cannot call non-function type {:?} at line {}", callee_ty, expr.span.line)),
                }
            }
            ExpressionNode::IndexAccess { target, index } => {
                let target_ty = self.check_expression(target)?;
                let index_ty = self.check_expression(index)?;
                
                match target_ty {
                    Type::List(inner) => {
                        if !index_ty.is_assignable_to(&Type::Int) {
                            return Err(format!("List index must be an integer at line {}", expr.span.line));
                        }
                        Ok(*inner)
                    }
                    Type::Map(k, v) => {
                        if !index_ty.is_assignable_to(&k) {
                            return Err(format!("Map index type mismatch: expected {:?}, got {:?} at line {}", *k, index_ty, expr.span.line));
                        }
                        Ok(*v)
                    }
                    Type::Any => Ok(Type::Any),
                    _ => Err(format!("Cannot index into type {:?} at line {}", target_ty, expr.span.line)),
                }
            }
            ExpressionNode::PropertyAccess { target, property } => {
                let target_ty = self.check_expression(target)?;
                match target_ty {
                    Type::List(_inner) => {
                        if crate::stdlib::is_list_method(property) {
                            Ok(Type::Any)
                        } else {
                            Ok(Type::Any)
                        }
                    }
                    Type::String => {
                        if crate::stdlib::is_string_method(property) {
                            Ok(Type::Any)
                        } else {
                            Ok(Type::Any)
                        }
                    }
                    Type::Map(_, _) => {
                        if crate::stdlib::is_map_method(property) {
                            Ok(Type::Any)
                        } else {
                            Ok(Type::Any)
                        }
                    }
                    _ => Ok(Type::Any),
                }
            }
            ExpressionNode::Where { target, condition } => {
                let target_ty = self.check_expression(target)?;
                
                let item_ty = match &target_ty {
                    Type::List(inner) => *inner.clone(),
                    _ => Type::Any,
                };
                
                self.push_scope();
                self.define_variable("item".to_string(), item_ty);
                let cond_ty = self.check_expression(condition)?;
                self.pop_scope();
                
                if !cond_ty.is_assignable_to(&Type::Bool) {
                    return Err(format!("Where condition must be boolean at line {}", expr.span.line));
                }
                Ok(target_ty)
            }
            ExpressionNode::Select { target, mapping } |
            ExpressionNode::Aggregate { target, mapping, .. } => {
                let target_ty = self.check_expression(target)?;
                
                let item_ty = match &target_ty {
                    Type::List(inner) => *inner.clone(),
                    _ => Type::Any,
                };
                
                self.push_scope();
                self.define_variable("item".to_string(), item_ty);
                let map_ty = self.check_expression(mapping)?;
                self.pop_scope();
                
                if matches!(expr.node, ExpressionNode::Select { .. }) {
                    Ok(Type::List(Box::new(map_ty)))
                } else {
                    Ok(Type::Any) // Aggregates usually return numbers, but max/min could return map_ty
                }
            }
            ExpressionNode::StringInterpolation(exprs) => {
                for e in exprs {
                    self.check_expression(e)?;
                }
                Ok(Type::String)
            }
            ExpressionNode::Cast(target, type_name) => {
                self.check_expression(target)?;
                Ok(Type::from_string(type_name))
            }
            ExpressionNode::Read(target) | ExpressionNode::Get(target) => {
                self.check_expression(target)?;
                Ok(Type::String) // File contents and HTTP responses are strings
            }
            ExpressionNode::Closure { params, body } => {
                self.push_scope();
                let mut p_tys = Vec::new();
                for param in params {
                    self.define_variable(param.clone(), Type::Any);
                    p_tys.push(Type::Any);
                }
                let ret_ty = self.check_expression(body)?;
                self.pop_scope();
                Ok(Type::Function(p_tys, Box::new(ret_ty)))
            }
            ExpressionNode::Await(expr) => {
                let promise_type = self.check_expression(expr)?;
                match promise_type {
                    Type::Promise(inner) => Ok(*inner),
                    Type::Any => Ok(Type::Any),
                    _ => Err(format!("Await requires a Promise type, got {:?} at line {}", promise_type, expr.span.line)),
                }
            }
        }
    }
}
