use std::path::PathBuf;

use ret_core::{
    env::REnv, manager::EnvManagerType, r_installation::RInstallationKind, Configuration, Locator,
};
use ret_rig::Rig;

#[test]
fn try_from_classifies_rig_managed_installation() {
    let locator = Rig::new();
    let mut config = Configuration::default();
    config.rig_executable = Some(PathBuf::from("/usr/local/bin/rig"));
    locator.configure(&config);

    let env = REnv::new(
        PathBuf::from("/opt/R/4.4.1/bin/R"),
        Some(PathBuf::from("/opt/R/4.4.1")),
        Some("4.4.1".to_string()),
    );

    let installation = locator
        .try_from(&env)
        .expect("rig path should be classified");

    assert_eq!(installation.kind, Some(RInstallationKind::Rig));
    assert_eq!(installation.home, Some(PathBuf::from("/opt/R/4.4.1")));
    assert_eq!(
        installation.executable,
        Some(PathBuf::from("/opt/R/4.4.1/bin/R"))
    );
    assert_eq!(installation.version.as_deref(), Some("4.4.1"));

    let manager = installation.manager.expect("rig manager should be present");
    assert_eq!(manager.tool, EnvManagerType::Rig);
    assert_eq!(manager.executable, PathBuf::from("/usr/local/bin/rig"));
}

#[test]
fn try_from_ignores_non_rig_paths() {
    let locator = Rig::new();
    let env = REnv::new(
        PathBuf::from("/tmp/fake-r/bin/R"),
        Some(PathBuf::from("/tmp/fake-r")),
        Some("4.4.1".to_string()),
    );

    assert!(locator.try_from(&env).is_none());
}
