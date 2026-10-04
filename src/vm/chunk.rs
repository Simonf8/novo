use super::opcode::OpCode;
use crate::interpreter::Value;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<OpCode>,
    pub constants: Vec<Value>,
    pub lines: Vec<usize>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            code: Vec::new(),
            constants: Vec::new(),
            lines: Vec::new(),
        }
    }

    pub fn write(&mut self, opcode: OpCode, line: usize) {
        self.code.push(opcode);
        self.lines.push(line);
    }

    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.push(value);
        self.constants.len() - 1
    }

    pub fn emit_jump(&mut self, opcode: OpCode, line: usize) -> usize {
        self.write(opcode, line);
        self.code.len() - 1
    }

    pub fn patch_jump(&mut self, offset: usize) {
        let jump = self.code.len() - 1 - offset;
        match &mut self.code[offset] {
            OpCode::JumpIfFalse(j) | OpCode::Jump(j) => {
                *j = jump;
            }
            _ => panic!("Attempted to patch a non-jump instruction"),
        }
    }
}
