use std::{env, path::PathBuf};

use ret_conda::{
    manager::{find_conda_binary, find_mamba_binary},
    Conda,
};
use ret_core::{
    manager::EnvManagerType,
    os_environment::EnvironmentApi,
    r_installation::{RInstallation, RInstallationKind},
    Configuration, Locator,
};
use ret_homebrew::Homebrew;
use ret_reporter::collect;

#[cfg(windows)]
use ret_windows_registry::WindowsRegistry;
#[cfg(windows)]
use std::process::Command;

fn collect_from_locator<L: Locator>(
    locator: &L,
    configuration: &Configuration,
) -> Vec<RInstallation> {
    let reporter = collect::create_reporter();
    locator.configure(configuration);
    locator.find(&reporter);
    let installations = reporter
        .installations
        .lock()
        .expect("installations mutex poisoned")
        .clone();
    installations
}

fn collect_managers_from_locator<L: Locator>(
    locator: &L,
    configuration: &Configuration,
) -> Vec<ret_core::manager::EnvManager> {
    let reporter = collect::create_reporter();
    locator.configure(configuration);
    locator.find(&reporter);
    let managers = reporter
        .managers
        .lock()
        .expect("managers mutex poisoned")
        .clone();
    managers
}

#[test]
fn real_conda_locator_discovers_r_installations_when_available() {
    let environment = EnvironmentApi::new();
    let locator = Conda::from(&environment);

    let conda_executable = env::var_os("RET_CONDA_EXECUTABLE")
        .map(PathBuf::from)
        .or_else(find_conda_binary)
        .or_else(find_mamba_binary);
    let Some(conda_executable) = conda_executable else {
        eprintln!("skipping conda real-env test: no conda or mamba executable found");
        return;
    };

    let config = Configuration {
        conda_executable: Some(conda_executable),
        ..Configuration::default()
    };

    let managers = collect_managers_from_locator(&locator, &config);
    assert!(
        managers
            .iter()
            .any(|manager| matches!(manager.tool, EnvManagerType::Conda | EnvManagerType::Mamba)),
        "expected conda locator to report at least one conda-family manager"
    );

    let installations = collect_from_locator(&locator, &config);
    if installations.is_empty() {
        eprintln!("skipping conda installation assertions: no real conda R installation found");
        return;
    }

    assert!(installations.iter().all(|installation| {
        installation.kind == Some(RInstallationKind::Conda)
            && installation
                .executable
                .as_ref()
                .is_some_and(|path| path.exists())
            && installation.home.as_ref().is_some_and(|path| path.exists())
    }));
}

#[cfg(unix)]
#[test]
fn real_homebrew_locator_discovers_r_installations_when_available() {
    if !has_homebrew_r_installation() {
        eprintln!("skipping homebrew real-env test: no Homebrew R installation found");
        return;
    }

    let environment = EnvironmentApi::new();
    let locator = Homebrew::from(&environment);
    let config = Configuration::default();

    let installations = collect_from_locator(&locator, &config);
    assert!(
        !installations.is_empty(),
        "expected homebrew locator to discover at least one real R installation"
    );
    assert!(installations.iter().all(|installation| {
        installation.kind == Some(RInstallationKind::Homebrew)
            && installation
                .executable
                .as_ref()
                .is_some_and(|path| path.exists())
            && installation.home.as_ref().is_some_and(|path| path.exists())
    }));

    let managers = collect_managers_from_locator(&locator, &config);
    assert!(
        managers
            .iter()
            .any(|manager| manager.tool == EnvManagerType::Homebrew),
        "expected homebrew locator to report the brew manager"
    );
}

#[cfg(windows)]
#[test]
fn real_windows_registry_locator_discovers_registered_r_installations_when_available() {
    if !has_registered_windows_r_installation() {
        eprintln!("skipping windows-registry real-env test: no registered R installation found");
        return;
    }

    let locator = WindowsRegistry::new();
    let config = Configuration::default();
    let installations = collect_from_locator(&locator, &config);

    assert!(
        !installations.is_empty(),
        "expected windows registry locator to discover at least one registered R installation"
    );
    assert!(installations.iter().all(|installation| {
        installation.kind == Some(RInstallationKind::WindowsRegistry)
            && installation
                .executable
                .as_ref()
                .is_some_and(|path| path.exists())
            && installation.home.as_ref().is_some_and(|path| path.exists())
    }));
}

#[cfg(unix)]
fn has_homebrew_r_installation() -> bool {
    homebrew_cellar_roots().into_iter().any(|root| {
        if !root.is_dir() {
            return false;
        }

        std::fs::read_dir(root)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(Result::ok))
            .map(|entry| entry.path())
            .any(|formula| {
                formula
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name == "r" || name.starts_with("r@"))
                    .unwrap_or(false)
            })
    })
}

#[cfg(unix)]
fn homebrew_cellar_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/Cellar"),
        PathBuf::from("/usr/local/Cellar"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/Cellar"),
    ]
}

#[cfg(windows)]
fn has_registered_windows_r_installation() -> bool {
    let queries = [
        r"HKLM\SOFTWARE\R-core\R",
        r"HKLM\SOFTWARE\WOW6432Node\R-core\R",
    ];

    queries.into_iter().any(|query| {
        Command::new("reg")
            .args(["query", query])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}
