// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_fs::path::{norm_case, resolve_symlink};
use std::path::{Path, PathBuf};

pub fn is_conda_install(path: &Path) -> bool {
    if (path.join("condabin").exists() || path.join("envs").exists())
        && path.join("conda-meta").exists()
    {
        if let Some(parent) = path.parent() {
            if let Some(parent) = parent.parent() {
                if (parent.join("condabin").exists() || parent.join("envs").exists())
                    && parent.join("conda-meta").exists()
                {
                    return false;
                }
            }
        }
        return true;
    }

    false
}

pub fn is_conda_env(path: &Path) -> bool {
    path.join("conda-meta").is_dir() && !path.join("conda-meta").join("pixi").is_file()
}

pub fn get_conda_installation_used_to_create_conda_env(env_path: &Path) -> Option<PathBuf> {
    if let Some(parent) = env_path.ancestors().nth(2) {
        if is_conda_install(parent) {
            return Some(parent.to_path_buf());
        }
    }

    if let Some(line) = get_conda_creation_line_from_history(env_path) {
        if let Some(conda_dir) = get_conda_dir_from_cmd(line) {
            if is_conda_install(&conda_dir) {
                return Some(conda_dir);
            }
            if let Some(parent) = conda_dir.parent() {
                if is_conda_install(parent) {
                    return Some(parent.to_path_buf());
                }
            }
        }
    }

    if is_conda_install(env_path) {
        Some(env_path.to_path_buf())
    } else {
        None
    }
}

pub fn get_conda_creation_line_from_history(env_path: &Path) -> Option<String> {
    let conda_meta_history = env_path.join("conda-meta").join("history");
    let reader = std::fs::read_to_string(conda_meta_history).ok()?;
    reader.lines().map(str::trim).find_map(|line| {
        if line.to_ascii_lowercase().starts_with("# cmd:")
            && line.to_ascii_lowercase().contains(" create -")
        {
            Some(line.to_string())
        } else {
            None
        }
    })
}

pub fn get_conda_env_name(prefix: &Path, conda_dir: &Option<PathBuf>) -> Option<String> {
    let mut name = if is_conda_install(prefix) {
        Some("base".to_string())
    } else {
        prefix
            .file_name()
            .map(|name| name.to_str().unwrap_or_default().to_string())
    };

    if let Some(conda_dir) = conda_dir {
        if !prefix.starts_with(conda_dir) {
            name = get_conda_env_name_from_history_file(prefix);
        }
    }

    name
}

fn get_conda_env_name_from_history_file(env_path: &Path) -> Option<String> {
    let name = env_path
        .file_name()
        .map(|name| name.to_str().unwrap_or_default().to_string())?;

    if let Some(line) = get_conda_creation_line_from_history(env_path) {
        if is_conda_env_name_in_cmd(line, &name) {
            return Some(name);
        }
    }

    None
}

fn is_conda_env_name_in_cmd(cmd_line: String, name: &str) -> bool {
    cmd_line.contains(format!("-n {name}").as_str())
        || cmd_line.contains(format!("--name {name}").as_str())
}

fn get_conda_dir_from_cmd(cmd_line: String) -> Option<PathBuf> {
    let start_index = cmd_line.to_ascii_lowercase().find("# cmd:")? + "# cmd:".len();
    let end_index = cmd_line.to_ascii_lowercase().find(" create -")?;
    let conda_executable = PathBuf::from(cmd_line[start_index..end_index].trim().to_string());
    let conda_executable = resolve_symlink(&conda_executable).unwrap_or(conda_executable);

    let parent = conda_executable.parent()?;
    let folder = parent.file_name()?.to_string_lossy().to_ascii_lowercase();
    if folder == "bin" || folder == "scripts" || folder == "condabin" {
        return parent.parent().map(norm_case);
    }

    Some(norm_case(parent))
}

#[cfg(test)]
mod tests {
    use super::{get_conda_dir_from_cmd, is_conda_env, is_conda_install};
    use std::path::PathBuf;

    #[test]
    fn parses_conda_dir_from_history_command() {
        let result = get_conda_dir_from_cmd(
            "# cmd: /opt/miniconda3/bin/conda create -n analysis".to_string(),
        );
        assert_eq!(result, Some(PathBuf::from("/opt/miniconda3")));
    }

    #[test]
    fn parses_conda_dir_from_condabin_history() {
        let result = get_conda_dir_from_cmd(
            "# cmd: /home/user/miniconda3/condabin/conda create -n test".to_string(),
        );
        assert_eq!(result, Some(PathBuf::from("/home/user/miniconda3")));
    }

    #[cfg(windows)]
    #[test]
    fn parses_conda_dir_from_scripts_history() {
        let result = get_conda_dir_from_cmd(
            r"# cmd: C:\Users\user\miniconda3\Scripts\conda.exe create -n env1".to_string(),
        );
        assert_eq!(result, Some(PathBuf::from(r"C:\Users\user\miniconda3")));
    }

    #[test]
    fn is_conda_env_requires_conda_meta() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let path = tmp.path();

        // Without conda-meta, not a conda env.
        assert!(!is_conda_env(path));

        // With conda-meta directory, it IS a conda env.
        std::fs::create_dir_all(path.join("conda-meta")).expect("create conda-meta");
        assert!(is_conda_env(path));

        // With pixi marker, it is NOT a conda env (it's a pixi env).
        std::fs::write(path.join("conda-meta").join("pixi"), "").expect("create pixi marker");
        assert!(!is_conda_env(path));
    }

    #[test]
    fn is_conda_install_requires_condabin_and_conda_meta() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let path = tmp.path();

        // Neither condabin nor conda-meta exist.
        assert!(!is_conda_install(path));

        // Only condabin.
        std::fs::create_dir_all(path.join("condabin")).expect("create condabin");
        assert!(!is_conda_install(path));

        // condabin + conda-meta => conda install.
        std::fs::create_dir_all(path.join("conda-meta")).expect("create conda-meta");
        assert!(is_conda_install(path));
    }
}
