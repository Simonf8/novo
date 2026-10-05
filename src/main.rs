mod ast;
mod interpreter;
mod lexer;
mod parser;
mod vm;
#[allow(dead_code)]
mod compiler;
mod type_checker;
mod ir;
mod ir_builder;
mod ir_ssa;
mod ir_opt;
mod ir_backend;
mod linter;
mod formatter;
pub mod stdlib;
pub mod jit;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn print_usage() {
    println!("Novo — Fast, intent-driven general-purpose programming language\n");
    println!("Usage:");
    println!("    novo [command] [options] <file.nv>\n");
    println!("Commands:");
    println!("    run <file.nv>             Execute a Novo program (default)");
    println!("    check <file.nv>           Validate syntax, types, and IR generation without executing");
    println!("    test [dir|file]           Discover and run test suite with differential verification");
    println!("    bench <file.nv>           Run repeated microbenchmark measurements");
    println!("    fmt <file|dir> [-w]       Format Novo source code to canonical style");
    println!("    lint <file|dir>           Run static analysis and linter for code health");
    println!("    new <project_name>        Scaffold a new Novo project");
    println!("    ir <file.nv>              Emit optimized SSA intermediate representation\n");
    println!("Options:");
    println!("    --write, -w                Write formatted code directly to file (for fmt)");
    println!("    --engine=<vm|ast|diff|jit> Execution engine (default: vm)");
    println!("    --iterations=<N>, -n <N>   Benchmark iterations (default: 5)");
    println!("    --help, -h                 Show this help message");
    println!("    --version, -v              Show version information");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return;
    }

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("novo 0.1.0");
        return;
    }

    let mut command = "run";
    let mut target = String::new();
    let mut engine = "vm";
    let mut iterations = 5usize;
    let mut write_in_place = false;

    let mut arg_idx = 1;
    let first = &args[1];
    if !first.starts_with('-') && !first.ends_with(".nv") {
        command = first.as_str();
        arg_idx = 2;
    }

    let mut i = arg_idx;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with("--engine=") {
            engine = &arg[9..];
        } else if arg.starts_with("--iterations=") {
            if let Ok(n) = arg[13..].parse::<usize>() {
                iterations = n.max(1);
            }
        } else if (arg == "-n" || arg == "--iterations") && i + 1 < args.len() {
            i += 1;
            if let Ok(n) = args[i].parse::<usize>() {
                iterations = n.max(1);
            }
        } else if arg == "--write" || arg == "-w" {
            write_in_place = true;
        } else if !arg.starts_with('-') {
            target = arg.clone();
        }
        i += 1;
    }

    match command {
        "run" => {
            if target.is_empty() {
                eprintln!("Error: 'run' requires a .nv file path.");
                std::process::exit(1);
            }
            execute_run(&target, engine);
        }
        "check" => {
            if target.is_empty() {
                eprintln!("Error: 'check' requires a .nv file path.");
                std::process::exit(1);
            }
            execute_check(&target);
        }
        "ir" => {
            if target.is_empty() {
                eprintln!("Error: 'ir' requires a .nv file path.");
                std::process::exit(1);
            }
            execute_ir(&target);
        }
        "test" => {
            let test_target = if target.is_empty() { "tests".to_string() } else { target };
            execute_test(&test_target, engine);
        }
        "bench" => {
            if target.is_empty() {
                eprintln!("Error: 'bench' requires a .nv file path.");
                std::process::exit(1);
            }
            execute_bench(&target, iterations, engine);
        }
        "fmt" => {
            if target.is_empty() {
                eprintln!("Error: 'fmt' requires a .nv file or directory path.");
                std::process::exit(1);
            }
            execute_fmt(&target, write_in_place);
        }
        "lint" => {
            if target.is_empty() {
                eprintln!("Error: 'lint' requires a .nv file or directory path.");
                std::process::exit(1);
            }
            execute_lint(&target);
        }
        "new" => {
            if target.is_empty() {
                eprintln!("Error: 'new' requires a project name.");
                std::process::exit(1);
            }
            execute_new(&target);
        }
        other => {
            eprintln!("Unknown command: '{}'. Run 'novo --help' for available commands.", other);
            std::process::exit(1);
        }
    }
}

fn execute_run(filename: &str, engine: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", filename, e);
            std::process::exit(1);
        }
    };

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

    let mut type_checker = type_checker::TypeChecker::new();
    if let Err(e) = type_checker.check_program(&ast) {
        print_error(&source, &e, "Type error");
        std::process::exit(1);
    }

    if engine == "jit" {
        let builder = ir_builder::IRBuilder::new();
        let ir = match builder.build(&ast) {
            Ok(ir) => ir,
            Err(e) => {
                print_error(&source, &e, "IR Compilation error");
                std::process::exit(1);
            }
        };

        if let Ok(mut jit) = jit::JitEngine::new() {
            for (i, func) in ir.iter().enumerate() {
                if jit::JitEngine::can_compile(func) {
                    let _ = jit.compile_function(i, func);
                }
            }
        }

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
    } else if engine == "vm" {
        let builder = ir_builder::IRBuilder::new();
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

fn execute_check(filename: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", filename, e);
            std::process::exit(1);
        }
    };

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

    let mut type_checker = type_checker::TypeChecker::new();
    if let Err(e) = type_checker.check_program(&ast) {
        print_error(&source, &e, "Type error");
        std::process::exit(1);
    }

    let builder = ir_builder::IRBuilder::new();
    let ir = match builder.build(&ast) {
        Ok(ir) => ir,
        Err(e) => {
            print_error(&source, &e, "IR Compilation error");
            std::process::exit(1);
        }
    };

    let num_funcs = ir.len();
    let num_blocks: usize = ir.iter().map(|f| f.blocks.len()).sum();
    let num_insts: usize = ir.iter().map(|f| f.blocks.values().map(|b| b.instructions.len()).sum::<usize>()).sum();

    let backend = ir_backend::IRBackend::new(ir);
    if let Err(e) = backend.generate() {
        print_error(&source, &e, "Codegen error");
        std::process::exit(1);
    }

    println!("Check passed: '{}' (AST: {} statements, IR: {} functions, {} blocks, {} instructions).",
             filename, ast.statements.len(), num_funcs, num_blocks, num_insts);
}

fn execute_ir(filename: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", filename, e);
            std::process::exit(1);
        }
    };

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

    let builder = ir_builder::IRBuilder::new();
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
}

fn discover_tests(target: &str) -> Vec<PathBuf> {
    let p = Path::new(target);
    let mut files = Vec::new();

    if p.is_file() {
        if target.ends_with(".nv") {
            files.push(p.to_path_buf());
        }
        return files;
    }

    if let Ok(entries) = fs::read_dir(p) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("nv") {
                files.push(path);
            }
        }
    }

    files.sort();
    files
}

fn execute_test(target: &str, engine_mode: &str) {
    let test_files = discover_tests(target);
    if test_files.is_empty() {
        eprintln!("No .nv test files found in target '{}'.", target);
        std::process::exit(1);
    }

    let current_exe = env::current_exe().expect("Failed to locate current executable");

    println!("Running {} test(s) in '{}' (mode: {})...\n", test_files.len(), target, engine_mode);

    let mut passed = 0;
    let mut failed = 0;
    let suite_start = Instant::now();

    for file in &test_files {
        let display_name = file.display().to_string();
        print!("test {:<40} ... ", display_name);

        let start = Instant::now();

        if engine_mode == "diff" {
            let ast_res = Command::new(&current_exe)
                .arg("run")
                .arg(file)
                .arg("--engine=ast")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();

            let vm_res = Command::new(&current_exe)
                .arg("run")
                .arg(file)
                .arg("--engine=vm")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();

            let elapsed = start.elapsed();

            match (ast_res, vm_res) {
                (Ok(ast_out), Ok(vm_out)) => {
                    let ast_exit = ast_out.status.code().unwrap_or(-1);
                    let vm_exit = vm_out.status.code().unwrap_or(-1);

                    let ast_stdout = String::from_utf8_lossy(&ast_out.stdout).trim().to_string();
                    let vm_stdout = String::from_utf8_lossy(&vm_out.stdout).trim().to_string();

                    if ast_exit != vm_exit {
                        println!("FAILED (exit code mismatch: AST={}, VM={}) [{} ms]",
                                 ast_exit, vm_exit, elapsed.as_millis());
                        failed += 1;
                    } else if ast_stdout != vm_stdout {
                        println!("FAILED (output mismatch) [{} ms]", elapsed.as_millis());
                        eprintln!("  --- AST stdout ---\n{}", ast_stdout);
                        eprintln!("  --- VM  stdout ---\n{}", vm_stdout);
                        failed += 1;
                    } else {
                        println!("ok [{} ms]", elapsed.as_millis());
                        passed += 1;
                    }
                }
                (Err(e), _) | (_, Err(e)) => {
                    println!("FAILED (process execution error: {})", e);
                    failed += 1;
                }
            }
        } else {
            let res = Command::new(&current_exe)
                .arg("run")
                .arg(file)
                .arg(format!("--engine={}", engine_mode))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();

            let elapsed = start.elapsed();

            match res {
                Ok(out) => {
                    let is_error_test = display_name.contains("error");
                    let success = if is_error_test {
                        !out.status.success()
                    } else {
                        out.status.success()
                    };

                    if success {
                        println!("ok [{} ms]", elapsed.as_millis());
                        passed += 1;
                    } else {
                        let err_msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        if is_error_test {
                            println!("FAILED (expected error, but succeeded) [{} ms]", elapsed.as_millis());
                        } else {
                            println!("FAILED (exit code: {}) [{} ms]",
                                     out.status.code().unwrap_or(-1), elapsed.as_millis());
                            if !err_msg.is_empty() {
                                eprintln!("  {}", err_msg);
                            }
                        }
                        failed += 1;
                    }
                }
                Err(e) => {
                    println!("FAILED (failed to execute: {})", e);
                    failed += 1;
                }
            }
        }
    }

    let total_elapsed = suite_start.elapsed();
    println!("\nTest result: {} passed, {} failed in {:.2}s", passed, failed, total_elapsed.as_secs_f64());

    if failed > 0 {
        std::process::exit(1);
    }
}

fn execute_bench(filename: &str, iterations: usize, engine: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", filename, e);
            std::process::exit(1);
        }
    };

    println!("Novo Benchmark: '{}' (engine: {}, iterations: {})", filename, engine, iterations);
    println!("{:-<60}", "");

    // 1. Measure compilation overhead
    let compile_start = Instant::now();
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
    let mut type_checker = type_checker::TypeChecker::new();
    if let Err(e) = type_checker.check_program(&ast) {
        print_error(&source, &e, "Type error");
        std::process::exit(1);
    }

    let compile_duration = compile_start.elapsed();
    println!("Compilation Time:       {:.3} ms", compile_duration.as_secs_f64() * 1000.0);

    // 2. Measure execution iterations
    let current_exe = env::current_exe().expect("Failed to locate current executable");
    let mut durations: Vec<f64> = Vec::with_capacity(iterations);

    for iter in 1..=iterations {
        let run_start = Instant::now();
        let status = Command::new(&current_exe)
            .arg("run")
            .arg(filename)
            .arg(format!("--engine={}", engine))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status();

        let elapsed_ms = run_start.elapsed().as_secs_f64() * 1000.0;

        match status {
            Ok(s) if s.success() => {
                durations.push(elapsed_ms);
            }
            Ok(s) => {
                eprintln!("Iteration {} failed with exit code {}", iter, s.code().unwrap_or(-1));
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Iteration {} failed to run: {}", iter, e);
                std::process::exit(1);
            }
        }
    }

    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let count = durations.len() as f64;
    let min = durations.first().copied().unwrap_or(0.0);
    let max = durations.last().copied().unwrap_or(0.0);
    let sum: f64 = durations.iter().sum();
    let mean = sum / count;

    let median = if durations.len() % 2 == 1 {
        durations[durations.len() / 2]
    } else {
        (durations[durations.len() / 2 - 1] + durations[durations.len() / 2]) / 2.0
    };

    let variance = durations.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / count;
    let std_dev = variance.sqrt();

    println!("Execution Statistics ({} runs):", iterations);
    println!("  Mean:                 {:.3} ms (±{:.3} ms)", mean, std_dev);
    println!("  Median:               {:.3} ms", median);
    println!("  Min:                  {:.3} ms", min);
    println!("  Max:                  {:.3} ms", max);
    println!("{:-<60}", "");
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

fn execute_fmt(target: &str, write_in_place: bool) {
    let files = discover_tests(target);
    if files.is_empty() {
        eprintln!("Error: No .nv files found in '{}'", target);
        std::process::exit(1);
    }

    let mut formatted_count = 0;
    let mut unchanged_count = 0;

    for file in &files {
        let filename = file.display().to_string();
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading '{}': {}", filename, e);
                continue;
            }
        };

        let mut lexer = lexer::Lexer::new(&source);
        let tokens = match lexer.tokenize() {
            Ok(t) => t,
            Err(e) => {
                print_error(&source, &e, "Lexer error");
                continue;
            }
        };

        let mut parser = parser::Parser::new(tokens);
        let ast = match parser.parse() {
            Ok(p) => p,
            Err(e) => {
                print_error(&source, &e, "Syntax error");
                continue;
            }
        };

        let formatted = formatter::Formatter::new().format_program(&ast);

        if write_in_place {
            if formatted.trim() != source.trim() {
                if let Err(e) = fs::write(file, &formatted) {
                    eprintln!("Error writing '{}': {}", filename, e);
                } else {
                    println!("Formatted: {}", filename);
                    formatted_count += 1;
                }
            } else {
                unchanged_count += 1;
            }
        } else if files.len() == 1 {
            print!("{}", formatted);
        } else if formatted.trim() != source.trim() {
            println!("Needs formatting: {}", filename);
            formatted_count += 1;
        } else {
            unchanged_count += 1;
        }
    }

    if write_in_place || files.len() > 1 {
        println!("Fmt complete: {} formatted, {} unchanged.", formatted_count, unchanged_count);
    }
}

fn execute_lint(target: &str) {
    let files = discover_tests(target);
    if files.is_empty() {
        eprintln!("Error: No .nv files found in '{}'", target);
        std::process::exit(1);
    }

    let mut total_warnings = 0;
    let mut total_errors = 0;

    for file in &files {
        let filename = file.display().to_string();
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading '{}': {}", filename, e);
                continue;
            }
        };

        let mut lexer = lexer::Lexer::new(&source);
        let tokens = match lexer.tokenize() {
            Ok(t) => t,
            Err(e) => {
                print_error(&source, &e, "Lexer error");
                total_errors += 1;
                continue;
            }
        };

        let mut parser = parser::Parser::new(tokens);
        let ast = match parser.parse() {
            Ok(p) => p,
            Err(e) => {
                print_error(&source, &e, "Syntax error");
                total_errors += 1;
                continue;
            }
        };

        let mut linter = linter::Linter::new();
        let diagnostics = linter.lint_program(&ast);

        if !diagnostics.is_empty() {
            let lines: Vec<&str> = source.lines().collect();
            for diag in &diagnostics {
                let sev_str = match diag.severity {
                    linter::LintSeverity::Warning => {
                        total_warnings += 1;
                        "warning"
                    }
                    linter::LintSeverity::Error => {
                        total_errors += 1;
                        "error"
                    }
                    linter::LintSeverity::Hint => "hint",
                };

                println!("{}:{}:{}: {}: {}", filename, diag.span.line, diag.span.column, sev_str, diag.message);
                if diag.span.line > 0 && diag.span.line <= lines.len() {
                    let line_content = lines[diag.span.line - 1];
                    println!("    {}", line_content);
                    let padding = " ".repeat(diag.span.column.saturating_sub(1));
                    let carets = "^".repeat(diag.span.length.max(1));
                    println!("    {}{}", padding, carets);
                }
                if let Some(ref sug) = diag.suggestion {
                    println!("    Help: {}\n", sug);
                }
            }
        }
    }

    if total_warnings == 0 && total_errors == 0 {
        println!("All clean! No lint issues found in {} file(s).", files.len());
    } else {
        println!("Lint finished with {} warning(s), {} error(s).", total_warnings, total_errors);
        if total_errors > 0 {
            std::process::exit(1);
        }
    }
}

fn execute_new(project_name: &str) {
    let p = Path::new(project_name);
    if p.exists() {
        eprintln!("Error: Directory '{}' already exists.", project_name);
        std::process::exit(1);
    }

    let src_dir = p.join("src");
    let tests_dir = p.join("tests");

    if let Err(e) = fs::create_dir_all(&src_dir) {
        eprintln!("Failed to create '{}/src': {}", project_name, e);
        std::process::exit(1);
    }
    if let Err(e) = fs::create_dir_all(&tests_dir) {
        eprintln!("Failed to create '{}/tests': {}", project_name, e);
        std::process::exit(1);
    }

    let toml_content = format!(
r#"[package]
name = "{}"
version = "0.1.0"
description = "A modern Novo application"
entry = "src/main.nv"

[dependencies]
"#, project_name);

    let main_nv = r#"# Main entry point for the Novo application

function main():
    show "Hello, Novo!"

main()
"#;

    let test_nv = r#"# Starter test suite

function test_example():
    val = 21 * 2
    show val

test_example()
"#;

    let gitignore = r#"target/
*.log
"#;

    let _ = fs::write(p.join("novo.toml"), toml_content);
    let _ = fs::write(src_dir.join("main.nv"), main_nv);
    let _ = fs::write(tests_dir.join("main_test.nv"), test_nv);
    let _ = fs::write(p.join(".gitignore"), gitignore);

    println!("Created new Novo project '{}':", project_name);
    println!("    {}/novo.toml", project_name);
    println!("    {}/src/main.nv", project_name);
    println!("    {}/tests/main_test.nv", project_name);
    println!("    {}/.gitignore", project_name);
    println!("\nTo get started:\n    cd {}\n    novo run src/main.nv\n    novo test tests", project_name);
}

