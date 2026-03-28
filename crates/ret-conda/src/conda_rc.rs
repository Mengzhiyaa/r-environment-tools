// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::trace;
use ret_core::os_environment::Environment;
use ret_fs::path::expand_path;
use std::{
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::YamlLoader;

/// Extracted environment directories from `.condarc` files.
#[derive(Debug, Default)]
pub struct CondarcEnvDirs {
    pub env_dirs: Vec<PathBuf>,
}

/// Parse all known `.condarc` search paths and extract `envs_dirs` / `envs_path`.
pub fn get_conda_rc_env_dirs(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut all_dirs = vec![];

    for rc_path in get_conda_rc_search_paths(environment) {
        if rc_path.is_file() {
            if let Some(parsed) = parse_conda_rc_file(&rc_path) {
                all_dirs.extend(parsed.env_dirs);
            }
        } else if rc_path.is_dir() {
            // .condarc.d directory — scan for yaml/yml files
            if let Ok(entries) = fs::read_dir(&rc_path) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_file() {
                        let ext = path
                            .extension()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase();
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase();
                        if ext == "yaml" || ext == "yml" || name.contains("condarc") {
                            if let Some(parsed) = parse_conda_rc_file(&path) {
                                all_dirs.extend(parsed.env_dirs);
                            }
                        }
                    }
                }
            }
        }
    }

    all_dirs.sort();
    all_dirs.dedup();
    all_dirs
}

pub(crate) fn get_conda_rc_search_paths(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut paths = vec![];

    // System-level paths
    if cfg!(windows) {
        if let Some(program_data) = environment.get_env_var("ProgramData".to_string()) {
            let pd = PathBuf::from(&program_data);
            for name in ["conda", "miniconda", "miniconda3"] {
                paths.push(pd.join(name).join(".condarc"));
                paths.push(pd.join(name).join("condarc"));
            }
        }
    } else {
        for prefix in ["/etc/conda", "/var/lib/conda"] {
            paths.push(PathBuf::from(prefix).join(".condarc"));
            paths.push(PathBuf::from(prefix).join("condarc"));
        }
    }

    // CONDA_ROOT / CONDA_PREFIX
    for var in ["CONDA_ROOT", "CONDA_PREFIX"] {
        if let Some(val) = environment.get_env_var(var.to_string()) {
            let dir = expand_path(PathBuf::from(val));
            paths.push(dir.join(".condarc"));
            paths.push(dir.join("condarc"));
        }
    }

    // User home
    if let Some(home) = environment.get_user_home() {
        paths.push(home.join(".condarc"));
        paths.push(home.join(".conda").join(".condarc"));
        paths.push(home.join(".conda").join("condarc"));
        paths.push(home.join(".config").join("conda").join(".condarc"));
        paths.push(home.join(".config").join("conda").join("condarc"));
        paths.push(home.join(".config").join("conda").join("condarc.d"));
        paths.push(home.join(".mambarc"));
    }

    // CONDARC env var
    if let Some(condarc) = environment.get_env_var("CONDARC".to_string()) {
        paths.push(expand_path(PathBuf::from(condarc)));
    }

    paths.sort();
    paths.dedup();
    paths
}

fn parse_conda_rc_file(path: &Path) -> Option<CondarcEnvDirs> {
    let contents = fs::read_to_string(path).ok()?;
    let result = parse_conda_rc_contents(&contents);
    if let Some(ref parsed) = result {
        if !parsed.env_dirs.is_empty() {
            trace!(
                "Parsed .condarc {:?}, env_dirs: {:?}",
                path,
                parsed.env_dirs
            );
        }
    }
    result
}

pub fn parse_conda_rc_contents(contents: &str) -> Option<CondarcEnvDirs> {
    let docs = YamlLoader::load_from_str(contents).ok()?;
    if docs.is_empty() {
        return Some(CondarcEnvDirs::default());
    }

    let doc = &docs[0];
    let mut env_dirs = vec![];

    // https://docs.conda.io/projects/conda/en/latest/user-guide/configuration/use-condarc.html
    for key in ["envs_dirs", "envs_path"] {
        if let Some(items) = doc[key].as_vec() {
            for item in items {
                if let Some(s) = item.as_str() {
                    let s = s.trim();
                    if !s.is_empty() {
                        env_dirs.push(expand_path(PathBuf::from(s)));
                    }
                }
            }
        }
    }

    Some(CondarcEnvDirs { env_dirs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_envs_dirs() {
        let contents = r#"
channels:
  - conda-forge
envs_dirs:
  - /opt/conda/envs
  - ~/my-envs
"#;
        let result = parse_conda_rc_contents(contents).unwrap();
        assert_eq!(result.env_dirs.len(), 2);
        assert_eq!(result.env_dirs[0], PathBuf::from("/opt/conda/envs"));
    }

    #[test]
    fn parse_envs_path() {
        let contents = r#"
envs_path:
  - /custom/path/envs
"#;
        let result = parse_conda_rc_contents(contents).unwrap();
        assert_eq!(result.env_dirs, vec![PathBuf::from("/custom/path/envs")]);
    }

    #[test]
    fn parse_empty_rc() {
        let contents = r#"
channels:
  - defaults
"#;
        let result = parse_conda_rc_contents(contents).unwrap();
        assert!(result.env_dirs.is_empty());
    }

    #[test]
    fn parse_both_keys() {
        let contents = r#"
envs_dirs:
  - /dir1
envs_path:
  - /dir2
"#;
        let result = parse_conda_rc_contents(contents).unwrap();
        assert_eq!(result.env_dirs.len(), 2);
        assert!(result.env_dirs.contains(&PathBuf::from("/dir1")));
        assert!(result.env_dirs.contains(&PathBuf::from("/dir2")));
    }
}
