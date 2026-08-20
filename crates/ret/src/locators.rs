use ret_chocolatey::Chocolatey;
use ret_conda::Conda;
use ret_core::{
    env::REnv,
    os_environment::Environment,
    r_installation::{DiscoverySource, RInstallation, RInstallationBuilder, RInstallationKind},
    Locator,
};
use ret_guix::Guix;
use ret_homebrew::Homebrew;
use ret_linux_global_r::LinuxGlobalR;
use ret_mac_framework::MacFramework;
use ret_macports::MacPorts;
use ret_module::EnvironmentModule;
use ret_nix::Nix;
use ret_pixi::Pixi;
use ret_r_utils::env::ResolvedRInstallation;
use ret_reporter::installation::merge_installations;
use ret_rig::Rig;
use ret_rversions::RVersions;
use ret_scoop::Scoop;
use ret_spack::Spack;
use std::path::PathBuf;
use std::sync::Arc;

pub fn create_locators(environment: &dyn Environment) -> Arc<Vec<Arc<dyn Locator>>> {
    create_locators_with_conda(environment, Arc::new(Conda::from(environment)))
}

pub fn create_locators_with_conda(
    environment: &dyn Environment,
    conda: Arc<Conda>,
) -> Arc<Vec<Arc<dyn Locator>>> {
    let mut locators: Vec<Arc<dyn Locator>> = vec![];

    if cfg!(windows) {
        use ret_windows_hq::WindowsHq;
        use ret_windows_registry::WindowsRegistry;
        locators.push(Arc::new(WindowsRegistry::new()));
        locators.push(Arc::new(WindowsHq::from(environment)));
        locators.push(Arc::new(Scoop::from(environment)));
        locators.push(Arc::new(Chocolatey::from(environment)));
    }

    // Pixi must come before Conda so its environments are not
    // misidentified as plain Conda environments.
    locators.push(Arc::new(Pixi::from(environment)));
    locators.push(conda);

    locators.push(Arc::new(Rig::from(environment)));

    if cfg!(unix) {
        locators.push(Arc::new(Homebrew::from(environment)));
        locators.push(Arc::new(Nix::from(environment)));
        locators.push(Arc::new(Guix::from(environment)));
        locators.push(Arc::new(Spack::from(environment)));
        locators.push(Arc::new(RVersions::from(environment)));
        locators.push(Arc::new(EnvironmentModule::from(environment)));
    }

    if std::env::consts::OS == "macos" {
        locators.push(Arc::new(MacPorts::new()));
        locators.push(Arc::new(MacFramework::new()));
    }

    if std::env::consts::OS != "macos" && std::env::consts::OS != "windows" {
        locators.push(Arc::new(LinuxGlobalR::new()));
    }

    Arc::new(locators)
}

pub fn identify_r_installation_using_locators(
    env: &REnv,
    locators: &[Arc<dyn Locator>],
    global_search_paths: &[PathBuf],
) -> Option<RInstallation> {
    let discovered = identify_installation_without_resolution(env, locators);

    let Some(resolved) = ResolvedRInstallation::from(&env.executable) else {
        return discovered;
    };

    let resolved_env = resolved.to_r_env();
    if let Some(installation) = identify_installation_without_resolution(&resolved_env, locators) {
        let installation =
            mark_locator_discovery(merge_resolved_installation(installation, &resolved));
        resolved.add_to_cache(installation.clone());
        return Some(installation);
    }

    if let Some(installation) = discovered {
        let installation =
            mark_locator_discovery(merge_resolved_installation(installation, &resolved));
        resolved.add_to_cache(installation.clone());
        return Some(installation);
    }

    let fallback_kind = infer_fallback_kind(&resolved, global_search_paths);
    let installation = create_unknown_installation(resolved.clone(), fallback_kind);
    resolved.add_to_cache(installation.clone());
    Some(installation)
}

pub(crate) fn identify_installation_without_resolution(
    env: &REnv,
    locators: &[Arc<dyn Locator>],
) -> Option<RInstallation> {
    locators
        .iter()
        .filter_map(|locator| locator.try_from(env))
        .reduce(|existing, new| merge_installations(&existing, &new))
}

fn create_unknown_installation(
    resolved: ResolvedRInstallation,
    fallback_kind: Option<RInstallationKind>,
) -> RInstallation {
    RInstallationBuilder::new(fallback_kind)
        .executable(Some(resolved.executable))
        .home(Some(resolved.home))
        .version(Some(resolved.version))
        .arch(Some(resolved.arch))
        .known_executables(resolved.known_executables)
        .symlinks(resolved.symlinks)
        .build()
}

fn merge_resolved_installation(
    installation: RInstallation,
    resolved: &ResolvedRInstallation,
) -> RInstallation {
    let mut symlinks = installation.symlinks.clone().unwrap_or_default();
    symlinks.append(&mut resolved.symlinks.clone().unwrap_or_default());
    symlinks.sort();
    symlinks.dedup();

    let mut known_executables = installation.known_executables.clone().unwrap_or_default();
    known_executables.push(resolved.executable.clone());
    known_executables.append(&mut resolved.known_executables.clone().unwrap_or_default());
    known_executables.sort();
    known_executables.dedup();

    RInstallationBuilder::from_installation(installation)
        .executable(Some(resolved.executable.clone()))
        .home(Some(resolved.home.clone()))
        .version(Some(resolved.version.clone()))
        .arch(Some(resolved.arch.clone()))
        .known_executables(Some(known_executables))
        .symlinks(Some(symlinks))
        .build()
}

fn mark_locator_discovery(installation: RInstallation) -> RInstallation {
    RInstallationBuilder::from_installation(installation)
        .add_discovery_source(DiscoverySource::Locator)
        .build()
}

fn infer_fallback_kind(
    resolved: &ResolvedRInstallation,
    global_search_paths: &[PathBuf],
) -> Option<RInstallationKind> {
    let mut known_executables = resolved.known_executables.clone().unwrap_or_default();
    known_executables.push(resolved.executable.clone());

    for executable in known_executables {
        if let Some(bin) = executable.parent() {
            if global_search_paths.contains(&bin.to_path_buf()) {
                return Some(RInstallationKind::GlobalPaths);
            }
        }
    }

    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::identify_r_installation_using_locators;
    use ret_core::{
        arch::Architecture,
        env::REnv,
        r_installation::{
            DiscoverySource, RInstallation, RInstallationBuilder, RInstallationKind,
            RVersionsOverlay,
        },
        reporter::Reporter,
        Configuration, Locator, LocatorKind,
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::Arc,
    };
    use tempfile::TempDir;

    struct FakeRInstallation {
        _temp_dir: TempDir,
        executable: PathBuf,
    }

    impl FakeRInstallation {
        fn new(version: &str, arch: &str) -> Self {
            let temp_dir = tempfile::Builder::new()
                .prefix("ret-locators-")
                .tempdir()
                .expect("failed to create temp dir");
            let home = temp_dir.path().join("R");
            let bin = home.join("bin");
            fs::create_dir_all(&bin).expect("failed to create fake R bin directory");

            let executable = bin.join("R");
            write_fake_r_runtime(&executable, &home, version, arch);

            Self {
                _temp_dir: temp_dir,
                executable,
            }
        }
    }

    fn write_fake_r_runtime(path: &Path, home: &Path, version: &str, arch: &str) {
        let script = format!(
            "#!/bin/sh\ncat <<EOF\nret-r-installation-info\n{version}\n{}\n{arch}\nEOF\n",
            home.display()
        );
        fs::write(path, script).expect("failed to write fake R runtime");
        let mut permissions = fs::metadata(path)
            .expect("failed to stat fake R runtime")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("failed to chmod fake R runtime");
    }

    struct TestLocator;

    struct OverlayLocator;

    impl Locator for TestLocator {
        fn get_kind(&self) -> LocatorKind {
            LocatorKind::Homebrew
        }

        fn configure(&self, _config: &Configuration) {}

        fn find(&self, _reporter: &dyn Reporter) {}

        fn supported_categories(&self) -> Vec<RInstallationKind> {
            vec![RInstallationKind::Homebrew]
        }

        fn try_from(&self, env: &REnv) -> Option<RInstallation> {
            let home = env.home.clone()?;
            Some(
                RInstallationBuilder::new(Some(RInstallationKind::Homebrew))
                    .executable(Some(env.executable.clone()))
                    .home(Some(home))
                    .version(env.version.clone())
                    .arch(
                        env.arch
                            .clone()
                            .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                    )
                    .build(),
            )
        }
    }

    impl Locator for OverlayLocator {
        fn get_kind(&self) -> LocatorKind {
            LocatorKind::LinuxGlobal
        }

        fn configure(&self, _config: &Configuration) {}

        fn find(&self, _reporter: &dyn Reporter) {}

        fn supported_categories(&self) -> Vec<RInstallationKind> {
            vec![RInstallationKind::LinuxGlobal]
        }

        fn try_from(&self, env: &REnv) -> Option<RInstallation> {
            Some(
                RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
                    .display_name(Some("Production R".to_string()))
                    .executable(Some(env.executable.clone()))
                    .home(env.home.clone())
                    .version(env.version.clone())
                    .arch(env.arch.clone())
                    .discovered_by(vec![DiscoverySource::Locator, DiscoverySource::RVersions])
                    .rversions_overlay(Some(RVersionsOverlay {
                        label: Some("Production R".to_string()),
                        script: None,
                        repo: Some("https://ppm.example.com/cran/latest".to_string()),
                        library: Some("/opt/R-libs".to_string()),
                        module: None,
                        module_startup_command: None,
                    }))
                    .build(),
            )
        }
    }

    #[test]
    fn identify_installation_prefers_resolved_arch_over_raw_inference() {
        let fake = FakeRInstallation::new("4.4.1", "aarch64-unknown-linux-gnu");
        let locators: Vec<Arc<dyn Locator>> = vec![Arc::new(TestLocator)];
        let env = REnv::new(fake.executable.clone(), None, None);

        let installation =
            identify_r_installation_using_locators(&env, &locators, &[]).expect("expected match");

        assert_eq!(installation.version.as_deref(), Some("4.4.1"));
        assert_eq!(installation.arch, Some(Architecture::Arm64));
    }

    #[test]
    fn identify_installation_merges_multiple_matching_locators() {
        let fake = FakeRInstallation::new("4.4.1", "x86_64-pc-linux-gnu");
        let locators: Vec<Arc<dyn Locator>> = vec![Arc::new(TestLocator), Arc::new(OverlayLocator)];
        let env = REnv::new(fake.executable.clone(), None, None);

        let installation =
            identify_r_installation_using_locators(&env, &locators, &[]).expect("expected match");

        assert_eq!(installation.display_name.as_deref(), Some("Production R"));
        assert_eq!(installation.kind, Some(RInstallationKind::Homebrew));
        assert!(installation.locator_metadata.is_none());
        assert_eq!(
            installation
                .rversions_overlay
                .as_ref()
                .and_then(|overlay| overlay.repo.as_deref()),
            Some("https://ppm.example.com/cran/latest")
        );
        assert_eq!(
            installation.discovered_by,
            vec![DiscoverySource::Locator, DiscoverySource::RVersions]
        );
    }
}
