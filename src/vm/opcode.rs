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
    
    AddInt,
    SubtractInt,
    MultiplyInt,
    
    True,
    False,
    Null,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterEqual,
    LessEqual,

    EqualInt,
    NotEqualInt,
    LessInt,
    LessEqualInt,
    GreaterInt,
    GreaterEqualInt,
    
    JumpIfFalse(usize),
    Jump(usize),
    Loop(usize),
    
    Call(usize),
    Return,
    
    ImportPython(usize),
    
    BuildList(usize),
    BuildMap(usize),
    IndexGet,
    IndexSet,
    PropertyGet(usize),
    PropertySet(usize),
    
    Read,
    Get,
    Cast(usize),
    StringInterpolate(usize),

    Print,
    AggregateSum(usize),
    AggregateAverage(usize),
    AggregateMax(usize),
    AggregateMin(usize),
    Select(usize),
    Where(usize),
    TryStart(usize),
    TryEnd,
    SpawnConcurrent(usize),
    Await,
}
