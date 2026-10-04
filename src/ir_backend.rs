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

        // 1. Assign local slots using backward liveness and greedy graph coloring
        let (val_to_slot, total_slots) = allocate_slots(func);
        let num_params = func.params.len();
        let int_values = infer_integer_values(func);

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
            let mut successors = match &func.blocks.get(&u).unwrap().terminator {
                Terminator::Branch(dst) => vec![*dst],
                Terminator::CondBranch { true_block, false_block, .. } => {
                    vec![*true_block, *false_block]
                }
                _ => vec![],
            };
            for (_, inst) in &func.blocks.get(&u).unwrap().instructions {
                if let Instruction::TryStart(catch_b) = inst {
                    successors.push(*catch_b);
                }
            }
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
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::AddInt, 0);
                        } else {
                            chunk.write(OpCode::Add, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Sub(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::SubtractInt, 0);
                        } else {
                            chunk.write(OpCode::Subtract, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Mul(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::MultiplyInt, 0);
                        } else {
                            chunk.write(OpCode::Multiply, 0);
                        }
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
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::EqualInt, 0);
                        } else {
                            chunk.write(OpCode::Equal, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Neq(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::NotEqualInt, 0);
                        } else {
                            chunk.write(OpCode::NotEqual, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Lt(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::LessInt, 0);
                        } else {
                            chunk.write(OpCode::Less, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Le(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::LessEqualInt, 0);
                        } else {
                            chunk.write(OpCode::LessEqual, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Gt(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::GreaterInt, 0);
                        } else {
                            chunk.write(OpCode::Greater, 0);
                        }
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Ge(l, r) => {
                        self.emit_load(&mut chunk, *l, &val_to_slot);
                        self.emit_load(&mut chunk, *r, &val_to_slot);
                        if int_values.contains(l) && int_values.contains(r) {
                            chunk.write(OpCode::GreaterEqualInt, 0);
                        } else {
                            chunk.write(OpCode::GreaterEqual, 0);
                        }
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
                    Instruction::GetProperty(obj, prop) => {
                        self.emit_load(&mut chunk, *obj, &val_to_slot);
                        let idx = chunk.add_constant(Value::String(prop.clone()));
                        chunk.write(OpCode::PropertyGet(idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::IndexGet(target, index) => {
                        self.emit_load(&mut chunk, *target, &val_to_slot);
                        self.emit_load(&mut chunk, *index, &val_to_slot);
                        chunk.write(OpCode::IndexGet, 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::IndexSet(target, index, val) => {
                        self.emit_load(&mut chunk, *target, &val_to_slot);
                        self.emit_load(&mut chunk, *index, &val_to_slot);
                        self.emit_load(&mut chunk, *val, &val_to_slot);
                        chunk.write(OpCode::IndexSet, 0);
                        chunk.write(OpCode::Pop, 0);
                    }
                    Instruction::PropertySet(target, prop, val) => {
                        self.emit_load(&mut chunk, *target, &val_to_slot);
                        let idx = chunk.add_constant(Value::String(prop.clone()));
                        self.emit_load(&mut chunk, *val, &val_to_slot);
                        chunk.write(OpCode::PropertySet(idx), 0);
                        chunk.write(OpCode::Pop, 0);
                    }
                    Instruction::BuildList(args) => {
                        for a in args {
                            self.emit_load(&mut chunk, *a, &val_to_slot);
                        }
                        chunk.write(OpCode::BuildList(args.len()), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::BuildMap(pairs) => {
                        for (k, v) in pairs {
                            self.emit_load(&mut chunk, *k, &val_to_slot);
                            self.emit_load(&mut chunk, *v, &val_to_slot);
                        }
                        chunk.write(OpCode::BuildMap(pairs.len()), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::ImportPython(module) => {
                        let idx = chunk.add_constant(Value::String(module.clone()));
                        chunk.write(OpCode::ImportPython(idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::Phi(_) => {
                        // Handled during SSA deconstruction before branches
                    }
                    Instruction::MakeClass(name, methods) => {
                        let mut method_map = HashMap::new();
                        for (m_name, m_idx) in methods {
                            let compiled = compiled_funcs.get(m_idx).expect("Method chunk not found").clone();
                            let target = &self.functions[*m_idx];
                            let val = Value::CompiledFunction {
                                chunk: compiled,
                                arity: target.params.len(),
                            };
                            method_map.insert(m_name.clone(), val);
                        }
                        let class_val = Value::Class {
                            name: name.clone(),
                            methods: method_map,
                        };
                        let const_idx = chunk.add_constant(class_val);
                        chunk.write(OpCode::Constant(const_idx), 0);
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
                    }
                    Instruction::TryStart(catch_block) => {
                        let jmp = chunk.emit_jump(OpCode::TryStart(0), 0);
                        pending_jumps.push((jmp, *catch_block));
                    }
                    Instruction::TryEnd => {
                        chunk.write(OpCode::TryEnd, 0);
                    }
                    Instruction::CatchException => {
                        self.emit_store(&mut chunk, *val_id, &val_to_slot);
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
                    let false_jump = chunk.emit_jump(OpCode::JumpIfFalse(0), 0);

                    // True branch
                    self.emit_phi_copies(&mut chunk, *b_id, *true_block, func, &val_to_slot);
                    if let Some(&target) = block_offsets.get(true_block) {
                        let offset = chunk.code.len() + 1 - target;
                        chunk.write(OpCode::Loop(offset), 0);
                    } else {
                        let jmp = chunk.emit_jump(OpCode::Jump(0), 0);
                        pending_jumps.push((jmp, *true_block));
                    }

                    // False branch landing
                    let false_landing = chunk.code.len();
                    let offset = false_landing - (false_jump + 1);
                    chunk.code[false_jump] = OpCode::JumpIfFalse(offset);

                    self.emit_phi_copies(&mut chunk, *b_id, *false_block, func, &val_to_slot);
                    if let Some(&target) = block_offsets.get(false_block) {
                        let offset = chunk.code.len() + 1 - target;
                        chunk.write(OpCode::Loop(offset), 0);
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

        // 4. Patch all pending forward jumps
        for (jmp_offset, dst) in pending_jumps {
            let target_offset = *block_offsets
                .get(&dst)
                .unwrap_or_else(|| panic!("Target block {:?} not found in block_offsets", dst));
            let offset = target_offset - (jmp_offset + 1);
            match &mut chunk.code[jmp_offset] {
                OpCode::Jump(j) | OpCode::JumpIfFalse(j) | OpCode::TryStart(j) => {
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
            let mut copies: Vec<(ValueId, ValueId)> = Vec::new();
            for (phi_val, inst) in &target_block.instructions {
                if let Instruction::Phi(operands) = inst {
                    for (from_block, incoming_val) in operands {
                        if *from_block == src {
                            let src_slot = val_to_slot[incoming_val];
                            let dst_slot = val_to_slot[phi_val];
                            if src_slot != dst_slot {
                                copies.push((*incoming_val, *phi_val));
                            }
                            break;
                        }
                    }
                }
            }

            // Buffer strategy: push all sources onto the operand stack
            for (src_val, _) in &copies {
                self.emit_load(chunk, *src_val, val_to_slot);
            }
            // Pop in reverse order directly into the destination slots
            for (_, phi_val) in copies.iter().rev() {
                self.emit_store(chunk, *phi_val, val_to_slot);
            }
        }
    }
}

fn get_instruction_uses(inst: &Instruction) -> Vec<ValueId> {
    match inst {
        Instruction::Add(l, r)
        | Instruction::Sub(l, r)
        | Instruction::Mul(l, r)
        | Instruction::Div(l, r)
        | Instruction::Mod(l, r)
        | Instruction::Eq(l, r)
        | Instruction::Neq(l, r)
        | Instruction::Lt(l, r)
        | Instruction::Le(l, r)
        | Instruction::Gt(l, r)
        | Instruction::Ge(l, r)
        | Instruction::IndexGet(l, r) => vec![*l, *r],
        Instruction::Call(c, args) => {
            let mut u = vec![*c];
            u.extend(args.iter().copied());
            u
        }
        Instruction::GetProperty(o, _)
        | Instruction::Spawn(o)
        | Instruction::Await(o)
        | Instruction::StoreGlobal(_, o)
        | Instruction::StoreLocal(_, o)
        | Instruction::Cast(o, _)
        | Instruction::Print(o) => vec![*o],
        Instruction::PropertySet(o, _, v) => vec![*o, *v],
        Instruction::IndexSet(o, i, v) => vec![*o, *i, *v],
        Instruction::BuildList(args) => args.clone(),
        Instruction::BuildMap(pairs) => {
            let mut u = Vec::new();
            for (k, v) in pairs {
                u.push(*k);
                u.push(*v);
            }
            u
        }
        Instruction::Phi(operands) => operands.iter().map(|(_, v)| *v).collect(),
        Instruction::Constant(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadGlobal(_)
        | Instruction::MakeFunction(_, _)
        | Instruction::MakeClass(_, _)
        | Instruction::TryStart(_)
        | Instruction::TryEnd
        | Instruction::CatchException
        | Instruction::ImportPython(_) => vec![],
    }
}

fn get_terminator_uses(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::CondBranch { condition, .. } => vec![*condition],
        Terminator::Return(Some(v)) => vec![*v],
        _ => vec![],
    }
}

/// Allocates local stack slots using backward liveness analysis and greedy graph coloring.
fn allocate_slots(func: &FunctionIR) -> (HashMap<ValueId, usize>, usize) {
    let num_params = func.params.len();
    let mut val_to_slot: HashMap<ValueId, usize> = HashMap::new();
    for (i, p) in func.params.iter().enumerate() {
        val_to_slot.insert(*p, i);
    }

    let mut block_defs: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut block_phi_uses: HashMap<BlockId, HashMap<BlockId, HashSet<ValueId>>> = HashMap::new();
    let mut block_uses: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut succs: HashMap<BlockId, Vec<BlockId>> = HashMap::new();

    for (b_id, block) in &func.blocks {
        let mut defs = HashSet::new();
        let mut phi_uses: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
        let mut uses = HashSet::new();

        for (val_id, inst) in &block.instructions {
            if let Instruction::Phi(operands) = inst {
                for (from_b, in_val) in operands {
                    phi_uses.entry(*from_b).or_default().insert(*in_val);
                }
                defs.insert(*val_id);
            } else {
                for u in get_instruction_uses(inst) {
                    if !defs.contains(&u) {
                        uses.insert(u);
                    }
                }
                let is_void = matches!(
                    inst,
                    Instruction::Print(_)
                        | Instruction::StoreGlobal(_, _)
                        | Instruction::StoreLocal(_, _)
                        | Instruction::PropertySet(_, _, _)
                        | Instruction::IndexSet(_, _, _)
                        | Instruction::TryStart(_)
                        | Instruction::TryEnd
                );
                if !is_void {
                    defs.insert(*val_id);
                }
            }
        }

        for u in get_terminator_uses(&block.terminator) {
            if !defs.contains(&u) {
                uses.insert(u);
            }
        }

        let mut s = match &block.terminator {
            Terminator::Branch(dst) => vec![*dst],
            Terminator::CondBranch { true_block, false_block, .. } => {
                vec![*true_block, *false_block]
            }
            _ => vec![],
        };
        for (_, inst) in &block.instructions {
            if let Instruction::TryStart(catch_b) = inst {
                s.push(*catch_b);
            }
        }

        block_defs.insert(*b_id, defs);
        block_phi_uses.insert(*b_id, phi_uses);
        block_uses.insert(*b_id, uses);
        succs.insert(*b_id, s);
    }

    // Backward dataflow for live_in and live_out
    let mut live_in: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut changed = true;

    while changed {
        changed = false;
        for (b_id, _) in &func.blocks {
            let mut new_live_out = HashSet::new();
            if let Some(s_list) = succs.get(b_id) {
                for s in s_list {
                    if let Some(s_in) = live_in.get(s) {
                        for &v in s_in {
                            if !block_defs[s].contains(&v) {
                                new_live_out.insert(v);
                            }
                        }
                    }
                    if let Some(edge_phis) = block_phi_uses.get(s).and_then(|m| m.get(b_id)) {
                        for &v in edge_phis {
                            new_live_out.insert(v);
                        }
                    }
                }
            }

            let mut new_live_in = block_uses[b_id].clone();
            for &v in &new_live_out {
                if !block_defs[b_id].contains(&v) {
                    new_live_in.insert(v);
                }
            }

            if live_out.get(b_id) != Some(&new_live_out) {
                live_out.insert(*b_id, new_live_out);
                changed = true;
            }
            if live_in.get(b_id) != Some(&new_live_in) {
                live_in.insert(*b_id, new_live_in);
                changed = true;
            }
        }
    }

    // Build interference graph
    let mut interferes: HashMap<ValueId, HashSet<ValueId>> = HashMap::new();
    for (b_id, block) in &func.blocks {
        let mut live = live_out.get(b_id).cloned().unwrap_or_default();
        for u in get_terminator_uses(&block.terminator) {
            live.insert(u);
        }

        for (val_id, inst) in block.instructions.iter().rev() {
            if !matches!(inst, Instruction::Print(_)) {
                for &other in &live {
                    if other != *val_id {
                        interferes.entry(*val_id).or_default().insert(other);
                        interferes.entry(other).or_default().insert(*val_id);
                    }
                }
                live.remove(val_id);
            }
            if !matches!(inst, Instruction::Phi(_)) {
                for u in get_instruction_uses(inst) {
                    live.insert(u);
                }
            }
        }
    }

    // Parameters interfere with each other and anything live at entry
    for (i, p1) in func.params.iter().enumerate() {
        for p2 in func.params.iter().skip(i + 1) {
            interferes.entry(*p1).or_default().insert(*p2);
            interferes.entry(*p2).or_default().insert(*p1);
        }
        if let Some(entry_live) = live_in.get(&func.entry_block) {
            for &v in entry_live {
                if v != *p1 {
                    interferes.entry(*p1).or_default().insert(v);
                    interferes.entry(v).or_default().insert(*p1);
                }
            }
        }
    }

    // Greedy coloring
    let mut all_vals = Vec::new();
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            let is_void = matches!(
                inst,
                Instruction::Print(_)
                    | Instruction::StoreGlobal(_, _)
                    | Instruction::StoreLocal(_, _)
                    | Instruction::PropertySet(_, _, _)
                    | Instruction::IndexSet(_, _, _)
                    | Instruction::TryStart(_)
                    | Instruction::TryEnd
            );
            if !is_void && !val_to_slot.contains_key(val_id) {
                all_vals.push(*val_id);
            }
        }
    }
    all_vals.sort_by_key(|v| v.0);
    all_vals.dedup();

    for val_id in all_vals {
        let neighbors = interferes.get(&val_id).cloned().unwrap_or_default();
        let mut used_slots = HashSet::new();
        for neighbor in neighbors {
            if let Some(&slot) = val_to_slot.get(&neighbor) {
                used_slots.insert(slot);
            }
        }
        let mut slot = num_params;
        while used_slots.contains(&slot) {
            slot += 1;
        }
        val_to_slot.insert(val_id, slot);
    }

    let total_slots = val_to_slot
        .values()
        .copied()
        .max()
        .map_or(num_params, |m| m + 1);

    (val_to_slot, total_slots)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InferredType {
    Unknown,
    Integer,
    NonInteger,
}

fn infer_integer_values(func: &FunctionIR) -> HashSet<ValueId> {
    let mut types: HashMap<ValueId, InferredType> = HashMap::new();

    // 1. Initial categorization of known producers
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            match inst {
                Instruction::Constant(Value::Integer(_)) => {
                    types.insert(*val_id, InferredType::Integer);
                }
                Instruction::Constant(_)
                | Instruction::LoadGlobal(_)
                | Instruction::ImportPython(_)
                | Instruction::BuildList(_)
                | Instruction::BuildMap(_)
                | Instruction::MakeFunction(_, _)
                | Instruction::MakeClass(_, _)
                | Instruction::CatchException
                | Instruction::GetProperty(_, _)
                | Instruction::IndexGet(_, _) => {
                    types.insert(*val_id, InferredType::NonInteger);
                }
                _ => {}
            }
        }
    }

    // 2. Optimistic seeding for Phi nodes:
    // If a Phi has an incoming operand from a dominating/predecessor block that is Integer,
    // and no incoming operand that is known NonInteger, tentatively hypothesize Integer.
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            if let Instruction::Phi(operands) = inst {
                let has_int = operands.iter().any(|(_, v)| types.get(v) == Some(&InferredType::Integer));
                let has_non_int = operands.iter().any(|(_, v)| types.get(v) == Some(&InferredType::NonInteger));
                if has_non_int {
                    types.insert(*val_id, InferredType::NonInteger);
                } else if has_int {
                    types.insert(*val_id, InferredType::Integer);
                }
            }
        }
    }

    // 3. Monotone fixed-point propagation
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 20 {
        changed = false;
        iterations += 1;

        for block in func.blocks.values() {
            for (val_id, inst) in &block.instructions {
                let current = types.get(val_id).copied().unwrap_or(InferredType::Unknown);
                let next = match inst {
                    Instruction::Add(l, r)
                    | Instruction::Sub(l, r)
                    | Instruction::Mul(l, r)
                    | Instruction::Mod(l, r) => {
                        let tl = types.get(l).copied().unwrap_or(InferredType::Unknown);
                        let tr = types.get(r).copied().unwrap_or(InferredType::Unknown);
                        if tl == InferredType::NonInteger || tr == InferredType::NonInteger {
                            InferredType::NonInteger
                        } else if tl == InferredType::Integer && tr == InferredType::Integer {
                            InferredType::Integer
                        } else {
                            InferredType::Unknown
                        }
                    }
                    Instruction::Phi(operands) => {
                        let mut all_int = true;
                        let mut any_non_int = false;
                        for (_, v) in operands {
                            if *v == *val_id {
                                continue;
                            }
                            let tv = types.get(v).copied().unwrap_or(InferredType::Unknown);
                            if tv == InferredType::NonInteger {
                                any_non_int = true;
                                break;
                            }
                            if tv != InferredType::Integer {
                                all_int = false;
                            }
                        }
                        if any_non_int {
                            InferredType::NonInteger
                        } else if all_int && !operands.is_empty() {
                            InferredType::Integer
                        } else {
                            current
                        }
                    }
                    _ => current,
                };

                if next != current {
                    types.insert(*val_id, next);
                    changed = true;
                }
            }
        }
    }

    types
        .into_iter()
        .filter_map(|(v, t)| if t == InferredType::Integer { Some(v) } else { None })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::vm::VM;

    #[test]
    fn test_specialized_integer_opcode_emission() {
        let mut func = FunctionIR::new("main".to_string());
        let b0 = func.new_block();
        func.entry_block = b0;

        let v1 = func.new_value();
        let v2 = func.new_value();
        let v3 = func.new_value();

        let block = func.blocks.get_mut(&b0).unwrap();
        block.instructions.push((v1, Instruction::Constant(Value::Integer(40))));
        block.instructions.push((v2, Instruction::Constant(Value::Integer(2))));
        block.instructions.push((v3, Instruction::Add(v1, v2)));
        block.terminator = Terminator::Return(Some(v3));

        let backend = IRBackend::new(vec![func]);
        let chunk = backend.generate().unwrap();

        assert!(chunk.code.contains(&OpCode::AddInt));

        let mut vm = VM::new();
        assert_eq!(vm.interpret(chunk), crate::vm::InterpretResult::Ok);
        assert_eq!(vm.stack.pop(), Some(Value::Integer(42)));
    }

    #[test]
    fn test_class_and_method_execution() {
        let source = "
class Greeter:
    function hello():
        return 42

g = Greeter()
return g.hello()
";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let ast = crate::parser::Parser::new(tokens).parse().unwrap();
        let ir = crate::ir_builder::IRBuilder::new().build(&ast).unwrap();
        let chunk = crate::ir_backend::IRBackend::new(ir).generate().unwrap();

        let mut vm = VM::new();
        assert_eq!(vm.interpret(chunk), crate::vm::InterpretResult::Ok);
        assert_eq!(vm.stack.pop(), Some(Value::Integer(42)));
    }

    #[test]
    fn test_try_catch_execution() {
        let source = "
try:
    x = undefined_variable
catch err:
    x = 99
return x
";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let ast = crate::parser::Parser::new(tokens).parse().unwrap();
        let ir = crate::ir_builder::IRBuilder::new().build(&ast).unwrap();
        let chunk = crate::ir_backend::IRBackend::new(ir).generate().unwrap();

        let mut vm = VM::new();
        assert_eq!(vm.interpret(chunk), crate::vm::InterpretResult::Ok);
        assert_eq!(vm.stack.pop(), Some(Value::Integer(99)));
    }
}

