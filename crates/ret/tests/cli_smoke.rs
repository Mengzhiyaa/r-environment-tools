#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

struct FakeRInstallation {
    _temp_dir: TempDir,
    home: PathBuf,
    executable: PathBuf,
}

impl FakeRInstallation {
    fn new(version: &str) -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ret-cli-smoke-")
            .tempdir()
            .expect("failed to create temp dir");
        let home = temp_dir.path().join("R");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).expect("failed to create fake R bin directory");

        let executable = bin.join("R");
        write_fake_r_runtime(&executable, &home, version);
        write_fake_r_runtime(&bin.join("Rscript"), &home, version);

        Self {
            _temp_dir: temp_dir,
            home,
            executable,
        }
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

fn run_ret<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(ret_executable());
    command.args(args);

    let output = command.output().expect("failed to run ret");
    assert!(
        output.status.success(),
        "ret failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn parse_stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout was not valid JSON")
}

#[test]
fn find_json_reports_fake_installation() {
    let fake = FakeRInstallation::new("4.4.1");
    let cache_dir = tempfile::tempdir().expect("failed to create cache dir");
    let output = run_ret([
        "find",
        fake.home.to_str().expect("invalid path"),
        "--json",
        "--cache-directory",
        cache_dir.path().to_str().expect("invalid cache path"),
    ]);

    let value = parse_stdout_json(&output);

    let installations = value["installations"]
        .as_array()
        .expect("installations should be an array");
    let installation = installations
        .iter()
        .find(|item| item["home"] == fake.home.to_string_lossy().as_ref())
        .expect("fake installation was not reported");

    assert_eq!(
        installation["executable"],
        fake.executable.to_string_lossy().as_ref()
    );
    assert_eq!(installation["version"], "4.4.1");
    assert_eq!(installation["arch"], "x64");
}

#[test]
fn resolve_json_reports_installation() {
    let fake = FakeRInstallation::new("4.2.0");
    let cache_dir = tempfile::tempdir().expect("failed to create cache dir");
    let output = run_ret([
        "resolve",
        fake.executable.to_str().expect("invalid path"),
        "--json",
        "--cache-directory",
        cache_dir.path().to_str().expect("invalid cache path"),
    ]);

    let value = parse_stdout_json(&output);
    assert_eq!(value["home"], fake.home.to_string_lossy().as_ref());
    assert_eq!(
        value["executable"],
        fake.executable.to_string_lossy().as_ref()
    );
    assert_eq!(value["version"], "4.2.0");
}
