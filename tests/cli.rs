use std::process::Command;

#[test]
fn missing_file_is_reported_without_crashing() {
    for args in [
        &["run", "no_such_file.nv"][..],
        &["check", "no_such_file.nv", "--json"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_novo"))
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert!(
            format!("{stdout}{stderr}").contains("cannot read"),
            "{args:?}: {stdout}{stderr}"
        );
    }
}
