// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{
    conda_rc::get_conda_rc_env_dirs,
    manager::{find_conda_binary, find_mamba_binary},
    utils::{is_conda_env, is_conda_install},
};
use log::trace;
use ret_core::os_environment::Environment;
use ret_fs::path::{expand_path, norm_case, resolve_symlink};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::SystemTime,
};

/// Discover all conda environment paths purely from the filesystem.
///
/// This replaces `conda info --json` on the critical path by reading from:
/// 1. `~/.conda/environments.txt`
/// 2. `.condarc` `envs_dirs` / `envs_path`
/// 3. Known conda install locations (platform-specific)
/// 4. conda/mamba executables on `PATH`
/// 5. User-configured conda executable (derived install dir)
///
/// Each discovered root is expanded via [`get_environments`] to include the
/// base env itself and all `envs/*` children.
pub fn get_conda_environment_paths(
    environment: &dyn Environment,
    conda_executable: &Option<PathBuf>,
) -> Vec<PathBuf> {
    let start = SystemTime::now();

    let mut env_paths = thread::scope(|s| {
        let mut all = vec![];
        for handle in [
            s.spawn(|| get_conda_envs_from_environment_txt(environment)),
            s.spawn(|| get_conda_rc_env_dirs(environment)),
            s.spawn(|| get_known_conda_install_locations(environment)),
            s.spawn(get_conda_dirs_from_path_executables),
            s.spawn(|| get_conda_dir_from_executable(conda_executable)),
        ] {
            if let Ok(mut paths) = handle.join() {
                all.append(&mut paths);
            }
        }
        all
    });

    env_paths = env_paths.iter().map(norm_case).collect();
    env_paths.sort();
    env_paths.dedup();

    // Expand each discovered path into individual environments
    let mut result: Vec<PathBuf> = env_paths
        .iter()
        .filter(|p| p.exists())
        .flat_map(|path| get_environments(path))
        .collect();

    result.sort();
    result.dedup();

    trace!(
        "Conda filesystem discovery found {} environments in {:?}",
        result.len(),
        start.elapsed().unwrap_or_default()
    );
    result
}

/// Read `~/.conda/environments.txt` for known conda environment paths.
pub(crate) fn get_conda_envs_from_environment_txt(environment: &dyn Environment) -> Vec<PathBuf> {
    let home = environment.get_user_home();
    let Some(home) = home else {
        return vec![];
    };

    let environment_txt = home.join(".conda").join("environments.txt");
    let Ok(contents) = fs::read_to_string(&environment_txt) else {
        return vec![];
    };

    trace!("Found environments.txt at {:?}", environment_txt);
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| norm_case(PathBuf::from(line)))
        .filter(|p| p.exists())
        .collect()
}

/// Get known conda install locations based on platform-specific well-known paths
/// and environment variables.
#[cfg(unix)]
fn get_known_conda_install_locations(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut known_paths = vec![
        PathBuf::from("/opt/conda"),
        PathBuf::from("/anaconda"),
        PathBuf::from("/anaconda3"),
        PathBuf::from("/miniconda"),
        PathBuf::from("/miniconda3"),
        PathBuf::from("/miniforge3"),
        PathBuf::from("/micromamba"),
    ];

    if let Some(home) = environment.get_user_home() {
        let prefixes = vec![
            home.clone(),
            home.join("opt"),
            home.join(".conda"),
            home.join(".local"),
            PathBuf::from("/opt"),
            PathBuf::from("/usr/local"),
            PathBuf::from("/usr/share"),
        ];

        if std::env::consts::OS == "macos" {
            known_paths.push(PathBuf::from("/opt/homebrew/anaconda3"));
            known_paths.push(PathBuf::from("/opt/homebrew/miniconda3"));
            known_paths.push(PathBuf::from("/opt/homebrew/miniforge3"));
        } else {
            known_paths.push(PathBuf::from("/home/linuxbrew/.linuxbrew/miniconda3"));
            known_paths.push(PathBuf::from("/home/linuxbrew/.linuxbrew/miniforge3"));
        }

        for prefix in prefixes {
            for name in [
                "anaconda",
                "anaconda3",
                "miniconda",
                "miniconda3",
                "miniforge3",
                "micromamba",
            ] {
                known_paths.push(prefix.join(name));
            }
        }

        // ~/.conda itself may contain envs/ subdirectory
        known_paths.push(home.join(".conda"));
    }

    // Environment variables
    for var in [
        "CONDA_ROOT",
        "CONDA_PREFIX",
        "MAMBA_ROOT_PREFIX",
        "CONDA_DIR",
    ] {
        if let Some(val) = environment.get_env_var(var.to_string()) {
            known_paths.push(expand_path(PathBuf::from(val)));
        }
    }
    // Derive from CONDA_EXE
    if let Some(conda_exe) = environment.get_env_var("CONDA_EXE".to_string()) {
        if let Some(dir) = get_conda_dir_from_exe_path(&PathBuf::from(conda_exe)) {
            known_paths.push(dir);
        }
    }

    known_paths.sort();
    known_paths.dedup();
    known_paths.into_iter().filter(|p| p.exists()).collect()
}

#[cfg(windows)]
fn get_known_conda_install_locations(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut known_paths = vec![];

    // Standard Windows locations
    let env_vars = [
        environment.get_env_var("USERPROFILE".to_string()),
        environment.get_env_var("ProgramData".to_string()),
        environment.get_env_var("ALLUSERSPROFILE".to_string()),
    ];

    for env_val in env_vars.into_iter().flatten() {
        let base = PathBuf::from(env_val);
        for name in [
            "anaconda3",
            "miniconda3",
            "miniforge3",
            "micromamba",
            "Anaconda3",
            "Miniconda3",
            "Miniforge3",
        ] {
            known_paths.push(base.join(name));
        }
    }

    if let Some(home_drive) = environment.get_env_var("HOMEDRIVE".to_string()) {
        let mut drive = home_drive.clone();
        if drive.ends_with(':') {
            drive = format!("{}\\", drive);
        }
        for name in [
            "anaconda3",
            "miniconda",
            "miniconda3",
            "miniforge3",
            "micromamba",
        ] {
            known_paths.push(PathBuf::from(&drive).join(name));
        }
    }

    if let Some(home) = environment.get_user_home() {
        for name in [
            "anaconda",
            "anaconda3",
            "miniconda",
            "miniconda3",
            "miniforge3",
            "micromamba",
        ] {
            known_paths.push(home.join(name));
        }
        known_paths.push(home.join(".conda"));
        known_paths.push(
            home.join("AppData")
                .join("Local")
                .join("conda")
                .join("conda"),
        );
    }

    // Environment variables
    for var in ["CONDA_ROOT", "CONDA_PREFIX"] {
        if let Some(val) = environment.get_env_var(var.to_string()) {
            known_paths.push(expand_path(PathBuf::from(val)));
        }
    }
    if let Some(conda_exe) = environment.get_env_var("CONDA_EXE".to_string()) {
        if let Some(dir) = get_conda_dir_from_exe_path(&PathBuf::from(conda_exe)) {
            known_paths.push(dir);
        }
    }

    known_paths.sort();
    known_paths.dedup();
    known_paths.into_iter().filter(|p| p.exists()).collect()
}

/// Derive conda install directory from the user-configured conda executable.
fn get_conda_dir_from_executable(conda_executable: &Option<PathBuf>) -> Vec<PathBuf> {
    let Some(exe) = conda_executable else {
        return vec![];
    };
    get_conda_dir_from_exe_path(exe).into_iter().collect()
}

/// Derive conda install directories from conda/mamba binaries available on PATH.
fn get_conda_dirs_from_path_executables() -> Vec<PathBuf> {
    let mut dirs = vec![];

    if let Some(conda_bin) = find_conda_binary() {
        if let Some(dir) = get_conda_dir_from_exe_path(&conda_bin) {
            dirs.push(dir);
        }
    }
    if let Some(mamba_bin) = find_mamba_binary() {
        if let Some(dir) = get_conda_dir_from_exe_path(&mamba_bin) {
            dirs.push(dir);
        }
    }

    dirs.sort();
    dirs.dedup();
    dirs
}

/// Given a conda/mamba executable path, derive the conda install directory.
pub(crate) fn get_conda_dir_from_exe_path(exe: &Path) -> Option<PathBuf> {
    let exe = resolve_conda_executable(exe)?;

    // The exe might be a file or just a path
    let parent = if exe.is_file() {
        exe.parent()?
    } else {
        // Treat as directory
        if is_conda_env(&exe) {
            return Some(exe.to_path_buf());
        }
        return exe.parent().and_then(|p| {
            if is_conda_env(p) {
                Some(p.to_path_buf())
            } else {
                None
            }
        });
    };

    // exe is in the root prefix directory
    if is_conda_env(parent) {
        return Some(parent.to_path_buf());
    }
    // exe is in bin/ or Scripts/ or condabin/
    if let Some(grandparent) = parent.parent() {
        if is_conda_env(grandparent) {
            return Some(grandparent.to_path_buf());
        }
    }
    None
}

fn resolve_conda_executable(exe: &Path) -> Option<PathBuf> {
    let resolved = resolve_symlink(&exe).unwrap_or_else(|| exe.to_path_buf());
    if resolved.is_file() {
        return Some(resolved);
    }

    // Bare command names like `conda` or `mamba` need PATH lookup.
    if exe.parent().is_none() {
        let name = exe.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.starts_with("mamba") || name.starts_with("micromamba") {
            let found = find_mamba_binary()?;
            return Some(resolve_symlink(&found).unwrap_or(found));
        }
        if name.starts_with("conda") {
            let found = find_conda_binary()?;
            return Some(resolve_symlink(&found).unwrap_or(found));
        }
    }

    Some(resolved)
}

/// Expand a conda directory into individual environment paths.
///
/// If `conda_dir` is a conda installation root:
///   - Includes the root itself (base env)
///   - Includes all `envs/*` children that are conda envs
///
/// If `conda_dir` is a conda env (not root):
///   - Includes it directly
///   - Checks grandparent for base env discovery
///
/// Otherwise, checks for `envs/` subdirectory.
pub fn get_environments(conda_dir: &Path) -> Vec<PathBuf> {
    let mut envs = vec![];

    if is_conda_install(conda_dir) {
        // Root itself is also an env (base env)
        envs.push(conda_dir.to_path_buf());

        // All envs/ children
        if let Ok(entries) = fs::read_dir(conda_dir.join("envs")) {
            envs.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| is_conda_env(p)),
            );
        }

        // Check .condarc in the install folder for additional env_dirs
        // (not parsing here — already handled by conda_rc module)
    } else if is_conda_env(conda_dir) {
        envs.push(conda_dir.to_path_buf());

        // If this is under <root>/envs/<name>, discover the root too
        if let Some(parent) = conda_dir.parent() {
            if parent.file_name().map(|n| n == "envs").unwrap_or(false) {
                if let Some(grandparent) = parent.parent() {
                    if is_conda_install(grandparent) && !envs.contains(&grandparent.to_path_buf()) {
                        envs.append(&mut get_environments(grandparent));
                    }
                }
            }
        }
    } else if conda_dir.join("envs").exists() {
        // Directory containing environments (e.g., ~/.conda)
        if let Ok(entries) = fs::read_dir(conda_dir.join("envs")) {
            envs.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| is_conda_env(p)),
            );
        }
    } else {
        // The dir could be the `envs` directory itself
        if let Ok(entries) = fs::read_dir(conda_dir) {
            envs.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| is_conda_env(p)),
            );
        }
    }

    envs.sort();
    envs.dedup();
    envs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_environments_includes_base_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Create conda install structure
        fs::create_dir_all(root.join("conda-meta")).unwrap();
        fs::create_dir_all(root.join("condabin")).unwrap();
        fs::create_dir_all(root.join("envs").join("myenv").join("conda-meta")).unwrap();

        let envs = get_environments(root);
        assert!(
            envs.contains(&root.to_path_buf()),
            "base env should be included"
        );
        assert!(
            envs.contains(&root.join("envs").join("myenv")),
            "named env should be included"
        );
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn get_environments_from_child_discovers_base() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Create conda install structure
        fs::create_dir_all(root.join("conda-meta")).unwrap();
        fs::create_dir_all(root.join("condabin")).unwrap();
        fs::create_dir_all(root.join("envs").join("child").join("conda-meta")).unwrap();

        // Discover from the child env
        let envs = get_environments(&root.join("envs").join("child"));
        assert!(
            envs.contains(&root.to_path_buf()),
            "base env should be discovered from child"
        );
        assert!(envs.contains(&root.join("envs").join("child")));
    }

    #[test]
    fn get_environments_non_conda_dir_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = get_environments(tmp.path());
        assert!(envs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn get_conda_dir_from_symlinked_executable_resolves_root() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        // Canonicalize to resolve macOS /var -> /private/var symlink
        let root = tmp.path().canonicalize().unwrap().join("miniforge3");
        let condabin = root.join("condabin");
        std::fs::create_dir_all(root.join("conda-meta")).unwrap();
        std::fs::create_dir_all(&condabin).unwrap();

        let real = condabin.join("conda");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();

        let link_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join("conda");
        symlink(&real, &link).unwrap();

        assert_eq!(get_conda_dir_from_exe_path(&link), Some(root));
    }
}
