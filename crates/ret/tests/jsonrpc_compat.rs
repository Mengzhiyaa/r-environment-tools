#![cfg(unix)]

use serde_json::{json, Value};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
};
use tempfile::TempDir;

static REQUEST_ID: AtomicU32 = AtomicU32::new(1);

struct FakeRInstallation {
    temp_dir: TempDir,
    home: PathBuf,
    executable: PathBuf,
}

impl FakeRInstallation {
    fn new(version: &str) -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ret-jsonrpc-compat-")
            .tempdir()
            .expect("failed to create temp dir");
        let home = temp_dir.path().join("R");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).expect("failed to create fake R bin directory");

        let executable = bin.join("R");
        write_fake_r_runtime(&executable, &home, version);
        write_fake_r_runtime(&bin.join("Rscript"), &home, version);

        Self {
            temp_dir,
            home,
            executable,
        }
    }

    fn with_reported_home(version: &str, reported_home: &Path) -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ret-jsonrpc-compat-")
            .tempdir()
            .expect("failed to create temp dir");
        let home = temp_dir.path().join("R");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).expect("failed to create fake R bin directory");

        let executable = bin.join("R");
        write_fake_r_runtime(&executable, reported_home, version);
        write_fake_r_runtime(&bin.join("Rscript"), reported_home, version);

        Self {
            temp_dir,
            home,
            executable,
        }
    }

    fn root(&self) -> &Path {
        self.temp_dir.path()
    }
}

fn write_fake_r_runtime(path: &Path, home: &Path, version: &str) {
    let script = format!(
        "#!/bin/sh\ncat <<EOF\nret-r-installation-info\n{version}\n{}\nx86_64-pc-linux-gnu\nEOF\n",
        home.display()
    );
    fs::write(path, script).expect("failed to write fake R runtime");
    let mut permissions = fs::metadata(path)
        .expect("failed to stat fake R runtime")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to chmod fake R runtime");
}

fn ret_executable() -> PathBuf {
    if let Some(path) = env::var_os("CARGO_BIN_EXE_ret") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("failed to determine workspace root");
    let executable = workspace_root.join("target").join("debug").join("ret");

    assert!(
        executable.is_file(),
        "ret executable not found at {}",
        executable.display()
    );

    executable
}

struct RetJsonRpcClient {
    process: Child,
    stdout: BufReader<ChildStdout>,
    installations: Vec<Value>,
    managers: Vec<Value>,
    telemetry: Vec<Value>,
    logs: Vec<Value>,
}

impl RetJsonRpcClient {
    fn spawn() -> Self {
        let mut process = Command::new(ret_executable())
            .arg("server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn ret server");

        let stdout = process.stdout.take().expect("failed to take ret stdout");

        Self {
            process,
            stdout: BufReader::new(stdout),
            installations: vec![],
            managers: vec![],
            telemetry: vec![],
            logs: vec![],
        }
    }

    fn send_request(&mut self, method: &str, params: Value) -> Value {
        let id = REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let request = serde_json::to_string(&request).expect("failed to serialize request");
        let message = format!("Content-Length: {}\r\n\r\n{}", request.len(), request);

        let stdin = self
            .process
            .stdin
            .as_mut()
            .expect("failed to get ret stdin");
        stdin
            .write_all(message.as_bytes())
            .expect("failed to write request");
        stdin.flush().expect("failed to flush request");

        loop {
            let message = self.read_message();
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                self.record_notification(
                    method,
                    message.get("params").cloned().unwrap_or(Value::Null),
                );
                continue;
            }

            let response_id = message
                .get("id")
                .and_then(Value::as_u64)
                .expect("response did not include id");
            assert_eq!(response_id as u32, id, "unexpected response id");

            if let Some(error) = message.get("error") {
                panic!("jsonrpc request {method} failed: {error}");
            }
            return message.get("result").cloned().unwrap_or(Value::Null);
        }
    }

    fn take_installations(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.installations)
    }

    fn take_telemetry(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.telemetry)
    }

    fn read_message(&mut self) -> Value {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            self.stdout
                .read_line(&mut line)
                .expect("failed to read jsonrpc header");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(len) = trimmed.strip_prefix("Content-Length: ") {
                content_length = Some(
                    len.parse::<usize>()
                        .expect("failed to parse jsonrpc content length"),
                );
            }
        }

        let content_length = content_length.expect("missing Content-Length header");
        let mut body = vec![0u8; content_length];
        self.stdout
            .read_exact(&mut body)
            .expect("failed to read jsonrpc body");
        serde_json::from_slice(&body).expect("failed to parse jsonrpc message")
    }

    fn record_notification(&mut self, method: &str, params: Value) {
        match method {
            "installation" => self.installations.push(params),
            "manager" => self.managers.push(params),
            "telemetry" => self.telemetry.push(params),
            "log" => self.logs.push(params),
            other => panic!("unexpected notification method {other}"),
        }
    }
}

impl Drop for RetJsonRpcClient {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn refresh_search_paths_override_configured_directories() {
    let configured = FakeRInstallation::new("4.2.1");
    let requested = FakeRInstallation::new("4.3.0");
    let requested_root = requested.root().to_path_buf();
    let mut client = RetJsonRpcClient::spawn();

    let configure_result = client.send_request(
        "configure",
        json!({
            "workspaceDirectories": [configured.home],
        }),
    );
    assert!(configure_result.is_null());

    let refresh_result = client.send_request(
        "refresh",
        json!({
            "searchPaths": [requested_root],
        }),
    );
    assert!(refresh_result["duration"].as_u64().is_some());
    assert!(refresh_result["refreshId"].as_u64().is_some());

    let installations = client.take_installations();
    assert!(
        !installations.is_empty(),
        "refresh should report installations for the requested tree"
    );
    assert!(
        installations
            .iter()
            .any(|inst| inst["home"] == requested.home.to_string_lossy().as_ref()),
        "requested installation should be reported"
    );
    assert!(
        installations.iter().all(|inst| !matches!(
            inst["home"].as_str(),
            Some(home) if home == configured.home.to_string_lossy().as_ref()
        )),
        "configured workspace directories must not leak into refresh(searchPaths)"
    );
    assert!(
        installations.iter().all(|inst| {
            inst["home"]
                .as_str()
                .or_else(|| inst["executable"].as_str())
                .map(|path| path.starts_with(requested.root().to_string_lossy().as_ref()))
                .unwrap_or(false)
        }),
        "refresh(searchPaths) should stay within the requested tree"
    );
}

#[test]
fn find_resolve_conda_info_and_clear_cache() {
    let fake = FakeRInstallation::new("4.4.0");
    let cache_dir = tempfile::tempdir().expect("failed to create cache dir");
    let marker = cache_dir.path().join("marker");
    let mut client = RetJsonRpcClient::spawn();

    let configure_result = client.send_request(
        "configure",
        json!({
            "cacheDirectory": cache_dir.path().to_path_buf(),
        }),
    );
    assert!(configure_result.is_null());

    let find_result =
        client.send_request("find", json!({ "searchPath": fake.root().to_path_buf() }));
    let installations = find_result
        .as_array()
        .expect("find should return an array of RInstallation");
    let found = installations
        .iter()
        .find(|inst| inst["home"] == fake.home.to_string_lossy().as_ref())
        .expect("find did not return the fake installation");
    assert_eq!(found["version"], "4.4.0");

    let resolve_result = client.send_request("resolve", json!({ "executable": fake.executable }));
    assert_eq!(resolve_result["home"], fake.home.to_string_lossy().as_ref());
    assert_eq!(
        resolve_result["executable"],
        fake.executable.to_string_lossy().as_ref()
    );
    assert_eq!(resolve_result["version"], "4.4.0");

    let conda_info = client.send_request("condaInfo", Value::Null);
    assert!(
        conda_info.get("canSpawnConda").is_some(),
        "condaInfo should return telemetry fields"
    );

    fs::create_dir_all(cache_dir.path()).expect("failed to ensure cache dir exists");
    fs::write(&marker, "marker").expect("failed to write cache marker");
    assert!(
        marker.exists(),
        "cache marker should exist before clearCache"
    );

    let clear_result = client.send_request("clearCache", Value::Null);
    assert!(clear_result.is_null());
    assert!(
        !cache_dir.path().exists(),
        "clearCache should remove the configured cache directory"
    );
}

#[test]
fn refresh_and_resolve_emit_telemetry_events() {
    let workspace = FakeRInstallation::new("4.5.0");
    let mismatched =
        FakeRInstallation::with_reported_home("4.5.1", &workspace.root().join("resolved-home"));
    let mut client = RetJsonRpcClient::spawn();

    let configure_result = client.send_request(
        "configure",
        json!({
            "workspaceDirectories": [workspace.root().to_path_buf()],
        }),
    );
    assert!(configure_result.is_null());

    let refresh_result = client.send_request("refresh", Value::Null);
    assert!(refresh_result["duration"].as_u64().is_some());
    let refresh_id = refresh_result["refreshId"]
        .as_u64()
        .expect("refresh should return a refresh id");
    let _ = client.send_request("condaInfo", Value::Null);

    let telemetry = client.take_telemetry();
    let event_names = telemetry
        .iter()
        .filter_map(|item| item["event"].as_str())
        .collect::<Vec<_>>();
    assert!(
        event_names.contains(&"GlobalEnvironmentsSearchCompleted"),
        "refresh should emit global search telemetry"
    );
    assert!(
        event_names.contains(&"GlobalPathVariableEnvironmentsSearchCompleted"),
        "refresh should emit path search telemetry"
    );
    assert!(
        event_names.contains(&"AllSearchPathsEnvironmentsSearchCompleted"),
        "refresh should emit explicit search telemetry"
    );
    assert!(
        event_names.contains(&"SearchCompleted"),
        "refresh should emit search completion telemetry"
    );
    assert!(
        event_names.contains(&"RefreshPerformance"),
        "refresh should emit performance telemetry"
    );
    let progress = telemetry
        .iter()
        .filter(|item| item["event"] == "RefreshProgress")
        .collect::<Vec<_>>();
    assert!(
        !progress.is_empty(),
        "refresh should emit progress telemetry"
    );
    assert!(progress
        .iter()
        .all(|item| { item["data"]["refreshProgress"]["refreshId"].as_u64() == Some(refresh_id) }));
    let progress_json = serde_json::to_string(&progress).expect("failed to serialize progress");
    assert!(
        !progress_json.contains(workspace.root().to_string_lossy().as_ref()),
        "progress telemetry must not contain workspace paths"
    );

    let _ = client.send_request("resolve", json!({ "executable": mismatched.executable }));
    let _ = client.send_request("condaInfo", Value::Null);
    let telemetry = client.take_telemetry();
    let inaccuracy = telemetry
        .iter()
        .find(|item| item["event"] == "InaccurateEnvironmentInfo")
        .expect("resolve should emit inaccuracy telemetry");
    assert_eq!(
        inaccuracy["data"]["inaccurateEnvironmentInfo"]["invalidPrefix"],
        Value::Bool(true)
    );
}
