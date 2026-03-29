// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::warn;
use ret_core::{
    env::REnv,
    os_environment::Environment,
    r_installation::{DiscoverySource, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind,
};
use ret_r_utils::executable::{
    find_executable, find_executables, should_search_for_installations_in_path,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tracing::{info_span, instrument};

use crate::locators::identify_r_installation_using_locators;
use ret_conda::utils::is_conda_env;
use ret_pixi::is_pixi_env;

pub struct Summary {
    pub total: Duration,
    pub locators: BTreeMap<LocatorKind, Duration>,
    pub breakdown: BTreeMap<&'static str, Duration>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchScope {
    Global(RInstallationKind),
    SearchPaths,
}

#[instrument(skip(reporter, configuration, locators, environment), fields(search_scope = ?search_scope))]
pub fn find_and_report_installations(
    reporter: &dyn Reporter,
    configuration: Configuration,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    environment: &dyn Environment,
    search_scope: Option<SearchScope>,
) -> Arc<Mutex<Summary>> {
    let summary = Arc::new(Mutex::new(Summary {
        total: Duration::from_secs(0),
        locators: BTreeMap::new(),
        breakdown: BTreeMap::new(),
    }));
    let start = std::time::Instant::now();

    let workspace_directories = configuration.workspace_directories.unwrap_or_default();
    let environment_directories = configuration.environment_directories.unwrap_or_default();
    let search_directories = configuration.search_directories.unwrap_or_default();
    let executables = configuration.executables.unwrap_or_default();
    let search_global = match search_scope {
        Some(SearchScope::Global(_)) => true,
        Some(SearchScope::SearchPaths) => false,
        None => true,
    };
    let search_kind = match search_scope {
        Some(SearchScope::Global(kind)) => Some(kind),
        _ => None,
    };

    thread::scope(|scope| {
        scope.spawn(|| {
            let _span = info_span!("locators_phase").entered();
            let start = std::time::Instant::now();
            if search_global {
                thread::scope(|scope| {
                    for locator in locators.iter() {
                        if let Some(kind) = &search_kind {
                            if !locator.supported_categories().contains(kind) {
                                continue;
                            }
                        }
                        let locator = locator.clone();
                        let summary = summary.clone();
                        scope.spawn(move || {
                            let start = std::time::Instant::now();
                            locator.find(reporter);
                            summary
                                .lock()
                                .unwrap()
                                .locators
                                .insert(locator.get_kind(), start.elapsed());
                        });
                    }
                });
            }
            summary
                .lock()
                .unwrap()
                .breakdown
                .insert("Locators", start.elapsed());
        });

        let summary_for_path = summary.clone();
        scope.spawn(move || {
            let _span = info_span!("path_search_phase").entered();
            let start = std::time::Instant::now();
            if search_global {
                let global_search_paths = environment.get_know_global_search_locations();
                find_r_installations_in_paths(
                    &global_search_paths,
                    reporter,
                    locators,
                    false,
                    &global_search_paths,
                    DiscoverySource::GlobalPaths,
                );
            }
            summary_for_path
                .lock()
                .unwrap()
                .breakdown
                .insert("Path", start.elapsed());
        });

        let summary_for_explicit = summary.clone();
        scope.spawn(move || {
            let _span = info_span!("explicit_search_phase").entered();
            let start = std::time::Instant::now();
            let global_search_paths = environment.get_know_global_search_locations();
            let mut directories_to_search = if search_global {
                [
                    workspace_directories,
                    environment_directories,
                    search_directories,
                ]
                .concat()
            } else {
                search_directories
            };
            directories_to_search.sort();
            directories_to_search.dedup();

            for directory in directories_to_search {
                find_r_installations_in_directory_recursive(
                    &directory,
                    reporter,
                    locators,
                    &global_search_paths,
                );
            }
            if !executables.is_empty() {
                identify_r_executables_using_locators(
                    executables,
                    locators,
                    reporter,
                    &global_search_paths,
                    Some(DiscoverySource::ExplicitSearch),
                );
            }
            summary_for_explicit
                .lock()
                .unwrap()
                .breakdown
                .insert("SearchPaths", start.elapsed());
        });
    });

    summary.lock().unwrap().total = start.elapsed();
    summary
}

#[instrument(skip(reporter, locators, global_search_paths), fields(directory = %directory.display()))]
pub fn find_r_installations_in_directory_recursive(
    directory: &PathBuf,
    reporter: &dyn Reporter,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    global_search_paths: &[PathBuf],
) {
    let mut paths_to_search_first = vec![
        directory.to_path_buf(),
        directory.join("bin"),
        directory.join("lib").join("R"),
        directory.join("lib").join("R").join("bin"),
        directory.join("Resources"),
        directory.join("Resources").join("bin"),
    ];

    // Add all subdirectories of .pixi/envs/** so Pixi environments
    // in the workspace are discovered.
    if let Ok(reader) = fs::read_dir(directory.join(".pixi").join("envs")) {
        reader
            .filter_map(Result::ok)
            .filter(|d| d.path().is_dir())
            .map(|p| p.path())
            .for_each(|p| paths_to_search_first.push(p));
    }

    paths_to_search_first.sort();
    paths_to_search_first.dedup();

    find_r_installations_in_paths(
        &paths_to_search_first,
        reporter,
        locators,
        true,
        global_search_paths,
        DiscoverySource::ExplicitSearch,
    );

    // If this is a conda or pixi env folder itself, do not recurse further.
    if is_conda_env(directory) || is_pixi_env(directory) {
        return;
    }

    if let Ok(reader) = fs::read_dir(directory) {
        let subdirectories = reader
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.path())
            .filter(|path| should_search_for_installations_in_path(path))
            .collect::<Vec<_>>();

        find_r_installations_in_paths(
            &subdirectories,
            reporter,
            locators,
            true,
            global_search_paths,
            DiscoverySource::ExplicitSearch,
        );
    }
}

fn find_r_installations_in_paths(
    paths: &[PathBuf],
    reporter: &dyn Reporter,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    explicit_search: bool,
    global_search_paths: &[PathBuf],
    discovery_source: DiscoverySource,
) {
    if paths.is_empty() {
        return;
    }

    thread::scope(|scope| {
        for item in paths {
            let item = item.clone();
            let locators = locators.clone();
            scope.spawn(move || {
                let executables = if explicit_search {
                    find_executable(&item).into_iter().collect::<Vec<_>>()
                } else {
                    find_executables(&item)
                };
                identify_r_executables_using_locators(
                    executables,
                    &locators,
                    reporter,
                    global_search_paths,
                    Some(discovery_source),
                );
            });
        }
    });
}

#[instrument(skip(locators, reporter, global_search_paths), fields(executable_count = executables.len()))]
pub fn identify_r_executables_using_locators(
    executables: Vec<PathBuf>,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    reporter: &dyn Reporter,
    global_search_paths: &[PathBuf],
    discovery_source: Option<DiscoverySource>,
) {
    for executable in executables {
        let raw_env = REnv::new(executable.clone(), None, None);
        if let Some(installation) =
            identify_r_installation_using_locators(&raw_env, locators, global_search_paths)
        {
            let installation = if let Some(discovery_source) = discovery_source {
                RInstallationBuilder::from_installation(installation)
                    .add_discovery_source(discovery_source)
                    .build()
            } else {
                installation
            };
            if let Some(manager) = &installation.manager {
                reporter.report_manager(manager);
            }
            reporter.report_installation(&installation);
        } else {
            warn!("Unknown R installation {:?}", executable);
        }
    }
}
