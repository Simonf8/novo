use crate::source::Span;

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub stmts: Vec<Stmt>,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Expr(Expr),
    Assign {
        target: Expr,
        ty: Option<TypeExpr>,
        value: Expr,
    },
    AugAssign {
        target: Expr,
        op: BinOp,
        value: Expr,
    },
    Show(Vec<Expr>),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        count: Expr,
        body: Block,
    },
    For {
        vars: Vec<Ident>,
        iter: Expr,
        body: Block,
    },
    Function(FunctionDef),
    Class(ClassDef),
    Return(Option<Expr>),
    Break,
    Continue,
    Pass,
    Try {
        body: Block,
        var: Ident,
        handler: Block,
    },
    Use(Use),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: Ident,
    pub fields: Vec<FieldDef>,
    pub methods: Vec<FunctionDef>,
}

#[derive(Debug, Clone)]
pub enum Use {
    Module {
        name: Ident,
        alias: Option<Ident>,
    },
    File {
        path: String,
        alias: Option<Ident>,
    },
    Python {
        module: String,
        alias: Option<Ident>,
    },
}

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    MatMul,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    In,
}

impl BinOp {
    pub const ALL: &[BinOp] = &[
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::FloorDiv,
        BinOp::Mod,
        BinOp::Pow,
        BinOp::MatMul,
        BinOp::Eq,
        BinOp::Ne,
        BinOp::Lt,
        BinOp::Le,
        BinOp::Gt,
        BinOp::Ge,
        BinOp::And,
        BinOp::Or,
        BinOp::In,
    ];

    pub fn index(self) -> u32 {
        BinOp::ALL
            .iter()
            .position(|b| *b == self)
            .expect("listed in ALL") as u32
    }

    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::FloorDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
            BinOp::MatMul => "@",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::In => "in",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryOp {
    Where,
    Select,
    Sum,
    Average,
    Min,
    Max,
}

impl QueryOp {
    pub fn from_word(word: &str) -> Option<QueryOp> {
        Some(match word {
            "where" => QueryOp::Where,
            "select" => QueryOp::Select,
            "sum" => QueryOp::Sum,
            "average" => QueryOp::Average,
            "min" => QueryOp::Min,
            "max" => QueryOp::Max,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            QueryOp::Where => "where",
            QueryOp::Select => "select",
            QueryOp::Sum => "sum",
            QueryOp::Average => "average",
            QueryOp::Min => "min",
            QueryOp::Max => "max",
        }
    }
}

#[derive(Debug, Clone)]
pub enum FStrPart {
    Lit(String),
    Expr { expr: Expr, spec: Option<String> },
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum IndexItem {
    Single(Expr),
    Slice {
        start: Option<Expr>,
        end: Option<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FStrPart>),
    Bool(bool),
    None,
    Name(String),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Field {
        target: Box<Expr>,
        name: Ident,
    },
    Index {
        target: Box<Expr>,
        items: Vec<IndexItem>,
    },
    Lambda {
        params: Vec<Ident>,
        types: Vec<Option<TypeExpr>>,
        body: Box<Expr>,
    },
    Query {
        op: QueryOp,
        target: Box<Expr>,
        body: Option<Box<Expr>>,
        parallel: bool,
    },
    Together(Box<Expr>),
    Await(Box<Expr>),
    RunParallel(Vec<Expr>),
}
