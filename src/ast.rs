
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

impl Expression {
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = span;
        self
    }
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

#[allow(dead_code)]
impl Statement {
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = span;
        self
    }
}

impl From<StatementNode> for Statement {
    fn from(node: StatementNode) -> Self {
        Statement { node, span: Span::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionNode {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Identifier(String),
    List(Vec<Expression>),
    Concurrent(Box<Expression>),
    Cast(Box<Expression>, String),
    Read(Box<Expression>),
    Get(Box<Expression>),
    Map(Vec<(String, Expression)>),
    IndexAccess {
        target: Box<Expression>,
        index: Box<Expression>,
    },
    PropertyAccess {
        target: Box<Expression>,
        property: String,
    },
    Where {
        target: Box<Expression>,
        condition: Box<Expression>,
    },
    Select {
        target: Box<Expression>,
        mapping: Box<Expression>,
    },
    Aggregate {
        target: Box<Expression>,
        mapping: Box<Expression>,
        op: String,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    BinaryOp {
        left: Box<Expression>,
        operator: String,
        right: Box<Expression>,
    },
    StringInterpolation(Vec<Expression>),
    Closure {
        params: Vec<String>,
        body: Box<Expression>, // Or Block if multi-line, but let's stick to Expression for now
    },
    ImportPython(String),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementNode {
    Assignment {
        name: String,
        value: Expression,
        type_annotation: Option<String>,
    },
    IndexAssignment {
        target: Expression,
        index: Expression,
        value: Expression,
    },
    PropertyAssignment {
        target: Expression,
        property: String,
        value: Expression,
    },
    Show {
        value: Expression,
    },
    If {
        condition: Expression,
        body: Block,
        otherwise: Option<Box<Statement>>,
    },
    Repeat {
        count: Expression,
        body: Block,
    },
    FunctionDeclaration {
        name: String,
        params: Vec<(String, Option<String>)>,
        return_type: Option<String>,
        body: Block,
    },
    UsePython {
        module: String,
        alias: Option<String>,
    },
    Return {
        value: Option<Expression>,
    },
    Save {
        data: Expression,
        destination: Expression,
    },
    ClassDef {
        name: String,
        methods: Vec<Statement>, // Actually a list of FunctionDeclarations
    },
    TryCatch {
        try_block: Block,
        catch_var: String,
        catch_block: Block,
    },
    While {
        condition: Expression,
        body: Block,
    },
    Break,
    Continue,
    UseModule {
        path: String,
        alias: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
}
