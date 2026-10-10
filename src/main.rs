mod check;
mod diagnostic;
mod eval;
mod lsp;
mod native;
mod python;
mod source;
mod syntax;

use diagnostic::Diagnostic;
use source::SourceMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const USAGE: &str = "\
Novo: a fast, statically typed language that reads like Python.

Usage:
    novo run <file.nv> [args...]     Check and run a program
             [--interp]              (without compiling anything to native code)
    novo check <file.nv> [--json]    Type-check without running
    novo test [path]                 Run test_*.nv and *_test.nv files
    novo bench <file.nv> [-n N]      Time repeated runs of a program
    novo new <name>                  Create a project
    novo fmt [paths...] [--check]    Format files in place (or list unformatted ones)
    novo [repl]                      Start the interactive prompt
    novo lsp                         Run the language server on stdio

Options:
    -h, --help       Show this help
    -V, --version    Show the version
";

fn main() {
    // candle otherwise re-reads /proc/cpuinfo to count cores on every matrix multiply.
    if std::env::var_os("RAYON_NUM_THREADS").is_none()
        && let Ok(n) = std::thread::available_parallelism()
    {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("RAYON_NUM_THREADS", n.to_string()) };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("repl") => cmd_repl(),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            0
        }
        Some("-V" | "--version" | "version") => {
            println!("novo {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("run") => cmd_run(&args[1..]),
        Some("check") => cmd_check(&args[1..]),
        Some("test") => cmd_test(&args[1..]),
        Some("bench") => cmd_bench(&args[1..]),
        Some("new") => cmd_new(&args[1..]),
        Some("fmt") => cmd_fmt(&args[1..]),
        Some("lsp") => lsp::serve(),
        Some(file) if file.ends_with(".nv") => cmd_run(&args),
        Some(other) => {
            eprintln!("error: unknown command `{other}`\n\n{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn render(sources: &SourceMap, diags: &[Diagnostic]) {
    let color = std::io::stderr().is_terminal();
    for d in diags {
        eprint!("{}", d.render(sources, color));
    }
}

fn file_arg<'a>(args: &'a [String], command: &str) -> Result<&'a String, i32> {
    args.iter().find(|a| !a.starts_with('-')).ok_or_else(|| {
        eprintln!("error: `novo {command}` needs a .nv file");
        2
    })
}

fn cmd_run(args: &[String]) -> i32 {
    let file = match file_arg(args, "run") {
        Ok(f) => f,
        Err(code) => return code,
    };
    let program_args: Vec<String> = args.iter().skip_while(|a| *a != file).cloned().collect();
    let native = !args
        .iter()
        .take_while(|a| *a != file)
        .any(|a| a == "--interp");
    let mut sources = SourceMap::default();
    let program = match check::check(&mut sources, Path::new(file)) {
        Ok(p) => p,
        Err(diags) => {
            render(&sources, &diags);
            return 1;
        }
    };
    match eval::run(program, program_args, native) {
        Ok(()) => 0,
        Err(e) => {
            render_runtime_error(&sources, e);
            1
        }
    }
}

fn render_runtime_error(sources: &SourceMap, e: eval::RtError) {
    let mut diag = Diagnostic::error(e.span, e.message);
    for (function, call) in &e.trace {
        let file = sources.get(call.file);
        let (line, col) = file.line_col(call.lo as usize);
        diag = diag.with_note(format!(
            "in `{function}`, called from {}:{line}:{col}",
            file.path.display()
        ));
    }
    render(sources, &[diag]);
}

fn cmd_fmt(args: &[String]) -> i32 {
    let check = args.iter().any(|a| a == "--check");
    let mut paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let mut files = Vec::new();
    for p in paths {
        collect_files(&p, &mut files, |p| p.extension().is_some_and(|e| e == "nv"));
    }
    let mut sources = SourceMap::default();
    let mut code = 0;
    for path in files {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", path.display());
                code = 1;
                continue;
            }
        };
        let file = sources.add(path.display().to_string(), text.clone());
        match syntax::format::format(file, &text) {
            Ok(formatted) if formatted == text => {}
            Ok(_) if check => {
                println!("{}", path.display());
                code = 1;
            }
            Ok(formatted) => {
                if let Err(e) = std::fs::write(&path, formatted) {
                    eprintln!("error: cannot write {}: {e}", path.display());
                    code = 1;
                }
            }
            Err(d) => {
                render(&sources, &[d]);
                code = 1;
            }
        }
    }
    code
}

/// Reads inputs from stdin and runs each against the state the earlier ones left.
fn cmd_repl() -> i32 {
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        println!(
            "Novo {} interactive prompt. Enter :quit or press Ctrl-D to leave.",
            env!("CARGO_PKG_VERSION")
        );
    }
    let mut checker = check::session::Session::default();
    let mut runner = eval::Session::default();
    let mut lines = std::io::stdin().lines();
    loop {
        let Some(input) = read_input(&mut lines, interactive) else {
            return 0;
        };
        match input.trim() {
            "" => continue,
            ":quit" | ":q" | "exit" => return 0,
            _ => {}
        }
        match checker.submit(&input) {
            Ok(snippet) => {
                if let Err(e) = runner.run(snippet.program, snippet.run) {
                    render_runtime_error(checker.sources(), e);
                }
            }
            Err(diags) => render(checker.sources(), &diags),
        }
    }
}

/// Reads one input: a line, or a block that continues until a blank line
/// when the first line opens one or leaves brackets unclosed.
fn read_input(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
    interactive: bool,
) -> Option<String> {
    use std::io::Write;
    let prompt = |p: &str| {
        if interactive {
            print!("{p}");
            let _ = std::io::stdout().flush();
        }
    };
    prompt(">>> ");
    let first = lines.next()?.ok()?;
    let mut text = first.clone() + "\n";
    let mut depth = bracket_depth(&first);
    let block = first.trim_end().ends_with(':');
    if depth > 0 || block {
        loop {
            prompt("... ");
            let Some(Ok(line)) = lines.next() else { break };
            if line.trim().is_empty() && depth <= 0 {
                break;
            }
            depth += bracket_depth(&line);
            text.push_str(&line);
            text.push('\n');
            if depth <= 0 && !block {
                break;
            }
        }
    }
    Some(text)
}

fn bracket_depth(line: &str) -> i32 {
    let mut depth = 0;
    let mut quote = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => break,
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn cmd_check(args: &[String]) -> i32 {
    let file = match file_arg(args, "check") {
        Ok(f) => f,
        Err(code) => return code,
    };
    let json = args.iter().any(|a| a == "--json");
    let mut sources = SourceMap::default();
    match check::check(&mut sources, Path::new(file)) {
        Ok(_) => {
            if !json {
                println!("{file}: ok");
            }
            0
        }
        Err(diags) => {
            if json {
                for d in &diags {
                    println!("{}", d.to_json(&sources));
                }
            } else {
                render(&sources, &diags);
            }
            1
        }
    }
}

fn is_test_file(path: &Path) -> bool {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    path.extension().is_some_and(|e| e == "nv")
        && (stem.starts_with("test_") || stem.ends_with("_test"))
}

/// Collects matching files under `path`, skipping hidden directories and build output.
fn collect_files(path: &Path, out: &mut Vec<PathBuf>, wanted: fn(&Path) -> bool) {
    if !path.is_dir() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for entry in entries {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if entry.is_dir() {
            if !name.starts_with('.') && name != "target" {
                collect_files(&entry, out, wanted);
            }
        } else if wanted(&entry) {
            out.push(entry);
        }
    }
}

fn cmd_test(args: &[String]) -> i32 {
    let target = PathBuf::from(
        args.iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
            .unwrap_or("tests"),
    );
    let mut files = Vec::new();
    collect_files(&target, &mut files, is_test_file);
    if files.is_empty() {
        eprintln!(
            "error: no test files (test_*.nv or *_test.nv) under {}",
            target.display()
        );
        return 1;
    }
    let exe = std::env::current_exe().expect("current executable");
    let mut failed = 0;
    for file in &files {
        let start = Instant::now();
        let output = Command::new(&exe).arg("run").arg(file).output();
        let ms = start.elapsed().as_millis();
        match output {
            Ok(out) if out.status.success() => println!("PASS {} ({ms} ms)", file.display()),
            Ok(out) => {
                failed += 1;
                println!("FAIL {} ({ms} ms)", file.display());
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {}: {e}", file.display());
            }
        }
    }
    println!("\n{} passed, {failed} failed", files.len() - failed);
    i32::from(failed > 0)
}

fn cmd_bench(args: &[String]) -> i32 {
    let file = match file_arg(args, "bench") {
        Ok(f) => f,
        Err(code) => return code,
    };
    let runs = args
        .iter()
        .position(|a| a == "-n")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);
    let exe = std::env::current_exe().expect("current executable");
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let status = Command::new(&exe)
            .arg("run")
            .arg(file)
            .stdout(std::process::Stdio::null())
            .status();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        match status {
            Ok(s) if s.success() => times.push(elapsed),
            _ => {
                eprintln!("error: `{file}` failed; run it with `novo run` to see why");
                return 1;
            }
        }
    }
    let mean = times.iter().sum::<f64>() / runs as f64;
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    let max = times.iter().copied().fold(0.0, f64::max);
    let sd = (times.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / runs as f64).sqrt();
    println!(
        "{file}: {runs} runs, mean {mean:.2} ms, min {min:.2} ms, max {max:.2} ms, std dev {sd:.2} ms"
    );
    0
}

fn cmd_new(args: &[String]) -> i32 {
    let Some(name) = args.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("error: `novo new` needs a project name");
        return 2;
    };
    let root = Path::new(name);
    if root.exists() {
        eprintln!("error: `{name}` already exists");
        return 1;
    }
    let files = [
        (
            "src/greeting.nv",
            "function greet(name: String) -> String:\n    return f\"Hello, {name}!\"\n".to_string(),
        ),
        (
            "src/main.nv",
            "use \"greeting.nv\"\n\nshow greeting.greet(\"Novo\")\n".to_string(),
        ),
        (
            "tests/greeting_test.nv",
            "use \"../src/greeting.nv\"\n\nassert_eq(greeting.greet(\"Ada\"), \"Hello, Ada!\")\n"
                .to_string(),
        ),
        (
            "novo.toml",
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"src/main.nv\"\n"),
        ),
        (".gitignore", "target/\n".to_string()),
    ];
    for (path, text) in &files {
        let path = root.join(path);
        let written = std::fs::create_dir_all(path.parent().expect("file has a parent"))
            .and_then(|_| std::fs::write(&path, text));
        if let Err(e) = written {
            eprintln!("error: cannot write {}: {e}", path.display());
            return 1;
        }
    }
    println!("Created {name}/. Try:\n    cd {name}\n    novo run src/main.nv\n    novo test");
    0
}
