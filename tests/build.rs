use std::process::Command;

#[test]
fn bundled_program_runs_without_its_sources() {
    let dir = std::env::temp_dir().join(format!("novo-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("app/lib")).unwrap();
    std::fs::write(
        dir.join("app/main.nv"),
        "use \"lib/greet.nv\" as greet\nuse env\nshow greet.hello(env.args()[1])\n",
    )
    .unwrap();
    std::fs::write(dir.join("app/lib/greet.nv"), "use \"../shared.nv\" as shared\nfunction hello(name: String) -> String:\n    return shared.prefix() + name\n").unwrap();
    std::fs::write(
        dir.join("app/shared.nv"),
        "function prefix() -> String:\n    return \"hello, \"\n",
    )
    .unwrap();
    let exe = dir.join(if cfg!(windows) { "hello.exe" } else { "hello" });

    let built = Command::new(env!("CARGO_BIN_EXE_novo"))
        .arg("build")
        .arg(dir.join("app/main.nv"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    std::fs::remove_dir_all(dir.join("app")).unwrap();

    let ran = Command::new(&exe).arg("Ada").output().unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).replace("\r\n", "\n"),
        "hello, Ada\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
