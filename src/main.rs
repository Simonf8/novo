mod ast;
mod interpreter;
mod lexer;
mod parser;
mod vm;
mod compiler;
mod type_checker;
mod ir;
mod ir_builder;
mod ir_backend;

use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: novo <command> [options] <file.nv>");
        println!("Commands:");
        println!("  run    Run a .nv file (default)");
        println!("  check  Type check a .nv file");
        return;
    }

    let mut command = "run";
    let mut filename = "";
    let mut engine = "vm"; // Make VM the default engine

    let mut arg_idx = 1;
    if !args[arg_idx].starts_with("-") && !args[arg_idx].ends_with(".nv") {
        command = &args[arg_idx];
        arg_idx += 1;
    }

    for arg in args.iter().skip(arg_idx) {
        if arg.starts_with("--engine=") {
            engine = &arg[9..];
        } else if !arg.starts_with("-") {
            filename = arg;
        }
    }

    if filename.is_empty() {
        println!("Usage: novo {} [--engine=ast|vm] <file.nv>", command);
        return;
    }

    let source = fs::read_to_string(filename).expect("Failed to read file");

    let mut lexer = lexer::Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(e) => {
            print_error(&source, &e, "Lexer error");
            std::process::exit(1);
        }
    };

    let mut parser = parser::Parser::new(tokens);
    let ast = match parser.parse() {
        Ok(p) => p,
        Err(e) => {
            print_error(&source, &e, "Syntax error");
            std::process::exit(1);
        }
    };

    if command == "ir" {
        let mut builder = ir_builder::IRBuilder::new();
        match builder.build(&ast) {
            Ok(functions) => {
                for f in functions {
                    print!("{}", f);
                }
            }
            Err(e) => {
                print_error(&source, &e, "IR build error");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut type_checker = type_checker::TypeChecker::new();
    if let Err(e) = type_checker.check_program(&ast) {
        print_error(&source, &e, "Type error");
        std::process::exit(1);
    }

    if command == "check" {
        println!("Type checking completed successfully.");
        return;
    }

    if engine == "vm" {
        let mut builder = ir_builder::IRBuilder::new();
        let ir = match builder.build(&ast) {
            Ok(ir) => ir,
            Err(e) => {
                print_error(&source, &e, "IR Compilation error");
                std::process::exit(1);
            }
        };
        
        let backend = ir_backend::IRBackend::new(ir);
        match backend.generate() {
            Ok(chunk) => {
                let mut vm = vm::VM::new();
                match vm.interpret(chunk) {
                    vm::InterpretResult::Ok => {}
                    vm::InterpretResult::RuntimeError(e) => {
                        eprintln!("VM Runtime Error: {}", e);
                        print_error(&source, &e, "VM Runtime Error");
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                print_error(&source, &e, "Compiler error");
                std::process::exit(1);
            }
        }
    } else {
        let mut interpreter = interpreter::Interpreter::new();
        if let Err(e) = interpreter.interpret(&ast) {
            print_error(&source, &e, "Runtime error");
            std::process::exit(1);
        }
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
