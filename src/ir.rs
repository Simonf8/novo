
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
    
    /// Cast value to type
    Cast(ValueId, Type),
    
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
