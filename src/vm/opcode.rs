#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    Constant(usize),
    Pop,
    
    GetLocal(usize),
    SetLocal(usize),
    GetGlobal(usize),
    SetGlobal(usize),
    PushContextItem,
    PopContextItem,
    
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    
    True,
    False,
    Null,
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
    AggregateSum,
    AggregateAverage,
    AggregateMax,
    AggregateMin,
    TryStart(usize),
    TryEnd,
}
