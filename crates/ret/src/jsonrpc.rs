// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::build_resolve_output;
use crate::find::{
    find_and_report_installations, find_r_installations_in_directory_recursive,
    identify_r_executables_using_locators, SearchScope,
};
use crate::initialize_tracing;
use crate::locators::create_locators_with_conda;
use crate::resolve::resolve_installation;
use lazy_static::lazy_static;
use ret_conda::Conda;
use ret_core::{
    os_environment::{Environment, EnvironmentApi},
    r_installation::{RInstallation, RInstallationKind},
    reporter::Reporter,
    telemetry::{
        inaccurate_environment::InaccurateEnvironmentInfo, refresh_performance::RefreshPerformance,
        TelemetryEvent,
    },
    Configuration, Locator,
};
use ret_fs::glob::expand_glob_patterns;
use ret_jsonrpc::{
    send_error, send_reply,
    server::{start_server, HandlersKeyedByMethodName},
};
use ret_r_utils::cache::{clear_cache, set_cache_directory};
use ret_reporter::{cache::CacheReporter, collect, jsonrpc};
use serde::{Deserialize, Serialize};
use serde_json::{self, json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    thread,
    time::{Duration, Instant},
};

lazy_static! {
    static ref REFRESH_LOCK: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
}

pub struct Context {
    configuration: RwLock<Configuration>,
    conda_locator: Arc<Conda>,
    locators: Arc<Vec<Arc<dyn Locator>>>,
    os_environment: Arc<dyn Environment>,
}

pub fn start_jsonrpc_server() {
    initialize_tracing(false);

    let environment = EnvironmentApi::new();
    let conda_locator = Arc::new(Conda::from(&environment));
    let context = Context {
        locators: create_locators_with_conda(&environment, conda_locator.clone()),
        configuration: RwLock::new(Configuration::default()),
        conda_locator,
        os_environment: Arc::new(environment),
    };

    let mut handlers = HandlersKeyedByMethodName::new(Arc::new(context));
    handlers.add_request_handler("configure", handle_configure);
    handlers.add_request_handler("refresh", handle_refresh);
    handlers.add_request_handler("find", handle_find);
    handlers.add_request_handler("resolve", handle_resolve);
    handlers.add_request_handler("condaInfo", handle_conda_info);
    handlers.add_request_handler("clear", handle_clear_cache);
    handlers.add_request_handler("clearCache", handle_clear_cache);
    start_server(&handlers)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureOptions {
    pub workspace_directories: Option<Vec<PathBuf>>,
    pub environment_directories: Option<Vec<PathBuf>>,
    pub search_directories: Option<Vec<PathBuf>>,
    pub executables: Option<Vec<PathBuf>>,
    pub conda_executable: Option<PathBuf>,
    pub rig_executable: Option<PathBuf>,
    pub cache_directory: Option<PathBuf>,
}

pub fn handle_configure(context: Arc<Context>, id: u32, params: Value) {
    match serde_json::from_value::<ConfigureOptions>(params.clone()) {
        Ok(configure_options) => {
            thread::spawn(move || {
                let workspace_directories =
                    expand_configured_directories(&configure_options.workspace_directories);
                let environment_directories =
                    expand_configured_directories(&configure_options.environment_directories);
                let search_directories =
                    expand_configured_directories(&configure_options.search_directories);
                let executables = configure_options.executables.map(|paths| {
                    expand_glob_patterns(&paths)
                        .into_iter()
                        .filter(|path| path.is_file())
                        .collect::<Vec<_>>()
                });

                let mut configuration = context.configuration.write().unwrap();
                configuration.workspace_directories = workspace_directories;
                configuration.environment_directories = environment_directories;
                configuration.search_directories = search_directories;
                configuration.executables = executables;
                configuration.conda_executable = configure_options.conda_executable.clone();
                configuration.rig_executable = configure_options.rig_executable.clone();
                if let Some(cache_directory) = configure_options.cache_directory {
                    set_cache_directory(cache_directory.clone());
                    configuration.cache_directory = Some(cache_directory);
                }
                let config = configuration.clone();
                drop(configuration);

                for locator in context.locators.iter() {
                    locator.configure(&config);
                }

                send_reply(id, None::<()>);
            });
        }
        Err(err) => send_error(
            Some(id),
            -4,
            format!("Failed to parse configure {params:?}: {err}"),
        ),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOptions {
    pub search_kind: Option<String>,
    pub search_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefreshResult {
    duration: u128,
}

impl RefreshResult {
    pub fn new(duration: Duration) -> RefreshResult {
        RefreshResult {
            duration: duration.as_millis(),
        }
    }
}

pub fn handle_refresh(context: Arc<Context>, id: u32, params: Value) {
    let params = match params {
        Value::Null | Value::Array(_) => serde_json::json!({}),
        _ => params,
    };
    match serde_json::from_value::<Option<RefreshOptions>>(params.clone()) {
        Ok(refresh_options) => {
            let refresh_options = refresh_options.unwrap_or(RefreshOptions {
                search_kind: None,
                search_paths: None,
            });
            thread::spawn(move || {
                let _lock = REFRESH_LOCK.lock().expect("refresh lock poisoned");
                let config = context.configuration.read().unwrap().clone();
                let report_only = refresh_options
                    .search_kind
                    .as_deref()
                    .and_then(parse_search_kind);
                let reporter = Arc::new(CacheReporter::new(Arc::new(jsonrpc::create_reporter(
                    report_only,
                ))));

                let (config, search_scope) = build_refresh_config(&refresh_options, config);
                for locator in context.locators.iter() {
                    locator.configure(&config);
                }

                let start = Instant::now();
                let telemetry_config = config.clone();
                let telemetry_scope = search_scope.clone();
                let summary = find_and_report_installations(
                    reporter.as_ref(),
                    config,
                    &context.locators,
                    context.os_environment.as_ref(),
                    search_scope,
                );
                let elapsed = start.elapsed();
                send_reply(id, Some(RefreshResult::new(elapsed)));

                let summary = summary.lock().expect("summary mutex poisoned");
                emit_refresh_telemetry(
                    reporter.as_ref(),
                    &summary,
                    &telemetry_config,
                    telemetry_scope.as_ref(),
                );
            });
        }
        Err(err) => send_error(
            Some(id),
            -4,
            format!("Failed to parse refresh {params:?}: {err}"),
        ),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindOptions {
    pub search_path: PathBuf,
}

pub fn handle_find(context: Arc<Context>, id: u32, params: Value) {
    match serde_json::from_value::<FindOptions>(params.clone()) {
        Ok(find_options) => {
            thread::spawn(move || {
                let config = context.configuration.read().unwrap().clone();
                for locator in context.locators.iter() {
                    locator.configure(&config);
                }

                let global_search_paths = context.os_environment.get_know_global_search_locations();
                let reporter = CacheReporter::new(Arc::new(collect::create_reporter()));
                if find_options.search_path.is_file() {
                    identify_r_executables_using_locators(
                        vec![find_options.search_path.clone()],
                        &context.locators,
                        &reporter,
                        &global_search_paths,
                        Some(ret_core::r_installation::DiscoverySource::ExplicitSearch),
                    );
                } else {
                    find_r_installations_in_directory_recursive(
                        &find_options.search_path,
                        &reporter,
                        &context.locators,
                        &global_search_paths,
                    );
                }

                let installations = reporter.get_installations();
                let payload = build_find_response(installations);
                send_reply(id, payload);
            });
        }
        Err(err) => send_error(
            Some(id),
            -4,
            format!("Failed to parse find {params:?}: {err}"),
        ),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveOptions {
    pub executable: PathBuf,
}

pub fn handle_resolve(context: Arc<Context>, id: u32, params: Value) {
    match serde_json::from_value::<ResolveOptions>(params.clone()) {
        Ok(resolve_options) => {
            thread::spawn(move || {
                let configuration = context.configuration.read().unwrap().clone();
                for locator in context.locators.iter() {
                    locator.configure(&configuration);
                }

                let result: Option<RInstallation> = resolve_installation(
                    &resolve_options.executable,
                    &context.locators,
                    context.os_environment.as_ref(),
                )
                .map(|result| {
                    if let Some(resolved) = result.resolved {
                        if let Some(inaccuracy) =
                            detect_inaccurate_environment(&result.discovered, &resolved)
                        {
                            let reporter = jsonrpc::create_reporter(None);
                            reporter.report_telemetry(&TelemetryEvent::InaccurateEnvironmentInfo(
                                inaccuracy,
                            ));
                        }
                        resolved
                    } else {
                        result.discovered
                    }
                });
                match result {
                    Some(installation) => {
                        let result = build_resolve_output(installation);
                        send_reply(id, Some(result));
                    }
                    None => send_error(
                        Some(id),
                        -4,
                        format!(
                            "Failed to resolve installation {:?}",
                            resolve_options.executable
                        ),
                    ),
                }
            });
        }
        Err(err) => send_error(
            Some(id),
            -4,
            format!("Failed to parse resolve {params:?}: {err}"),
        ),
    }
}

pub fn handle_conda_info(context: Arc<Context>, id: u32, _params: Value) {
    thread::spawn(move || {
        let conda_executable = context
            .configuration
            .read()
            .unwrap()
            .conda_executable
            .clone();
        let info = context
            .conda_locator
            .get_info_for_telemetry(conda_executable);
        send_reply(id, Some(info));
    });
}

pub fn handle_clear_cache(_context: Arc<Context>, id: u32, _params: Value) {
    thread::spawn(move || match clear_cache() {
        Ok(_) => send_reply(id, None::<()>),
        Err(err) => send_error(Some(id), -4, format!("Failed to clear cache {err:?}")),
    });
}

fn build_refresh_config(
    refresh_options: &RefreshOptions,
    mut config: Configuration,
) -> (Configuration, Option<SearchScope>) {
    if let Some(search_paths) = &refresh_options.search_paths {
        let expanded = expand_glob_patterns(search_paths);
        config.workspace_directories = None;
        config.environment_directories = None;
        config.search_directories = Some(
            expanded
                .iter()
                .filter(|path| path.is_dir())
                .cloned()
                .collect(),
        );
        config.executables = Some(
            expanded
                .iter()
                .filter(|path| path.is_file())
                .cloned()
                .collect(),
        );
    }

    let search_scope = if refresh_options.search_paths.is_some() {
        Some(SearchScope::SearchPaths)
    } else {
        refresh_options
            .search_kind
            .as_deref()
            .and_then(parse_search_kind)
            .map(SearchScope::Global)
    };
    (config, search_scope)
}

fn expand_configured_directories(paths: &Option<Vec<PathBuf>>) -> Option<Vec<PathBuf>> {
    let mut directories = paths
        .as_ref()
        .map(|paths| expand_glob_patterns(paths))
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    directories.sort();
    directories.dedup();

    if directories.is_empty() {
        None
    } else {
        Some(directories)
    }
}

fn parse_search_kind(kind: &str) -> Option<RInstallationKind> {
    serde_json::from_value::<RInstallationKind>(json!(kind)).ok()
}

fn build_find_response(installations: Vec<RInstallation>) -> Option<Value> {
    if installations.is_empty() {
        return None;
    }
    Some(json!(installations))
}

fn emit_refresh_telemetry(
    reporter: &dyn Reporter,
    summary: &crate::find::Summary,
    config: &Configuration,
    search_scope: Option<&SearchScope>,
) {
    let path_duration = summary.breakdown.get("Path").copied().unwrap_or_default();
    let locators_duration = summary
        .breakdown
        .get("Locators")
        .copied()
        .unwrap_or_default();
    let search_paths_duration = summary
        .breakdown
        .get("SearchPaths")
        .copied()
        .unwrap_or_default();
    let custom_search_path_count = count_configured_search_paths(config);

    if !matches!(search_scope, Some(SearchScope::SearchPaths)) {
        reporter.report_telemetry(&TelemetryEvent::GlobalEnvironmentsSearchCompleted(
            locators_duration + path_duration,
        ));
        reporter.report_telemetry(
            &TelemetryEvent::GlobalPathVariableEnvironmentsSearchCompleted(path_duration),
        );
    }
    if custom_search_path_count > 0 {
        reporter.report_telemetry(&TelemetryEvent::AllSearchPathsEnvironmentsSearchCompleted(
            search_paths_duration,
            custom_search_path_count,
        ));
    }
    reporter.report_telemetry(&TelemetryEvent::SearchCompleted(summary.total));
    reporter.report_telemetry(&TelemetryEvent::RefreshPerformance(RefreshPerformance {
        total: summary.total.as_millis(),
        locators: summary
            .locators
            .iter()
            .map(|(key, value)| (format!("{key:?}"), value.as_millis()))
            .collect::<BTreeMap<String, u128>>(),
        breakdown: summary
            .breakdown
            .iter()
            .map(|(key, value)| (key.to_string(), value.as_millis()))
            .collect::<BTreeMap<String, u128>>(),
    }));
}

fn count_configured_search_paths(config: &Configuration) -> u32 {
    [
        config
            .workspace_directories
            .as_ref()
            .map(Vec::len)
            .unwrap_or(0),
        config
            .environment_directories
            .as_ref()
            .map(Vec::len)
            .unwrap_or(0),
        config
            .search_directories
            .as_ref()
            .map(Vec::len)
            .unwrap_or(0),
        config.executables.as_ref().map(Vec::len).unwrap_or(0),
    ]
    .into_iter()
    .sum::<usize>() as u32
}

fn detect_inaccurate_environment(
    discovered: &RInstallation,
    resolved: &RInstallation,
) -> Option<InaccurateEnvironmentInfo> {
    let invalid_executable = discovered
        .executable
        .as_ref()
        .map(|_| discovered.executable != resolved.executable);
    let executable_not_in_symlinks = match (&discovered.executable, &resolved.known_executables) {
        (Some(executable), Some(known_executables)) => {
            Some(!known_executables.contains(executable))
        }
        _ => None,
    };
    let invalid_prefix = discovered
        .home
        .as_ref()
        .map(|_| discovered.home != resolved.home);
    let invalid_version = discovered
        .version
        .as_ref()
        .map(|_| discovered.version != resolved.version);
    let invalid_arch = discovered
        .arch
        .as_ref()
        .map(|_| discovered.arch != resolved.arch);
    let inaccuracy = InaccurateEnvironmentInfo {
        kind: resolved.kind.or(discovered.kind),
        invalid_executable,
        executable_not_in_symlinks,
        invalid_prefix,
        invalid_version,
        invalid_arch,
    };

    if [
        inaccuracy.invalid_executable,
        inaccuracy.executable_not_in_symlinks,
        inaccuracy.invalid_prefix,
        inaccuracy.invalid_version,
        inaccuracy.invalid_arch,
    ]
    .into_iter()
    .flatten()
    .any(|value| value)
    {
        Some(inaccuracy)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_find_response, count_configured_search_paths, detect_inaccurate_environment,
        parse_search_kind, RefreshOptions,
    };
    use crate::find::SearchScope;
    use ret_core::{
        arch::Architecture,
        r_installation::{RInstallationBuilder, RInstallationKind},
        Configuration,
    };
    use std::path::PathBuf;

    #[test]
    fn parses_search_kind_into_r_kind() {
        assert_eq!(parse_search_kind("Conda"), Some(RInstallationKind::Conda));
        assert_eq!(
            parse_search_kind("MacFramework"),
            Some(RInstallationKind::MacFramework)
        );
        assert_eq!(
            parse_search_kind("WindowsRegistry"),
            Some(RInstallationKind::WindowsRegistry)
        );
        assert_eq!(parse_search_kind("Poetry"), None);
        // PET aliases no longer accepted
        assert_eq!(parse_search_kind("MacPythonOrg"), None);
    }

    #[test]
    fn build_refresh_config_preserves_configured_directories_when_only_kind_changes() {
        let config = Configuration {
            search_directories: Some(vec![PathBuf::from("/tmp/workspace")]),
            ..Configuration::default()
        };
        let refresh = RefreshOptions {
            search_kind: Some("Conda".to_string()),
            search_paths: None,
        };

        let (config, scope) = super::build_refresh_config(&refresh, config);
        assert_eq!(
            config.search_directories,
            Some(vec![PathBuf::from("/tmp/workspace")])
        );
        assert_eq!(scope, Some(SearchScope::Global(RInstallationKind::Conda)));
    }

    #[test]
    fn build_refresh_config_uses_search_paths_scope_when_paths_are_provided() {
        let refresh = RefreshOptions {
            search_kind: Some("Conda".to_string()),
            search_paths: Some(vec![PathBuf::from("/tmp/workspace")]),
        };

        let (_, scope) = super::build_refresh_config(&refresh, Configuration::default());
        assert_eq!(scope, Some(SearchScope::SearchPaths));
    }

    #[test]
    fn build_find_response_uses_ret_array_shape() {
        let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda))
            .executable(Some(PathBuf::from("/tmp/R/bin/R")))
            .home(Some(PathBuf::from("/tmp/R")))
            .version(Some("4.4.1".to_string()))
            .arch(Some(Architecture::X64))
            .build();

        let value = build_find_response(vec![installation]).expect("expected payload");
        let items = value.as_array().expect("response should be an array");
        assert_eq!(items.len(), 1);
        let expected_home = ret_fs::path::norm_case(PathBuf::from("/tmp/R"));
        assert_eq!(
            items[0]["home"],
            expected_home.to_string_lossy().to_string()
        );
        assert_eq!(items[0]["kind"], "Conda");
    }

    #[test]
    fn count_configured_search_paths_includes_directories_and_executables() {
        let config = Configuration {
            workspace_directories: Some(vec![PathBuf::from("/tmp/workspace")]),
            environment_directories: Some(vec![
                PathBuf::from("/tmp/envs-a"),
                PathBuf::from("/tmp/envs-b"),
            ]),
            search_directories: Some(vec![PathBuf::from("/tmp/search")]),
            executables: Some(vec![PathBuf::from("/tmp/R/bin/R")]),
            ..Configuration::default()
        };

        assert_eq!(count_configured_search_paths(&config), 5);
    }

    #[test]
    fn detect_inaccurate_environment_reports_prefix_mismatch() {
        let discovered = RInstallationBuilder::new(Some(RInstallationKind::GlobalPaths))
            .executable(Some(PathBuf::from("/tmp/discovered/bin/R")))
            .home(Some(PathBuf::from("/tmp/discovered")))
            .build();
        let resolved = RInstallationBuilder::new(Some(RInstallationKind::GlobalPaths))
            .executable(Some(PathBuf::from("/tmp/discovered/bin/R")))
            .home(Some(PathBuf::from("/tmp/resolved")))
            .symlinks(Some(vec![PathBuf::from("/tmp/discovered/bin/R")]))
            .build();

        let inaccuracy =
            detect_inaccurate_environment(&discovered, &resolved).expect("expected telemetry");
        assert_eq!(inaccuracy.kind, Some(RInstallationKind::GlobalPaths));
        assert_eq!(inaccuracy.invalid_prefix, Some(true));
        assert_eq!(inaccuracy.invalid_executable, Some(false));
    }
}
