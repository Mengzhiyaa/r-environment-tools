// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//
// CI integration test for R environment discovery.
//
// This test is gated behind the `ci` feature flag and runs only in CI
// where real R environments (System R, Conda R, Pixi R, etc.) are installed.
//
// Mirrors python-environment-tools crates/pet/tests/ci_test.rs.

use std::{path::PathBuf, sync::Once};

use ret::{
    find::{find_and_report_installations, identify_r_executables_using_locators},
    locators::{create_locators, identify_r_installation_using_locators},
    resolve::resolve_installation,
};
use ret_core::{
    os_environment::{Environment, EnvironmentApi},
    r_installation::{RInstallation, RInstallationKind},
    Configuration, Locator,
};
use ret_reporter::{cache::CacheReporter, collect};

use std::sync::Arc;

mod common;

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        env_logger::builder()
            .filter(None, log::LevelFilter::Trace)
            .init();
    });
}

/// Helper: discover all R installations on this machine.
fn discover_all_installations() -> Vec<RInstallation> {
    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let workspace_dir =
        PathBuf::from(std::env::var("GITHUB_WORKSPACE").unwrap_or_else(|_| ".".to_string()));
    let config = Configuration {
        workspace_directories: Some(vec![workspace_dir]),
        ..Configuration::default()
    };
    for locator in locators.iter() {
        locator.configure(&config);
    }

    let reporter = Arc::new(collect::create_reporter());
    let cache_reporter = CacheReporter::new(reporter.clone());
    find_and_report_installations(&cache_reporter, config, &locators, &environment, None);
    drop(cache_reporter);

    let installations = reporter
        .installations
        .lock()
        .expect("installations mutex poisoned")
        .clone();
    installations
}

/// Verification 1:
/// For each discovered R installation, verify that the executable and home
/// paths actually exist and that basic metadata (version, kind) is populated.
///
/// Verification 2:
/// For each installation, given only the executable, verify we can get the
/// same information using `locator.try_from` (without running a full scan).
///
/// Verification 3:
/// Use `resolve_installation` to validate that resolution produces consistent
/// results with discovery.
#[cfg_attr(feature = "ci", test)]
#[allow(dead_code)]
fn verify_validity_of_discovered_r_installations() {
    setup();

    let installations = discover_all_installations();

    assert!(
        !installations.is_empty(),
        "CI should discover at least one R installation"
    );

    for installation in &installations {
        let executable = installation
            .executable
            .as_ref()
            .expect("installation must have executable");

        // Verification 1: paths exist and metadata is populated.
        assert!(
            executable.exists(),
            "Executable does not exist: {}",
            executable.display()
        );
        if let Some(home) = &installation.home {
            assert!(
                home.exists(),
                "Home does not exist: {} for {:?}",
                home.display(),
                installation.kind
            );
        }
        assert!(
            installation.version.is_some(),
            "Version must be populated for {:?} at {}",
            installation.kind,
            executable.display()
        );

        // Verification 2: try_from with the executable yields same kind.
        verify_try_from_produces_same_kind(executable, installation);

        // Verification 3: resolve produces consistent results.
        verify_resolve_produces_consistent_result(executable, installation);
    }
}

fn verify_try_from_produces_same_kind(executable: &PathBuf, original: &RInstallation) {
    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let global_search_paths = environment.get_know_global_search_locations();

    let env = ret_core::env::REnv::new(executable.clone(), None, None);
    if let Some(resolved) =
        identify_r_installation_using_locators(&env, &locators, &global_search_paths)
    {
        assert_eq!(
            resolved.kind, original.kind,
            "Kind mismatch for try_from with {:?}: got {:?}, expected {:?}",
            executable, resolved.kind, original.kind
        );
    }
    // Some locators may not resolve from a bare executable (e.g. Conda
    // needs prior lookup), so we don't panic if None.
}

fn verify_resolve_produces_consistent_result(executable: &PathBuf, original: &RInstallation) {
    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let config = Configuration::default();
    for locator in locators.iter() {
        locator.configure(&config);
    }

    if let Some(result) = resolve_installation(executable, &locators, &environment) {
        let resolved = result.resolved.unwrap_or(result.discovered);
        assert_eq!(
            resolved.kind, original.kind,
            "Kind mismatch for resolve with {:?}: got {:?}, expected {:?}",
            executable, resolved.kind, original.kind
        );
        if let (Some(expected_version), Some(resolved_version)) =
            (&original.version, &resolved.version)
        {
            assert!(
                common::does_version_match(expected_version, resolved_version),
                "Version mismatch for {:?}: discovered={:?}, resolved={:?}",
                executable,
                expected_version,
                resolved_version
            );
        }
    }
}

/// Verify that the `find` method in JSON RPC can locate an installation
/// given its executable, without spawning R.
fn verify_find_with_executable(executable: &PathBuf, original: &RInstallation) {
    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let global_search_paths = environment.get_know_global_search_locations();

    let collect_reporter = Arc::new(collect::create_reporter());
    let reporter = CacheReporter::new(collect_reporter.clone());
    identify_r_executables_using_locators(
        vec![executable.clone()],
        &locators,
        &reporter,
        &global_search_paths,
    );

    let found = collect_reporter
        .installations
        .lock()
        .expect("installations mutex poisoned")
        .clone();

    assert!(
        !found.is_empty(),
        "find should locate R installation at {:?}, details => {:?}",
        executable,
        original
    );

    assert_eq!(
        found[0].kind, original.kind,
        "Kind mismatch for find with {:?}",
        executable
    );
}

/// Check that Conda R environments are correctly discovered (if installed in CI).
#[cfg_attr(feature = "ci", test)]
#[allow(dead_code)]
fn check_if_conda_r_exists() {
    setup();

    let installations = discover_all_installations();

    let conda_installations: Vec<_> = installations
        .iter()
        .filter(|i| i.kind == Some(RInstallationKind::Conda))
        .collect();

    if conda_installations.is_empty() {
        eprintln!("No Conda R installations found in CI (this may be expected)");
        return;
    }

    for installation in &conda_installations {
        assert!(
            installation.executable.is_some(),
            "Conda R installation must have an executable"
        );
        assert!(
            installation.home.is_some(),
            "Conda R installation must have a home directory"
        );
        assert!(
            installation.version.is_some(),
            "Conda R installation must have a version"
        );
    }
}

/// For each installation, verify we can reconstruct the environment using
/// the find API (exercising the JSONRPC code path without spawning R).
#[cfg_attr(feature = "ci", test)]
#[allow(dead_code)]
fn verify_find_api_for_each_discovered_installation() {
    setup();

    let installations = discover_all_installations();

    for installation in &installations {
        if let Some(executable) = &installation.executable {
            verify_find_with_executable(executable, installation);

            // Also check via known symlinks.
            if let Some(symlinks) = &installation.symlinks {
                for symlink in symlinks {
                    verify_find_with_executable(symlink, installation);
                }
            }
        }
    }
}
