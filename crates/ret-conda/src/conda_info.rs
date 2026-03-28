// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::{error, trace, warn};
use ret_fs::path::resolve_symlink;
use ret_r_utils::executable::new_silent_command;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CondaInfo {
    pub executable: PathBuf,
    pub envs: Vec<PathBuf>,
    pub conda_prefix: Option<PathBuf>,
    pub conda_version: String,
    pub root_prefix: Option<PathBuf>,
}

#[derive(Debug, serde::Deserialize)]
struct CondaInfoJson {
    pub envs: Option<Vec<PathBuf>>,
    pub conda_prefix: Option<PathBuf>,
    pub conda_version: Option<String>,
    pub root_prefix: Option<PathBuf>,
}

impl CondaInfo {
    pub fn from(executable: Option<PathBuf>) -> Option<CondaInfo> {
        let executable = if cfg!(windows) {
            executable.clone().unwrap_or("conda".into())
        } else {
            let executable = executable.unwrap_or("conda".into());
            resolve_symlink(&executable).unwrap_or(executable)
        };

        let result = new_silent_command(&executable)
            .arg("info")
            .arg("--json")
            .output();
        trace!("Executing Conda manager: {:?} info --json", executable);

        match result {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    match serde_json::from_str::<CondaInfoJson>(stdout.trim()) {
                        Ok(info) => Some(CondaInfo {
                            executable,
                            envs: info.envs.unwrap_or_default(),
                            conda_prefix: info.conda_prefix,
                            conda_version: info.conda_version.unwrap_or_default(),
                            root_prefix: info.root_prefix,
                        }),
                        Err(err) => {
                            error!(
                                "Conda execution for {:?} produced unparsable JSON: {:?}",
                                executable, err
                            );
                            None
                        }
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    if executable.to_string_lossy() != "conda" {
                        warn!(
                            "Failed to get conda info using {:?} ({:?}) {}",
                            executable,
                            output.status.code().unwrap_or_default(),
                            stderr
                        );
                    }
                    None
                }
            }
            Err(err) => {
                if executable.to_string_lossy() != "conda" {
                    warn!(
                        "Failed to execute conda info using {:?}: {}",
                        executable, err
                    );
                }
                None
            }
        }
    }

    pub fn all_envs(&self) -> Vec<PathBuf> {
        let mut envs = self.envs.clone();
        if let Some(root_prefix) = &self.root_prefix {
            envs.push(root_prefix.clone());
        }
        if let Some(conda_prefix) = &self.conda_prefix {
            envs.push(conda_prefix.clone());
        }
        envs.sort();
        envs.dedup();
        envs
    }
}
