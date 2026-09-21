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

/// Ordered layouts shared by discovery, directory resolution and broken-link reporting.
/// The platform and architecture arguments keep the rules testable on every host.
pub fn executable_candidates_for_platform(
    root: &Path,
    windows: bool,
    architecture: &str,
) -> Vec<PathBuf> {
    let mut directories = if windows {
        vec![
            root.join("Scripts"),
            root.join("Library").join("bin"),
            root.join("Library/lib/R/bin"),
            root.join("Library/lib64/R/bin"),
        ]
    } else {
        Vec::new()
    };
    directories.push(root.join("bin"));
    if windows {
        let arches = match architecture {
            "aarch64" | "arm64" => ["aarch64", "arm64", "x64", "i386"],
            "x86" | "i386" | "i686" => ["i386", "x64", "aarch64", "arm64"],
            _ => ["x64", "aarch64", "arm64", "i386"],
        };
        for arch in arches {
            directories.push(root.join("bin").join(arch));
            // Also accept a bin directory supplied directly by the caller.
            directories.push(root.join(arch));
        }
    }
    directories.extend([
        root.join("lib").join("R").join("bin"),
        root.join("lib64").join("R").join("bin"),
        root.join("Resources").join("bin"),
        root.to_path_buf(),
    ]);
    let names = if windows {
        ["R.exe", "Rscript.exe"]
    } else {
        ["R", "Rscript"]
    };
    directories
        .into_iter()
        .flat_map(|directory| names.map(|name| directory.join(name)))
        .collect()
}

pub fn executable_candidates(root: &Path) -> Vec<PathBuf> {
    executable_candidates_for_platform(root, cfg!(windows), std::env::consts::ARCH)
}

pub fn find_executable(install_path: &Path) -> Option<PathBuf> {
    find_executables(install_path).into_iter().next()
}

pub fn find_executable_or_broken(install_path: &Path) -> ExecutableResult {
    if is_broken_symlink(install_path) {
        return ExecutableResult::Broken(install_path.to_path_buf());
    }
    if let Some(path) = find_executable(install_path) {
        return ExecutableResult::Found(path);
    }
    if let Some(path) = executable_candidates(install_path)
        .into_iter()
        .find(|path| is_broken_symlink(path))
    {
        return ExecutableResult::Broken(path);
    }
    ExecutableResult::NotFound
}

pub fn find_executables<T: AsRef<Path>>(install_path: T) -> Vec<PathBuf> {
    let root = install_path.as_ref();
    if root.is_file() {
        return if is_r_executable_name(root) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let candidates = executable_candidates(root);
    let mut executables = candidates
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect::<Vec<_>>();
    // Retain support for case variants without changing the preferred candidate order.
    let directories = candidates
        .iter()
        .filter_map(|path| path.parent())
        .collect::<std::collections::BTreeSet<_>>();
    for directory in directories {
        if let Ok(entries) = fs::read_dir(directory) {
            let mut extra = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_r_executable_name(path))
                .collect::<Vec<_>>();
            extra.sort();
            for path in extra {
                if !executables.contains(&path) {
                    executables.push(path);
                }
            }
        }
    }
    executables
}

pub fn is_path_within(path: &Path, root: &Path) -> bool {
    // Preserve a direct lexical match before normalizing case. This matters on
    // Windows when `path` does not exist yet (for example, a Chocolatey shim):
    // `norm_case` cannot expand a nonexistent path, while the original paths
    // still provide an unambiguous containment check.
    path.starts_with(root)
        || norm_case(path).starts_with(norm_case(root))
        || match (fs::canonicalize(path), fs::canonicalize(root)) {
            (Ok(path), Ok(root)) => path.starts_with(root),
            _ => false,
        }
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

#[cfg(test)]
mod candidate_tests {
    use super::*;

    #[test]
    fn path_within_accepts_nonexistent_descendant() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp
            .path()
            .join("bin")
            .join(if cfg!(windows) { "R.exe" } else { "R" });

        assert!(is_path_within(&child, temp.path()));
        assert!(!is_path_within(
            &temp.path().with_file_name("sibling").join("R"),
            temp.path()
        ));
    }

    #[test]
    fn directory_helpers_share_unix_layouts() {
        for layout in [
            "bin/R",
            "lib/R/bin/R",
            "lib64/R/bin/R",
            "Resources/bin/R",
            "R",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let executable = temp.path().join(layout);
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, "runtime").unwrap();
            if cfg!(unix) {
                assert_eq!(find_executable(temp.path()), Some(executable.clone()));
                assert!(find_executables(temp.path()).contains(&executable));
                assert!(
                    matches!(find_executable_or_broken(temp.path()), ExecutableResult::Found(path) if path == executable)
                );
            }
            assert!(
                executable_candidates_for_platform(temp.path(), false, "x86_64")
                    .contains(&executable)
            );
        }
    }

    #[test]
    fn windows_layouts_include_every_architecture_and_conda_entrypoint() {
        let root = Path::new("prefix");
        for architecture in ["x86_64", "aarch64", "x86"] {
            let candidates = executable_candidates_for_platform(root, true, architecture);
            for layout in [
                "bin/x64/R.exe",
                "bin/i386/R.exe",
                "bin/aarch64/R.exe",
                "bin/arm64/R.exe",
                "Scripts/R.exe",
                "Library/bin/R.exe",
            ] {
                assert!(candidates.contains(&root.join(layout)), "missing {layout}");
            }
            let preferred = match architecture {
                "aarch64" => "aarch64",
                "x86" => "i386",
                _ => "x64",
            };
            let first = candidates
                .iter()
                .position(|path| path == &root.join("bin").join(preferred).join("R.exe"))
                .unwrap();
            for other in ["x64", "i386", "aarch64"] {
                if other != preferred {
                    assert!(first < candidates.iter().position(|path| path == &root.join("bin").join(other).join("R.exe")).unwrap());
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn internal_layout_broken_links_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("lib64/R/bin/R");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("missing", &executable).unwrap();
        assert!(
            matches!(find_executable_or_broken(temp.path()), ExecutableResult::Broken(path) if path == executable)
        );
    }
}
