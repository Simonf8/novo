use crate::interpreter::Value;
use crate::ir::{BlockId, FunctionIR, Instruction, Terminator, ValueId};
use crate::vm::chunk::Chunk;
use crate::vm::opcode::OpCode;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct IRBackend {
    functions: Vec<FunctionIR>,
}

impl IRBackend {
    pub fn new(functions: Vec<FunctionIR>) -> Self {
        IRBackend { functions }
    }

    pub fn generate(self) -> Result<Chunk, String> {
        let mut compiled_funcs: HashMap<usize, Arc<Chunk>> = HashMap::new();

        // Compile functions from 1..N first (leaf functions)
        for idx in (1..self.functions.len()).rev() {
            let func = &self.functions[idx];
            let chunk = self.compile_function(func, &compiled_funcs)?;
            compiled_funcs.insert(idx, Arc::new(chunk));
        }

        // Compile top-level function (0)
        if !self.functions.is_empty() {
            let top_chunk = self.compile_function(&self.functions[0], &compiled_funcs)?;
            return Ok(top_chunk);
        }

        Ok(Chunk::new())
    }

    fn compile_function(
        &self,
        func: &FunctionIR,
        compiled_funcs: &HashMap<usize, Arc<Chunk>>,
    ) -> Result<Chunk, String> {
        let mut chunk = Chunk::new();

        // 1. Assign local slots to parameters and all SSA ValueIds
        let mut val_to_slot: HashMap<ValueId, usize> = HashMap::new();
        let num_params = func.params.len();
        for (i, p_id) in func.params.iter().enumerate() {
            val_to_slot.insert(*p_id, i);
        }

        let mut next_slot = num_params;
        let mut sorted_blocks: Vec<&crate::ir::BasicBlock> = func.blocks.values().collect();
        sorted_blocks.sort_by_key(|b| b.id.0);

        for block in &sorted_blocks {
            for (val_id, inst) in &block.instructions {
                if !matches!(inst, Instruction::Print(_)) {
                    val_to_slot.insert(*val_id, next_slot);
                    next_slot += 1;
                }
            }
        }

        let total_slots = next_slot;

        // 2. Pre-allocate local slots on VM stack
        for _ in num_params..total_slots {
            chunk.write(OpCode::Null, 0);
        }

        // 3. Layout blocks in RPO order
        let mut block_offsets: HashMap<BlockId, usize> = HashMap::new();
        let mut pending_jumps: Vec<(usize, BlockId)> = Vec::new();

        let mut block_order = Vec::new();
        let mut visited = HashSet::new();

        fn post_order(
            u: BlockId,
            func: &FunctionIR,
            visited: &mut HashSet<BlockId>,
            order: &mut Vec<BlockId>,
        ) {
            visited.insert(u);
            let successors = match &func.blocks.get(&u).unwrap().terminator {
                Terminator::Branch(dst) => vec![*dst],
                Terminator::CondBranch { true_block, false_block, .. } => {
                    vec![*true_block, *false_block]
                }
                _ => vec![],
            };
            for v in successors {
                if !visited.contains(&v) && func.blocks.contains_key(&v) {
                    post_order(v, func, visited, order);
                }
            }
            order.push(u);
        }

        if func.blocks.contains_key(&func.entry_block) {
            post_order(func.entry_block, func, &mut visited, &mut block_order);
            block_order.reverse();
        }
        for b_id in func.blocks.keys() {
            if !visited.contains(b_id) {
                block_order.push(*b_id);
            }
        }

        for b_id in &block_order {
            let block = func.blocks.get(b_id).unwrap();
            block_offsets.insert(*b_id, chunk.code.len());

            for (val_id, inst) in &block.instructions {
                match inst {
                    Instruction::Constant(c) => {
                        let idx = chunk.add_constant(c.clone());
                        chunk.write(OpCode::Constant(idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Add(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Add, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Sub(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Subtract, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Mul(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Multiply, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Div(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Divide, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Mod(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Modulo, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Eq(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Equal, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Neq(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::NotEqual, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Lt(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Less, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Le(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::LessEqual, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Gt(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::Greater, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Ge(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        chunk.write(OpCode::GreaterEqual, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Call(callee, args) => {
                        self.emit_load(&mut chunk, *callee, &val_to_slot);
                        for a in args {
                            self.emit_load(&mut chunk, *a, &val_to_slot);
                        }
                        chunk.write(OpCode::Call(args.len()), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Print(v) => {
                        self.emit_load(&mut chunk, *v, &val_to_slot);
                        chunk.write(OpCode::Print, 0);
                    }
                    Instruction::LoadGlobal(name) => {
                        let idx = chunk.add_constant(Value::String(name.clone()));
                        chunk.write(OpCode::GetGlobal(idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::StoreGlobal(name, v) => {
                        self.emit_load(&mut chunk, *v, &val_to_slot);
                        let idx = chunk.add_constant(Value::String(name.clone()));
                        chunk.write(OpCode::SetGlobal(idx), 0);
                        chunk.write(OpCode::Pop, 0);
                    }
                    Instruction::MakeFunction(idx, _) => {
                        let compiled = compiled_funcs.get(idx).expect("Func chunk not found").clone();
                        let target = &self.functions[*idx];
                        let val = Value::CompiledFunction {
                            chunk: compiled,
                            arity: target.params.len(),
                        };
                        let const_idx = chunk.add_constant(val);
                        chunk.write(OpCode::Constant(const_idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Phi(_) => {
                        // Handled during SSA deconstruction before branches
                    }
                    _ => unimplemented!("Instruction {:?} not implemented in IRBackend", inst),
                }
            }

            // Handle terminator
            match &block.terminator {
                Terminator::Return(Some(v)) => {
                    self.emit_load(&mut chunk, *v, &val_to_slot);
                    chunk.write(OpCode::Return, 0);
                }
                Terminator::Return(None) => {
                    chunk.write(OpCode::Null, 0);
                    chunk.write(OpCode::Return, 0);
                }
                Terminator::Branch(dst) => {
                    self.emit_phi_copies(&mut chunk, *b_id, *dst, func, &val_to_slot);

                    if let Some(&target) = block_offsets.get(dst) {
                        let offset = chunk.code.len() + 1 - target;
                        chunk.write(OpCode::Loop(offset), 0);
                    } else {
                        let jmp = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp, *dst));
                    }
                }
                Terminator::CondBranch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    self.emit_load(&mut chunk, *condition, &val_to_slot);
                    let jmp_false = chunk.emit_jump(OpCode::JumpIfFalse(0), 0);
                    pending_jumps.push((jmp_false, *false_block));

                    // True branch
                    if let Some(&target) = block_offsets.get(true_block) {
                        let offset = chunk.code.len() + 1 - target;
                        chunk.write(OpCode::Loop(offset), 0);
                    } else {
                        let jmp_true = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp_true, *true_block));
                    }
                }
                Terminator::Unreachable => {
                    chunk.write(OpCode::Null, 0);
                    chunk.write(OpCode::Return, 0);
                }
            }
        }

        // 4. Patch all pending forward jumps
        for (jmp_offset, dst) in pending_jumps {
            let target_offset = *block_offsets
                .get(&dst)
                .unwrap_or_else(|| panic!("Target block {:?} not found in block_offsets", dst));
            let offset = target_offset - (jmp_offset + 1);
            match &mut chunk.code[jmp_offset] {
                OpCode::Jump(j) | OpCode::JumpIfFalse(j) => {
                    *j = offset;
                }
                _ => unreachable!(),
            }
        }

        Ok(chunk)
    }

    fn emit_load(&self, chunk: &mut Chunk, val_id: ValueId, val_to_slot: &HashMap<ValueId, usize>) {
        if let Some(&slot) = val_to_slot.get(&val_id) {
            chunk.write(OpCode::GetLocal(slot), 0);
        } else {
            panic!("ValueId {:?} not found in val_to_slot", val_id);
        }
    }

    fn emit_store(&self, chunk: &mut Chunk, val_id: ValueId, val_to_slot: &HashMap<ValueId, usize>) {
        if let Some(&slot) = val_to_slot.get(&val_id) {
            chunk.write(OpCode::SetLocal(slot), 0);
            chunk.write(OpCode::Pop, 0);
        } else {
            panic!("ValueId {:?} not found in val_to_slot", val_id);
        }
    }

    fn emit_phi_copies(
        &self,
        chunk: &mut Chunk,
        src: BlockId,
        dst: BlockId,
        func: &FunctionIR,
        val_to_slot: &HashMap<ValueId, usize>,
    ) {
        if let Some(target_block) = func.blocks.get(&dst) {
            for (phi_val, inst) in &target_block.instructions {
                if let Instruction::Phi(operands) = inst {
                    for (from_block, incoming_val) in operands {
                        if *from_block == src {
                            self.emit_load(chunk, *incoming_val, val_to_slot);
                            self.emit_store(chunk, *phi_val, val_to_slot);
                            break;
                        }
                    }
                }
            }
        }
    }
}
