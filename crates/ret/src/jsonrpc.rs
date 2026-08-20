// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::build_resolve_output;
use crate::find::{
    find_and_report_installations, find_r_installations_in_directory_recursive,
    identify_r_executables_using_locators, RefreshContext, SearchScope,
};
use crate::initialize_tracing;
use crate::locators::create_locators_with_conda;
use crate::resolve::resolve_installation;
use log::{error, warn};
use ret_conda::Conda;
use ret_core::{
    os_environment::{Environment, EnvironmentApi},
    r_installation::{RInstallation, RInstallationKind},
    reporter::Reporter,
    telemetry::{
        inaccurate_environment::InaccurateEnvironmentInfo, refresh_performance::RefreshPerformance,
        TelemetryEvent,
    },
    Configuration, Locator, RefreshStatePersistence, RefreshStateSyncScope,
};
use ret_fs::glob::expand_glob_patterns;
use ret_fs::path::norm_case;
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
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_REFRESH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default)]
struct ConfigurationState {
    generation: u64,
    config: Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefreshKey {
    options: RefreshOptions,
    configuration_generation: u64,
}

#[derive(Debug)]
struct ActiveRefresh {
    key: RefreshKey,
    request_ids: Vec<u32>,
}

#[derive(Debug, Default)]
enum RefreshCoordinatorState {
    #[default]
    Idle,
    Running(ActiveRefresh),
    Completing(ActiveRefresh),
}

#[derive(Debug, Default)]
struct RefreshCoordinator {
    state: Mutex<RefreshCoordinatorState>,
    changed: Condvar,
}

enum RefreshRegistration {
    Start,
    Joined,
    Wait,
}

impl RefreshCoordinator {
    fn register(&self, request_id: u32, key: RefreshKey) -> RefreshRegistration {
        let mut state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        match &mut *state {
            RefreshCoordinatorState::Idle => {
                *state = RefreshCoordinatorState::Running(ActiveRefresh {
                    key,
                    request_ids: vec![request_id],
                });
                RefreshRegistration::Start
            }
            RefreshCoordinatorState::Running(active)
            | RefreshCoordinatorState::Completing(active)
                if active.key == key =>
            {
                active.request_ids.push(request_id);
                RefreshRegistration::Joined
            }
            RefreshCoordinatorState::Running(_) | RefreshCoordinatorState::Completing(_) => {
                RefreshRegistration::Wait
            }
        }
    }

    fn wait_until_idle(&self) {
        let state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        drop(
            self.changed
                .wait_while(state, |state| {
                    !matches!(state, RefreshCoordinatorState::Idle)
                })
                .expect("refresh coordinator condvar poisoned"),
        );
    }

    fn begin_completion(&self, key: &RefreshKey) {
        let mut state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        match std::mem::replace(&mut *state, RefreshCoordinatorState::Idle) {
            RefreshCoordinatorState::Running(active) if active.key == *key => {
                *state = RefreshCoordinatorState::Completing(active);
            }
            other => {
                *state = other;
                panic!("refresh coordinator completion key/state mismatch");
            }
        }
    }

    fn drain_request_ids(&self, key: &RefreshKey) -> Vec<u32> {
        let mut state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        match &mut *state {
            RefreshCoordinatorState::Completing(active) if active.key == *key => {
                std::mem::take(&mut active.request_ids)
            }
            RefreshCoordinatorState::Idle => Vec::new(),
            _ => panic!("refresh coordinator drain key/state mismatch"),
        }
    }

    fn complete_if_empty(&self, key: &RefreshKey) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        match &*state {
            RefreshCoordinatorState::Completing(active)
                if active.key == *key && active.request_ids.is_empty() =>
            {
                *state = RefreshCoordinatorState::Idle;
                self.changed.notify_all();
                true
            }
            RefreshCoordinatorState::Completing(active) if active.key == *key => false,
            _ => panic!("refresh coordinator completion key/state mismatch"),
        }
    }

    fn force_idle(&self, key: &RefreshKey) {
        let mut state = self
            .state
            .lock()
            .expect("refresh coordinator mutex poisoned");
        let owns_state = match &*state {
            RefreshCoordinatorState::Running(active)
            | RefreshCoordinatorState::Completing(active) => active.key == *key,
            RefreshCoordinatorState::Idle => false,
        };
        if owns_state {
            *state = RefreshCoordinatorState::Idle;
            self.changed.notify_all();
        }
    }
}

struct RefreshGuard<'a> {
    coordinator: &'a RefreshCoordinator,
    key: RefreshKey,
    completed: bool,
}

impl<'a> RefreshGuard<'a> {
    fn new(coordinator: &'a RefreshCoordinator, key: RefreshKey) -> Self {
        Self {
            coordinator,
            key,
            completed: false,
        }
    }

    fn begin_completion(&self) {
        self.coordinator.begin_completion(&self.key);
    }

    fn finish_replies(&mut self, result: Result<&RefreshResult, &str>) {
        loop {
            for request_id in self.coordinator.drain_request_ids(&self.key) {
                match result {
                    Ok(result) => send_reply(request_id, Some(result.clone())),
                    Err(message) => send_error(Some(request_id), -4, message.to_string()),
                }
            }
            if self.coordinator.complete_if_empty(&self.key) {
                self.completed = true;
                return;
            }
        }
    }
}

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.coordinator.force_idle(&self.key);
        }
    }
}

struct GenerationGuardedReporter {
    reporter: Arc<dyn Reporter>,
    configuration: Arc<RwLock<ConfigurationState>>,
    generation: u64,
}

impl GenerationGuardedReporter {
    fn report_if_current(&self, report: impl FnOnce(&dyn Reporter)) {
        let state = self
            .configuration
            .read()
            .expect("configuration rwlock poisoned");
        if state.generation == self.generation {
            report(self.reporter.as_ref());
        }
    }
}

impl Reporter for GenerationGuardedReporter {
    fn report_manager(&self, manager: &ret_core::manager::EnvManager) {
        self.report_if_current(|reporter| reporter.report_manager(manager));
    }

    fn report_installation(&self, installation: &RInstallation) {
        self.report_if_current(|reporter| reporter.report_installation(installation));
    }

    fn report_telemetry(&self, event: &TelemetryEvent) {
        self.report_if_current(|reporter| reporter.report_telemetry(event));
    }
}

pub struct Context {
    configuration: Arc<RwLock<ConfigurationState>>,
    configure_in_progress: Mutex<()>,
    conda_locator: Arc<Conda>,
    locators: Arc<Vec<Arc<dyn Locator>>>,
    os_environment: Arc<dyn Environment>,
    refresh_coordinator: RefreshCoordinator,
}

pub fn start_jsonrpc_server() {
    initialize_tracing(false);

    let environment = EnvironmentApi::new();
    let conda_locator = Arc::new(Conda::from(&environment));
    let context = Context {
        locators: create_locators_with_conda(&environment, conda_locator.clone()),
        configuration: Arc::new(RwLock::new(ConfigurationState::default())),
        configure_in_progress: Mutex::new(()),
        conda_locator,
        os_environment: Arc::new(environment),
        refresh_coordinator: RefreshCoordinator::default(),
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
                let executables = configure_options.executables.as_ref().map(|paths| {
                    expand_glob_patterns(paths)
                        .into_iter()
                        .filter(|path| path.is_file())
                        .collect::<Vec<_>>()
                });

                if let Err(message) = apply_configuration(
                    &context,
                    configure_options,
                    workspace_directories,
                    environment_directories,
                    search_directories,
                    executables,
                ) {
                    send_error(Some(id), -4, message);
                    return;
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

fn apply_configuration(
    context: &Context,
    options: ConfigureOptions,
    workspace_directories: Option<Vec<PathBuf>>,
    environment_directories: Option<Vec<PathBuf>>,
    search_directories: Option<Vec<PathBuf>>,
    executables: Option<Vec<PathBuf>>,
) -> Result<(), String> {
    let _configure_guard = context
        .configure_in_progress
        .lock()
        .expect("configure mutex poisoned");
    let (previous_config, mut next_config, next_generation) = {
        let state = context
            .configuration
            .read()
            .expect("configuration rwlock poisoned");
        (
            state.config.clone(),
            state.config.clone(),
            state.generation + 1,
        )
    };

    next_config.workspace_directories = workspace_directories;
    next_config.environment_directories = environment_directories;
    next_config.search_directories = search_directories;
    next_config.executables = executables;
    next_config.conda_executable = options.conda_executable;
    next_config.rig_executable = options.rig_executable;
    if let Some(cache_directory) = options.cache_directory.clone() {
        next_config.cache_directory = Some(cache_directory);
    }

    let configured = panic::catch_unwind(AssertUnwindSafe(|| {
        for locator in context.locators.iter() {
            locator.configure(&next_config);
        }
        if let Some(cache_directory) = options.cache_directory {
            set_cache_directory(cache_directory);
        }
    }));
    if let Err(payload) = configured {
        for locator in context.locators.iter() {
            locator.configure(&previous_config);
        }
        return Err(format!(
            "Configuration generation {next_generation} failed: {}",
            panic_message(payload.as_ref())
        ));
    }

    let mut state = context
        .configuration
        .write()
        .expect("configuration rwlock poisoned");
    state.config = next_config;
    state.generation = next_generation;
    Ok(())
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOptions {
    pub search_kind: Option<RInstallationKind>,
    pub search_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    duration: u128,
    refresh_id: u64,
}

impl RefreshResult {
    pub fn new(duration: Duration, refresh_id: u64) -> RefreshResult {
        RefreshResult {
            duration: duration.as_millis(),
            refresh_id,
        }
    }
}

fn parse_refresh_options(params: Value) -> Result<RefreshOptions, serde_json::Error> {
    let params = match params {
        Value::Null => json!({}),
        Value::Array(values) if values.is_empty() => json!({}),
        other => other,
    };
    serde_json::from_value::<Option<RefreshOptions>>(params).map(|options| {
        let mut options = options.unwrap_or_default();
        if let Some(search_paths) = options.search_paths.take() {
            let mut paths = expand_glob_patterns(&search_paths)
                .into_iter()
                .map(norm_case)
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            options.search_paths = Some(paths);
        }
        options
    })
}

struct RefreshExecution {
    result: RefreshResult,
    reporter: Arc<CacheReporter>,
    summary: Arc<Mutex<crate::find::Summary>>,
    config: Configuration,
    search_scope: Option<SearchScope>,
}

fn create_request_locators(
    context: &Context,
    config: &Configuration,
) -> Arc<Vec<Arc<dyn Locator>>> {
    let conda = Arc::new(Conda::from_shared_environment_cache(
        context.os_environment.as_ref(),
        context.conda_locator.as_ref(),
    ));
    let locators = create_locators_with_conda(context.os_environment.as_ref(), conda);
    for locator in locators.iter() {
        locator.configure(config);
    }
    locators
}

fn refresh_sync_scope(search_scope: Option<&SearchScope>) -> RefreshStateSyncScope {
    match search_scope {
        Some(SearchScope::Global(kind)) => RefreshStateSyncScope::GlobalFiltered(*kind),
        Some(SearchScope::SearchPaths) => RefreshStateSyncScope::Workspace,
        None => RefreshStateSyncScope::Full,
    }
}

fn sync_refresh_locator_state(
    target: &[Arc<dyn Locator>],
    source: &[Arc<dyn Locator>],
    search_scope: Option<&SearchScope>,
) {
    assert_eq!(target.len(), source.len(), "refresh locator graphs drifted");
    let scope = refresh_sync_scope(search_scope);
    for (target, source) in target.iter().zip(source.iter()) {
        assert_eq!(
            target.get_kind(),
            source.get_kind(),
            "refresh locator order drifted"
        );
        if matches!(
            target.refresh_state(),
            RefreshStatePersistence::SyncedDiscoveryState
        ) {
            target.sync_refresh_state_from(source.as_ref(), &scope);
        }
    }
}

fn execute_refresh(
    context: &Context,
    options: &RefreshOptions,
    configuration: &ConfigurationState,
) -> RefreshExecution {
    let refresh = RefreshContext {
        id: NEXT_REFRESH_ID.fetch_add(1, Ordering::Relaxed),
        generation: configuration.generation,
        started: Instant::now(),
    };
    let (config, search_scope) = build_refresh_config(options, configuration.config.clone());
    let locators = create_request_locators(context, &config);
    let guarded_reporter = GenerationGuardedReporter {
        reporter: Arc::new(jsonrpc::create_reporter(options.search_kind)),
        configuration: context.configuration.clone(),
        generation: configuration.generation,
    };
    let reporter = Arc::new(CacheReporter::new(Arc::new(guarded_reporter)));
    report_progress(
        reporter.as_ref(),
        refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressPhase::Refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressStatus::Started,
        None,
    );
    let summary = find_and_report_installations(
        reporter.as_ref(),
        config.clone(),
        &locators,
        context.os_environment.as_ref(),
        search_scope.clone(),
        Some(refresh),
    );

    let merge_started = Instant::now();
    report_progress(
        reporter.as_ref(),
        refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressPhase::Merge,
        ret_core::telemetry::refresh_progress::RefreshProgressStatus::Started,
        None,
    );
    let state = context
        .configuration
        .read()
        .expect("configuration rwlock poisoned");
    if state.generation == configuration.generation {
        sync_refresh_locator_state(
            context.locators.as_ref(),
            locators.as_ref(),
            search_scope.as_ref(),
        );
    } else {
        warn!(
            "Skipping state sync for stale refresh generation {} (current {})",
            configuration.generation, state.generation
        );
    }
    drop(state);
    report_progress(
        reporter.as_ref(),
        refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressPhase::Merge,
        ret_core::telemetry::refresh_progress::RefreshProgressStatus::Completed,
        Some(merge_started.elapsed()),
    );

    let duration = summary.lock().expect("summary mutex poisoned").total;
    report_progress(
        reporter.as_ref(),
        refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressPhase::Refresh,
        ret_core::telemetry::refresh_progress::RefreshProgressStatus::Completed,
        Some(refresh.started.elapsed()),
    );
    RefreshExecution {
        result: RefreshResult::new(duration, refresh.id),
        reporter,
        summary,
        config,
        search_scope,
    }
}

fn report_progress(
    reporter: &dyn Reporter,
    refresh: RefreshContext,
    phase: ret_core::telemetry::refresh_progress::RefreshProgressPhase,
    status: ret_core::telemetry::refresh_progress::RefreshProgressStatus,
    phase_elapsed: Option<Duration>,
) {
    reporter.report_telemetry(&TelemetryEvent::RefreshProgress(
        ret_core::telemetry::refresh_progress::RefreshProgress {
            refresh_id: refresh.id,
            generation: refresh.generation,
            phase,
            status,
            elapsed_ms: refresh.started.elapsed().as_millis(),
            phase_elapsed_ms: phase_elapsed.map(|elapsed| elapsed.as_millis()),
            locator: None,
        },
    ));
}

pub fn handle_refresh(context: Arc<Context>, id: u32, params: Value) {
    match parse_refresh_options(params.clone()) {
        Ok(refresh_options) => {
            thread::spawn(move || loop {
                let configuration = context
                    .configuration
                    .read()
                    .expect("configuration rwlock poisoned")
                    .clone();
                let key = RefreshKey {
                    options: refresh_options.clone(),
                    configuration_generation: configuration.generation,
                };
                match context.refresh_coordinator.register(id, key.clone()) {
                    RefreshRegistration::Joined => return,
                    RefreshRegistration::Wait => context.refresh_coordinator.wait_until_idle(),
                    RefreshRegistration::Start => {
                        let mut guard = RefreshGuard::new(&context.refresh_coordinator, key);
                        let execution = panic::catch_unwind(AssertUnwindSafe(|| {
                            execute_refresh(&context, &refresh_options, &configuration)
                        }));
                        guard.begin_completion();
                        match execution {
                            Ok(execution) => {
                                guard.finish_replies(Ok(&execution.result));
                                let summary =
                                    execution.summary.lock().expect("summary mutex poisoned");
                                emit_refresh_telemetry(
                                    execution.reporter.as_ref(),
                                    &summary,
                                    &execution.config,
                                    execution.search_scope.as_ref(),
                                );
                            }
                            Err(payload) => {
                                error!("Refresh failed: {}", panic_message(payload.as_ref()));
                                guard.finish_replies(Err("Refresh failed unexpectedly"));
                            }
                        }
                        return;
                    }
                }
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
                let config = context
                    .configuration
                    .read()
                    .expect("configuration rwlock poisoned")
                    .config
                    .clone();
                let locators = create_request_locators(&context, &config);

                let global_search_paths = context.os_environment.get_know_global_search_locations();
                let reporter = CacheReporter::new(Arc::new(collect::create_reporter()));
                if find_options.search_path.is_file() {
                    identify_r_executables_using_locators(
                        vec![find_options.search_path.clone()],
                        &locators,
                        &reporter,
                        &global_search_paths,
                        Some(ret_core::r_installation::DiscoverySource::ExplicitSearch),
                    );
                } else {
                    find_r_installations_in_directory_recursive(
                        &find_options.search_path,
                        &reporter,
                        &locators,
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
                let configuration = context
                    .configuration
                    .read()
                    .expect("configuration rwlock poisoned")
                    .config
                    .clone();
                let locators = create_request_locators(&context, &configuration);

                let result: Option<RInstallation> = resolve_installation(
                    &resolve_options.executable,
                    &locators,
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
        let config = context.configuration.read().unwrap().config.clone();
        let conda = Conda::from_shared_environment_cache(
            context.os_environment.as_ref(),
            context.conda_locator.as_ref(),
        );
        conda.configure(&config);
        let info = conda.get_info_for_telemetry(config.conda_executable);
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
        refresh_options.search_kind.map(SearchScope::Global)
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
        ConfigurationState, GenerationGuardedReporter, RefreshCoordinator, RefreshKey,
        RefreshOptions, RefreshRegistration,
    };
    use crate::find::SearchScope;
    use ret_core::{
        arch::Architecture,
        manager::EnvManager,
        r_installation::{RInstallationBuilder, RInstallationKind},
        reporter::Reporter,
        telemetry::TelemetryEvent,
        Configuration, LocatorKind, RefreshStatePersistence,
    };
    use std::{
        path::PathBuf,
        sync::{Arc, Barrier, Mutex, RwLock},
        thread,
    };

    #[derive(Default)]
    struct RecordingReporter {
        installations: Mutex<usize>,
    }

    struct BlockingReporter {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl Reporter for BlockingReporter {
        fn report_manager(&self, _manager: &EnvManager) {}

        fn report_installation(&self, _installation: &ret_core::r_installation::RInstallation) {
            self.entered.wait();
            self.release.wait();
        }
    }

    impl Reporter for RecordingReporter {
        fn report_manager(&self, _manager: &EnvManager) {}

        fn report_installation(&self, _installation: &ret_core::r_installation::RInstallation) {
            *self.installations.lock().unwrap() += 1;
        }

        fn report_telemetry(&self, _event: &TelemetryEvent) {}
    }

    #[test]
    fn build_refresh_config_preserves_configured_directories_when_only_kind_changes() {
        let config = Configuration {
            search_directories: Some(vec![PathBuf::from("/tmp/workspace")]),
            ..Configuration::default()
        };
        let refresh = RefreshOptions {
            search_kind: Some(RInstallationKind::Conda),
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
            search_kind: Some(RInstallationKind::Conda),
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

    #[test]
    fn stale_generation_reporter_drops_installations() {
        let configuration = Arc::new(RwLock::new(ConfigurationState {
            generation: 2,
            config: Configuration::default(),
        }));
        let recording = Arc::new(RecordingReporter::default());
        let reporter = GenerationGuardedReporter {
            reporter: recording.clone(),
            configuration: configuration.clone(),
            generation: 1,
        };

        reporter.report_installation(&Default::default());
        assert_eq!(*recording.installations.lock().unwrap(), 0);

        configuration.write().unwrap().generation = 1;
        reporter.report_installation(&Default::default());
        assert_eq!(*recording.installations.lock().unwrap(), 1);
    }

    #[test]
    fn generation_cannot_change_while_a_notification_is_being_sent() {
        let configuration = Arc::new(RwLock::new(ConfigurationState {
            generation: 1,
            config: Configuration::default(),
        }));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let reporter = Arc::new(GenerationGuardedReporter {
            reporter: Arc::new(BlockingReporter {
                entered: entered.clone(),
                release: release.clone(),
            }),
            configuration: configuration.clone(),
            generation: 1,
        });
        let report_thread = thread::spawn(move || {
            reporter.report_installation(&Default::default());
        });
        entered.wait();
        assert!(
            configuration.try_write().is_err(),
            "generation lock was released before the notification completed"
        );
        release.wait();
        report_thread.join().unwrap();
        configuration.write().unwrap().generation = 2;
    }

    #[test]
    fn refresh_coordinator_joins_identical_requests_and_recovers_on_drop() {
        let coordinator = RefreshCoordinator::default();
        let key = RefreshKey {
            options: RefreshOptions::default(),
            configuration_generation: 4,
        };
        assert!(matches!(
            coordinator.register(10, key.clone()),
            RefreshRegistration::Start
        ));
        assert!(matches!(
            coordinator.register(11, key.clone()),
            RefreshRegistration::Joined
        ));
        coordinator.begin_completion(&key);
        assert_eq!(coordinator.drain_request_ids(&key), vec![10, 11]);
        assert!(coordinator.complete_if_empty(&key));

        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert!(matches!(
                coordinator.register(12, key.clone()),
                RefreshRegistration::Start
            ));
            let _guard = super::RefreshGuard::new(&coordinator, key.clone());
            panic!("simulated refresh panic");
        }));
        assert!(panic_result.is_err());
        assert!(matches!(
            coordinator.register(13, key),
            RefreshRegistration::Start
        ));
    }

    #[test]
    fn locator_graph_pins_refresh_state_contracts() {
        let environment = ret_core::os_environment::EnvironmentApi::new();
        let conda = Arc::new(ret_conda::Conda::from(&environment));
        let locators = crate::locators::create_locators_with_conda(&environment, conda);
        let state = |kind| {
            locators
                .iter()
                .find(|locator| locator.get_kind() == kind)
                .map(|locator| locator.refresh_state())
        };

        assert_eq!(
            state(LocatorKind::Conda),
            Some(RefreshStatePersistence::SyncedDiscoveryState)
        );
        assert_eq!(
            state(LocatorKind::Rig),
            Some(RefreshStatePersistence::ConfiguredOnly)
        );
        assert_eq!(
            state(LocatorKind::Pixi),
            Some(RefreshStatePersistence::Stateless)
        );
        if cfg!(target_os = "linux") {
            assert_eq!(
                state(LocatorKind::LinuxGlobal),
                Some(RefreshStatePersistence::SelfHydratingCache)
            );
            assert_eq!(
                state(LocatorKind::RVersions),
                Some(RefreshStatePersistence::Stateless)
            );
        }
    }
}
