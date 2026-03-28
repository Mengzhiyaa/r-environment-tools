#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

use ret_r_utils::executable::{
    find_executable, find_executable_or_broken, find_executables, is_r_executable_name,
    ExecutableResult,
};
use tempfile::TempDir;

struct FakeInstall {
    _temp_dir: TempDir,
    home: PathBuf,
}

impl FakeInstall {
    fn new() -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ret-r-utils-exe-")
            .tempdir()
            .expect("failed to create temp dir");
        let home = temp_dir.path().join("R");
        fs::create_dir_all(home.join("bin")).expect("failed to create bin dir");
        Self {
            _temp_dir: temp_dir,
            home,
        }
    }

    fn write_runtime(&self, name: &str) -> PathBuf {
        let path = self.home.join("bin").join(name);
        write_executable(&path);
        path
    }
}

fn write_executable(path: &Path) {
    fs::write(path, "#!/bin/sh\nexit 0\n").expect("failed to write file");
    let mut permissions = fs::metadata(path)
        .expect("failed to stat file")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to chmod file");
}

#[test]
fn find_executable_prefers_r_binary_in_bin_directory() {
    let install = FakeInstall::new();
    let r = install.write_runtime("R");
    let rscript = install.write_runtime("Rscript");

    assert_eq!(find_executable(&install.home), Some(r.clone()));

    let executables = find_executables(&install.home);
    assert!(executables.contains(&r));
    assert!(executables.contains(&rscript));
}

#[test]
fn broken_symlink_is_reported() {
    let install = FakeInstall::new();
    let broken = install.home.join("bin").join("R");
    symlink(install.home.join("bin").join("missing"), &broken).expect("failed to create symlink");

    match find_executable_or_broken(&install.home) {
        ExecutableResult::Broken(path) => assert_eq!(path, broken),
        other => panic!("expected broken executable, got {other:?}"),
    }
}

#[test]
fn executable_name_filter_only_accepts_r_binaries() {
    assert!(is_r_executable_name(Path::new("R")));
    assert!(is_r_executable_name(Path::new("rscript")));
    assert!(!is_r_executable_name(Path::new("python")));
}
