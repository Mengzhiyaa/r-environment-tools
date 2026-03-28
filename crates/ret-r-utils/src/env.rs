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

use crate::{cache::create_cache, executable::new_silent_command};

const R_INFO_SEPARATOR: &str = "ret-r-installation-info";
const R_INFO_CMD: &str = "cat('ret-r-installation-info\\n');cat(paste(R.version$major, R.version$minor, sep='.'), '\\n', sep='');cat(normalizePath(R.home(), winslash='/', mustWork=FALSE), '\\n', sep='');cat(R.version$arch, '\\n', sep='')";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRInstallation {
    pub executable: PathBuf,
    pub home: PathBuf,
    pub version: String,
    pub arch: Architecture,
    pub symlinks: Option<Vec<PathBuf>>,
}

impl ResolvedRInstallation {
    pub fn to_r_env(&self) -> REnv {
        let mut env = REnv::new(
            self.executable.clone(),
            Some(self.home.clone()),
            Some(self.version.clone()),
        );
        env.symlinks.clone_from(&self.symlinks);
        env.arch = Some(self.arch.clone());
        env
    }

    pub fn add_to_cache(&self, installation: RInstallation) {
        let symlinks = installation.symlinks.clone().unwrap_or_default();
        if symlinks.contains(&self.executable)
            && installation.version.clone().unwrap_or_default() == self.version
            && installation.home.clone().unwrap_or_default() == self.home
            && installation.arch == Some(self.arch.clone())
        {
            let cache = create_cache(self.executable.clone());
            let entry = cache.lock().expect("cache mutex poisoned");
            entry.track_symlinks(symlinks)
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

            let mut symlinks = vec![norm_case(executable.to_path_buf())];
            if let Ok(canonical) = fs::canonicalize(executable) {
                symlinks.push(norm_case(canonical));
            }

            for candidate in preferred_executables(&home) {
                if candidate.exists() {
                    symlinks.push(norm_case(candidate));
                }
            }

            symlinks.sort();
            symlinks.dedup();

            let preferred_executable = preferred_executables(&home)
                .into_iter()
                .find(|candidate| candidate.exists())
                .unwrap_or_else(|| norm_case(executable.to_path_buf()));

            Some(ResolvedRInstallation {
                executable: preferred_executable,
                home,
                version,
                arch,
                symlinks: Some(symlinks),
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
