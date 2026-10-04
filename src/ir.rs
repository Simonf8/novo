
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Function(Vec<Type>, Box<Type>),
    Promise(Box<Type>),
    Any,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// Load a constant
    Constant(crate::interpreter::Value),
    
    /// Binary operations
    Add(ValueId, ValueId),
    Sub(ValueId, ValueId),
    Mul(ValueId, ValueId),
    Div(ValueId, ValueId),
    Mod(ValueId, ValueId),
    
    /// Comparisons
    Eq(ValueId, ValueId),
    Neq(ValueId, ValueId),
    Lt(ValueId, ValueId),
    Le(ValueId, ValueId),
    Gt(ValueId, ValueId),
    Ge(ValueId, ValueId),

    /// Call a function or builtin
    Call(ValueId, Vec<ValueId>),
    
    /// Read property or method
    GetProperty(ValueId, String),
    
    /// Spawn concurrent task
    Spawn(ValueId),
    
    /// Await a promise
    Await(ValueId),
    
    /// Variable access (before full SSA conversion)
    LoadLocal(String),
    StoreLocal(String, ValueId),
    LoadGlobal(String),
    StoreGlobal(String, ValueId),
    /// Object allocation
    BuildList(Vec<ValueId>),
    BuildMap(Vec<(ValueId, ValueId)>),
    MakeFunction(usize, Vec<String>), // function index, captures
    
    /// Properties & Indexing
    IndexGet(ValueId, ValueId),
    IndexSet(ValueId, ValueId, ValueId),
    PropertySet(ValueId, String, ValueId),
    
    /// Cast value to type
    Cast(ValueId, Type),
    
    /// Print value to standard output
    Print(ValueId),
    
    /// Phi node for SSA
    Phi(Vec<(BlockId, ValueId)>),
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<(ValueId, Instruction)>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Branch(BlockId),
    CondBranch {
        condition: ValueId,
        true_block: BlockId,
        false_block: BlockId,
    },
    Return(Option<ValueId>),
    Unreachable,
}

#[derive(Debug)]
pub struct FunctionIR {
    pub name: String,
    pub params: Vec<ValueId>,
    pub blocks: HashMap<BlockId, BasicBlock>,
    pub entry_block: BlockId,
    next_value_id: usize,
    next_block_id: usize,
}

impl FunctionIR {
    pub fn new(name: String) -> Self {
        Self {
            name,
            params: Vec::new(),
            blocks: HashMap::new(),
            entry_block: BlockId(0),
            next_value_id: 0,
            next_block_id: 0,
        }
    }
    
    pub fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        self.blocks.insert(id, BasicBlock {
            id,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        id
    }
    
    pub fn new_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value_id);
        self.next_value_id += 1;
        id
    }
}

impl std::fmt::Display for ValueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::Constant(c) => write!(f, "const {:?}", c),
            Instruction::Add(l, r) => write!(f, "add {}, {}", l, r),
            Instruction::Sub(l, r) => write!(f, "sub {}, {}", l, r),
            Instruction::Mul(l, r) => write!(f, "mul {}, {}", l, r),
            Instruction::Div(l, r) => write!(f, "div {}, {}", l, r),
            Instruction::Mod(l, r) => write!(f, "mod {}, {}", l, r),
            Instruction::Eq(l, r) => write!(f, "eq {}, {}", l, r),
            Instruction::Neq(l, r) => write!(f, "neq {}, {}", l, r),
            Instruction::Lt(l, r) => write!(f, "lt {}, {}", l, r),
            Instruction::Le(l, r) => write!(f, "le {}, {}", l, r),
            Instruction::Gt(l, r) => write!(f, "gt {}, {}", l, r),
            Instruction::Ge(l, r) => write!(f, "ge {}, {}", l, r),
            Instruction::Call(callee, args) => {
                write!(f, "call {}(", callee)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            },
            Instruction::GetProperty(obj, prop) => write!(f, "getprop {}, \"{}\"", obj, prop),
            Instruction::Spawn(val) => write!(f, "spawn {}", val),
            Instruction::Await(val) => write!(f, "await {}", val),
            Instruction::LoadLocal(name) => write!(f, "loadlocal \"{}\"", name),
            Instruction::StoreLocal(name, val) => write!(f, "storelocal \"{}\", {}", name, val),
            Instruction::LoadGlobal(name) => write!(f, "loadglobal \"{}\"", name),
            Instruction::StoreGlobal(name, val) => write!(f, "storeglobal \"{}\", {}", name, val),
            Instruction::BuildList(args) => {
                write!(f, "buildlist [")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, "]")
            },
            Instruction::BuildMap(_) => write!(f, "buildmap ..."),
            Instruction::MakeFunction(idx, _) => write!(f, "makefunc func{}", idx),
            Instruction::IndexGet(obj, idx) => write!(f, "indexget {}, {}", obj, idx),
            Instruction::IndexSet(obj, idx, val) => write!(f, "indexset {}, {} = {}", obj, idx, val),
            Instruction::PropertySet(obj, prop, val) => write!(f, "propset {}, \"{}\" = {}", obj, prop, val),
            Instruction::Cast(val, ty) => write!(f, "cast {}, {:?}", val, ty),
            Instruction::Print(val) => write!(f, "print {}", val),
            Instruction::Phi(args) => {
                write!(f, "phi [")?;
                for (i, (b, v)) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: {}", b, v)?;
                }
                write!(f, "]")
            }
        }
    }
}

impl std::fmt::Display for Terminator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Terminator::Branch(b) => write!(f, "br {}", b),
            Terminator::CondBranch { condition, true_block, false_block } => {
                write!(f, "br_if {}, {}, {}", condition, true_block, false_block)
            }
            Terminator::Return(Some(v)) => write!(f, "ret {}", v),
            Terminator::Return(None) => write!(f, "ret void"),
            Terminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

impl std::fmt::Display for FunctionIR {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "func {}() {{", self.name)?;
        
        let mut sorted_blocks: Vec<_> = self.blocks.values().collect();
        sorted_blocks.sort_by_key(|b| b.id.0);
        
        for block in sorted_blocks {
            writeln!(f, "{}:", block.id)?;
            for (val_id, inst) in &block.instructions {
                writeln!(f, "  {} = {}", val_id, inst)?;
            }
            writeln!(f, "  {}", block.terminator)?;
        }
        writeln!(f, "}}")
    }
}
