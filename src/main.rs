mod ast;
mod interpreter;
mod lexer;
mod parser;

use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: novo <file.nv>");
        return;
    }

    let filename = &args[1];
    let source = fs::read_to_string(filename).expect("Failed to read file");

    let mut lexer = lexer::Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(e) => {
            print_error(&source, &e, "Lexer error");
            return;
        }
    };

    let mut parser = parser::Parser::new(tokens);
    let ast = match parser.parse() {
        Ok(p) => p,
        Err(e) => {
            print_error(&source, &e, "Syntax error");
            return;
        }
    };

    let mut interpreter = interpreter::Interpreter::new();
    if let Err(e) = interpreter.interpret(&ast) {
        eprintln!("Runtime error: {}", e);
    }
}

fn print_error(source: &str, error: &str, error_type: &str) {
    if let Some(pos) = error.rfind(" at line ") {
        let msg = &error[..pos];
        let rest = &error[pos + 9..];
        if let Some(comma_pos) = rest.find(", col ") {
            let line_str = &rest[..comma_pos];
            let col_str = &rest[comma_pos + 6..];
            if let (Ok(line), Ok(col)) = (line_str.parse::<usize>(), col_str.parse::<usize>()) {
                let lines: Vec<&str> = source.lines().collect();
                if line > 0 && line <= lines.len() {
                    eprintln!("\n{}: {}\n", error_type, msg);
                    let line_text = lines[line - 1];
                    eprintln!("{:>4} | {}", line, line_text);
                    let padding = " ".repeat(col.saturating_sub(1));
                    eprintln!("     | {}^", padding);
                    return;
                }
            }
        }
    }
    eprintln!("{}: {}", error_type, error);
}
