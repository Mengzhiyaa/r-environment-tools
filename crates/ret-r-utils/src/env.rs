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
    process::probe_output,
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
    let start = SystemTime::now();
    let mut command = new_silent_command(executable);
    if is_rscript(executable) {
        command.args(["--vanilla", "-e", R_INFO_CMD]);
    } else {
        command.args(["--vanilla", "-s", "-e", R_INFO_CMD]);
    }
    match probe_output(&mut command) {
        Ok(output) if output.status.success() => {
            trace!("Executed {:?} in {:?}", executable, start.elapsed());
            parse_installation_details(executable, &String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => {
            error!("R runtime {:?} exited with {}", executable, output.status);
            None
        }
        Err(error) => {
            error!("Failed to execute R runtime {:?}: {}", executable, error);
            None
        }
    }
}

fn is_rscript(executable: &Path) -> bool {
    executable
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with("rscript")
}

fn parse_installation_details(executable: &Path, stdout: &str) -> Option<ResolvedRInstallation> {
    let (_, payload) = stdout.split_once(R_INFO_SEPARATOR)?;
    let lines = payload
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() < 3 {
        return None;
    }
    let home = norm_case(PathBuf::from(lines[1]));
    let arch_str = lines[2].to_lowercase();
    let arch = if arch_str.contains("aarch64") || arch_str.contains("arm64") {
        Architecture::Arm64
    } else if arch_str.contains("x86_64") || arch_str.contains("amd64") {
        Architecture::X64
    } else if arch_str.contains("i386") || arch_str.contains("i686") || arch_str == "x86" {
        Architecture::X86
    } else {
        error!("Unsupported R architecture: {}", lines[2]);
        return None;
    };
    // A launch wrapper can carry package paths or activation state. Never replace
    // it with R.home()/bin/R, nor list other architectures as equivalent aliases.
    let mut known_executables = vec![norm_case(executable)];
    if let Ok(canonical) = fs::canonicalize(executable) {
        known_executables.push(norm_case(canonical));
    }
    let known_executables = normalize_executable_paths(known_executables);
    let symlinks = filter_symlink_paths(known_executables.clone());
    Some(ResolvedRInstallation {
        executable: norm_case(executable),
        home,
        version: lines[0].to_string(),
        arch,
        known_executables: Some(known_executables),
        symlinks: (!symlinks.is_empty()).then_some(symlinks),
    })
}

/// Probe inside the activation shell, without sharing the ordinary executable cache.
/// Different modules can expose the same executable with different library settings.
#[cfg(unix)]
pub fn resolve_with_startup(
    executable: Option<&Path>,
    startup: &str,
) -> Option<ResolvedRInstallation> {
    use ret_core::shell::quote_shell_argument;
    let launcher = executable
        .map(|path| quote_shell_argument(&path.to_string_lossy()))
        .unwrap_or_else(|| "$(command -v R)".to_string());
    let script = format!(
        "{startup} && _ret_r_exe={launcher} && [ -n \"$_ret_r_exe\" ] && printf 'ret-r-launcher\\n%s\\n' \"$_ret_r_exe\" && case \"$_ret_r_exe\" in *Rscript|*Rscript.exe) \"$_ret_r_exe\" --vanilla -e {info};; *) \"$_ret_r_exe\" --vanilla -s -e {info};; esac",
        info = quote_shell_argument(R_INFO_CMD)
    );
    let output = probe_output(new_silent_command("sh").args(["-c", &script])).ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (_, payload) = stdout.split_once("ret-r-launcher\n")?;
    let (executable, info) = payload.split_once('\n')?;
    parse_installation_details(Path::new(executable.trim_end_matches('\r')), info)
}

#[cfg(not(unix))]
pub fn resolve_with_startup(
    _executable: Option<&Path>,
    _startup: &str,
) -> Option<ResolvedRInstallation> {
    None
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn resolving_x86_metadata_does_not_select_an_existing_x64_binary() {
        let temp = tempfile::tempdir().unwrap();
        let x86 = temp.path().join("bin/i386/R.exe");
        let x64 = temp.path().join("bin/x64/R.exe");
        for path in [&x86, &x64] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "runtime").unwrap();
        }
        let output = format!(
            "ret-r-installation-info\n4.1.3\n{}\ni386\n",
            temp.path().display()
        );
        let resolved = parse_installation_details(&x86, &output).unwrap();
        assert_eq!(resolved.executable, norm_case(&x86));
        assert_eq!(resolved.arch, Architecture::X86);
        assert!(!resolved.known_executables.unwrap().contains(&x64));
    }
}
