// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//
// Homebrew container integration test for R environment discovery.
//
// This test runs inside a Homebrew container (homebrew/brew) where R
// is installed via `brew install r`. It validates that the Homebrew
// locator correctly discovers the installation.

use std::sync::Arc;

use ret::{find::find_and_report_installations, locators::create_locators};
use ret_core::{os_environment::EnvironmentApi, r_installation::RInstallationKind, Configuration};
use ret_reporter::{cache::CacheReporter, collect};

#[cfg_attr(feature = "ci-homebrew-container", test)]
#[allow(dead_code)]
fn homebrew_container_discovers_r() {
    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let config = Configuration::default();
    for locator in locators.iter() {
        locator.configure(&config);
    }

    let reporter = Arc::new(collect::create_reporter());
    find_and_report_installations(
        &CacheReporter::new(reporter.clone()),
        config,
        &locators,
        &environment,
        None,
    );

    let installations = reporter
        .installations
        .lock()
        .expect("installations mutex poisoned")
        .clone();

    let homebrew_installs: Vec<_> = installations
        .iter()
        .filter(|i| i.kind == Some(RInstallationKind::Homebrew))
        .collect();

    assert!(
        !homebrew_installs.is_empty(),
        "Expected at least one Homebrew R installation in container, found: {:?}",
        installations
            .iter()
            .map(|i| format!("{:?}", i.kind))
            .collect::<Vec<_>>()
    );

    for installation in &homebrew_installs {
        assert!(
            installation.executable.is_some(),
            "Homebrew R installation must have an executable"
        );
        assert!(
            installation.executable.as_ref().unwrap().exists(),
            "Homebrew R executable must exist: {:?}",
            installation.executable
        );
        assert!(
            installation.version.is_some(),
            "Homebrew R installation must have a version"
        );
        assert!(
            installation.home.is_some(),
            "Homebrew R installation must have a home directory"
        );
    }
}
