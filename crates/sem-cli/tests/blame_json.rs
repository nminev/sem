use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct TestRepo {
    path: PathBuf,
}

impl TestRepo {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sem-cli-{name}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary repo");
        Self { path }
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");

    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn blame_json_for_file_with_no_entities_emits_empty_array() {
    let repo = TestRepo::new("blame-empty-json");

    git(&repo.path, &["init", "-q"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    git(&repo.path, &["config", "user.name", "Test User"]);
    git(&repo.path, &["config", "commit.gpgsign", "false"]);

    fs::write(repo.path.join("empty.json"), "{}\n").expect("write fixture");

    git(&repo.path, &["add", "-A"]);
    git(&repo.path, &["commit", "-q", "-m", "v1"]);

    let output = Command::new(env!("CARGO_BIN_EXE_sem"))
        .args(["blame", "empty.json", "--json"])
        .current_dir(&repo.path)
        .output()
        .expect("run sem blame");

    assert!(
        output.status.success(),
        "sem blame failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[]\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}
