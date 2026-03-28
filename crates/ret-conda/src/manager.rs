// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{
    conda_info::CondaInfo,
    utils::{get_conda_installation_used_to_create_conda_env, is_conda_env, is_conda_install},
};
use ret_core::manager::{EnvManager, EnvManagerType};
use std::{
    env,
    path::{Path, PathBuf},
};

fn get_conda_executable(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let relative_paths = vec![
        PathBuf::from("Scripts").join("conda.exe"),
        PathBuf::from("Scripts").join("conda.bat"),
        PathBuf::from("bin").join("conda.exe"),
        PathBuf::from("bin").join("conda.bat"),
        PathBuf::from("condabin").join("conda.bat"),
        PathBuf::from("condabin").join("conda.exe"),
    ];
    #[cfg(unix)]
    let relative_paths = vec![
        PathBuf::from("bin").join("conda"),
        PathBuf::from("condabin").join("conda"),
    ];

    for relative_path in relative_paths {
        let executable = path.join(relative_path);
        if executable.exists() {
            return Some(executable);
        }
    }

    None
}

fn get_mamba_executable(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let relative_paths = vec![
        PathBuf::from("Scripts").join("mamba.exe"),
        PathBuf::from("Scripts").join("mamba.bat"),
        PathBuf::from("Scripts").join("micromamba.exe"),
        PathBuf::from("Scripts").join("micromamba.bat"),
        PathBuf::from("bin").join("mamba.exe"),
        PathBuf::from("bin").join("mamba.bat"),
        PathBuf::from("bin").join("micromamba.exe"),
        PathBuf::from("bin").join("micromamba.bat"),
        PathBuf::from("condabin").join("mamba.bat"),
        PathBuf::from("condabin").join("mamba.exe"),
        PathBuf::from("condabin").join("micromamba.bat"),
        PathBuf::from("condabin").join("micromamba.exe"),
    ];
    #[cfg(unix)]
    let relative_paths = vec![
        PathBuf::from("bin").join("mamba"),
        PathBuf::from("bin").join("micromamba"),
        PathBuf::from("condabin").join("mamba"),
        PathBuf::from("condabin").join("micromamba"),
    ];

    for relative_path in relative_paths {
        let executable = path.join(relative_path);
        if executable.exists() {
            return Some(executable);
        }
    }

    None
}

#[cfg(windows)]
fn get_conda_bin_names() -> Vec<&'static str> {
    vec!["conda.exe", "conda.bat"]
}

#[cfg(unix)]
fn get_conda_bin_names() -> Vec<&'static str> {
    vec!["conda"]
}

#[cfg(windows)]
fn get_mamba_bin_names() -> Vec<&'static str> {
    vec!["mamba.exe", "mamba.bat", "micromamba.exe", "micromamba.bat"]
}

#[cfg(unix)]
fn get_mamba_bin_names() -> Vec<&'static str> {
    vec!["mamba", "micromamba"]
}

pub fn find_conda_binary() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for bin in get_conda_bin_names() {
            let executable = directory.join(bin);
            if executable.is_file() || executable.is_symlink() {
                return Some(executable);
            }
        }
    }
    None
}

pub fn find_mamba_binary() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for bin in get_mamba_bin_names() {
            let executable = directory.join(bin);
            if executable.is_file() || executable.is_symlink() {
                return Some(executable);
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct CondaManager {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub conda_dir: Option<PathBuf>,
    pub manager_type: EnvManagerType,
}

impl CondaManager {
    pub fn to_manager(&self) -> EnvManager {
        EnvManager {
            tool: self.manager_type,
            executable: self.executable.clone(),
            version: self.version.clone(),
        }
    }

    pub fn from(path: &Path) -> Option<CondaManager> {
        if !is_conda_env(path) {
            return None;
        }

        let conda_dir = get_conda_installation_used_to_create_conda_env(path).or_else(|| {
            if is_conda_install(path) {
                Some(path.to_path_buf())
            } else {
                None
            }
        })?;

        get_conda_manager(&conda_dir).or_else(|| get_mamba_manager(&conda_dir))
    }

    pub fn from_info(
        executable: &Path,
        info: &CondaInfo,
        manager_type: EnvManagerType,
    ) -> Option<CondaManager> {
        Some(CondaManager {
            executable: executable.to_path_buf(),
            version: if info.conda_version.is_empty() {
                None
            } else {
                Some(info.conda_version.clone())
            },
            conda_dir: info
                .root_prefix
                .clone()
                .or_else(|| info.conda_prefix.clone()),
            manager_type,
        })
    }
}

pub fn is_mamba_executable(executable: &Path) -> bool {
    if let Some(name) = executable.file_name().and_then(|name| name.to_str()) {
        let name = name.to_ascii_lowercase();
        name.starts_with("mamba") || name.starts_with("micromamba")
    } else {
        false
    }
}

pub fn get_conda_manager(path: &Path) -> Option<CondaManager> {
    let executable = get_conda_executable(path)?;
    Some(CondaManager {
        executable,
        version: None,
        conda_dir: Some(path.to_path_buf()),
        manager_type: EnvManagerType::Conda,
    })
}

pub fn get_mamba_manager(path: &Path) -> Option<CondaManager> {
    let executable = get_mamba_executable(path)?;
    Some(CondaManager {
        executable,
        version: None,
        conda_dir: Some(path.to_path_buf()),
        manager_type: EnvManagerType::Mamba,
    })
}

#[cfg(test)]
mod tests {
    use super::is_mamba_executable;
    use std::path::Path;

    #[test]
    fn detects_mamba_and_micromamba_binaries() {
        assert!(is_mamba_executable(Path::new("/tmp/mamba")));
        assert!(is_mamba_executable(Path::new("/tmp/micromamba")));
        assert!(!is_mamba_executable(Path::new("/tmp/conda")));
    }
}
