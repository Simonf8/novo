import re

def process_ast():
    with open('src/ast.rs', 'r') as f:
        content = f.read()

    span_def = """
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl Span {
    pub fn merge(&self, other: &Span) -> Span {
        if self.line == other.line {
            Span {
                line: self.line,
                column: self.column,
                length: (other.column + other.length) - self.column,
            }
        } else {
            self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub node: ExpressionNode,
    pub span: Span,
}

impl From<ExpressionNode> for Expression {
    fn from(node: ExpressionNode) -> Self {
        Expression { node, span: Span::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub node: StatementNode,
    pub span: Span,
}

impl From<StatementNode> for Statement {
    fn from(node: StatementNode) -> Self {
        Statement { node, span: Span::default() }
    }
}
"""

    content = content.replace("pub enum Expression {", "pub enum ExpressionNode {")
    content = content.replace("pub enum Statement {", "pub enum StatementNode {")
    content = content.replace("Box<Expression>", "Box<Expression>") # stays the same!
    
    with open('src/ast.rs', 'w') as f:
        f.write(span_def + "\n" + content)

def process_parser():
    with open('src/parser.rs', 'r') as f:
        content = f.read()
    
    # Replace Expression:: with ExpressionNode:: 
    # But wait! We need to add .into() if it's being returned or assigned to an Expression.
    # A simple regex for Ok(Expression::...) -> Ok(ExpressionNode::...).into() no, Ok(ExpressionNode::....into())
    
    # Let's just replace Expression:: with ExpressionNode::
    content = content.replace("Expression::", "ExpressionNode::")
    content = content.replace("Statement::", "StatementNode::")
    
    # Now we need to append .into() where it builds the node.
    # We can do this by regex or by hand. Actually, the rust compiler will tell us exactly where `.into()` is needed!
    # We can just write a script that runs `cargo check`, parses the errors, and inserts `.into()`!
    
    with open('src/parser.rs', 'w') as f:
        f.write(content)

def process_interpreter():
    with open('src/interpreter.rs', 'r') as f:
        content = f.read()
    
    # Replace match expr { with match &expr.node {
    content = content.replace("match expr {", "match &expr.node {")
    content = content.replace("match stmt {", "match &stmt.node {")
    content = content.replace("match &**target {", "match &target.node {")
    content = content.replace("match &**path_expr {", "match &path_expr.node {")
    
    content = content.replace("Expression::", "ExpressionNode::")
    content = content.replace("Statement::", "StatementNode::")
    
    with open('src/interpreter.rs', 'w') as f:
        f.write(content)

process_ast()
process_parser()
process_interpreter()
