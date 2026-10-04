#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    Constant(usize),
    Pop,
    
    GetLocal(usize),
    SetLocal(usize),
    GetGlobal(usize),
    SetGlobal(usize),
    
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Negate,
    
    True,
    False,
    Null,
    Not,
    
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterEqual,
    LessEqual,
    
    JumpIfFalse(usize),
    Jump(usize),
    Loop(usize),
    
    Call(usize),
    Return,
    
    BuildList(usize),
    BuildMap(usize),
    IndexGet,
    IndexSet,
    PropertyGet(usize),
    PropertySet(usize),
    
    Print,
}
