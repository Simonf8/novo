//! Runs every program in `tests/programs` on each engine and compares stdout
//! with the matching `.out` file. Set `NOVO_BLESS=1` to rewrite the `.out`
//! files from the VM's output.
//!
//! Leading comment directives:
//!   `# engines: interp`     engines to run (default: all)
//!   `# requires: python`    skip unless built with `--features python`
//!   `# expect-error: text`  must exit non-zero with `text` in stderr
//!   `# xfail: interp`       known to fail on these engines

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Engines and the extra `novo run` flags that select them.
const ENGINES: &[(&str, &[&str])] = &[("native", &[]), ("interp", &["--interp"])];

#[derive(Default)]
struct Directives {
    engines: Option<Vec<String>>,
    requires: Vec<String>,
    expect_error: Option<String>,
    xfail: Vec<String>,
}

fn directives(source: &str) -> Directives {
    let mut d = Directives::default();
    for line in source.lines() {
        let Some(comment) = line.trim().strip_prefix('#') else {
            break;
        };
        let Some((key, value)) = comment.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "engines" => d.engines = Some(value.split_whitespace().map(String::from).collect()),
            "requires" => d
                .requires
                .extend(value.split_whitespace().map(String::from)),
            "expect-error" => d.expect_error = Some(value.to_string()),
            "xfail" => d.xfail.extend(value.split_whitespace().map(String::from)),
            _ => {}
        }
    }
    d
}

fn feature_enabled(name: &str) -> bool {
    match name {
        "python" => cfg!(feature = "python"),
        _ => false,
    }
}

fn programs() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let mut files: Vec<_> = fs::read_dir(dir)
        .expect("tests/programs exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "nv"))
        .collect();
    files.sort();
    files
}

fn check(path: &Path, bless: bool) -> Result<(), String> {
    let source = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let d = directives(&source);
    if d.requires.iter().any(|r| !feature_enabled(r)) {
        return Ok(());
    }
    let expected_path = path.with_extension("out");
    let engines: Vec<String> = d
        .engines
        .clone()
        .unwrap_or_else(|| ENGINES.iter().map(|(name, _)| name.to_string()).collect());

    let mut errors = Vec::new();
    for engine in &engines {
        let bless = bless && d.xfail.is_empty() && engine == &engines[0];
        let result = run_engine(path, engine, &d, bless, &expected_path);
        match (result, d.xfail.contains(engine)) {
            (Err(e), false) => errors.push(e),
            (Ok(()), true) => errors.push(format!("[{engine}] passes now; remove it from `xfail`")),
            _ => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn run_engine(
    path: &Path,
    engine: &str,
    d: &Directives,
    bless: bool,
    expected_path: &Path,
) -> Result<(), String> {
    let flags = ENGINES
        .iter()
        .find(|(name, _)| *name == engine)
        .map(|(_, f)| *f)
        .ok_or_else(|| format!("unknown engine `{engine}`"))?;
    let output = Command::new(env!("CARGO_BIN_EXE_novo"))
        .arg("run")
        .args(flags)
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr);

    match &d.expect_error {
        Some(needle) => {
            if output.status.success() {
                return Err(format!(
                    "[{engine}] expected an error containing {needle:?}, but the program succeeded"
                ));
            }
            if !stderr.contains(needle.as_str()) {
                return Err(format!(
                    "[{engine}] expected stderr to contain {needle:?}, got:\n{stderr}"
                ));
            }
        }
        None if !output.status.success() => {
            return Err(format!(
                "[{engine}] exited with {}:\n{stderr}",
                output.status
            ));
        }
        None => {}
    }

    if bless {
        fs::write(expected_path, &stdout).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let expected = fs::read_to_string(expected_path)
        .map_err(|_| {
            format!(
                "missing {}; run with NOVO_BLESS=1 to create it",
                expected_path.display()
            )
        })?
        .replace("\r\n", "\n");
    if stdout != expected {
        return Err(format!(
            "[{engine}] output differs\n--- expected\n{expected}--- actual\n{stdout}"
        ));
    }
    Ok(())
}

#[test]
fn golden_programs() {
    let bless = std::env::var_os("NOVO_BLESS").is_some();
    let failures: Vec<String> = programs()
        .iter()
        .filter_map(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            check(path, bless).err().map(|e| format!("{name}: {e}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} golden program(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn examples_type_check() {
    let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).expect("examples exists") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "nv") {
                continue;
            }
            let output = Command::new(env!("CARGO_BIN_EXE_novo"))
                .arg("check")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} does not type-check:\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn examples_are_formatted() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let output = Command::new(env!("CARGO_BIN_EXE_novo"))
        .arg("fmt")
        .arg("--check")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "run `novo fmt examples`; unformatted:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
