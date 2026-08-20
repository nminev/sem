use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Output, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tempfile::TempDir;

struct ImpactFixture {
    repo: TempDir,
    home: TempDir,
    cache: TempDir,
}

impl ImpactFixture {
    fn new() -> Self {
        let fixture = Self {
            repo: TempDir::new().unwrap(),
            home: TempDir::new().unwrap(),
            cache: TempDir::new().unwrap(),
        };
        fixture.initialize_repo();
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sem"));
        command
            .current_dir(self.repo.path())
            .env("HOME", self.home.path())
            .env("SEM_CACHE_DIR", self.cache.path())
            .env("SEM_NO_UPDATE_CHECK", "1")
            .env("SEM_NO_AUTOWARM", "1");
        command
    }

    fn command_without_sidecar(&self) -> Command {
        let mut command = self.command();
        command.env("SEM_NO_SIDECAR", "1");
        command
    }

    #[cfg(unix)]
    fn start_resident(&self) -> ResidentGuard {
        let child = self
            .command()
            .args(["mcp", "--resident"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let resident = ResidentGuard(child);
        wait_for_sidecar_impact(self.home.path());
        resident
    }

    fn initialize_repo(&self) {
        git(self.repo.path(), &["init", "-q"]);
        git(
            self.repo.path(),
            &["config", "user.email", "test@example.com"],
        );
        git(self.repo.path(), &["config", "user.name", "Test"]);
        git(self.repo.path(), &["config", "commit.gpgsign", "false"]);

        fs::write(
            self.repo.path().join("a.ts"),
            "export function source() { return 1; }\n",
        )
        .unwrap();
        fs::write(
            self.repo.path().join("b.ts"),
            "import { source } from './a';\nexport function consume() { return source(); }\n",
        )
        .unwrap();
        fs::write(
            self.repo.path().join("c.ts"),
            "import { consume } from './b';\nexport function transitive() { return consume(); }\n",
        )
        .unwrap();
        fs::write(
            self.repo.path().join("a.test.ts"),
            "import { source } from './a';\ntest('source works', () => source());\n",
        )
        .unwrap();

        git(
            self.repo.path(),
            &["add", "a.ts", "b.ts", "c.ts", "a.test.ts"],
        );
        git(self.repo.path(), &["commit", "-q", "-m", "fixture"]);
    }
}

#[cfg(unix)]
struct ResidentGuard(std::process::Child);

#[cfg(unix)]
impl Drop for ResidentGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

fn describe(output: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", describe(output));
}

fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout should be JSON: {error}\n{}", describe(output)))
}

fn assert_timing_document(output: &Output, expected_source: &str) -> serde_json::Value {
    let timings: serde_json::Value =
        serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
            panic!(
                "stderr should contain timing JSON: {error}\n{}",
                describe(output)
            )
        });
    assert_eq!(timings["command"], "impact");
    assert_eq!(timings["source"], expected_source);
    assert!(timings["totalMs"].is_number(), "{timings}");
    assert!(
        timings["phases"]
            .as_array()
            .is_some_and(|phases| !phases.is_empty()),
        "{timings}"
    );
    timings
}

#[cfg(unix)]
fn start_stalled_sidecar(repo: &Path, home: &Path) -> JoinHandle<()> {
    use std::os::unix::net::UnixListener;

    let canonical = repo.canonicalize().unwrap();
    let hash = canonical
        .to_string_lossy()
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    let socket_dir = home.join(".sem").join("sock");
    fs::create_dir_all(&socket_dir).unwrap();
    let listener = UnixListener::bind(socket_dir.join(format!("{hash:016x}.sock"))).unwrap();
    listener.set_nonblocking(true).unwrap();

    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((_stream, _)) => {
                    std::thread::sleep(Duration::from_millis(500));
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fake sidecar request timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fake sidecar accept failed: {error}"),
            }
        }
    })
}

fn start_fake_cloud(remote: &str) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());

    let repos = serde_json::json!([{
        "id": "repo-1",
        "cloneUrl": remote,
        "status": "ready",
    }])
    .to_string();
    let impact = serde_json::json!({
        "dependencies": [{
            "id": "dep-1",
            "name": "dependency",
            "entityType": "function",
            "filePath": "dependency.ts",
        }],
        "dependents": [{
            "id": "dependent-1",
            "name": "direct_dependent",
            "entityType": "function",
            "filePath": "direct.ts",
        }],
        "transitiveImpact": [
            {
                "id": "dependent-1",
                "name": "direct_dependent",
                "entityType": "function",
                "filePath": "direct.ts",
            },
            {
                "id": "transitive-1",
                "name": "transitive_dependent",
                "entityType": "function",
                "filePath": "transitive.ts",
            },
        ],
    })
    .to_string();

    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut requests = Vec::new();
        for body in [repos, impact] {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fake cloud request timed out");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fake cloud accept failed: {error}"),
                }
            };
            requests.push(read_http_request(&mut stream));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body,
            )
            .unwrap();
        }
        requests
    });

    (endpoint, server)
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];

    loop {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);

        let Some(headers_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..headers_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        if request.len() >= headers_end + 4 + content_length {
            break;
        }
    }

    String::from_utf8(request).unwrap()
}

#[cfg(unix)]
fn wait_for_sidecar_impact(home: &Path) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(10);
    let socket_dir = home.join(".sem").join("sock");

    while Instant::now() < deadline {
        let socket = fs::read_dir(&socket_dir).ok().and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.extension().is_some_and(|ext| ext == "sock"))
        });

        if let Some(socket) = socket {
            if let Ok(mut stream) = UnixStream::connect(socket) {
                let timeout = Some(Duration::from_millis(500));
                let _ = stream.set_read_timeout(timeout);
                let _ = stream.set_write_timeout(timeout);
                let request = serde_json::json!({
                    "op": "impact",
                    "name": "consume",
                    "file": "b.ts",
                    "depth": 2,
                });
                if writeln!(stream, "{request}").is_ok() {
                    let mut response = String::new();
                    if BufReader::new(stream).read_line(&mut response).is_ok()
                        && serde_json::from_str::<serde_json::Value>(&response)
                            .ok()
                            .and_then(|value| value["ok"].as_bool())
                            == Some(true)
                    {
                        return;
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(25));
    }

    panic!("resident sidecar did not become ready");
}

#[cfg(unix)]
#[test]
fn a_resident_sidecar_preserves_results_and_emits_requested_timings() {
    let fixture = ImpactFixture::new();
    let scenarios: &[&[&str]] = &[
        &["impact", "consume", "--file", "b.ts", "--deps", "--json"],
        &[
            "impact",
            "source",
            "--file",
            "a.ts",
            "--dependents",
            "--json",
        ],
    ];

    let expected: Vec<_> = scenarios
        .iter()
        .map(|args| {
            let output = fixture
                .command_without_sidecar()
                .args(*args)
                .arg("--no-cache")
                .output()
                .unwrap();
            assert_success(&output);
            stdout_json(&output)
        })
        .collect();

    let _resident = fixture.start_resident();

    for (args, expected) in scenarios.iter().zip(expected) {
        let output = fixture
            .command()
            .env("SEM_TIMINGS", "json")
            .args(*args)
            .output()
            .unwrap();

        assert_success(&output);
        assert_eq!(stdout_json(&output), expected);
        assert_timing_document(&output, "sidecar");
    }
}

#[cfg(unix)]
#[test]
fn a_resident_sidecar_keeps_stderr_empty_when_timings_are_not_requested() {
    let fixture = ImpactFixture::new();
    let _resident = fixture.start_resident();

    let output = fixture
        .command()
        .args(["impact", "consume", "--file", "b.ts", "--deps", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    assert_eq!(stdout_json(&output)["entity"]["name"], "consume");
    assert!(output.stderr.is_empty(), "{}", describe(&output));
}

#[cfg(unix)]
#[test]
fn a_failed_sidecar_probe_reports_its_own_latency() {
    let fixture = ImpactFixture::new();
    let sidecar = start_stalled_sidecar(fixture.repo.path(), fixture.home.path());

    let output = fixture
        .command()
        .env("SEM_TIMINGS", "json")
        .args(["impact", "consume", "--file", "b.ts", "--deps", "--json"])
        .output()
        .unwrap();
    sidecar.join().unwrap();

    assert_success(&output);
    assert_eq!(stdout_json(&output)["entity"]["name"], "consume");
    let timings = assert_timing_document(&output, "local");
    let phases = timings["phases"].as_array().unwrap();
    let sidecar_probe = phases
        .iter()
        .find(|phase| phase["name"] == "sidecar_probe")
        .unwrap_or_else(|| panic!("sidecar probe should have its own phase: {timings}"));
    assert!(
        sidecar_probe["durationMs"].as_f64().unwrap() >= 200.0,
        "the stalled sidecar latency should be attributed to its probe: {timings}"
    );
}

#[test]
fn a_disk_cache_hit_preserves_results_and_emits_requested_timings() {
    let fixture = ImpactFixture::new();
    let args = ["impact", "consume", "--file", "b.ts", "--deps", "--json"];

    let uncached = fixture
        .command_without_sidecar()
        .args(args)
        .arg("--no-cache")
        .output()
        .unwrap();
    assert_success(&uncached);

    let warmup = fixture
        .command_without_sidecar()
        .args(args)
        .output()
        .unwrap();
    assert_success(&warmup);

    let cached = fixture
        .command_without_sidecar()
        .env("SEM_TIMINGS", "json")
        .args(args)
        .output()
        .unwrap();
    assert_success(&cached);

    let expected = stdout_json(&uncached);
    assert_eq!(stdout_json(&warmup), expected);
    assert_eq!(stdout_json(&cached), expected);
    assert_timing_document(&cached, "disk-cache");
}

#[test]
fn a_local_query_emits_requested_timings() {
    let fixture = ImpactFixture::new();

    let output = fixture
        .command_without_sidecar()
        .env("SEM_TIMINGS", "json")
        .args([
            "impact",
            "consume",
            "--file",
            "b.ts",
            "--deps",
            "--json",
            "--no-cache",
        ])
        .output()
        .unwrap();

    assert_success(&output);
    assert_eq!(stdout_json(&output)["entity"]["name"], "consume");
    assert_timing_document(&output, "local");
}

#[test]
fn a_cloud_query_preserves_results_and_emits_requested_timings() {
    let fixture = ImpactFixture::new();
    let remote = "https://github.com/example/impact-fixture.git";
    git(fixture.repo.path(), &["remote", "add", "origin", remote]);
    let (endpoint, server) = start_fake_cloud(remote);

    let output = fixture
        .command_without_sidecar()
        .env_remove("SEM_LOCAL")
        .env_remove("SEM_NO_NETWORK")
        .env("SEM_CLOUD", "1")
        .env("SEM_TOKEN", "test-token")
        .env("SEM_CLOUD_ENDPOINT", endpoint)
        .env("SEM_TIMINGS", "json")
        .args(["impact", "cloud_target", "--json"])
        .output()
        .unwrap();
    let requests = server.join().unwrap();

    assert_success(&output);
    assert_eq!(
        stdout_json(&output),
        serde_json::json!({
            "entity": { "name": "cloud_target", "file": "" },
            "dependencies": [{
                "entityId": "dep-1",
                "name": "dependency",
                "type": "function",
                "file": "dependency.ts",
            }],
            "dependents": [{
                "entityId": "dependent-1",
                "name": "direct_dependent",
                "type": "function",
                "file": "direct.ts",
            }],
            "impact": {
                "total": 2,
                "entities": [
                    {
                        "entityId": "dependent-1",
                        "name": "direct_dependent",
                        "type": "function",
                        "file": "direct.ts",
                    },
                    {
                        "entityId": "transitive-1",
                        "name": "transitive_dependent",
                        "type": "function",
                        "file": "transitive.ts",
                    },
                ],
            },
            "tests": [],
        })
    );
    assert_timing_document(&output, "cloud");

    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/repos HTTP/1.1"));
    assert!(requests[1].starts_with("POST /v1/repos/repo-1/impact HTTP/1.1"));
    assert!(requests
        .iter()
        .all(|request| request.contains("Authorization: Bearer test-token")));
    let request_body: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(request_body["targetEntity"], "cloud_target");
    assert_eq!(request_body["targetFile"], "");
}

#[test]
fn each_impact_mode_reports_the_expected_relationships() {
    let fixture = ImpactFixture::new();

    let dependencies = fixture
        .command_without_sidecar()
        .args(["impact", "consume", "--file", "b.ts", "--deps", "--json"])
        .arg("--no-cache")
        .output()
        .unwrap();
    assert_success(&dependencies);
    let dependencies = stdout_json(&dependencies);
    assert_eq!(dependencies["entity"]["name"], "consume");
    assert!(dependencies["dependencies"]
        .as_array()
        .is_some_and(|entities| entities.iter().any(|entity| entity["name"] == "source")));

    let dependents = fixture
        .command_without_sidecar()
        .args([
            "impact",
            "source",
            "--file",
            "a.ts",
            "--dependents",
            "--json",
        ])
        .arg("--no-cache")
        .output()
        .unwrap();
    assert_success(&dependents);
    let dependents = stdout_json(&dependents);
    assert!(dependents["dependents"]
        .as_array()
        .is_some_and(|entities| entities.iter().any(|entity| entity["name"] == "consume")));

    let tests = fixture
        .command_without_sidecar()
        .args(["impact", "source", "--file", "a.ts", "--tests", "--json"])
        .arg("--no-cache")
        .output()
        .unwrap();
    assert_success(&tests);
    let tests = stdout_json(&tests);
    assert!(tests["tests"]
        .as_array()
        .is_some_and(|entities| entities.iter().any(|entity| entity["file"] == "a.test.ts")));

    let all = fixture
        .command_without_sidecar()
        .args(["impact", "source", "--file", "a.ts", "--json"])
        .arg("--no-cache")
        .output()
        .unwrap();
    assert_success(&all);
    let all = stdout_json(&all);
    assert!(all["impact"]["entities"]
        .as_array()
        .is_some_and(|entities| entities.iter().any(|entity| entity["name"] == "consume")));
    assert!(all["tests"]
        .as_array()
        .is_some_and(|entities| entities.iter().any(|entity| entity["file"] == "a.test.ts")));
}
