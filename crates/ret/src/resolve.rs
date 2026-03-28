// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{path::PathBuf, sync::Arc};

use log::warn;
use ret_core::{
    env::REnv,
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    Locator,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};

#[derive(Debug)]
pub struct ResolvedInstallation {
    pub discovered: RInstallation,
    pub resolved: Option<RInstallation>,
}

pub fn resolve_installation(
    executable: &PathBuf,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
    os_environment: &dyn Environment,
) -> Option<ResolvedInstallation> {
    let mut executable = executable.to_owned();
    if executable.is_dir() {
        executable = find_executable(&executable)?;
    }

    let env = REnv::new(executable.clone(), None, None);
    let global_search_paths: Vec<PathBuf> = os_environment.get_know_global_search_locations();
    let discovered = identify_installation_without_resolution(&env, locators);

    let info = ResolvedRInstallation::from(&executable);
    let discovered = discovered.or_else(|| {
        info.as_ref()
            .map(|_| create_unknown_installation_from_raw(&env, &global_search_paths))
    });

    match (discovered, info) {
        (Some(discovered), Some(info)) => {
            let resolved_env = info.to_r_env();
            let resolved_base = identify_installation_without_resolution(&resolved_env, locators)
                .unwrap_or_else(|| discovered.clone());
            let mut symlinks = resolved_base.symlinks.clone().unwrap_or_default();
            symlinks.push(info.executable.clone());
            symlinks.append(&mut info.symlinks.clone().unwrap_or_default());
            symlinks.sort();
            symlinks.dedup();

            let resolved = RInstallationBuilder::from_installation(resolved_base)
                .executable(Some(info.executable.clone()))
                .home(Some(info.home.clone()))
                .version(Some(info.version.clone()))
                .arch(Some(info.arch.clone()))
                .symlinks(Some(symlinks))
                .build();

            info.add_to_cache(resolved.clone());

            Some(ResolvedInstallation {
                discovered,
                resolved: Some(resolved),
            })
        }
        (Some(discovered), None) => Some(ResolvedInstallation {
            discovered,
            resolved: None,
        }),
        (None, Some(_)) | (None, None) => {
            warn!("Unknown R installation {:?}", executable);
            None
        }
    }
}

fn identify_installation_without_resolution(
    env: &REnv,
    locators: &Arc<Vec<Arc<dyn Locator>>>,
) -> Option<RInstallation> {
    locators.iter().find_map(|locator| locator.try_from(env))
}

fn create_unknown_installation_from_raw(
    env: &REnv,
    global_search_paths: &[PathBuf],
) -> RInstallation {
    RInstallationBuilder::new(infer_fallback_kind(&env.executable, global_search_paths))
        .executable(Some(env.executable.clone()))
        .home(infer_home_from_executable(&env.executable))
        .symlinks(env.symlinks.clone())
        .build()
}

fn infer_fallback_kind(
    executable: &std::path::Path,
    global_search_paths: &[PathBuf],
) -> Option<RInstallationKind> {
    let Some(bin) = executable.parent() else {
        return None;
    };
    if global_search_paths.contains(&bin.to_path_buf()) {
        Some(RInstallationKind::GlobalPaths)
    } else {
        None
    }
}

fn infer_home_from_executable(executable: &std::path::Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let parent_name = parent.file_name()?.to_string_lossy().to_ascii_lowercase();

    if parent_name == "bin" {
        return parent.parent().map(|path| path.to_path_buf());
    }
    if parent_name == "x64" {
        let bin = parent.parent()?;
        if bin
            .file_name()?
            .to_string_lossy()
            .eq_ignore_ascii_case("bin")
        {
            return bin.parent().map(|path| path.to_path_buf());
        }
    }

    None
}
