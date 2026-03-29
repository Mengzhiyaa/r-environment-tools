// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use find::{find_and_report_installations, SearchScope};
use locators::create_locators;
use resolve::resolve_installation;
use ret_core::{
    os_environment::{Environment, EnvironmentApi},
    r_installation::RInstallationKind,
    Configuration, Locator,
};
use ret_fs::glob::expand_glob_patterns;
use ret_r_utils::cache::set_cache_directory;
use ret_reporter::{self, cache::CacheReporter, collect, stdio};
use serde_json::{json, Value};
use std::{collections::BTreeMap, env, path::PathBuf, sync::Arc, time::SystemTime};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub mod find;
pub mod jsonrpc;
pub mod locators;
pub mod resolve;

pub fn initialize_tracing(verbose: bool) {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let filter = if verbose {
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("ret=debug"))
        } else {
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
        };

        let use_json = env::var("RET_TRACE_FORMAT")
            .map(|v| v == "json")
            .unwrap_or(false);

        if use_json {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(std::io::stderr))
                .init();
        } else {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_timer(fmt::time::uptime())
                        .with_writer(std::io::stderr),
                )
                .init();
        }
    });
}

#[derive(Debug, Clone)]
pub struct FindOptions {
    pub print_list: bool,
    pub print_summary: bool,
    pub verbose: bool,
    pub search_paths: Option<Vec<PathBuf>>,
    pub cache_directory: Option<PathBuf>,
    pub kind: Option<RInstallationKind>,
    pub json: bool,
    pub rig_executable: Option<PathBuf>,
    pub conda_executable: Option<PathBuf>,
}

pub fn find_and_report_installations_stdio(options: FindOptions) {
    initialize_tracing(options.verbose);

    let now = SystemTime::now();
    let config = create_config(&options);
    let search_scope = options.kind.map(SearchScope::Global);

    if let Some(cache_directory) = options.cache_directory.clone() {
        set_cache_directory(cache_directory);
    }

    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    for locator in locators.iter() {
        locator.configure(&config);
    }

    if options.json {
        find_installations_json(&options, &locators, config, &environment, search_scope);
    } else {
        find_installations(&options, &locators, config, &environment, search_scope);
        println!("Completed in {}ms", now.elapsed().unwrap().as_millis());
    }
}

fn create_config(options: &FindOptions) -> Configuration {
    let mut config = Configuration::default();

    let mut search_paths = vec![];
    if let Some(paths) = options.search_paths.as_ref() {
        search_paths.extend(expand_glob_patterns(paths));
    }
    search_paths.sort();
    search_paths.dedup();

    config.search_directories = Some(
        search_paths
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect(),
    );
    config.executables = Some(
        search_paths
            .iter()
            .filter(|p| p.is_file())
            .cloned()
            .collect(),
    );
    config.conda_executable = options.conda_executable.clone();
    config.rig_executable = options.rig_executable.clone();
    config.cache_directory = options.cache_directory.clone();
    config
}

fn find_installations(
    options: &FindOptions,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    config: Configuration,
    environment: &dyn Environment,
    search_scope: Option<SearchScope>,
) {
    let kind = match search_scope {
        Some(SearchScope::Global(kind)) => Some(kind),
        _ => None,
    };
    let stdio_reporter = Arc::new(stdio::create_reporter(options.print_list, kind));
    let reporter = CacheReporter::new(stdio_reporter.clone());

    let summary =
        find_and_report_installations(&reporter, config, locators, environment, search_scope);
    let summary = summary.lock().expect("summary mutex poisoned");

    if options.print_summary && !summary.locators.is_empty() {
        println!();
        println!("Breakdown by each locator:");
        println!("--------------------------");
        for locator in summary.locators.iter() {
            println!("{:<20} : {:?}", format!("{:?}", locator.0), locator.1);
        }
        println!();
    }

    if options.print_summary && !summary.breakdown.is_empty() {
        println!("Breakdown for finding R installations:");
        println!("-------------------------------------");
        for item in summary.breakdown.iter() {
            println!("{:<20} : {:?}", item.0, item.1);
        }
        println!();
    }

    let summary = stdio_reporter.get_summary();
    if options.verbose && !summary.installation_paths.is_empty() {
        println!("Installation Paths:");
        println!("-------------------");
        for (kind, installations) in summary.installation_paths.iter() {
            let kind_str = kind
                .map(|value| format!("{value:?}"))
                .unwrap_or("Unknown".to_string());
            println!("\n{kind_str}:");
            for installation in installations {
                if let Some(executable) = &installation.executable {
                    println!("  - {}", executable.display());
                }
            }
        }
        println!();
    }

    if !summary.managers.is_empty() {
        println!("Managers:");
        println!("---------");
        for (key, value) in summary
            .managers
            .clone()
            .into_iter()
            .map(|(key, value)| (format!("{key:?}"), value))
            .collect::<BTreeMap<String, u16>>()
        {
            println!("{key:<20} : {value:?}");
        }
        println!();
    }

    if !summary.installations.is_empty() {
        let total = summary
            .installations
            .values()
            .fold(0, |acc, value| acc + value);
        println!("Installations ({total}):");
        println!("------------------");
        for (key, value) in summary
            .installations
            .clone()
            .into_iter()
            .map(|(key, value)| {
                (
                    key.map(|v| format!("{v:?}"))
                        .unwrap_or("Unknown".to_string()),
                    value,
                )
            })
            .collect::<BTreeMap<String, u16>>()
        {
            println!("{key:<20} : {value:?}");
        }
        println!();
    }
}

fn find_installations_json(
    options: &FindOptions,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    config: Configuration,
    environment: &dyn Environment,
    search_scope: Option<SearchScope>,
) {
    let reporter = CacheReporter::new(Arc::new(collect::create_reporter()));

    find_and_report_installations(&reporter, config, locators, environment, search_scope);

    let managers = reporter.get_managers();
    let mut installations = reporter.get_installations();

    if let Some(kind) = options.kind {
        installations.retain(|installation| installation.kind == Some(kind));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&build_find_output(managers, installations))
            .expect("failed to serialize R installations as JSON")
    );
}

pub fn resolve_report_stdio(
    executable: PathBuf,
    verbose: bool,
    cache_directory: Option<PathBuf>,
    json: bool,
) {
    initialize_tracing(verbose);

    let now = SystemTime::now();
    if let Some(cache_directory) = cache_directory.clone() {
        set_cache_directory(cache_directory);
    }

    let environment = EnvironmentApi::new();
    let locators = create_locators(&environment);
    let config = Configuration::default();
    for locator in locators.iter() {
        locator.configure(&config);
    }

    if let Some(result) = resolve_installation(&executable, &locators, &environment) {
        let installation = result.resolved.unwrap_or(result.discovered);
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&build_resolve_output(installation))
                    .expect("failed to serialize resolved installation")
            );
        } else {
            println!("{installation}");
            println!("Completed in {}ms", now.elapsed().unwrap().as_millis());
        }
    } else if !json {
        eprintln!(
            "Could not resolve R installation for {}",
            executable.display()
        );
    }
}

pub fn build_find_output(
    managers: Vec<ret_core::manager::EnvManager>,
    installations: Vec<ret_core::r_installation::RInstallation>,
) -> Value {
    json!({
        "managers": managers,
        "installations": installations,
    })
}

pub fn build_resolve_output(installation: ret_core::r_installation::RInstallation) -> Value {
    json!(installation)
}
