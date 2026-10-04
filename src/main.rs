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
        print_error(&source, &e, "Runtime error");
    }
}

fn print_error(source: &str, error: &str, error_type: &str) {
    if let Some(pos) = error.rfind(" at line ") {
        let msg = &error[..pos];
        let rest = &error[pos + 9..];
        if let Some(comma_pos) = rest.find(", col ") {
            let line_str = &rest[..comma_pos];
            let rest_after_col = &rest[comma_pos + 6..];
            
            let (col_str, len_str) = if let Some(len_pos) = rest_after_col.find(", len ") {
                (&rest_after_col[..len_pos], Some(&rest_after_col[len_pos + 6..]))
            } else {
                (rest_after_col, None)
            };
            
            if let (Ok(line), Ok(col)) = (line_str.parse::<usize>(), col_str.parse::<usize>()) {
                let len = len_str.and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                let lines: Vec<&str> = source.lines().collect();
                if line > 0 && line <= lines.len() {
                    let type_prefix = if error_type.is_empty() { "ERROR".to_string() } else { error_type.to_uppercase() };
                    eprintln!("\n{}: {}\n", type_prefix, msg);
                    let line_text = lines[line - 1];
                    eprintln!("    {}", line_text);
                    let padding = " ".repeat(col.saturating_sub(1));
                    let carets = "^".repeat(len.max(1));
                    eprintln!("    {}{}", padding, carets);
                    return;
                }
            }
        }
    }
    eprintln!("{}: {}", error_type, error);
}
