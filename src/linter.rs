use std::collections::HashMap;
use crate::ast::{Block, Expression, ExpressionNode, Program, Span, Statement, StatementNode};

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum LintSeverity {
    Warning,
    Error,
    Hint,
}

#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    pub span: Span,
    pub message: String,
    pub severity: LintSeverity,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone)]
struct VarInfo {
    span: Span,
    read: bool,
    assigned_count: usize,
}

pub struct Linter {
    scopes: Vec<HashMap<String, VarInfo>>,
    diagnostics: Vec<LintDiagnostic>,
}

impl Linter {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            diagnostics: Vec::new(),
        }
    }

    pub fn lint_program(&mut self, program: &Program) -> Vec<LintDiagnostic> {
        self.diagnostics.clear();
        self.scopes = vec![HashMap::new()];

        for stmt in &program.statements {
            self.lint_statement(stmt);
        }

        self.pop_scope();
        self.diagnostics.clone()
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            for (name, info) in scope {
                if !name.starts_with('_') && !info.read && name != "it" && name != "item" {
                    self.diagnostics.push(LintDiagnostic {
                        span: info.span,
                        message: format!("Variable '{}' is assigned but its value is never used", name),
                        severity: LintSeverity::Warning,
                        suggestion: Some(format!("Prefix with an underscore '_{}' if unused by design", name)),
                    });
                }
            }
        }
    }

    fn record_def(&mut self, name: &str, span: &Span) {
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(existing) = scope.get_mut(name) {
                existing.assigned_count += 1;
            } else {
                scope.insert(
                    name.to_string(),
                    VarInfo {
                        span: span.clone(),
                        read: false,
                        assigned_count: 1,
                    },
                );
            }
        }
    }

    fn record_use(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.read = true;
                return;
            }
        }
    }

    fn lint_statement(&mut self, stmt: &Statement) {
        match &stmt.node {
            StatementNode::Assignment { name, value, .. } => {
                // Check self-assignment
                if let ExpressionNode::Identifier(rhs_id) = &value.node {
                    if rhs_id == name {
                        self.diagnostics.push(LintDiagnostic {
                            span: stmt.span.clone(),
                            message: format!("Redundant self-assignment '{} = {}'", name, rhs_id),
                            severity: LintSeverity::Warning,
                            suggestion: Some("Remove this redundant assignment".to_string()),
                        });
                    }
                }

                self.lint_expression(value);
                self.record_def(name, &stmt.span);
            }
            StatementNode::IndexAssignment { target, index, value } => {
                self.lint_expression(target);
                self.lint_expression(index);
                self.lint_expression(value);
            }
            StatementNode::PropertyAssignment { target, value, .. } => {
                self.lint_expression(target);
                self.lint_expression(value);
            }
            StatementNode::Show { value } => {
                self.lint_expression(value);
            }
            StatementNode::If { condition, body, otherwise } => {
                // Check constant boolean condition
                if let ExpressionNode::Boolean(b) = &condition.node {
                    self.diagnostics.push(LintDiagnostic {
                        span: condition.span.clone(),
                        message: format!("Condition is always '{}'", b),
                        severity: LintSeverity::Hint,
                        suggestion: if *b {
                            Some("Remove the condition and keep the block directly".to_string())
                        } else {
                            Some("This block will never execute".to_string())
                        },
                    });
                }

                self.lint_expression(condition);
                self.lint_block(body);

                if let Some(other) = otherwise {
                    self.lint_statement(other);
                }
            }
            StatementNode::Repeat { count, body } => {
                self.lint_expression(count);
                self.lint_block(body);
            }
            StatementNode::While { condition, body } => {
                if let ExpressionNode::Boolean(false) = &condition.node {
                    self.diagnostics.push(LintDiagnostic {
                        span: condition.span.clone(),
                        message: "While condition is always 'false'".to_string(),
                        severity: LintSeverity::Warning,
                        suggestion: Some("This loop will never execute; consider removing it".to_string()),
                    });
                }
                self.lint_expression(condition);
                self.lint_block(body);
            }
            StatementNode::FunctionDeclaration { name, params, body, is_task, .. } => {
                self.record_def(name, &stmt.span);
                self.record_use(name); // function definition counts as declared
                self.push_scope();

                for (param_name, _) in params {
                    self.record_def(param_name, &stmt.span);
                }

                self.lint_block(body);
                self.pop_scope();

                if body.statements.is_empty() {
                    let kind = if *is_task { "Task" } else { "Function" };
                    self.diagnostics.push(LintDiagnostic {
                        span: stmt.span.clone(),
                        message: format!("{} '{}' has an empty body", kind, name),
                        severity: LintSeverity::Hint,
                        suggestion: Some("Add an implementation or return a default value".to_string()),
                    });
                }
            }
            StatementNode::Return { value } => {
                if let Some(val) = value {
                    self.lint_expression(val);
                }
            }
            StatementNode::Save { data, destination } => {
                self.lint_expression(data);
                self.lint_expression(destination);
            }
            StatementNode::ClassDef { methods, .. } => {
                for m in methods {
                    self.lint_statement(m);
                }
            }
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                self.lint_block(try_block);
                self.push_scope();
                self.record_def(catch_var, &stmt.span);
                self.lint_block(catch_block);
                self.pop_scope();
            }
            StatementNode::Break | StatementNode::Continue => {}
            StatementNode::UsePython { .. } | StatementNode::UseModule { .. } => {}
        }
    }

    fn lint_block(&mut self, block: &Block) {
        self.push_scope();
        let mut unreachable = false;

        for stmt in &block.statements {
            if unreachable {
                self.diagnostics.push(LintDiagnostic {
                    span: stmt.span.clone(),
                    message: "Unreachable code detected".to_string(),
                    severity: LintSeverity::Warning,
                    suggestion: Some("Remove or restructure code after unconditional return/break/continue".to_string()),
                });
            }

            self.lint_statement(stmt);

            match &stmt.node {
                StatementNode::Return { .. } | StatementNode::Break | StatementNode::Continue => {
                    unreachable = true;
                }
                _ => {}
            }
        }

        self.pop_scope();
    }

    fn lint_expression(&mut self, expr: &Expression) {
        match &expr.node {
            ExpressionNode::Identifier(id) => {
                self.record_use(id);
            }
            ExpressionNode::BinaryOp { left, right, .. } => {
                self.lint_expression(left);
                self.lint_expression(right);
            }
            ExpressionNode::Call { callee, arguments } => {
                self.lint_expression(callee);
                for arg in arguments {
                    self.lint_expression(arg);
                }
            }
            ExpressionNode::List(items) => {
                for it in items {
                    self.lint_expression(it);
                }
            }
            ExpressionNode::Map(entries) => {
                for (_, val) in entries {
                    self.lint_expression(val);
                }
            }
            ExpressionNode::IndexAccess { target, index } => {
                self.lint_expression(target);
                self.lint_expression(index);
            }
            ExpressionNode::PropertyAccess { target, .. } => {
                self.lint_expression(target);
            }
            ExpressionNode::Where { target, condition } => {
                self.lint_expression(target);
                self.lint_expression(condition);
            }
            ExpressionNode::Select { target, mapping } => {
                self.lint_expression(target);
                self.lint_expression(mapping);
            }
            ExpressionNode::Aggregate { target, mapping, .. } => {
                self.lint_expression(target);
                self.lint_expression(mapping);
            }
            ExpressionNode::StringInterpolation(parts) => {
                for p in parts {
                    self.lint_expression(p);
                }
            }
            ExpressionNode::Closure { body, .. } => {
                self.lint_expression(body);
            }
            ExpressionNode::Concurrent(inner) => {
                self.lint_expression(inner);
            }
            ExpressionNode::RunInParallel(inner) => {
                self.lint_expression(inner);
            }
            ExpressionNode::Await(inner) => {
                self.lint_expression(inner);
            }
            ExpressionNode::Cast(inner, _)
            | ExpressionNode::Read(inner)
            | ExpressionNode::Get(inner) => {
                self.lint_expression(inner);
            }
            ExpressionNode::Integer(_)
            | ExpressionNode::Float(_)
            | ExpressionNode::String(_)
            | ExpressionNode::Boolean(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn test_unused_variable_and_unreachable_code() {
        let source = "
function test():
    a = 10
    b = 20
    return a
    c = 30
";
        let tokens = Lexer::new(source).tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut linter = Linter::new();
        let diags = linter.lint_program(&ast);

        assert!(diags.iter().any(|d| d.message.contains("Variable 'b' is assigned but its value is never used")));
        assert!(diags.iter().any(|d| d.message.contains("Unreachable code detected")));
    }
}

