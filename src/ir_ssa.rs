use crate::ir::{BlockId, FunctionIR, Instruction, Terminator, ValueId};
use std::collections::{HashMap, HashSet};

/// Splits critical edges in the CFG so that no block with multiple predecessors
/// has a predecessor with multiple successors.
fn split_critical_edges(func: &mut FunctionIR) {
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &b_id in func.blocks.keys() {
        preds.entry(b_id).or_default();
    }
    for (src_id, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch(dst) => {
                preds.entry(*dst).or_default().push(*src_id);
            }
            Terminator::CondBranch { true_block, false_block, .. } => {
                preds.entry(*true_block).or_default().push(*src_id);
                preds.entry(*false_block).or_default().push(*src_id);
            }
            _ => {}
        }
        for (_, inst) in &block.instructions {
            if let Instruction::TryStart(catch_b) = inst {
                preds.entry(*catch_b).or_default().push(*src_id);
            }
        }
    }

    let mut edges_to_split: Vec<(BlockId, bool, BlockId)> = Vec::new(); // (src, is_true_branch, dst)
    for (src_id, block) in &func.blocks {
        if let Terminator::CondBranch { true_block, false_block, .. } = &block.terminator {
            if preds[true_block].len() > 1 {
                edges_to_split.push((*src_id, true, *true_block));
            }
            if preds[false_block].len() > 1 {
                edges_to_split.push((*src_id, false, *false_block));
            }
        }
    }

    for (src_id, is_true, dst) in edges_to_split {
        let trampoline = func.new_block();
        let tramp_block = func.blocks.get_mut(&trampoline).unwrap();
        tramp_block.terminator = Terminator::Branch(dst);

        let src_block = func.blocks.get_mut(&src_id).unwrap();
        if let Terminator::CondBranch { true_block, false_block, .. } = &mut src_block.terminator {
            if is_true {
                *true_block = trampoline;
            } else {
                *false_block = trampoline;
            }
        }
    }
}

fn resolve_alias(mut val: ValueId, replacements: &HashMap<ValueId, ValueId>) -> ValueId {
    let mut visited = HashSet::new();
    while let Some(&next) = replacements.get(&val) {
        if !visited.insert(val) {
            break;
        }
        val = next;
    }
    val
}

/// Identifies back-edges using DFS from the entry block.
fn find_back_edges(func: &FunctionIR) -> HashSet<(BlockId, BlockId)> {
    let mut back_edges = HashSet::new();
    let mut state: HashMap<BlockId, u8> = HashMap::new(); // 0 = unvisited, 1 = visiting, 2 = done

    fn dfs(
        u: BlockId,
        func: &FunctionIR,
        state: &mut HashMap<BlockId, u8>,
        back_edges: &mut HashSet<(BlockId, BlockId)>,
    ) {
        state.insert(u, 1);
        let mut successors = match &func.blocks.get(&u).unwrap().terminator {
            Terminator::Branch(dst) => vec![*dst],
            Terminator::CondBranch { true_block, false_block, .. } => vec![*true_block, *false_block],
            _ => vec![],
        };
        for (_, inst) in &func.blocks.get(&u).unwrap().instructions {
            if let Instruction::TryStart(catch_b) = inst {
                successors.push(*catch_b);
            }
        }

        for v in successors {
            match state.get(&v).copied().unwrap_or(0) {
                1 => {
                    back_edges.insert((u, v));
                }
                0 => {
                    dfs(v, func, state, back_edges);
                }
                _ => {}
            }
        }
        state.insert(u, 2);
    }

    if func.blocks.contains_key(&func.entry_block) {
        dfs(func.entry_block, func, &mut state, &mut back_edges);
    }

    back_edges
}

struct SSAConverter<'a> {
    func: &'a mut FunctionIR,
    preds: HashMap<BlockId, Vec<BlockId>>,
    sealed_blocks: HashSet<BlockId>,
    defs: HashMap<(BlockId, String), ValueId>,
    incomplete_phis: HashMap<BlockId, Vec<(String, ValueId)>>,
    phi_nodes: HashMap<ValueId, (BlockId, String, Vec<(BlockId, ValueId)>)>,
    replacements: HashMap<ValueId, ValueId>,
}

impl<'a> SSAConverter<'a> {
    fn new(func: &'a mut FunctionIR, preds: HashMap<BlockId, Vec<BlockId>>) -> Self {
        Self {
            func,
            preds,
            sealed_blocks: HashSet::new(),
            defs: HashMap::new(),
            incomplete_phis: HashMap::new(),
            phi_nodes: HashMap::new(),
            replacements: HashMap::new(),
        }
    }

    fn read_variable(&mut self, var: &str, block_id: BlockId) -> ValueId {
        if let Some(&val) = self.defs.get(&(block_id, var.to_string())) {
            return resolve_alias(val, &self.replacements);
        }

        if !self.sealed_blocks.contains(&block_id) {
            // Incomplete phi for unsealed block
            let phi_val = self.func.new_value();
            self.defs.insert((block_id, var.to_string()), phi_val);
            self.incomplete_phis
                .entry(block_id)
                .or_default()
                .push((var.to_string(), phi_val));
            return phi_val;
        }

        let p = self.preds.get(&block_id).cloned().unwrap_or_default();
        if p.is_empty() {
            let null_val = self.func.new_value();
            self.func.blocks.get_mut(&block_id).unwrap().instructions.insert(
                0,
                (null_val, Instruction::Constant(crate::interpreter::Value::Null)),
            );
            self.defs.insert((block_id, var.to_string()), null_val);
            return null_val;
        }

        if p.len() == 1 {
            let val = self.read_variable(var, p[0]);
            self.defs.insert((block_id, var.to_string()), val);
            return val;
        }

        let phi_val = self.func.new_value();
        self.defs.insert((block_id, var.to_string()), phi_val);
        self.add_phi_operands(var, phi_val, block_id);
        phi_val
    }

    fn add_phi_operands(&mut self, var: &str, phi_val: ValueId, block_id: BlockId) {
        let p = self.preds.get(&block_id).cloned().unwrap_or_default();
        let mut operands = Vec::new();
        for pred in p {
            let op_val = self.read_variable(var, pred);
            operands.push((pred, op_val));
        }
        self.phi_nodes
            .insert(phi_val, (block_id, var.to_string(), operands));
    }

    fn seal_block(&mut self, block_id: BlockId) {
        if self.sealed_blocks.insert(block_id) {
            if let Some(incompletes) = self.incomplete_phis.remove(&block_id) {
                for (var, phi_val) in incompletes {
                    self.add_phi_operands(&var, phi_val, block_id);
                }
            }
        }
    }
}

pub fn convert_to_ssa(func: &mut FunctionIR) {
    // 1. Split critical edges
    split_critical_edges(func);

    // 2. Compute predecessors
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &b_id in func.blocks.keys() {
        preds.entry(b_id).or_default();
    }
    for (src_id, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch(dst) => {
                preds.entry(*dst).or_default().push(*src_id);
            }
            Terminator::CondBranch { true_block, false_block, .. } => {
                preds.entry(*true_block).or_default().push(*src_id);
                preds.entry(*false_block).or_default().push(*src_id);
            }
            _ => {}
        }
        for (_, inst) in &block.instructions {
            if let Instruction::TryStart(catch_b) = inst {
                preds.entry(*catch_b).or_default().push(*src_id);
            }
        }
    }

    // 3. Find back-edges to control block sealing
    let back_edges = find_back_edges(func);

    let mut forward_preds_count: HashMap<BlockId, usize> = HashMap::new();
    for (&b_id, p_list) in &preds {
        let count = p_list
            .iter()
            .filter(|&p| !back_edges.contains(&(*p, b_id)))
            .count();
        forward_preds_count.insert(b_id, count);
    }

    // 4. Process blocks in Reverse Post-Order (RPO)
    let mut rpo = Vec::new();
    let mut visited = HashSet::new();
    fn post_order(
        u: BlockId,
        func: &FunctionIR,
        visited: &mut HashSet<BlockId>,
        rpo: &mut Vec<BlockId>,
    ) {
        visited.insert(u);
        let mut successors = match &func.blocks.get(&u).unwrap().terminator {
            Terminator::Branch(dst) => vec![*dst],
            Terminator::CondBranch { true_block, false_block, .. } => vec![*true_block, *false_block],
            _ => vec![],
        };
        for (_, inst) in &func.blocks.get(&u).unwrap().instructions {
            if let Instruction::TryStart(catch_b) = inst {
                successors.push(*catch_b);
            }
        }
        for v in successors {
            if !visited.contains(&v) && func.blocks.contains_key(&v) {
                post_order(v, func, visited, rpo);
            }
        }
        rpo.push(u);
    }
    if func.blocks.contains_key(&func.entry_block) {
        post_order(func.entry_block, func, &mut visited, &mut rpo);
        rpo.reverse();
    }

    // Add any remaining unreachable blocks
    for &b_id in func.blocks.keys() {
        if !visited.contains(&b_id) {
            rpo.push(b_id);
        }
    }

    let mut ssa = SSAConverter::new(func, preds);

    for &b_id in &rpo {
        // If a block has NO back-edges targeting it, it can be sealed immediately upon reaching it
        let has_back_edges = ssa.preds.get(&b_id).map_or(false, |plist| {
            plist.iter().any(|&p| back_edges.contains(&(p, b_id)))
        });
        if !has_back_edges {
            ssa.seal_block(b_id);
        }

        let insts = ssa.func.blocks.get(&b_id).unwrap().instructions.clone();
        for (val_id, inst) in insts {
            match inst {
                Instruction::StoreLocal(name, val) => {
                    let actual_val = resolve_alias(val, &ssa.replacements);
                    ssa.defs.insert((b_id, name), actual_val);
                }
                Instruction::LoadLocal(name) => {
                    let reaching = ssa.read_variable(&name, b_id);
                    ssa.replacements.insert(val_id, reaching);
                }
                _ => {}
            }
        }
    }

    // Now seal any remaining blocks (loop headers)
    for &b_id in &rpo {
        ssa.seal_block(b_id);
    }

    // 5. Simplify trivial Phis
    let mut phi_replacements: HashMap<ValueId, ValueId> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (&phi_val, (_, _, operands)) in &ssa.phi_nodes {
            if phi_replacements.contains_key(&phi_val) {
                continue;
            }
            let mut unique_val: Option<ValueId> = None;
            let mut is_trivial = true;
            for (_, op) in operands {
                let resolved = resolve_alias(*op, &phi_replacements);
                let resolved = resolve_alias(resolved, &ssa.replacements);
                if resolved == phi_val {
                    continue;
                }
                match unique_val {
                    None => unique_val = Some(resolved),
                    Some(u) if u == resolved => {}
                    _ => {
                        is_trivial = false;
                        break;
                    }
                }
            }
            if is_trivial {
                if let Some(target) = unique_val {
                    phi_replacements.insert(phi_val, target);
                    changed = true;
                }
            }
        }
    }

    for (k, v) in &phi_replacements {
        ssa.replacements
            .insert(*k, resolve_alias(*v, &phi_replacements));
    }

    // 6. Insert remaining non-trivial Phis at the start of basic blocks
    for (phi_val, (block_id, _, operands)) in ssa.phi_nodes {
        if phi_replacements.contains_key(&phi_val) {
            continue;
        }
        let resolved_operands: Vec<(BlockId, ValueId)> = operands
            .into_iter()
            .map(|(b, v)| (b, resolve_alias(v, &ssa.replacements)))
            .collect();
        let block = ssa.func.blocks.get_mut(&block_id).unwrap();
        block
            .instructions
            .insert(0, (phi_val, Instruction::Phi(resolved_operands)));
    }

    // 7. Substitute and eliminate StoreLocal / LoadLocal
    let rep = &ssa.replacements;
    for block in ssa.func.blocks.values_mut() {
        for (_, inst) in &mut block.instructions {
            substitute_instruction(inst, rep);
        }
        substitute_terminator(&mut block.terminator, rep);

        block.instructions.retain(|(_, inst)| {
            !matches!(inst, Instruction::StoreLocal(_, _) | Instruction::LoadLocal(_))
        });
    }
}

fn substitute_instruction(inst: &mut Instruction, replacements: &HashMap<ValueId, ValueId>) {
    let rep = |v: &mut ValueId| {
        *v = resolve_alias(*v, replacements);
    };
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
        | Instruction::IndexGet(l, r) => {
            rep(l);
            rep(r);
        }
        Instruction::Call(callee, args) => {
            rep(callee);
            for a in args {
                rep(a);
            }
        }
        Instruction::GetProperty(obj, _)
        | Instruction::Await(obj)
        | Instruction::StoreLocal(_, obj)
        | Instruction::StoreGlobal(_, obj)
        | Instruction::Cast(obj, _)
        | Instruction::Read(obj)
        | Instruction::Get(obj)
        | Instruction::Where(obj, _)
        | Instruction::Select(obj, _)
        | Instruction::Aggregate(obj, _, _)
        | Instruction::Print(obj) => {
            rep(obj);
        }
        Instruction::StringInterpolate(args) => {
            for a in args {
                rep(a);
            }
        }
        Instruction::PropertySet(obj, _, val) => {
            rep(obj);
            rep(val);
        }
        Instruction::IndexSet(obj, idx, val) => {
            rep(obj);
            rep(idx);
            rep(val);
        }
        Instruction::BuildList(args) => {
            for a in args {
                rep(a);
            }
        }
        Instruction::BuildMap(args) => {
            for (k, v) in args {
                rep(k);
                rep(v);
            }
        }
        Instruction::Phi(args) => {
            for (_, v) in args {
                rep(v);
            }
        }
        Instruction::Constant(_)
        | Instruction::Spawn(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadGlobal(_)
        | Instruction::MakeFunction(_, _)
        | Instruction::MakeClass(_, _)
        | Instruction::TryStart(_)
        | Instruction::TryEnd
        | Instruction::CatchException
        | Instruction::ImportPython(_) => {}
    }
}

fn substitute_terminator(term: &mut Terminator, replacements: &HashMap<ValueId, ValueId>) {
    match term {
        Terminator::CondBranch { condition, .. } => {
            *condition = resolve_alias(*condition, replacements);
        }
        Terminator::Return(Some(v)) => {
            *v = resolve_alias(*v, replacements);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_builder::IRBuilder;

    #[test]
    fn test_ssa_if_phi() {
        let source = r#"
function test_if(c):
    x = 10
    if c > 0:
        x = 20
    otherwise:
        x = 30
    return x
"#;
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse().unwrap();
        let builder = IRBuilder::new();
        let functions = builder.build(&program).unwrap();

        // Check the compiled function
        let func = &functions[1];
        let mut has_phi = false;
        for block in func.blocks.values() {
            for (_, inst) in &block.instructions {
                if let Instruction::Phi(operands) = inst {
                    has_phi = true;
                    assert_eq!(operands.len(), 2);
                }
                // Verify no StoreLocal or LoadLocal remains
                assert!(!matches!(inst, Instruction::StoreLocal(_, _) | Instruction::LoadLocal(_)));
            }
        }
        assert!(has_phi, "Expected Phi node in if-else merge block");
    }

    #[test]
    fn test_ssa_while_phi() {
        let source = r#"
function test_while(n):
    total = 0
    i = 0
    while i < n:
        total = total + i
        i = i + 1
    return total
"#;
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse().unwrap();
        let builder = IRBuilder::new();
        let functions = builder.build(&program).unwrap();

        let func = &functions[1];
        let mut phi_count = 0;
        for block in func.blocks.values() {
            for (_, inst) in &block.instructions {
                if let Instruction::Phi(operands) = inst {
                    phi_count += 1;
                    assert_eq!(operands.len(), 2);
                }
                assert!(!matches!(inst, Instruction::StoreLocal(_, _) | Instruction::LoadLocal(_)));
            }
        }
        assert_eq!(phi_count, 2, "Expected 2 loop header phis for total and i");
    }
}

