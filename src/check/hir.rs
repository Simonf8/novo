//! The checked program: names resolved to slots, types attached to every
//! expression. Engines run this, never the surface AST.

use super::builtins::Builtin;
use super::types::{ClassId, Ty};
use crate::source::Span;
use crate::syntax::ast::{BinOp, QueryOp, UnaryOp};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone)]
pub struct Program {
    pub funcs: Vec<Func>,
    pub classes: Vec<Class>,
    pub globals: Vec<Global>,
    pub modules: Vec<Module>,
    pub entry: ModuleId,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub init: FuncId,
}

#[derive(Debug, Clone)]
pub struct Global {
    pub name: String,
    pub ty: Ty,
    /// Where it is first assigned.
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    /// Where it is declared; empty for locals the checker introduces.
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    /// Type variables the function is generic over; calls record a type for each.
    pub generics: Vec<u32>,
    /// The first `params` locals hold the arguments.
    pub params: usize,
    /// Default values for trailing parameters, used when a call leaves them out.
    pub defaults: Vec<Option<Expr>>,
    /// Locals that receive captured values, in capture order, after the parameters.
    pub captures: Vec<LocalId>,
    pub locals: Vec<Local>,
    pub body: Block,
    pub ty: Ty,
    /// The function's name in its definition.
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Class {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub methods: Vec<(String, FuncId)>,
    /// A variant of an enum: shown without field names and compared by value.
    pub variant: bool,
}

impl Class {
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    pub fn method(&self, name: &str) -> Option<FuncId> {
        self.methods
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| *f)
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
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
        place: Place,
        op: Option<BinOp>,
        value: Expr,
    },
    Show(Vec<Expr>),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Block,
    },
    While {
        cond: Expr,
        body: Block,
    },
    For {
        var: Place,
        iter: Expr,
        body: Block,
    },
    Repeat {
        count: Expr,
        it: LocalId,
        body: Block,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Try {
        body: Block,
        catches: Vec<Catch>,
    },
    InitModule(ModuleId),
}

/// A `catch` clause. With no classes it catches every error and stores its
/// message; otherwise it catches errors raised with an instance of one of
/// `classes` and stores that instance.
#[derive(Debug, Clone)]
pub struct Catch {
    pub classes: Vec<ClassId>,
    pub var: Place,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub enum Place {
    Local(LocalId),
    Global(GlobalId),
    Field { target: Expr, name: String },
    Index { target: Expr, index: Expr },
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FStrPart {
    Lit(String),
    Expr { expr: Expr, spec: Option<String> },
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Arc<str>),
    None,
    FStr(Vec<FStrPart>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Local(LocalId),
    Global(GlobalId),
    /// A named function used as a value, with the types chosen for its generics.
    Func {
        func: FuncId,
        inst: Vec<Ty>,
    },
    /// A closure over the current values of `captures`.
    Closure {
        func: FuncId,
        captures: Vec<Expr>,
    },
    /// The closure currently executing, used by nested functions to recurse.
    Recurse,
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Calls a function value.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Calls a known function directly; for methods the receiver is the first argument.
    CallFunc {
        func: FuncId,
        args: Vec<Expr>,
        inst: Vec<Ty>,
        native: NativeSlot,
    },
    CallBuiltin {
        builtin: Builtin,
        args: Vec<Expr>,
    },
    /// A method resolved from the receiver's run-time value.
    MethodCall {
        recv: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    New {
        class: ClassId,
        args: Vec<Expr>,
    },
    Field {
        target: Box<Expr>,
        name: String,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Slice {
        target: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    /// When `parallel` is set the body is a one-parameter closure applied to each item.
    Query {
        op: QueryOp,
        target: Box<Expr>,
        item: LocalId,
        body: Option<Box<Expr>>,
        parallel: bool,
    },
    /// Runs a zero-argument closure on another thread.
    Together(Box<Expr>),
    Await(Box<Expr>),
    /// Calls each zero-argument closure on its own thread and collects the results.
    RunParallel(Vec<Expr>),
    IntToFloat(Box<Expr>),
    PyImport(String),
    IfElse {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    /// Whether a value of an enum is the given variant.
    IsVariant {
        value: Box<Expr>,
        variant: ClassId,
    },
}

/// Per-call-site cache of the native code compiled for the callee, filled on first use.
#[derive(Debug, Clone, Default)]
pub struct NativeSlot(pub std::sync::Arc<std::sync::OnceLock<Option<crate::native::Entry>>>);
