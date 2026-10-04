use crate::ir::{FunctionIR, Instruction, Terminator, ValueId, BlockId};
use crate::vm::chunk::Chunk;
use crate::vm::opcode::OpCode;
use crate::interpreter::Value;
use std::collections::HashMap;

pub struct IRBackend {
    functions: Vec<FunctionIR>,
}

impl IRBackend {
    pub fn new(functions: Vec<FunctionIR>) -> Self {
        IRBackend { functions }
    }

    pub fn generate(self) -> Result<Chunk, String> {
        let mut main_chunk = Chunk::new();
        // Since main is always the first function if we didn't filter, wait... 
        // Wait, IRBuilder returns a list of functions, where main/top-level is which one?
        // Let's assume the top-level script is functions[0]
        
        let mut compiled_funcs = Vec::new();
        for func in &self.functions {
            compiled_funcs.push(self.compile_function(func)?);
        }
        
        // top-level is the last function? Wait, in compiler, top-level is what we return. 
        // Let's check ir_builder.rs: 0 is the top-level!
        if !compiled_funcs.is_empty() {
            main_chunk = compiled_funcs.remove(0);
        }

        Ok(main_chunk)
    }

    fn compile_function(&self, func: &FunctionIR) -> Result<Chunk, String> {
        let mut chunk = Chunk::new();
        let mut locals: Vec<String> = Vec::new();
        
        // Block jump tracking
        let mut block_offsets: HashMap<BlockId, usize> = HashMap::new();
        // forward jumps that need to be patched
        let mut pending_jumps: Vec<(usize, BlockId)> = Vec::new();

        // 1. Assign local slots for named variables
        // We can do this dynamically or pre-pass
        
        let mut value_to_slot: HashMap<ValueId, usize> = HashMap::new();
        
        // We will output instructions block by block
        let mut sorted_blocks: Vec<_> = func.blocks.values().collect();
        sorted_blocks.sort_by_key(|b| b.id.0); // simple linear ordering
        
        for block in sorted_blocks {
            block_offsets.insert(block.id, chunk.code.len());
            
            for (val_id, inst) in &block.instructions {
                match inst {
                    Instruction::Constant(c) => {
                        let idx = chunk.add_constant(c.clone());
                        chunk.write(OpCode::Constant(idx), 0);
                    }
                    Instruction::Add(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Add, 0);
                    }
                    Instruction::Sub(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Subtract, 0);
                    }
                    Instruction::Mul(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Multiply, 0);
                    }
                    Instruction::Div(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Divide, 0);
                    }
                    Instruction::Mod(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Modulo, 0);
                    }
                    Instruction::Eq(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Equal, 0);
                    }
                    Instruction::Neq(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::NotEqual, 0);
                    }
                    Instruction::Lt(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Less, 0);
                    }
                    Instruction::Le(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::LessEqual, 0);
                    }
                    Instruction::Gt(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::Greater, 0);
                    }
                    Instruction::Ge(l, r) => {
                        self.emit_load(&mut chunk, *l, &value_to_slot);
                        self.emit_load(&mut chunk, *r, &value_to_slot);
                        chunk.write(OpCode::GreaterEqual, 0);
                    }
                    Instruction::Call(callee, args) => {
                        self.emit_load(&mut chunk, *callee, &value_to_slot);
                        for a in args {
                            self.emit_load(&mut chunk, *a, &value_to_slot);
                        }
                        chunk.write(OpCode::Call(args.len()), 0);
                    }
                    Instruction::Print(val_id) => {
                        self.emit_load(&mut chunk, *val_id, &value_to_slot);
                        chunk.write(OpCode::Print, 0);
                    }
                    Instruction::LoadLocal(name) => {
                        if let Some(pos) = locals.iter().rposition(|l| l == name) {
                            chunk.write(OpCode::GetLocal(pos), 0);
                        } else {
                            // global? or just let it panic if not found in globals
                            let idx = chunk.add_constant(Value::String(name.clone()));
                            chunk.write(OpCode::GetGlobal(idx), 0);
                        }
                    }
                    Instruction::StoreLocal(name, v_id) => {
                        self.emit_load(&mut chunk, *v_id, &value_to_slot);
                        let pos = if let Some(p) = locals.iter().rposition(|l| l == name) {
                            p
                        } else {
                            locals.push(name.clone());
                            locals.len() - 1
                        };
                        chunk.write(OpCode::SetLocal(pos), 0);
                        chunk.write(OpCode::Pop, 0); // assignment evaluates to value but we pop it here
                        // wait, actually StoreLocal itself doesn't need to leave value on stack
                    }
                    Instruction::LoadGlobal(name) => {
                        let idx = chunk.add_constant(Value::String(name.clone()));
                        chunk.write(OpCode::GetGlobal(idx), 0);
                    }
                    Instruction::StoreGlobal(name, v_id) => {
                        self.emit_load(&mut chunk, *v_id, &value_to_slot);
                        let idx = chunk.add_constant(Value::String(name.clone()));
                        chunk.write(OpCode::SetGlobal(idx), 0);
                        chunk.write(OpCode::Pop, 0);
                    }
                    Instruction::MakeFunction(idx, _) => {
                        // wait, we don't have access to nested chunks right now in this pass!
                        // Oh, we could compile them first!
                    }
                    _ => unimplemented!("IR backend instruction not fully implemented yet"),
                }
                
                // Store the result of this instruction into a temporary slot
                // (except side-effect ones that don't produce a usable value, but for simplicity we can store everything)
                if !matches!(inst, Instruction::StoreLocal(_,_) | Instruction::StoreGlobal(_,_) | Instruction::PropertySet(_,_,_)) {
                    let slot = locals.len();
                    locals.push(format!("%{}", val_id.0));
                    value_to_slot.insert(*val_id, slot);
                    chunk.write(OpCode::SetLocal(slot), 0);
                    // wait, SetLocal does not pop, so we need to pop to keep stack balanced
                    chunk.write(OpCode::Pop, 0);
                }
            }
            
            // Handle terminator
            match &block.terminator {
                Terminator::Return(Some(v)) => {
                    self.emit_load(&mut chunk, *v, &value_to_slot);
                    chunk.write(OpCode::Return, 0);
                }
                Terminator::Return(None) => {
                    chunk.write(OpCode::Null, 0);
                    chunk.write(OpCode::Return, 0);
                }
                Terminator::Branch(b_id) => {
                    if let Some(target) = block_offsets.get(b_id) {
                        // backward jump
                        let jump = chunk.code.len() - target;
                        chunk.write(OpCode::Loop(jump), 0); // Loop takes backward offset
                    } else {
                        // forward jump
                        let jmp = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp, *b_id));
                    }
                }
                Terminator::CondBranch { condition, true_block, false_block } => {
                    self.emit_load(&mut chunk, *condition, &value_to_slot);
                    let jmp_false = chunk.emit_jump(OpCode::JumpIfFalse(0), 0);
                    chunk.write(OpCode::Pop, 0); // pop condition
                    
                    if let Some(target) = block_offsets.get(true_block) {
                        let jump = chunk.code.len() - target;
                        chunk.write(OpCode::Loop(jump), 0);
                    } else {
                        let jmp = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp, *true_block));
                    }
                    
                    chunk.patch_jump(jmp_false);
                    chunk.write(OpCode::Pop, 0); // pop condition on false path
                    
                    if let Some(target) = block_offsets.get(false_block) {
                        let jump = chunk.code.len() - target;
                        chunk.write(OpCode::Loop(jump), 0);
                    } else {
                        let jmp = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp, *false_block));
                    }
                }
                Terminator::Unreachable => {
                    chunk.write(OpCode::Null, 0);
                    chunk.write(OpCode::Return, 0);
                }
            }
        }
        
        // Patch forward jumps
        for (jmp_offset, block_id) in pending_jumps {
            if let Some(target) = block_offsets.get(&block_id) {
                let jump = *target - jmp_offset - 3;
                match chunk.code[jmp_offset] {
                    OpCode::Jump(_) => chunk.code[jmp_offset] = OpCode::Jump(jump),
                    OpCode::JumpIfFalse(_) => chunk.code[jmp_offset] = OpCode::JumpIfFalse(jump),
                    _ => unreachable!(),
                }
            }
        }
        
        Ok(chunk)
    }
    
    fn emit_load(&self, chunk: &mut Chunk, val_id: ValueId, value_to_slot: &HashMap<ValueId, usize>) {
        if let Some(slot) = value_to_slot.get(&val_id) {
            chunk.write(OpCode::GetLocal(*slot), 0);
        }
    }
}
