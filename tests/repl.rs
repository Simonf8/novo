use std::io::Write;
use std::process::{Command, Stdio};

fn repl(input: &str) -> (String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_novo"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start novo");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn state_persists_between_inputs() {
    let (out, err) = repl(
        "x = 3\nfunction sq(n: Int) -> Int:\n    return n * n\n\nsq(x) + 1\nxs = [1,\n  2]\nxs.push(x)\nxs\n",
    );
    assert_eq!(out, "10\n[1, 2, 3]\n");
    assert_eq!(err, "");
}

#[test]
fn failed_inputs_are_forgotten() {
    let (out, err) = repl("x = 1\ny = x + \"a\"\ny\nx = \"text\"\nx\nshow 1 / 0\nx + 1\n");
    assert_eq!(out, "1\n2\n");
    assert!(err.contains("cannot apply `+`"));
    assert!(err.contains("unknown name `y`"));
    assert!(err.contains("division by zero"));
}
