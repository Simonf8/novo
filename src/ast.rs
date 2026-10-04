#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Assignment {
        name: String,
        value: Expression,
        type_annotation: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
}
