use crate::interpreter::Value;
use crate::ir::{BlockId, FunctionIR, Instruction, Terminator, ValueId};
use std::collections::{HashMap, HashSet};

/// Runs the optimization pipeline on a FunctionIR until fixpoint.
pub fn optimize(func: &mut FunctionIR) {
    let mut changed = true;
    let mut iteration = 0;
    while changed && iteration < 10 {
        changed = false;
        changed |= constant_propagation_and_folding(func);
        changed |= algebraic_simplification(func);
        changed |= common_subexpression_elimination(func);
        changed |= simplify_cfg(func);
        changed |= dead_code_elimination(func);
        iteration += 1;
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
        | Instruction::Spawn(obj)
        | Instruction::Await(obj)
        | Instruction::StoreLocal(_, obj)
        | Instruction::StoreGlobal(_, obj)
        | Instruction::Cast(obj, _)
        | Instruction::Print(obj) => {
            rep(obj);
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

fn eval_add(l: &Value, r: &Value) -> Option<Value> {
    match (l, r) {
        (Value::Integer(x), Value::Integer(y)) => Some(Value::Integer(x + y)),
        (Value::Float(x), Value::Float(y)) => Some(Value::Float(x + y)),
        (Value::Integer(x), Value::Float(y)) => Some(Value::Float(*x as f64 + y)),
        (Value::Float(x), Value::Integer(y)) => Some(Value::Float(x + *y as f64)),
        (Value::String(x), Value::String(y)) => Some(Value::String(format!("{}{}", x, y))),
        _ => None,
    }
}

fn eval_sub(l: &Value, r: &Value) -> Option<Value> {
    match (l, r) {
        (Value::Integer(x), Value::Integer(y)) => Some(Value::Integer(x - y)),
        (Value::Float(x), Value::Float(y)) => Some(Value::Float(x - y)),
        (Value::Integer(x), Value::Float(y)) => Some(Value::Float(*x as f64 - y)),
        (Value::Float(x), Value::Integer(y)) => Some(Value::Float(x - *y as f64)),
        _ => None,
    }
}

fn eval_mul(l: &Value, r: &Value) -> Option<Value> {
    match (l, r) {
        (Value::Integer(x), Value::Integer(y)) => Some(Value::Integer(x * y)),
        (Value::Float(x), Value::Float(y)) => Some(Value::Float(x * y)),
        (Value::Integer(x), Value::Float(y)) => Some(Value::Float(*x as f64 * y)),
        (Value::Float(x), Value::Integer(y)) => Some(Value::Float(x * *y as f64)),
        _ => None,
    }
}

fn eval_div(l: &Value, r: &Value) -> Option<Value> {
    match (l, r) {
        (Value::Integer(x), Value::Integer(y)) if *y != 0 => {
            Some(Value::Float(*x as f64 / *y as f64))
        }
        (Value::Float(x), Value::Float(y)) if *y != 0.0 => Some(Value::Float(x / y)),
        (Value::Integer(x), Value::Float(y)) if *y != 0.0 => {
            Some(Value::Float(*x as f64 / y))
        }
        (Value::Float(x), Value::Integer(y)) if *y != 0 => {
            Some(Value::Float(x / *y as f64))
        }
        _ => None,
    }
}

fn eval_mod(l: &Value, r: &Value) -> Option<Value> {
    match (l, r) {
        (Value::Integer(x), Value::Integer(y)) if *y != 0 => Some(Value::Integer(x % y)),
        _ => None,
    }
}

fn eval_compare(l: &Value, r: &Value, op: &str) -> Option<Value> {
    match op {
        "==" => Some(Value::Boolean(l == r)),
        "!=" => Some(Value::Boolean(l != r)),
        "<" => match (l, r) {
            (Value::Integer(x), Value::Integer(y)) => Some(Value::Boolean(x < y)),
            (Value::Float(x), Value::Float(y)) => Some(Value::Boolean(x < y)),
            (Value::Integer(x), Value::Float(y)) => Some(Value::Boolean((*x as f64) < *y)),
            (Value::Float(x), Value::Integer(y)) => Some(Value::Boolean(*x < (*y as f64))),
            (Value::String(x), Value::String(y)) => Some(Value::Boolean(x < y)),
            _ => None,
        },
        "<=" => match (l, r) {
            (Value::Integer(x), Value::Integer(y)) => Some(Value::Boolean(x <= y)),
            (Value::Float(x), Value::Float(y)) => Some(Value::Boolean(x <= y)),
            (Value::Integer(x), Value::Float(y)) => Some(Value::Boolean((*x as f64) <= *y)),
            (Value::Float(x), Value::Integer(y)) => Some(Value::Boolean(*x <= (*y as f64))),
            (Value::String(x), Value::String(y)) => Some(Value::Boolean(x <= y)),
            _ => None,
        },
        ">" => match (l, r) {
            (Value::Integer(x), Value::Integer(y)) => Some(Value::Boolean(x > y)),
            (Value::Float(x), Value::Float(y)) => Some(Value::Boolean(x > y)),
            (Value::Integer(x), Value::Float(y)) => Some(Value::Boolean((*x as f64) > *y)),
            (Value::Float(x), Value::Integer(y)) => Some(Value::Boolean(*x > (*y as f64))),
            (Value::String(x), Value::String(y)) => Some(Value::Boolean(x > y)),
            _ => None,
        },
        ">=" => match (l, r) {
            (Value::Integer(x), Value::Integer(y)) => Some(Value::Boolean(x >= y)),
            (Value::Float(x), Value::Float(y)) => Some(Value::Boolean(x >= y)),
            (Value::Integer(x), Value::Float(y)) => Some(Value::Boolean((*x as f64) >= *y)),
            (Value::Float(x), Value::Integer(y)) => Some(Value::Boolean(*x >= (*y as f64))),
            (Value::String(x), Value::String(y)) => Some(Value::Boolean(x >= y)),
            _ => None,
        },
        _ => None,
    }
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

fn constant_propagation_and_folding(func: &mut FunctionIR) -> bool {
    let mut constants: HashMap<ValueId, Value> = HashMap::new();
    let mut changed = false;

    // First collect all existing constants
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            if let Instruction::Constant(c) = inst {
                constants.insert(*val_id, c.clone());
            }
        }
    }

    // Now fold instructions
    for block in func.blocks.values_mut() {
        for (val_id, inst) in &mut block.instructions {
            let folded = match inst {
                Instruction::Constant(_) => None,
                Instruction::Add(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_add(vl, vr)
                    } else {
                        None
                    }
                }
                Instruction::Sub(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_sub(vl, vr)
                    } else {
                        None
                    }
                }
                Instruction::Mul(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_mul(vl, vr)
                    } else {
                        None
                    }
                }
                Instruction::Div(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_div(vl, vr)
                    } else {
                        None
                    }
                }
                Instruction::Mod(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_mod(vl, vr)
                    } else {
                        None
                    }
                }
                Instruction::Eq(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, "==")
                    } else {
                        None
                    }
                }
                Instruction::Neq(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, "!=")
                    } else {
                        None
                    }
                }
                Instruction::Lt(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, "<")
                    } else {
                        None
                    }
                }
                Instruction::Le(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, "<=")
                    } else {
                        None
                    }
                }
                Instruction::Gt(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, ">")
                    } else {
                        None
                    }
                }
                Instruction::Ge(l, r) => {
                    if let (Some(vl), Some(vr)) = (constants.get(l), constants.get(r)) {
                        eval_compare(vl, vr, ">=")
                    } else {
                        None
                    }
                }
                Instruction::Phi(operands) => {
                    if !operands.is_empty() {
                        let mut unique_const: Option<Value> = None;
                        let mut all_same = true;
                        for (_, v) in operands {
                            if *v == *val_id {
                                continue;
                            }
                            if let Some(c) = constants.get(v) {
                                match &unique_const {
                                    None => unique_const = Some(c.clone()),
                                    Some(u) if u == c => {}
                                    _ => {
                                        all_same = false;
                                        break;
                                    }
                                }
                            } else {
                                all_same = false;
                                break;
                            }
                        }
                        if all_same {
                            unique_const
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(c) = folded {
                *inst = Instruction::Constant(c.clone());
                constants.insert(*val_id, c);
                changed = true;
            }
        }
    }

    changed
}

fn algebraic_simplification(func: &mut FunctionIR) -> bool {
    let mut changed = false;
    let mut replacements: HashMap<ValueId, ValueId> = HashMap::new();

    // Canonicalize commutative operations: ensure lhs.0 <= rhs.0
    for block in func.blocks.values_mut() {
        for (_, inst) in &mut block.instructions {
            match inst {
                Instruction::Add(l, r)
                | Instruction::Mul(l, r)
                | Instruction::Eq(l, r)
                | Instruction::Neq(l, r) => {
                    if l.0 > r.0 {
                        std::mem::swap(l, r);
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Identify constants for algebraic identities
    let mut known_ints: HashMap<ValueId, i64> = HashMap::new();
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            if let Instruction::Constant(Value::Integer(i)) = inst {
                known_ints.insert(*val_id, *i);
            }
        }
    }

    for block in func.blocks.values_mut() {
        for (val_id, inst) in &mut block.instructions {
            match inst {
                Instruction::Add(l, r) => {
                    if known_ints.get(r) == Some(&0) {
                        replacements.insert(*val_id, *l);
                        changed = true;
                    } else if known_ints.get(l) == Some(&0) {
                        replacements.insert(*val_id, *r);
                        changed = true;
                    }
                }
                Instruction::Sub(l, r) => {
                    if known_ints.get(r) == Some(&0) {
                        replacements.insert(*val_id, *l);
                        changed = true;
                    } else if l == r {
                        *inst = Instruction::Constant(Value::Integer(0));
                        changed = true;
                    }
                }
                Instruction::Mul(l, r) => {
                    if known_ints.get(r) == Some(&1) {
                        replacements.insert(*val_id, *l);
                        changed = true;
                    } else if known_ints.get(l) == Some(&1) {
                        replacements.insert(*val_id, *r);
                        changed = true;
                    } else if known_ints.get(r) == Some(&0) || known_ints.get(l) == Some(&0) {
                        *inst = Instruction::Constant(Value::Integer(0));
                        changed = true;
                    }
                }
                Instruction::Eq(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(true));
                    changed = true;
                }
                Instruction::Neq(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(false));
                    changed = true;
                }
                Instruction::Lt(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(false));
                    changed = true;
                }
                Instruction::Gt(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(false));
                    changed = true;
                }
                Instruction::Le(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(true));
                    changed = true;
                }
                Instruction::Ge(l, r) if l == r => {
                    *inst = Instruction::Constant(Value::Boolean(true));
                    changed = true;
                }
                _ => {}
            }
        }
    }

    if !replacements.is_empty() {
        for block in func.blocks.values_mut() {
            for (_, inst) in &mut block.instructions {
                substitute_instruction(inst, &replacements);
            }
            substitute_terminator(&mut block.terminator, &replacements);
        }
    }

    changed
}

fn compute_dominators(func: &FunctionIR) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut dom: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    let all_blocks: HashSet<BlockId> = func.blocks.keys().copied().collect();

    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &b in &all_blocks {
        preds.entry(b).or_default();
    }
    for (src, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch(dst) => {
                preds.entry(*dst).or_default().push(*src);
            }
            Terminator::CondBranch { true_block, false_block, .. } => {
                preds.entry(*true_block).or_default().push(*src);
                preds.entry(*false_block).or_default().push(*src);
            }
            _ => {}
        }
    }

    for &b in &all_blocks {
        if b == func.entry_block {
            let mut set = HashSet::new();
            set.insert(b);
            dom.insert(b, set);
        } else {
            dom.insert(b, all_blocks.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &all_blocks {
            if b == func.entry_block {
                continue;
            }
            let p_list = preds.get(&b).cloned().unwrap_or_default();
            if p_list.is_empty() {
                continue;
            }
            let mut new_set = dom.get(&p_list[0]).cloned().unwrap_or_default();
            for p in p_list.iter().skip(1) {
                if let Some(p_dom) = dom.get(p) {
                    new_set.retain(|x| p_dom.contains(x));
                }
            }
            new_set.insert(b);
            if dom.get(&b) != Some(&new_set) {
                dom.insert(b, new_set);
                changed = true;
            }
        }
    }

    dom
}

fn is_pure_instruction(inst: &Instruction) -> bool {
    matches!(
        inst,
        Instruction::Add(_, _)
            | Instruction::Sub(_, _)
            | Instruction::Mul(_, _)
            | Instruction::Div(_, _)
            | Instruction::Mod(_, _)
            | Instruction::Eq(_, _)
            | Instruction::Neq(_, _)
            | Instruction::Lt(_, _)
            | Instruction::Le(_, _)
            | Instruction::Gt(_, _)
            | Instruction::Ge(_, _)
            | Instruction::IndexGet(_, _)
            | Instruction::Cast(_, _)
    )
}

fn common_subexpression_elimination(func: &mut FunctionIR) -> bool {
    let dom = compute_dominators(func);
    let mut expressions: Vec<(BlockId, usize, ValueId, Instruction)> = Vec::new();
    let mut replacements: HashMap<ValueId, ValueId> = HashMap::new();
    let mut changed = false;

    // Collect blocks in sorted order for determinism
    let mut sorted_block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    sorted_block_ids.sort_by_key(|b| b.0);

    for &b_id in &sorted_block_ids {
        let block = &func.blocks[&b_id];
        for (idx, (val_id, inst)) in block.instructions.iter().enumerate() {
            if is_pure_instruction(inst) {
                let mut found_prev = None;
                for (prev_b, prev_idx, prev_val, prev_inst) in &expressions {
                    if prev_inst == inst {
                        let dominates = if *prev_b == b_id {
                            *prev_idx < idx
                        } else {
                            dom.get(&b_id).map_or(false, |d_set| d_set.contains(prev_b))
                        };
                        if dominates {
                            found_prev = Some(*prev_val);
                            break;
                        }
                    }
                }

                if let Some(prev_val) = found_prev {
                    replacements.insert(*val_id, prev_val);
                    changed = true;
                } else {
                    expressions.push((b_id, idx, *val_id, inst.clone()));
                }
            }
        }
    }

    if !replacements.is_empty() {
        for block in func.blocks.values_mut() {
            for (_, inst) in &mut block.instructions {
                substitute_instruction(inst, &replacements);
            }
            substitute_terminator(&mut block.terminator, &replacements);
        }
    }

    changed
}

fn simplify_cfg(func: &mut FunctionIR) -> bool {
    let mut changed = false;

    // 1. Fold conditional branches with known constant conditions
    let mut constants: HashMap<ValueId, Value> = HashMap::new();
    for block in func.blocks.values() {
        for (val_id, inst) in &block.instructions {
            if let Instruction::Constant(c) = inst {
                constants.insert(*val_id, c.clone());
            }
        }
    }

    for block in func.blocks.values_mut() {
        if let Terminator::CondBranch { condition, true_block, false_block } = &block.terminator {
            if let Some(cond_val) = constants.get(condition) {
                if is_truthy(cond_val) {
                    block.terminator = Terminator::Branch(*true_block);
                    changed = true;
                } else {
                    block.terminator = Terminator::Branch(*false_block);
                    changed = true;
                }
            }
        }
    }

    // 2. Remove unreachable blocks
    let mut reachable = HashSet::new();
    let mut queue = Vec::new();
    if func.blocks.contains_key(&func.entry_block) {
        reachable.insert(func.entry_block);
        queue.push(func.entry_block);
    }

    while let Some(u) = queue.pop() {
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
            if func.blocks.contains_key(&v) && reachable.insert(v) {
                queue.push(v);
            }
        }
    }

    let initial_count = func.blocks.len();
    func.blocks.retain(|b_id, _| reachable.contains(b_id));
    if func.blocks.len() < initial_count {
        changed = true;
    }

    // 3. Compute surviving predecessors
    let mut preds: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for &b_id in func.blocks.keys() {
        preds.entry(b_id).or_default();
    }
    for (src, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch(dst) => {
                preds.entry(*dst).or_default().insert(*src);
            }
            Terminator::CondBranch { true_block, false_block, .. } => {
                preds.entry(*true_block).or_default().insert(*src);
                preds.entry(*false_block).or_default().insert(*src);
            }
            _ => {}
        }
        for (_, inst) in &block.instructions {
            if let Instruction::TryStart(catch_b) = inst {
                preds.entry(*catch_b).or_default().insert(*src);
            }
        }
    }

    // 4. Prune dead incoming edges from Phis and collapse single-predecessor Phis
    let mut phi_replacements: HashMap<ValueId, ValueId> = HashMap::new();
    for (&b_id, block) in func.blocks.iter_mut() {
        let p_set = &preds[&b_id];
        for (phi_val, inst) in &mut block.instructions {
            if let Instruction::Phi(operands) = inst {
                operands.retain(|(src, _)| p_set.contains(src));
                if operands.len() == 1 {
                    phi_replacements.insert(*phi_val, operands[0].1);
                    changed = true;
                } else if !operands.is_empty() {
                    let first_val = operands[0].1;
                    if operands.iter().all(|(_, v)| *v == first_val) {
                        phi_replacements.insert(*phi_val, first_val);
                        changed = true;
                    }
                }
            }
        }
    }

    if !phi_replacements.is_empty() {
        for block in func.blocks.values_mut() {
            for (_, inst) in &mut block.instructions {
                substitute_instruction(inst, &phi_replacements);
            }
            substitute_terminator(&mut block.terminator, &phi_replacements);
            block.instructions.retain(|(v, _)| !phi_replacements.contains_key(v));
        }
    }

    changed
}

fn dead_code_elimination(func: &mut FunctionIR) -> bool {
    let mut changed = false;
    let mut iterating = true;

    while iterating {
        iterating = false;
        let mut used = HashSet::new();

        for block in func.blocks.values() {
            for (_, inst) in &block.instructions {
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
                        used.insert(*l);
                        used.insert(*r);
                    }
                    Instruction::Call(callee, args) => {
                        used.insert(*callee);
                        for a in args {
                            used.insert(*a);
                        }
                    }
                    Instruction::GetProperty(obj, _)
                    | Instruction::Spawn(obj)
                    | Instruction::Await(obj)
                    | Instruction::StoreLocal(_, obj)
                    | Instruction::StoreGlobal(_, obj)
                    | Instruction::Cast(obj, _)
                    | Instruction::Print(obj) => {
                        used.insert(*obj);
                    }
                    Instruction::PropertySet(obj, _, val) => {
                        used.insert(*obj);
                        used.insert(*val);
                    }
                    Instruction::IndexSet(obj, idx, val) => {
                        used.insert(*obj);
                        used.insert(*idx);
                        used.insert(*val);
                    }
                    Instruction::BuildList(args) => {
                        for a in args {
                            used.insert(*a);
                        }
                    }
                    Instruction::BuildMap(args) => {
                        for (k, v) in args {
                            used.insert(*k);
                            used.insert(*v);
                        }
                    }
                    Instruction::Phi(operands) => {
                        for (_, v) in operands {
                            used.insert(*v);
                        }
                    }
                    Instruction::Constant(_)
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

            match &block.terminator {
                Terminator::CondBranch { condition, .. } => {
                    used.insert(*condition);
                }
                Terminator::Return(Some(v)) => {
                    used.insert(*v);
                }
                _ => {}
            }
        }

        for block in func.blocks.values_mut() {
            let mut kept = Vec::new();
            for (val_id, inst) in block.instructions.drain(..) {
                let has_side_effects = matches!(
                    inst,
                    Instruction::Call(_, _)
                        | Instruction::Spawn(_)
                        | Instruction::Print(_)
                        | Instruction::StoreGlobal(_, _)
                        | Instruction::StoreLocal(_, _)
                        | Instruction::IndexSet(_, _, _)
                        | Instruction::PropertySet(_, _, _)
                        | Instruction::MakeFunction(_, _)
                        | Instruction::MakeClass(_, _)
                        | Instruction::TryStart(_)
                        | Instruction::TryEnd
                        | Instruction::CatchException
                        | Instruction::ImportPython(_)
                );

                if used.contains(&val_id) || has_side_effects {
                    kept.push((val_id, inst));
                } else {
                    changed = true;
                    iterating = true;
                }
            }
            block.instructions = kept;
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    #[test]
    fn test_constant_folding() {
        let mut func = FunctionIR::new("test".to_string());
        let b0 = func.new_block();
        func.entry_block = b0;

        let v1 = func.new_value();
        let v2 = func.new_value();
        let v3 = func.new_value();

        let block = func.blocks.get_mut(&b0).unwrap();
        block.instructions.push((v1, Instruction::Constant(Value::Integer(10))));
        block.instructions.push((v2, Instruction::Constant(Value::Integer(20))));
        block.instructions.push((v3, Instruction::Add(v1, v2)));
        block.terminator = Terminator::Return(Some(v3));

        optimize(&mut func);

        let block = &func.blocks[&b0];
        // v1 and v2 should be dead code eliminated since v3 folded into constant 30
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(
            block.instructions[0].1,
            Instruction::Constant(Value::Integer(30))
        );
    }

    #[test]
    fn test_dead_branch_elimination() {
        let mut func = FunctionIR::new("test_branch".to_string());
        let b0 = func.new_block();
        let b1 = func.new_block();
        let b2 = func.new_block();
        func.entry_block = b0;

        let cond = func.new_value();
        let block0 = func.blocks.get_mut(&b0).unwrap();
        block0.instructions.push((cond, Instruction::Constant(Value::Boolean(true))));
        block0.terminator = Terminator::CondBranch {
            condition: cond,
            true_block: b1,
            false_block: b2,
        };

        let val1 = func.new_value();
        let block1 = func.blocks.get_mut(&b1).unwrap();
        block1.instructions.push((val1, Instruction::Constant(Value::Integer(42))));
        block1.terminator = Terminator::Return(Some(val1));

        let val2 = func.new_value();
        let block2 = func.blocks.get_mut(&b2).unwrap();
        block2.instructions.push((val2, Instruction::Constant(Value::Integer(99))));
        block2.terminator = Terminator::Return(Some(val2));

        optimize(&mut func);

        // b2 is dead and should be removed
        assert!(func.blocks.contains_key(&b0));
        assert!(func.blocks.contains_key(&b1));
        assert!(!func.blocks.contains_key(&b2));
        assert_eq!(func.blocks[&b0].terminator, Terminator::Branch(b1));
    }
}
