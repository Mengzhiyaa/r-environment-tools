// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::warn;
use ret_core::{
    env::REnv,
    os_environment::Environment,
    r_installation::{DiscoverySource, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    telemetry::{
        refresh_progress::{RefreshProgress, RefreshProgressPhase, RefreshProgressStatus},
        TelemetryEvent,
    },
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
    time::{Duration, Instant},
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

#[derive(Debug, Clone, Copy)]
pub struct RefreshContext {
    pub id: u64,
    pub generation: u64,
    pub started: Instant,
}

fn report_refresh_progress(
    reporter: &dyn Reporter,
    refresh: Option<RefreshContext>,
    phase: RefreshProgressPhase,
    status: RefreshProgressStatus,
    phase_elapsed: Option<Duration>,
    locator: Option<String>,
) {
    let Some(refresh) = refresh else {
        return;
    };
    reporter.report_telemetry(&TelemetryEvent::RefreshProgress(RefreshProgress {
        refresh_id: refresh.id,
        generation: refresh.generation,
        phase,
        status,
        elapsed_ms: refresh.started.elapsed().as_millis(),
        phase_elapsed_ms: phase_elapsed.map(|elapsed| elapsed.as_millis()),
        locator,
    }));
}

#[instrument(skip(reporter, configuration, locators, environment), fields(search_scope = ?search_scope))]
pub fn find_and_report_installations(
    reporter: &dyn Reporter,
    configuration: Configuration,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    environment: &dyn Environment,
    search_scope: Option<SearchScope>,
    refresh: Option<RefreshContext>,
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
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::Locators,
                RefreshProgressStatus::Started,
                None,
                None,
            );
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
                            let locator_name = format!("{:?}", locator.get_kind());
                            report_refresh_progress(
                                reporter,
                                refresh,
                                RefreshProgressPhase::Locators,
                                RefreshProgressStatus::Started,
                                None,
                                Some(locator_name.clone()),
                            );
                            locator.find(reporter);
                            let elapsed = start.elapsed();
                            summary
                                .lock()
                                .unwrap()
                                .locators
                                .insert(locator.get_kind(), elapsed);
                            report_refresh_progress(
                                reporter,
                                refresh,
                                RefreshProgressPhase::Locators,
                                RefreshProgressStatus::Completed,
                                Some(elapsed),
                                Some(locator_name),
                            );
                        });
                    }
                });
            }
            let elapsed = start.elapsed();
            summary
                .lock()
                .unwrap()
                .breakdown
                .insert("Locators", elapsed);
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::Locators,
                RefreshProgressStatus::Completed,
                Some(elapsed),
                None,
            );
        });

        let summary_for_path = summary.clone();
        scope.spawn(move || {
            let _span = info_span!("path_search_phase").entered();
            let start = std::time::Instant::now();
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::Path,
                RefreshProgressStatus::Started,
                None,
                None,
            );
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
            let elapsed = start.elapsed();
            summary_for_path
                .lock()
                .unwrap()
                .breakdown
                .insert("Path", elapsed);
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::Path,
                RefreshProgressStatus::Completed,
                Some(elapsed),
                None,
            );
        });

        let summary_for_explicit = summary.clone();
        scope.spawn(move || {
            let _span = info_span!("explicit_search_phase").entered();
            let start = std::time::Instant::now();
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::SearchPaths,
                RefreshProgressStatus::Started,
                None,
                None,
            );
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
            let elapsed = start.elapsed();
            summary_for_explicit
                .lock()
                .unwrap()
                .breakdown
                .insert("SearchPaths", elapsed);
            report_refresh_progress(
                reporter,
                refresh,
                RefreshProgressPhase::SearchPaths,
                RefreshProgressStatus::Completed,
                Some(elapsed),
                None,
            );
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
    let mut paths_to_search = Vec::new();
    collect_recursive_search_paths(directory, &mut paths_to_search);
    paths_to_search.sort();
    paths_to_search.dedup();

    // Keep the existing parallel probing bounded for large workspace trees.
    for paths in paths_to_search.chunks(64) {
        find_r_installations_in_paths(
            paths,
            reporter,
            locators,
            true,
            global_search_paths,
            DiscoverySource::ExplicitSearch,
        );
    }
}

fn collect_recursive_search_paths(directory: &PathBuf, paths: &mut Vec<PathBuf>) {
    paths.extend([
        directory.to_path_buf(),
        directory.join("bin"),
        directory.join("lib").join("R"),
        directory.join("lib").join("R").join("bin"),
        directory.join("Resources"),
        directory.join("Resources").join("bin"),
    ]);

    // Environment prefixes are terminal nodes; searching inside package
    // directories would only rediscover the same installation.
    if is_conda_env(directory) || is_pixi_env(directory) {
        return;
    }
    if let Ok(reader) = fs::read_dir(directory) {
        for entry in reader.filter_map(Result::ok) {
            let is_directory = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let path = entry.path();
            if is_directory && should_search_for_installations_in_path(&path) {
                collect_recursive_search_paths(&path, paths);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::collect_recursive_search_paths;

    #[test]
    fn recursive_search_reaches_deep_installation_directories() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let r_home = temp
            .path()
            .join("toolchains")
            .join("stable")
            .join("lib")
            .join("R");
        std::fs::create_dir_all(&r_home).expect("failed to create nested R home");

        let mut paths = Vec::new();
        collect_recursive_search_paths(&temp.path().to_path_buf(), &mut paths);

        assert!(paths.contains(&r_home));
    }

    #[test]
    fn recursive_search_skips_ignored_dependency_trees() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let ignored = temp.path().join("node_modules").join("package");
        std::fs::create_dir_all(&ignored).expect("failed to create ignored directory");

        let mut paths = Vec::new();
        collect_recursive_search_paths(&temp.path().to_path_buf(), &mut paths);

        assert!(!paths.iter().any(|path| path.starts_with(&ignored)));
    }
}
