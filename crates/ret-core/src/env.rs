// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::PathBuf;

use crate::arch::Architecture;
use ret_fs::path::norm_case;

#[derive(Debug, Clone)]
pub struct REnv {
    /// Executable of the R installation.
    pub executable: PathBuf,
    /// Installation home, usually the directory returned by `R.home()`.
    pub home: Option<PathBuf>,
    /// Version of the R installation.
    pub version: Option<String>,
    /// Known symlinks or alternative executables (for example `Rscript`).
    pub symlinks: Option<Vec<PathBuf>>,
    /// Architecture of the R installation (e.g. from `R.version$arch`).
    pub arch: Option<Architecture>,
}

impl REnv {
    pub fn new(executable: PathBuf, home: Option<PathBuf>, version: Option<String>) -> Self {
        let executable = norm_case(executable);
        let home = home.map(norm_case).or_else(|| infer_home(&executable));

        Self {
            executable,
            home,
            version,
            symlinks: None,
            arch: None,
        }
    }
}

fn infer_home(executable: &PathBuf) -> Option<PathBuf> {
    let parent = executable.parent()?;

    // Windows multi-arch layout: <R_HOME>/bin/x64/R.exe
    if parent.ends_with("x64") || parent.ends_with("i386") {
        let bin = parent.parent()?;
        if bin.ends_with("bin") {
            return Some(norm_case(bin.parent()?.to_path_buf()));
        }
    }

    // Unix/macOS layout: <R_HOME>/bin/R
    if parent.ends_with("bin") {
        return Some(norm_case(parent.parent()?.to_path_buf()));
    }

    None
}
