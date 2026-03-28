// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use lazy_static::lazy_static;
use log::{trace, warn};
use regex::Regex;
use ret_core::arch::Architecture;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

lazy_static! {
    /// Matches r-base package filenames like `r-base-4.5.3-h1a7235e_0.json`
    static ref R_BASE_VERSION: Regex = Regex::new(r"^r-base-([\d+\.*]*)-.*\.json$")
        .expect("error parsing R-base package version regex");
    /// Matches r-base entries in conda-meta/history like
    /// `+https://.../<subdir>::r-base-4.5.3-h1a7235e_0`
    static ref R_BASE_VERSION_IN_HISTORY: Regex = Regex::new(r".*r-base-([\d+\.]*)-([^\s]+)")
        .expect("error parsing R-base version-in-history regex");
}

/// Metadata about an R installation extracted from conda-meta.
#[derive(Debug, Clone)]
pub struct RCondaPackageInfo {
    pub version: String,
    pub arch: Option<Architecture>,
}

enum HistoryLookup {
    Exact(RCondaPackageInfo),
    VersionOnly(String),
}

/// Package JSON structure from conda-meta/r-base-*.json
#[derive(Debug, Deserialize)]
struct CondaMetaPackage {
    version: Option<String>,
    /// e.g. "conda-forge" or "https://repo.anaconda.com/pkgs/main/linux-64"
    #[allow(dead_code)]
    channel: Option<String>,
    /// e.g. "linux-64", "osx-arm64", "win-64"
    subdir: Option<String>,
    /// e.g. "x86_64", "aarch64"
    arch: Option<String>,
}

impl RCondaPackageInfo {
    /// Try to extract R version and architecture from the conda-meta directory
    /// of a conda environment, without spawning R.
    pub fn from(env_path: &Path) -> Option<Self> {
        if let Some(lookup) = get_r_info_from_history(env_path) {
            return match lookup {
                HistoryLookup::Exact(info) => Some(info),
                HistoryLookup::VersionOnly(version) => {
                    get_r_info_from_package_files(env_path, Some(&version)).or(Some(
                        RCondaPackageInfo {
                            version,
                            arch: None,
                        },
                    ))
                }
            };
        }

        // Slow fallback: enumerate conda-meta/r-base-*.json files
        warn!(
            "Unable to find r-base in conda history for {:?}, trying slower approach",
            env_path
        );
        get_r_info_from_package_files(env_path, None)
    }
}

/// Fast path: parse conda-meta/history for the last r-base install entry,
/// then read the corresponding JSON file.
fn get_r_info_from_history(env_path: &Path) -> Option<HistoryLookup> {
    let history_path = env_path.join("conda-meta").join("history");
    let contents = fs::read_to_string(&history_path).ok()?;

    // Find the last line matching `+...:r-base-<version>-<hash>`
    // We need the LAST entry because conda appends chronologically.
    let matching_lines: Vec<&str> = contents
        .lines()
        .filter(|l| l.starts_with('+') && l.contains(":r-base-"))
        .collect();

    let line = matching_lines.last()?;

    let captures = R_BASE_VERSION_IN_HISTORY.captures(line)?;
    let version = captures.get(1)?.as_str();
    let hash = captures.get(2)?.as_str();

    let package_filename = format!("r-base-{}-{}.json", version, hash);
    let package_path = env_path.join("conda-meta").join(&package_filename);

    if let Some(info) = read_package_json(&package_path, version) {
        trace!(
            "R package info from history for {:?}: version={}, arch={:?}",
            env_path,
            info.version,
            info.arch
        );
        return Some(HistoryLookup::Exact(info));
    }

    // JSON file not found or unparseable, but we still have the version from history.
    // The caller will fall back to a filesystem scan to try to recover architecture.
    trace!(
        "R version {} from history but no JSON for {:?}",
        version,
        env_path
    );
    Some(HistoryLookup::VersionOnly(version.to_string()))
}

/// Slow fallback: enumerate conda-meta/r-base-*.json files.
fn get_r_info_from_package_files(
    env_path: &Path,
    preferred_version: Option<&str>,
) -> Option<RCondaPackageInfo> {
    let conda_meta = env_path.join("conda-meta");
    let entries = fs::read_dir(&conda_meta).ok()?;
    let mut matching = vec![];

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if filename.starts_with("r-base-") && filename.ends_with(".json") {
            if let Some(captures) = R_BASE_VERSION.captures(&filename) {
                if let Some(version) = captures.get(1) {
                    matching.push((version.as_str().to_string(), path));
                }
            }
        }
    }

    matching.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    if let Some(preferred_version) = preferred_version {
        if let Some((version, path)) = matching
            .iter()
            .find(|(version, _)| version == preferred_version)
        {
            return read_package_json(path, version).or(Some(RCondaPackageInfo {
                version: version.clone(),
                arch: None,
            }));
        }
    }

    if let Some((version, path)) = matching.into_iter().next() {
        return read_package_json(&path, &version).or(Some(RCondaPackageInfo {
            version,
            arch: None,
        }));
    }

    None
}

/// Read a conda-meta package JSON file and extract version + architecture.
fn read_package_json(path: &PathBuf, fallback_version: &str) -> Option<RCondaPackageInfo> {
    let contents = fs::read_to_string(path).ok()?;
    let pkg: CondaMetaPackage = serde_json::from_str(&contents).ok()?;

    let arch = parse_arch(&pkg);
    let version = pkg.version.unwrap_or_else(|| fallback_version.to_string());

    Some(RCondaPackageInfo { version, arch })
}

/// Parse architecture from conda-meta package JSON fields.
///
/// Uses the `arch` field (e.g. "x86_64", "aarch64") as primary source,
/// falling back to `subdir` (e.g. "linux-64", "osx-arm64").
///
/// This does NOT use PET's `channel.ends_with("64")` approach which
/// incorrectly classifies `osx-arm64` as X64.
fn parse_arch(pkg: &CondaMetaPackage) -> Option<Architecture> {
    // 1. Try explicit "arch" field
    if let Some(ref a) = pkg.arch {
        let a = a.to_lowercase();
        if a.contains("aarch64") || a.contains("arm64") {
            return Some(Architecture::Arm64);
        }
        if a.contains("x86_64") {
            return Some(Architecture::X64);
        }
        if a.contains("x86") || a.contains("i686") || a.contains("i386") {
            return Some(Architecture::X86);
        }
    }

    // 2. Fallback to "subdir" field
    if let Some(ref sd) = pkg.subdir {
        let sd = sd.to_lowercase();
        if sd.contains("aarch64") || sd.contains("arm64") {
            return Some(Architecture::Arm64);
        }
        if sd.contains("64") {
            return Some(Architecture::X64);
        }
        if sd.contains("32") {
            return Some(Architecture::X86);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_arch_x86_64() {
        let pkg = CondaMetaPackage {
            version: Some("4.5.3".into()),
            channel: Some("conda-forge".into()),
            subdir: Some("linux-64".into()),
            arch: Some("x86_64".into()),
        };
        assert_eq!(parse_arch(&pkg), Some(Architecture::X64));
    }

    #[test]
    fn parse_arch_arm64_from_arch_field() {
        let pkg = CondaMetaPackage {
            version: Some("4.5.3".into()),
            channel: Some("conda-forge".into()),
            subdir: Some("osx-arm64".into()),
            arch: Some("aarch64".into()),
        };
        assert_eq!(parse_arch(&pkg), Some(Architecture::Arm64));
    }

    #[test]
    fn parse_arch_arm64_from_subdir_fallback() {
        let pkg = CondaMetaPackage {
            version: Some("4.5.3".into()),
            channel: Some("conda-forge".into()),
            subdir: Some("osx-arm64".into()),
            arch: None,
        };
        assert_eq!(parse_arch(&pkg), Some(Architecture::Arm64));
    }

    #[test]
    fn parse_arch_linux_aarch64() {
        let pkg = CondaMetaPackage {
            version: None,
            channel: None,
            subdir: Some("linux-aarch64".into()),
            arch: None,
        };
        assert_eq!(parse_arch(&pkg), Some(Architecture::Arm64));
    }

    #[test]
    fn parse_arch_none() {
        let pkg = CondaMetaPackage {
            version: None,
            channel: None,
            subdir: None,
            arch: None,
        };
        assert_eq!(parse_arch(&pkg), None);
    }

    #[test]
    fn from_synthetic_conda_meta() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path();
        let conda_meta = env_path.join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();

        // Write history
        fs::write(
            conda_meta.join("history"),
            "==> 2024-01-01 00:00:00 <==\n\
             # cmd: conda create -n test r-base\n\
             +conda-forge/linux-64::r-base-4.3.1-h1a7235e_0\n",
        )
        .unwrap();

        // Write package JSON
        fs::write(
            conda_meta.join("r-base-4.3.1-h1a7235e_0.json"),
            r#"{"name":"r-base","version":"4.3.1","subdir":"linux-64","arch":"x86_64","channel":"conda-forge"}"#,
        )
        .unwrap();

        let info = RCondaPackageInfo::from(env_path).unwrap();
        assert_eq!(info.version, "4.3.1");
        assert_eq!(info.arch, Some(Architecture::X64));
    }

    #[test]
    fn from_history_without_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path();
        let conda_meta = env_path.join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();

        // History only, no JSON
        fs::write(
            conda_meta.join("history"),
            "+conda-forge/osx-arm64::r-base-4.5.0-hbadge_0\n",
        )
        .unwrap();

        let info = RCondaPackageInfo::from(env_path).unwrap();
        assert_eq!(info.version, "4.5.0");
        // No JSON → no arch info
        assert_eq!(info.arch, None);
    }

    #[test]
    fn history_without_exact_json_falls_back_to_matching_file_scan() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path();
        let conda_meta = env_path.join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();

        fs::write(
            conda_meta.join("history"),
            "+conda-forge/osx-arm64::r-base-4.5.0-hmissing_0\n",
        )
        .unwrap();

        // The exact hash from history is missing, but another file for the same version exists.
        fs::write(
            conda_meta.join("r-base-4.5.0-hpresent_1.json"),
            r#"{"name":"r-base","version":"4.5.0","subdir":"osx-arm64"}"#,
        )
        .unwrap();

        let info = RCondaPackageInfo::from(env_path).unwrap();
        assert_eq!(info.version, "4.5.0");
        assert_eq!(info.arch, Some(Architecture::Arm64));
    }

    #[test]
    fn fallback_to_file_enumeration() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path();
        let conda_meta = env_path.join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();

        // No history, but package JSON exists
        fs::write(
            conda_meta.join("r-base-4.2.0-habc123_1.json"),
            r#"{"name":"r-base","version":"4.2.0","subdir":"osx-arm64"}"#,
        )
        .unwrap();

        let info = RCondaPackageInfo::from(env_path).unwrap();
        assert_eq!(info.version, "4.2.0");
        assert_eq!(info.arch, Some(Architecture::Arm64));
    }

    #[test]
    fn no_r_base_returns_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path();
        let conda_meta = env_path.join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();

        // Empty history, no r-base JSON
        fs::write(conda_meta.join("history"), "# empty\n").unwrap();

        let info = RCondaPackageInfo::from(env_path);
        assert!(info.is_none());
    }

    #[test]
    fn regex_matches_history_line() {
        let line = "+https://conda.anaconda.org/conda-forge/linux-64::r-base-4.5.3-h1a7235e_0";
        let captures = R_BASE_VERSION_IN_HISTORY.captures(line).unwrap();
        assert_eq!(captures.get(1).unwrap().as_str(), "4.5.3");
        assert_eq!(captures.get(2).unwrap().as_str(), "h1a7235e_0");
    }

    #[test]
    fn regex_matches_filename() {
        let filename = "r-base-4.5.3-h1a7235e_0.json";
        let captures = R_BASE_VERSION.captures(filename).unwrap();
        assert_eq!(captures.get(1).unwrap().as_str(), "4.5.3");
    }
}
