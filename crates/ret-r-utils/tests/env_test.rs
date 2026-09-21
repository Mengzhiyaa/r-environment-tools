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
fn resolving_rscript_preserves_the_requested_entrypoint() {
    let fake = FakeRInstallation::new("4.1.3");

    let installation =
        ResolvedRInstallation::from(&fake.rscript).expect("failed to resolve fake runtime");

    assert_eq!(installation.executable, fake.rscript);
    assert_eq!(installation.home, fake.home);
    assert_eq!(installation.version, "4.1.3");
}

#[test]
fn deleted_installation_is_not_resolved_from_memory() {
    let fake = FakeRInstallation::new("4.4.0");
    assert!(ResolvedRInstallation::from(&fake.executable).is_some());
    fs::remove_dir_all(&fake.home).unwrap();
    assert!(ResolvedRInstallation::from(&fake.executable).is_none());
}

#[test]
fn unsuccessful_runtime_output_is_not_cached() {
    let fake = FakeRInstallation::new("4.4.0");
    let mut script = fs::read_to_string(&fake.executable).unwrap();
    script.push_str("exit 7\n");
    fs::write(&fake.executable, script).unwrap();
    assert!(ResolvedRInstallation::from(&fake.executable).is_none());
    write_fake_r_runtime(&fake.executable, &fake.home, "4.5.0");
    assert_eq!(
        ResolvedRInstallation::from(&fake.executable)
            .unwrap()
            .version,
        "4.5.0"
    );
}

#[test]
fn a_wrapper_keeps_its_activation_when_used_after_resolution() {
    use ret_core::shell::quote_shell_argument;
    let fake = FakeRInstallation::new("4.4.0");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = --vanilla ]; then
    printf 'ret-r-installation-info\n4.4.0\n%s\nx86_64\n' {}
else
    printf '%s' "${{RET_TEST_WRAPPER-missing}}"
fi
"#,
        quote_shell_argument(&fake.home.to_string_lossy())
    );
    fs::write(&fake.executable, script).unwrap();
    let wrapper = fake._temp_dir.path().join("wrapped").join("bin").join("R");
    fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
    fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
export RET_TEST_WRAPPER=kept
exec {} "$@"
"#,
            quote_shell_argument(&fake.executable.to_string_lossy())
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let resolved = ResolvedRInstallation::from(&wrapper).unwrap();
    assert_eq!(resolved.executable, wrapper);
    assert_eq!(resolved.home, fake.home);
    let output = std::process::Command::new(resolved.executable)
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"kept");
}
