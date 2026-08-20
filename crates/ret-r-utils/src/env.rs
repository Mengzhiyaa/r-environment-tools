// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::{error, trace};
use ret_core::{arch::Architecture, env::REnv, r_installation::RInstallation};
use ret_fs::path::norm_case;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    cache::create_cache,
    executable::{filter_symlink_paths, new_silent_command, normalize_executable_paths},
};

const R_INFO_SEPARATOR: &str = "ret-r-installation-info";
const R_INFO_CMD: &str = "cat('ret-r-installation-info\\n');cat(paste(R.version$major, R.version$minor, sep='.'), '\\n', sep='');cat(normalizePath(R.home(), winslash='/', mustWork=FALSE), '\\n', sep='');cat(R.version$arch, '\\n', sep='')";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRInstallation {
    pub executable: PathBuf,
    pub home: PathBuf,
    pub version: String,
    pub arch: Architecture,
    pub known_executables: Option<Vec<PathBuf>>,
    pub symlinks: Option<Vec<PathBuf>>,
}

impl ResolvedRInstallation {
    pub fn to_r_env(&self) -> REnv {
        let mut env = REnv::new(
            self.executable.clone(),
            Some(self.home.clone()),
            Some(self.version.clone()),
        );
        env.known_executables.clone_from(&self.known_executables);
        env.symlinks.clone_from(&self.symlinks);
        env.arch = Some(self.arch.clone());
        env
    }

    pub fn add_to_cache(&self, installation: RInstallation) {
        let known_executables = installation.known_executables.clone().unwrap_or_default();
        if known_executables.contains(&self.executable)
            && installation.version.clone().unwrap_or_default() == self.version
            && installation.home.clone().unwrap_or_default() == self.home
            && installation.arch == Some(self.arch.clone())
        {
            let cache = create_cache(self.executable.clone());
            let entry = cache.lock().expect("cache mutex poisoned");
            entry.track_executables(known_executables)
        } else {
            error!(
                "Invalid R installation being cached: {:?} expected {:?}",
                installation, self
            );
        }
    }

    pub fn from(executable: &Path) -> Option<Self> {
        let cache = create_cache(executable.to_path_buf());
        let entry = cache.lock().expect("cache mutex poisoned");
        if let Some(installation) = entry.get() {
            Some(installation)
        } else if let Some(installation) = get_installation_details(executable) {
            entry.store(installation.clone());
            Some(installation)
        } else {
            None
        }
    }
}

fn get_installation_details(executable: &Path) -> Option<ResolvedRInstallation> {
    let executable_str = executable.to_str()?;
    let start = SystemTime::now();

    let mut command = new_silent_command(executable_str);
    if executable
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with("rscript")
    {
        command.args(["--vanilla", "-e", R_INFO_CMD]);
    } else {
        command.args(["--vanilla", "-s", "-e", R_INFO_CMD]);
    }

    trace!("Executing R runtime: {} {}", executable_str, R_INFO_CMD);

    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            trace!(
                "Executed {:?} in {:?} & produced {:?}",
                executable,
                start.elapsed(),
                stdout
            );

            let (_, payload) = stdout.split_once(R_INFO_SEPARATOR)?;
            let lines = payload
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();

            if lines.len() < 3 {
                error!(
                    "R execution for {:?} produced insufficient output {:?}",
                    executable, stdout
                );
                return None;
            }

            let version = lines[0].to_string();
            let home = norm_case(PathBuf::from(lines[1]));
            let arch_str = lines[2].to_lowercase();
            let arch = if arch_str.contains("aarch64") || arch_str.contains("arm64") {
                Architecture::Arm64
            } else if arch_str.contains("x86_64") || arch_str.contains("64") {
                Architecture::X64
            } else {
                Architecture::X86
            };

            let preferred_candidates = preferred_executables(&home);
            let preferred_executable = preferred_candidates
                .iter()
                .find(|candidate| candidate.exists())
                .cloned()
                .map(norm_case)
                .unwrap_or_else(|| norm_case(executable));

            let mut known_executables = vec![norm_case(executable)];
            if let Ok(canonical) = fs::canonicalize(executable) {
                known_executables.push(norm_case(canonical));
            }

            for candidate in preferred_candidates {
                if candidate.exists() {
                    known_executables.push(norm_case(candidate));
                }
            }

            let known_executables = normalize_executable_paths(known_executables);
            let symlinks = filter_symlink_paths(known_executables.clone());

            Some(ResolvedRInstallation {
                executable: preferred_executable,
                home,
                version,
                arch,
                known_executables: Some(known_executables),
                symlinks: (!symlinks.is_empty()).then_some(symlinks),
            })
        }
        Err(err) => {
            error!("Failed to execute R runtime {:?}: {}", executable, err);
            None
        }
    }
}

fn preferred_executables(home: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![
            home.join("bin").join("x64").join("R.exe"),
            home.join("bin").join("R.exe"),
            home.join("bin").join("x64").join("Rscript.exe"),
            home.join("bin").join("Rscript.exe"),
        ]
    } else {
        vec![home.join("bin").join("R"), home.join("bin").join("Rscript")]
    }
}
