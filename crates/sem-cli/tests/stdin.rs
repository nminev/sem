use std::io::Write;
use std::process::{Command, Stdio};

fn run_sem_diff_stdin(input: &str, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sem"))
        .arg("diff")
        .arg("--stdin")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).unwrap();
    drop(stdin);

    child.wait_with_output().unwrap()
}

#[test]
fn diff_stdin_rejects_unknown_file_change_fields() {
    let output = run_sem_diff_stdin(
        r#"[{"filePath":"b.ts","oldPath":"a.ts","status":"renamed","beforeContent":"function foo(){}","afterContent":"function foo(){}"}]"#,
        &[],
    );

    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Error parsing stdin JSON"));
    assert!(stderr.contains("unknown field `oldPath`"));
}

#[test]
fn diff_stdin_accepts_old_file_path_for_renames() {
    let output = run_sem_diff_stdin(
        r#"[{"filePath":"b.ts","oldFilePath":"a.ts","status":"renamed","beforeContent":"function foo(){}","afterContent":"function foo(){}"}]"#,
        &["--format", "json"],
    );

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""moved":1"#));
    assert!(stdout.contains(r#""filePath":"b.ts""#));
    assert!(stdout.contains(r#""oldFilePath":"a.ts""#));
}
