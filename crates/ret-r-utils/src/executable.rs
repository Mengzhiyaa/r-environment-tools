// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use lazy_static::lazy_static;
use log::trace;
use regex::Regex;
use ret_fs::path::norm_case;
use std::ffi::OsStr;
use std::{
    fs,
    path::{Path, PathBuf},
};

lazy_static! {
    static ref WINDOWS_EXE: Regex =
        Regex::new(r"^(r|rscript)(\.exe)?$").expect("error parsing Windows executable regex");
    static ref UNIX_EXE: Regex =
        Regex::new(r"^(r|rscript)$").expect("error parsing Unix executable regex");
}

pub fn is_broken_symlink(path: &Path) -> bool {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return !path.exists();
        }
    }
    false
}

#[derive(Debug, Clone)]
pub enum ExecutableResult {
    Found(PathBuf),
    Broken(PathBuf),
    NotFound,
}

#[cfg(windows)]
pub fn find_executable(install_path: &Path) -> Option<PathBuf> {
    [
        install_path.join("Scripts").join("R.exe"),
        install_path.join("Scripts").join("Rscript.exe"),
        install_path.join("Library").join("bin").join("R.exe"),
        install_path.join("Library").join("bin").join("Rscript.exe"),
        install_path.join("bin").join("x64").join("R.exe"),
        install_path.join("bin").join("R.exe"),
        install_path.join("bin").join("Rscript.exe"),
        install_path.join("R.exe"),
        install_path.join("Rscript.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(unix)]
pub fn find_executable(install_path: &Path) -> Option<PathBuf> {
    [
        install_path.join("bin").join("R"),
        install_path.join("bin").join("Rscript"),
        install_path.join("R"),
        install_path.join("Rscript"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(windows)]
pub fn find_executable_or_broken(install_path: &Path) -> ExecutableResult {
    let candidates = [
        install_path.join("Scripts").join("R.exe"),
        install_path.join("Scripts").join("Rscript.exe"),
        install_path.join("Library").join("bin").join("R.exe"),
        install_path.join("Library").join("bin").join("Rscript.exe"),
        install_path.join("bin").join("x64").join("R.exe"),
        install_path.join("bin").join("R.exe"),
        install_path.join("bin").join("Rscript.exe"),
        install_path.join("R.exe"),
        install_path.join("Rscript.exe"),
    ];

    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return ExecutableResult::Found(path.clone());
    }
    if let Some(path) = candidates.iter().find(|path| is_broken_symlink(path)) {
        return ExecutableResult::Broken(path.clone());
    }
    ExecutableResult::NotFound
}

#[cfg(unix)]
pub fn find_executable_or_broken(install_path: &Path) -> ExecutableResult {
    let candidates = [
        install_path.join("bin").join("R"),
        install_path.join("bin").join("Rscript"),
        install_path.join("R"),
        install_path.join("Rscript"),
    ];

    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return ExecutableResult::Found(path.clone());
    }
    if let Some(path) = candidates.iter().find(|path| is_broken_symlink(path)) {
        return ExecutableResult::Broken(path.clone());
    }
    ExecutableResult::NotFound
}

pub fn find_executables<T: AsRef<Path>>(install_path: T) -> Vec<PathBuf> {
    let install_path = install_path.as_ref();
    let mut directories = vec![install_path.to_path_buf()];

    let bin = install_path.join("bin");
    if bin.is_dir() {
        directories.push(bin.clone());
        if cfg!(windows) {
            directories.push(bin.join("x64"));
            directories.push(bin.join("i386"));
        }
    }

    let mut executables = vec![];
    for directory in directories {
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries.filter_map(Result::ok) {
                let file = entry.path();
                if file.is_file() && is_r_executable_name(&file) {
                    executables.push(file);
                }
            }
        }
    }

    executables.sort();
    executables.dedup();
    executables
}

pub fn normalize_executable_paths<I>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut paths = paths.into_iter().map(norm_case).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

pub fn filter_symlink_paths<I>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut symlinks = paths
        .into_iter()
        .map(norm_case)
        .filter(|path| {
            path.symlink_metadata()
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    symlinks.sort();
    symlinks.dedup();
    symlinks
}

pub fn is_r_executable_name(exe: &Path) -> bool {
    let name = exe
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default()
        .to_lowercase();
    if cfg!(windows) {
        WINDOWS_EXE.is_match(&name)
    } else {
        UNIX_EXE.is_match(&name)
    }
}

pub fn should_search_for_installations_in_path<P: AsRef<Path>>(path: &P) -> bool {
    let folders_to_ignore = [
        "node_modules",
        ".cargo",
        ".devcontainer",
        ".github",
        ".git",
        ".cache",
        ".vscode",
        ".Rproj.user",
        "renv",
        "library",
    ];
    for folder in folders_to_ignore.iter() {
        if path.as_ref().ends_with(folder) {
            trace!("Ignoring folder: {:?}", path.as_ref());
            return false;
        }
    }

    true
}

#[cfg(target_os = "windows")]
pub fn new_silent_command(program: impl AsRef<OsStr>) -> std::process::Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(target_os = "windows"))]
pub fn new_silent_command(program: impl AsRef<OsStr>) -> std::process::Command {
    std::process::Command::new(program)
}
