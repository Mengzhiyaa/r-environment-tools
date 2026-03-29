#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use ret_r_utils::env::ResolvedRInstallation;
use tempfile::TempDir;

struct FakeRInstallation {
    _temp_dir: TempDir,
    home: PathBuf,
    executable: PathBuf,
    rscript: PathBuf,
}

impl FakeRInstallation {
    fn new(version: &str) -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ret-r-utils-env-")
            .tempdir()
            .expect("failed to create temp dir");
        let home = temp_dir.path().join("R");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).expect("failed to create bin dir");

        let executable = bin.join("R");
        let rscript = bin.join("Rscript");
        write_fake_r_runtime(&executable, &home, version);
        write_fake_r_runtime(&rscript, &home, version);

        Self {
            _temp_dir: temp_dir,
            home,
            executable,
            rscript,
        }
    }
}

fn write_fake_r_runtime(path: &Path, home: &Path, version: &str) {
    let script = format!(
        "#!/bin/sh\ncat <<EOF\nret-r-installation-info\n{version}\n{}\nx86_64-pc-linux-gnu\nEOF\n",
        home.display()
    );
    fs::write(path, script).expect("failed to write fake runtime");
    let mut permissions = fs::metadata(path)
        .expect("failed to stat runtime")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to chmod runtime");
}

#[test]
fn resolved_installation_uses_reported_home_and_version() {
    let fake = FakeRInstallation::new("4.4.0");

    let installation =
        ResolvedRInstallation::from(&fake.executable).expect("failed to resolve fake runtime");

    assert_eq!(installation.executable, fake.executable);
    assert_eq!(installation.home, fake.home);
    assert_eq!(installation.version, "4.4.0");
    assert_eq!(installation.arch.to_string(), "x86_64");
    assert!(installation
        .known_executables
        .unwrap_or_default()
        .contains(&fake.executable));
    assert!(installation.symlinks.unwrap_or_default().is_empty());
}

#[test]
fn resolving_rscript_prefers_primary_r_executable() {
    let fake = FakeRInstallation::new("4.1.3");

    let installation =
        ResolvedRInstallation::from(&fake.rscript).expect("failed to resolve fake runtime");

    assert_eq!(installation.executable, fake.executable);
    assert_eq!(installation.home, fake.home);
    assert_eq!(installation.version, "4.1.3");
}
