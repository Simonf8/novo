use crate::ir::{BlockId, FunctionIR, Instruction, Terminator};
use std::collections::HashMap;

#[cfg(target_os = "windows")]
const MEM_COMMIT: u32 = 0x00001000;
#[cfg(target_os = "windows")]
const MEM_RESERVE: u32 = 0x00002000;
#[cfg(target_os = "windows")]
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
#[cfg(target_os = "windows")]
const MEM_RELEASE: u32 = 0x00008000;

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(
        lpAddress: *mut std::ffi::c_void,
        dwSize: usize,
        flAllocationType: u32,
        flProtect: u32,
    ) -> *mut std::ffi::c_void;

    fn VirtualFree(
        lpAddress: *mut std::ffi::c_void,
        dwSize: usize,
        dwFreeType: u32,
    ) -> i32;
}

pub struct ExecutableBuffer {
    ptr: *mut u8,
    size: usize,
    capacity: usize,
}

impl ExecutableBuffer {
    pub fn new(capacity: usize) -> Result<Self, String> {
        let actual_cap = capacity.max(4096);
        #[cfg(target_os = "windows")]
        let ptr = unsafe {
            VirtualAlloc(
                std::ptr::null_mut(),
                actual_cap,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            ) as *mut u8
        };

        #[cfg(not(target_os = "windows"))]
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                actual_cap,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            ) as *mut u8
        };

        if ptr.is_null() {
            return Err("Failed to allocate executable memory for JIT".to_string());
        }

        Ok(ExecutableBuffer {
            ptr,
            size: 0,
            capacity: actual_cap,
        })
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    pub fn emit_u8(&mut self, b: u8) {
        assert!(self.size < self.capacity, "JIT buffer overflow");
        unsafe {
            *self.ptr.add(self.size) = b;
        }
        self.size += 1;
    }

    pub fn emit_slice(&mut self, slice: &[u8]) {
        for &b in slice {
            self.emit_u8(b);
        }
    }

    pub fn emit_u32(&mut self, val: u32) {
        self.emit_slice(&val.to_le_bytes());
    }

    pub fn emit_i32(&mut self, val: i32) {
        self.emit_slice(&val.to_le_bytes());
    }

    pub fn emit_u64(&mut self, val: u64) {
        self.emit_slice(&val.to_le_bytes());
    }

    pub fn emit_i64(&mut self, val: i64) {
        self.emit_slice(&val.to_le_bytes());
    }

    pub fn patch_i32(&mut self, offset: usize, val: i32) {
        assert!(offset + 4 <= self.size);
        let bytes = val.to_le_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.add(offset), 4);
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

impl Drop for ExecutableBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            #[cfg(target_os = "windows")]
            unsafe {
                VirtualFree(self.ptr as *mut std::ffi::c_void, 0, MEM_RELEASE);
            }
            #[cfg(not(target_os = "windows"))]
            unsafe {
                libc::munmap(self.ptr as *mut std::ffi::c_void, self.capacity);
            }
        }
    }
}

unsafe impl Send for ExecutableBuffer {}
unsafe impl Sync for ExecutableBuffer {}

pub struct JitEngine {
    buffer: ExecutableBuffer,
    function_offsets: HashMap<usize, usize>,
}

impl JitEngine {
    pub fn new() -> Result<Self, String> {
        let buffer = ExecutableBuffer::new(65536)?;
        Ok(JitEngine {
            buffer,
            function_offsets: HashMap::new(),
        })
    }

    pub fn can_compile(func: &FunctionIR) -> bool {
        for block in func.blocks.values() {
            for (_, inst) in &block.instructions {
                match inst {
                    Instruction::Constant(crate::interpreter::Value::Integer(_))
                    | Instruction::Add(_, _)
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
                    | Instruction::Call(_, _)
                    | Instruction::Phi(_) => {}
                    _ => return false,
                }
            }
            match &block.terminator {
                Terminator::Branch(_)
                | Terminator::CondBranch { .. }
                | Terminator::Return(_) => {}
                _ => return false,
            }
        }
        true
    }

    pub fn compile_function(
        &mut self,
        func_idx: usize,
        func: &FunctionIR,
    ) -> Result<usize, String> {
        let func_offset = self.buffer.len();
        self.function_offsets.insert(func_idx, func_offset);

        // Find max ValueId to size stack frame
        let mut max_val = func.params.len();
        for block in func.blocks.values() {
            for (val_id, _) in &block.instructions {
                if val_id.0 >= max_val {
                    max_val = val_id.0 + 1;
                }
            }
        }

        // Align stack frame to 16 bytes: (max_val * 8 + 32 shadow space) aligned
        let frame_bytes = ((max_val * 8 + 32 + 15) / 16) * 16;

        // Function Prologue:
        // push rbp
        // mov rbp, rsp
        // sub rsp, frame_bytes
        self.buffer.emit_u8(0x55); // push rbp
        self.buffer.emit_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        self.buffer.emit_slice(&[0x48, 0x81, 0xEC]); // sub rsp, imm32
        self.buffer.emit_u32(frame_bytes as u32);

        // Save incoming parameters from Windows x64 ABI (RCX, RDX, R8, R9) to slots
        // ValueId(i) maps to [rbp - 8 * (i + 1)]
        let slot_offset = |id: usize| -> i32 { -((id as i32 + 1) * 8) };

        for (i, _) in func.params.iter().enumerate() {
            let offset = slot_offset(i);
            match i {
                0 => {
                    // mov [rbp + offset], rcx
                    self.emit_mov_to_slot(offset, 1);
                }
                1 => {
                    // mov [rbp + offset], rdx
                    self.emit_mov_to_slot(offset, 2);
                }
                2 => {
                    // mov [rbp + offset], r8
                    self.emit_mov_reg8_to_slot(offset);
                }
                3 => {
                    // mov [rbp + offset], r9
                    self.emit_mov_reg9_to_slot(offset);
                }
                _ => {}
            }
        }

        let mut block_offsets: HashMap<BlockId, usize> = HashMap::new();
        let mut pending_jumps: Vec<(usize, BlockId)> = Vec::new();
        let mut pending_calls: Vec<(usize, usize)> = Vec::new();

        // Process entry block first, then others
        let mut block_order = vec![func.entry_block];
        for &id in func.blocks.keys() {
            if id != func.entry_block {
                block_order.push(id);
            }
        }

        for &block_id in &block_order {
            let block = &func.blocks[&block_id];
            block_offsets.insert(block_id, self.buffer.len());

            for (val_id, inst) in &block.instructions {
                let dst_off = slot_offset(val_id.0);

                match inst {
                    Instruction::Constant(crate::interpreter::Value::Integer(imm)) => {
                        // mov rax, imm64
                        self.buffer.emit_slice(&[0x48, 0xB8]);
                        self.buffer.emit_i64(*imm);
                        // mov [rbp + dst_off], rax
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Add(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        // add rax, rcx (48 01 C8)
                        self.buffer.emit_slice(&[0x48, 0x01, 0xC8]);
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Sub(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        // sub rax, rcx (48 29 C8)
                        self.buffer.emit_slice(&[0x48, 0x29, 0xC8]);
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Mul(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        // imul rax, rcx (48 0F AF C1)
                        self.buffer.emit_slice(&[0x48, 0x0F, 0xAF, 0xC1]);
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Div(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        // cqo (48 99)
                        self.buffer.emit_slice(&[0x48, 0x99]);
                        // idiv rcx (48 F7 F9)
                        self.buffer.emit_slice(&[0x48, 0xF7, 0xF9]);
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Mod(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        self.buffer.emit_slice(&[0x48, 0x99]);
                        self.buffer.emit_slice(&[0x48, 0xF7, 0xF9]);
                        // remainder is in rdx
                        self.emit_mov_to_slot(dst_off, 2);
                    }
                    Instruction::Lt(a, b)
                    | Instruction::Le(a, b)
                    | Instruction::Gt(a, b)
                    | Instruction::Ge(a, b)
                    | Instruction::Eq(a, b)
                    | Instruction::Neq(a, b) => {
                        self.emit_load_slot_to_rax(slot_offset(a.0));
                        self.emit_load_slot_to_rcx(slot_offset(b.0));
                        // cmp rax, rcx (48 39 C8)
                        self.buffer.emit_slice(&[0x48, 0x39, 0xC8]);
                        match inst {
                            Instruction::Lt(_, _) => self.buffer.emit_slice(&[0x0F, 0x9C, 0xC0]), // setl al
                            Instruction::Le(_, _) => self.buffer.emit_slice(&[0x0F, 0x9E, 0xC0]), // setle al
                            Instruction::Gt(_, _) => self.buffer.emit_slice(&[0x0F, 0x9F, 0xC0]), // setg al
                            Instruction::Ge(_, _) => self.buffer.emit_slice(&[0x0F, 0x9D, 0xC0]), // setge al
                            Instruction::Eq(_, _) => self.buffer.emit_slice(&[0x0F, 0x94, 0xC0]), // sete al
                            Instruction::Neq(_, _) => self.buffer.emit_slice(&[0x0F, 0x95, 0xC0]), // setne al
                            _ => unreachable!(),
                        }
                        // movzx rax, al (48 0F B6 C0)
                        self.buffer.emit_slice(&[0x48, 0x0F, 0xB6, 0xC0]);
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Call(callee, args) => {
                        // Load arguments into registers
                        for (i, arg) in args.iter().enumerate() {
                            let arg_off = slot_offset(arg.0);
                            match i {
                                0 => self.emit_load_slot_to_rcx(arg_off),
                                1 => self.emit_load_slot_to_rdx(arg_off),
                                2 => self.emit_load_slot_to_r8(arg_off),
                                3 => self.emit_load_slot_to_r9(arg_off),
                                _ => {}
                            }
                        }

                        // call rel32 (E8 rel32)
                        self.buffer.emit_u8(0xE8);
                        let call_site = self.buffer.len();
                        self.buffer.emit_i32(0); // placeholder
                        pending_calls.push((call_site, callee.0));

                        // Store return value from RAX
                        self.emit_mov_rax_to_slot(dst_off);
                    }
                    Instruction::Phi(incoming) => {
                        // Phi node is placed into dst_off from active incoming path
                        if let Some((_, src)) = incoming.first() {
                            self.emit_load_slot_to_rax(slot_offset(src.0));
                            self.emit_mov_rax_to_slot(dst_off);
                        }
                    }
                    _ => return Err(format!("Unsupported JIT instruction: {:?}", inst)),
                }
            }

            match &block.terminator {
                Terminator::Branch(target) => {
                    self.buffer.emit_u8(0xE9); // jmp rel32
                    let jump_site = self.buffer.len();
                    self.buffer.emit_i32(0);
                    pending_jumps.push((jump_site, *target));
                }
                Terminator::CondBranch { condition, true_block, false_block } => {
                    self.emit_load_slot_to_rax(slot_offset(condition.0));
                    // test rax, rax (48 85 C0)
                    self.buffer.emit_slice(&[0x48, 0x85, 0xC0]);
                    // jnz rel32 (0F 85 rel32)
                    self.buffer.emit_slice(&[0x0F, 0x85]);
                    let then_jump = self.buffer.len();
                    self.buffer.emit_i32(0);
                    pending_jumps.push((then_jump, *true_block));

                    // jmp rel32 to else_b
                    self.buffer.emit_u8(0xE9);
                    let else_jump = self.buffer.len();
                    self.buffer.emit_i32(0);
                    pending_jumps.push((else_jump, *false_block));
                }
                Terminator::Return(val) => {
                    if let Some(v) = val {
                        self.emit_load_slot_to_rax(slot_offset(v.0));
                    } else {
                        // xor rax, rax
                        self.buffer.emit_slice(&[0x48, 0x31, 0xC0]);
                    }
                    // Epilogue:
                    // mov rsp, rbp (48 89 EC)
                    // pop rbp (5D)
                    // ret (C3)
                    self.buffer.emit_slice(&[0x48, 0x89, 0xEC, 0x5D, 0xC3]);
                }
                _ => return Err("Unsupported JIT terminator".to_string()),
            }
        }

        // Patch pending jumps
        for (site, target) in pending_jumps {
            if let Some(&target_off) = block_offsets.get(&target) {
                let rel = (target_off as i64) - ((site + 4) as i64);
                self.buffer.patch_i32(site, rel as i32);
            } else {
                return Err(format!("Unresolved JIT block target {:?}", target));
            }
        }

        // Patch recursive self calls or known function calls
        for (site, target_func_idx) in pending_calls {
            let target_off = if target_func_idx == func_idx {
                func_offset
            } else if let Some(&off) = self.function_offsets.get(&target_func_idx) {
                off
            } else {
                func_offset // default to self for recursive functions
            };
            let rel = (target_off as i64) - ((site + 4) as i64);
            self.buffer.patch_i32(site, rel as i32);
        }

        Ok(func_offset)
    }

    fn emit_mov_rax_to_slot(&mut self, offset: i32) {
        // mov [rbp + offset], rax (48 89 85 offset32)
        self.buffer.emit_slice(&[0x48, 0x89, 0x85]);
        self.buffer.emit_i32(offset);
    }

    fn emit_mov_to_slot(&mut self, offset: i32, reg: u8) {
        // reg: 1 = rcx (48 89 8D), 2 = rdx (48 89 95)
        match reg {
            1 => self.buffer.emit_slice(&[0x48, 0x89, 0x8D]),
            2 => self.buffer.emit_slice(&[0x48, 0x89, 0x95]),
            _ => panic!("Unsupported reg"),
        }
        self.buffer.emit_i32(offset);
    }

    fn emit_mov_reg8_to_slot(&mut self, offset: i32) {
        // mov [rbp + offset], r8 (4C 89 85 offset32)
        self.buffer.emit_slice(&[0x4C, 0x89, 0x85]);
        self.buffer.emit_i32(offset);
    }

    fn emit_mov_reg9_to_slot(&mut self, offset: i32) {
        // mov [rbp + offset], r9 (4C 89 8D offset32)
        self.buffer.emit_slice(&[0x4C, 0x89, 0x8D]);
        self.buffer.emit_i32(offset);
    }

    fn emit_load_slot_to_rax(&mut self, offset: i32) {
        // mov rax, [rbp + offset] (48 8B 85 offset32)
        self.buffer.emit_slice(&[0x48, 0x8B, 0x85]);
        self.buffer.emit_i32(offset);
    }

    fn emit_load_slot_to_rcx(&mut self, offset: i32) {
        // mov rcx, [rbp + offset] (48 8B 8D offset32)
        self.buffer.emit_slice(&[0x48, 0x8B, 0x8D]);
        self.buffer.emit_i32(offset);
    }

    fn emit_load_slot_to_rdx(&mut self, offset: i32) {
        // mov rdx, [rbp + offset] (48 8B 95 offset32)
        self.buffer.emit_slice(&[0x48, 0x8B, 0x95]);
        self.buffer.emit_i32(offset);
    }

    fn emit_load_slot_to_r8(&mut self, offset: i32) {
        // mov r8, [rbp + offset] (4C 8B 85 offset32)
        self.buffer.emit_slice(&[0x4C, 0x8B, 0x85]);
        self.buffer.emit_i32(offset);
    }

    fn emit_load_slot_to_r9(&mut self, offset: i32) {
        // mov r9, [rbp + offset] (4C 8B 8D offset32)
        self.buffer.emit_slice(&[0x4C, 0x8B, 0x8D]);
        self.buffer.emit_i32(offset);
    }

    pub fn execute_fn1(&self, offset: usize, arg: i64) -> i64 {
        unsafe {
            let func_ptr = self.buffer.as_ptr().add(offset);
            let func: extern "C" fn(i64) -> i64 = std::mem::transmute(func_ptr);
            func(arg)
        }
    }

    pub fn execute_fn2(&self, offset: usize, a: i64, b: i64) -> i64 {
        unsafe {
            let func_ptr = self.buffer.as_ptr().add(offset);
            let func: extern "C" fn(i64, i64) -> i64 = std::mem::transmute(func_ptr);
            func(a, b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn add_inst(func: &mut FunctionIR, block: BlockId, inst: Instruction) -> ValueId {
        let val = func.new_value();
        func.blocks.get_mut(&block).unwrap().instructions.push((val, inst));
        val
    }

    fn set_term(func: &mut FunctionIR, block: BlockId, term: Terminator) {
        func.blocks.get_mut(&block).unwrap().terminator = term;
    }

    #[test]
    fn test_jit_arithmetic_execution() {
        let mut func = FunctionIR::new("test_calc".to_string());
        let x = func.new_value();
        func.params.push(x);
        let b0 = func.new_block();
        func.entry_block = b0;
        
        // (x * 3) + 7
        let c3 = add_inst(&mut func, b0, Instruction::Constant(crate::interpreter::Value::Integer(3)));
        let mul = add_inst(&mut func, b0, Instruction::Mul(x, c3));
        let c7 = add_inst(&mut func, b0, Instruction::Constant(crate::interpreter::Value::Integer(7)));
        let res = add_inst(&mut func, b0, Instruction::Add(mul, c7));
        set_term(&mut func, b0, Terminator::Return(Some(res)));

        let mut jit = JitEngine::new().unwrap();
        assert!(JitEngine::can_compile(&func));
        let offset = jit.compile_function(0, &func).unwrap();

        // 10 * 3 + 7 = 37
        let output = jit.execute_fn1(offset, 10);
        assert_eq!(output, 37);

        // 5 * 3 + 7 = 22
        let output2 = jit.execute_fn1(offset, 5);
        assert_eq!(output2, 22);
    }

    #[test]
    fn test_jit_recursive_fibonacci() {
        // function fib(n):
        //   if n <= 1: return n
        //   return fib(n - 1) + fib(n - 2)
        let mut func = FunctionIR::new("fib".to_string());
        let n = func.new_value();
        func.params.push(n);

        let b_entry = func.new_block();
        let b_then = func.new_block();
        let b_else = func.new_block();
        func.entry_block = b_entry;

        let c1 = add_inst(&mut func, b_entry, Instruction::Constant(crate::interpreter::Value::Integer(1)));
        let cond = add_inst(&mut func, b_entry, Instruction::Le(n, c1));
        set_term(&mut func, b_entry, Terminator::CondBranch {
            condition: cond,
            true_block: b_then,
            false_block: b_else,
        });

        // b_then: return n
        set_term(&mut func, b_then, Terminator::Return(Some(n)));

        // b_else: fib(n - 1) + fib(n - 2)
        let sub1 = add_inst(&mut func, b_else, Instruction::Sub(n, c1));
        let call1 = add_inst(&mut func, b_else, Instruction::Call(ValueId(0), vec![sub1]));
        let c2 = add_inst(&mut func, b_else, Instruction::Constant(crate::interpreter::Value::Integer(2)));
        let sub2 = add_inst(&mut func, b_else, Instruction::Sub(n, c2));
        let call2 = add_inst(&mut func, b_else, Instruction::Call(ValueId(0), vec![sub2]));
        let sum = add_inst(&mut func, b_else, Instruction::Add(call1, call2));
        set_term(&mut func, b_else, Terminator::Return(Some(sum)));

        let mut jit = JitEngine::new().unwrap();
        assert!(JitEngine::can_compile(&func));
        let offset = jit.compile_function(0, &func).unwrap();

        // fib(10) = 55
        let fib10 = jit.execute_fn1(offset, 10);
        assert_eq!(fib10, 55);

        // fib(20) = 6765
        let fib20 = jit.execute_fn1(offset, 20);
        assert_eq!(fib20, 6765);
    }
}
