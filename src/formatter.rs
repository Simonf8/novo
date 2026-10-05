use crate::ast::{Block, Expression, ExpressionNode, Program, Statement, StatementNode};

pub struct Formatter {
    indent_level: usize,
    output: String,
}

impl Formatter {
    pub fn new() -> Self {
        Self {
            indent_level: 0,
            output: String::new(),
        }
    }

    pub fn format_program(mut self, program: &Program) -> String {
        for (i, stmt) in program.statements.iter().enumerate() {
            if i > 0 {
                // Add blank line before top-level functions or classes
                if matches!(stmt.node, StatementNode::FunctionDeclaration { .. } | StatementNode::ClassDef { .. }) {
                    self.output.push('\n');
                }
            }
            self.format_statement(stmt);
            self.output.push('\n');
        }
        self.output
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent_level {
            self.output.push_str("    ");
        }
    }

    fn format_statement(&mut self, stmt: &Statement) {
        self.write_indent();
        match &stmt.node {
            StatementNode::Assignment { name, value, type_annotation } => {
                if let Some(annot) = type_annotation {
                    self.output.push_str(&format!("{}: {} = ", name, annot));
                } else {
                    self.output.push_str(&format!("{} = ", name));
                }
                self.format_expression(value);
            }
            StatementNode::IndexAssignment { target, index, value } => {
                self.format_expression(target);
                self.output.push('[');
                self.format_expression(index);
                self.output.push_str("] = ");
                self.format_expression(value);
            }
            StatementNode::PropertyAssignment { target, property, value } => {
                self.format_expression(target);
                self.output.push_str(&format!(".{} = ", property));
                self.format_expression(value);
            }
            StatementNode::Show { value } => {
                self.output.push_str("show ");
                self.format_expression(value);
            }
            StatementNode::If { condition, body, otherwise } => {
                self.output.push_str("if ");
                self.format_expression(condition);
                self.output.push_str(":\n");
                self.format_block(body);

                if let Some(other) = otherwise {
                    self.format_otherwise(other);
                }
            }
            StatementNode::Repeat { count, body } => {
                self.output.push_str("repeat ");
                self.format_expression(count);
                self.output.push_str(" times:\n");
                self.format_block(body);
            }
            StatementNode::While { condition, body } => {
                self.output.push_str("while ");
                self.format_expression(condition);
                self.output.push_str(":\n");
                self.format_block(body);
            }
            StatementNode::FunctionDeclaration { name, params, return_type, body, is_task } => {
                let kw = if *is_task { "task" } else { "function" };
                self.output.push_str(&format!("{} {}(", kw, name));
                for (i, (p_name, p_ty)) in params.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    if let Some(t) = p_ty {
                        self.output.push_str(&format!("{}: {}", p_name, t));
                    } else {
                        self.output.push_str(p_name);
                    }
                }
                self.output.push(')');
                if let Some(ret) = return_type {
                    self.output.push_str(&format!(" -> {}", ret));
                }
                self.output.push_str(":\n");
                self.format_block(body);
            }
            StatementNode::Return { value } => {
                if let Some(val) = value {
                    self.output.push_str("return ");
                    self.format_expression(val);
                } else {
                    self.output.push_str("return");
                }
            }
            StatementNode::Save { data, destination } => {
                self.output.push_str("save ");
                self.format_expression(data);
                self.output.push_str(" to ");
                self.format_expression(destination);
            }
            StatementNode::ClassDef { name, methods } => {
                self.output.push_str(&format!("class {}:\n", name));
                self.indent_level += 1;
                for m in methods {
                    self.format_statement(m);
                    self.output.push('\n');
                }
                self.indent_level -= 1;
            }
            StatementNode::TryCatch { try_block, catch_var, catch_block } => {
                self.output.push_str("try:\n");
                self.format_block(try_block);
                self.write_indent();
                self.output.push_str(&format!("catch {}:\n", catch_var));
                self.format_block(catch_block);
            }
            StatementNode::Break => {
                self.output.push_str("break");
            }
            StatementNode::Continue => {
                self.output.push_str("continue");
            }
            StatementNode::UsePython { module, alias } => {
                if let Some(al) = alias {
                    self.output.push_str(&format!("use python {} as {}", module, al));
                } else {
                    self.output.push_str(&format!("use python {}", module));
                }
            }
            StatementNode::UseModule { path, alias } => {
                if let Some(al) = alias {
                    self.output.push_str(&format!("use \"{}\" as {}", path, al));
                } else {
                    self.output.push_str(&format!("use \"{}\"", path));
                }
            }
        }
    }

    fn format_otherwise(&mut self, stmt: &Statement) {
        self.write_indent();
        if let StatementNode::If { condition, body, otherwise } = &stmt.node {
            self.output.push_str("otherwise if ");
            self.format_expression(condition);
            self.output.push_str(":\n");
            self.format_block(body);
            if let Some(next) = otherwise {
                self.format_otherwise(next);
            }
        } else if let StatementNode::Show { .. } | StatementNode::Assignment { .. } = &stmt.node {
            self.output.push_str("otherwise:\n");
            self.indent_level += 1;
            self.format_statement(stmt);
            self.output.push('\n');
            self.indent_level -= 1;
        } else {
            self.output.push_str("otherwise:\n");
            self.indent_level += 1;
            self.format_statement(stmt);
            self.output.push('\n');
            self.indent_level -= 1;
        }
    }

    fn format_block(&mut self, block: &Block) {
        self.indent_level += 1;
        for stmt in &block.statements {
            self.format_statement(stmt);
            self.output.push('\n');
        }
        self.indent_level -= 1;
    }

    fn format_expression(&mut self, expr: &Expression) {
        match &expr.node {
            ExpressionNode::Integer(i) => self.output.push_str(&i.to_string()),
            ExpressionNode::Float(f) => self.output.push_str(&f.to_string()),
            ExpressionNode::String(s) => self.output.push_str(&format!("\"{}\"", s)),
            ExpressionNode::Boolean(b) => self.output.push_str(&b.to_string()),
            ExpressionNode::Identifier(id) => self.output.push_str(id),
            ExpressionNode::List(items) => {
                self.output.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.format_expression(item);
                }
                self.output.push(']');
            }
            ExpressionNode::Map(entries) => {
                self.output.push('{');
                for (i, (key, val)) in entries.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.output.push_str(&format!("{}: ", key));
                    self.format_expression(val);
                }
                self.output.push('}');
            }
            ExpressionNode::IndexAccess { target, index } => {
                self.format_expression(target);
                self.output.push('[');
                self.format_expression(index);
                self.output.push(']');
            }
            ExpressionNode::PropertyAccess { target, property } => {
                self.format_expression(target);
                self.output.push_str(&format!(".{}", property));
            }
            ExpressionNode::Call { callee, arguments } => {
                self.format_expression(callee);
                self.output.push('(');
                for (i, arg) in arguments.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.format_expression(arg);
                }
                self.output.push(')');
            }
            ExpressionNode::BinaryOp { left, operator, right } => {
                self.format_expression(left);
                self.output.push_str(&format!(" {} ", operator));
                self.format_expression(right);
            }
            ExpressionNode::Where { target, condition } => {
                self.format_expression(target);
                self.output.push_str(" where ");
                self.format_expression(condition);
            }
            ExpressionNode::Select { target, mapping } => {
                self.format_expression(target);
                self.output.push_str(" select ");
                self.format_expression(mapping);
            }
            ExpressionNode::Aggregate { target, mapping, op } => {
                self.format_expression(target);
                self.output.push_str(&format!(" {} ", op));
                self.format_expression(mapping);
            }
            ExpressionNode::StringInterpolation(parts) => {
                self.output.push_str("$\"");
                for p in parts {
                    match &p.node {
                        ExpressionNode::String(s) => self.output.push_str(s),
                        _ => {
                            self.output.push('{');
                            self.format_expression(p);
                            self.output.push('}');
                        }
                    }
                }
                self.output.push('"');
            }
            ExpressionNode::Closure { params, body } => {
                self.output.push('|');
                self.output.push_str(&params.join(", "));
                self.output.push_str("| ");
                self.format_expression(body);
            }
            ExpressionNode::Concurrent(inner) => {
                self.output.push_str("together ");
                self.format_expression(inner);
            }
            ExpressionNode::RunInParallel(inner) => {
                self.output.push_str("run ");
                self.format_expression(inner);
                self.output.push_str(" in parallel");
            }
            ExpressionNode::Await(inner) => {
                self.output.push_str("await ");
                self.format_expression(inner);
            }
            ExpressionNode::Cast(inner, ty) => {
                self.format_expression(inner);
                self.output.push_str(&format!(" as {}", ty));
            }
            ExpressionNode::Read(inner) => {
                self.output.push_str("read ");
                self.format_expression(inner);
            }
            ExpressionNode::Get(inner) => {
                self.output.push_str("get ");
                self.format_expression(inner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn test_format_function_and_expression() {
        let source = "
function compute(a: int, b: int) -> int:
    return a + b
";
        let tokens = Lexer::new(source).tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let formatted = Formatter::new().format_program(&ast);
        assert!(formatted.contains("function compute(a: int, b: int) -> int:"));
        assert!(formatted.contains("    return a + b"));
    }
}

