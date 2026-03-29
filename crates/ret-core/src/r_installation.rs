// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use clap::{Parser, ValueEnum};
use log::error;
use ret_fs::path::norm_case;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

use crate::{arch::Architecture, manager::EnvManager};

#[derive(
    Parser,
    ValueEnum,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Debug,
    Hash,
    Ord,
    PartialOrd,
)]
pub enum RInstallationKind {
    Chocolatey,
    Conda,
    EnvironmentModule,
    Guix,
    Homebrew,
    LinuxGlobal,
    MacFramework,
    MacPorts,
    Nix,
    Pixi,
    Rig,
    Scoop,
    Spack,
    WindowsHq,
    WindowsRegistry,
    GlobalPaths,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Ord, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub enum DiscoverySource {
    Locator,
    GlobalPaths,
    ExplicitSearch,
    RVersions,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LocatorMetadata {
    Conda {
        environment_path: PathBuf,
    },
    Pixi {
        environment_path: PathBuf,
        manifest_path: Option<PathBuf>,
        environment_name: Option<String>,
    },
    Module {
        module_name: String,
        startup_command: String,
    },
}

impl LocatorMetadata {
    pub fn priority(&self) -> u8 {
        match self {
            LocatorMetadata::Pixi { .. } => 3,
            LocatorMetadata::Conda { .. } => 2,
            LocatorMetadata::Module { .. } => 1,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RVersionsOverlay {
    pub label: Option<String>,
    pub script: Option<String>,
    pub repo: Option<String>,
    pub library: Option<String>,
    pub module: Option<String>,
    pub module_startup_command: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RInstallation {
    pub display_name: Option<String>,
    pub name: Option<String>,
    pub executable: Option<PathBuf>,
    pub kind: Option<RInstallationKind>,
    pub version: Option<String>,
    pub home: Option<PathBuf>,
    pub manager: Option<EnvManager>,
    pub arch: Option<Architecture>,
    pub known_executables: Option<Vec<PathBuf>>,
    pub symlinks: Option<Vec<PathBuf>>,
    #[serde(default)]
    pub discovered_by: Vec<DiscoverySource>,
    #[serde(default)]
    pub locator_metadata: Option<LocatorMetadata>,
    #[serde(default)]
    pub rversions_overlay: Option<RVersionsOverlay>,
    #[serde(default)]
    pub script_path: Option<PathBuf>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub environment_variables: Option<HashMap<String, String>>,
    #[serde(default)]
    pub orthogonal: Option<bool>,
    pub error: Option<String>,
}

impl Ord for RInstallation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.executable
            .cmp(&other.executable)
            .then_with(|| self.home.cmp(&other.home))
    }
}

impl PartialOrd for RInstallation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for RInstallation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "R Installation ({})",
            self.kind
                .map(|v| format!("{v:?}"))
                .unwrap_or("Unknown".to_string())
        )?;
        if let Some(name) = &self.display_name {
            writeln!(f, "   Display-Name: {name}")?;
        }
        if let Some(name) = &self.name {
            writeln!(f, "   Name        : {name}")?;
        }
        if let Some(exe) = &self.executable {
            writeln!(f, "   Executable  : {}", exe.display())?;
        }
        if let Some(version) = &self.version {
            writeln!(f, "   Version     : {version}")?;
        }
        if let Some(home) = &self.home {
            writeln!(f, "   Home        : {}", home.display())?;
        }
        if let Some(script_path) = &self.script_path {
            writeln!(f, "   Script Path : {}", script_path.display())?;
        }
        if let Some(startup_command) = &self.startup_command {
            writeln!(f, "   Startup Cmd : {startup_command}")?;
        }
        if let Some(rversions_overlay) = &self.rversions_overlay {
            if let Some(label) = &rversions_overlay.label {
                writeln!(f, "   RVersions   : label={label}")?;
            }
            if let Some(script) = &rversions_overlay.script {
                writeln!(f, "               : script={script}")?;
            }
            if let Some(repo) = &rversions_overlay.repo {
                writeln!(f, "               : repo={repo}")?;
            }
            if let Some(library) = &rversions_overlay.library {
                writeln!(f, "               : library={library}")?;
            }
            if let Some(module) = &rversions_overlay.module {
                writeln!(f, "               : module={module}")?;
            }
            if let Some(module_startup_command) = &rversions_overlay.module_startup_command {
                writeln!(f, "               : moduleStartup={module_startup_command}")?;
            }
        }
        if let Some(arch) = &self.arch {
            writeln!(f, "   Architecture: {arch}")?;
        }
        if let Some(orthogonal) = &self.orthogonal {
            writeln!(f, "   Orthogonal  : {orthogonal}")?;
        }
        if let Some(manager) = &self.manager {
            writeln!(
                f,
                "   Manager     : {:?}, {}",
                manager.tool,
                manager.executable.display()
            )?;
        }
        if let Some(environment_variables) = &self.environment_variables {
            let mut keys = environment_variables.keys().collect::<Vec<_>>();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                let value = &environment_variables[key];
                if i == 0 {
                    writeln!(f, "   Env Vars    : {key}={value}")?;
                } else {
                    writeln!(f, "               : {key}={value}")?;
                }
            }
        }
        if let Some(known_executables) = &self.known_executables {
            for (i, executable) in known_executables.iter().enumerate() {
                if i == 0 {
                    writeln!(f, "   Known Executables: {}", executable.display())?;
                } else {
                    writeln!(f, "                    : {}", executable.display())?;
                }
            }
        }
        if let Some(symlinks) = &self.symlinks {
            for (i, symlink) in symlinks.iter().enumerate() {
                if i == 0 {
                    writeln!(f, "   Symlinks    : {}", symlink.display())?;
                } else {
                    writeln!(f, "               : {}", symlink.display())?;
                }
            }
        }
        if let Some(error) = &self.error {
            writeln!(f, "   Error       : {error}")?;
        }
        Ok(())
    }
}

impl RInstallation {
    /// Returns a key suitable for deduplicating installations.
    pub fn get_environment_key(&self) -> Option<PathBuf> {
        if let Some(exe) = &self.executable {
            Some(exe.clone())
        } else if let Some(home) = &self.home {
            Some(home.clone())
        } else {
            error!(
                "Failed to report installation due to lack of exe & home: {:?}",
                self
            );
            None
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RInstallationBuilder {
    display_name: Option<String>,
    name: Option<String>,
    executable: Option<PathBuf>,
    kind: Option<RInstallationKind>,
    version: Option<String>,
    home: Option<PathBuf>,
    manager: Option<EnvManager>,
    arch: Option<Architecture>,
    known_executables: Option<Vec<PathBuf>>,
    symlinks: Option<Vec<PathBuf>>,
    discovered_by: Vec<DiscoverySource>,
    locator_metadata: Option<LocatorMetadata>,
    rversions_overlay: Option<RVersionsOverlay>,
    script_path: Option<PathBuf>,
    startup_command: Option<String>,
    environment_variables: Option<HashMap<String, String>>,
    orthogonal: Option<bool>,
    error: Option<String>,
}

impl RInstallationBuilder {
    pub fn new(kind: Option<RInstallationKind>) -> Self {
        Self {
            display_name: None,
            name: None,
            executable: None,
            kind,
            version: None,
            home: None,
            manager: None,
            arch: None,
            known_executables: None,
            symlinks: None,
            discovered_by: Vec::new(),
            locator_metadata: None,
            rversions_overlay: None,
            script_path: None,
            startup_command: None,
            environment_variables: None,
            orthogonal: None,
            error: None,
        }
    }

    pub fn from_installation(installation: RInstallation) -> Self {
        Self {
            display_name: installation.display_name,
            name: installation.name,
            executable: installation.executable,
            kind: installation.kind,
            version: installation.version,
            home: installation.home,
            manager: installation.manager,
            arch: installation.arch,
            known_executables: installation.known_executables,
            symlinks: installation.symlinks,
            discovered_by: installation.discovered_by,
            locator_metadata: installation.locator_metadata,
            rversions_overlay: installation.rversions_overlay,
            script_path: installation.script_path,
            startup_command: installation.startup_command,
            environment_variables: installation.environment_variables,
            orthogonal: installation.orthogonal,
            error: installation.error,
        }
    }

    pub fn display_name(mut self, display_name: Option<String>) -> Self {
        self.display_name = display_name;
        self
    }

    pub fn name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    pub fn executable(mut self, executable: Option<PathBuf>) -> Self {
        self.executable = executable.map(norm_case);
        self
    }

    pub fn kind(mut self, kind: Option<RInstallationKind>) -> Self {
        self.kind = kind;
        self
    }

    pub fn version(mut self, version: Option<String>) -> Self {
        self.version = version;
        self
    }

    pub fn home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home.map(norm_case);
        self
    }

    pub fn manager(mut self, manager: Option<EnvManager>) -> Self {
        self.manager = manager;
        self
    }

    pub fn arch(mut self, arch: Option<Architecture>) -> Self {
        self.arch = arch;
        self
    }

    pub fn known_executables(mut self, known_executables: Option<Vec<PathBuf>>) -> Self {
        self.known_executables = known_executables.map(normalize_paths);
        self
    }

    pub fn symlinks(mut self, symlinks: Option<Vec<PathBuf>>) -> Self {
        self.symlinks = symlinks.map(normalize_paths);
        self
    }

    pub fn discovered_by(mut self, discovered_by: Vec<DiscoverySource>) -> Self {
        let mut discovered_by = discovered_by;
        discovered_by.sort();
        discovered_by.dedup();
        self.discovered_by = discovered_by;
        self
    }

    pub fn add_discovery_source(mut self, source: DiscoverySource) -> Self {
        if !self.discovered_by.contains(&source) {
            self.discovered_by.push(source);
            self.discovered_by.sort();
            self.discovered_by.dedup();
        }
        self
    }

    pub fn locator_metadata(mut self, locator_metadata: Option<LocatorMetadata>) -> Self {
        self.locator_metadata = locator_metadata;
        self
    }

    pub fn rversions_overlay(mut self, rversions_overlay: Option<RVersionsOverlay>) -> Self {
        self.rversions_overlay = rversions_overlay;
        self
    }

    pub fn script_path(mut self, script_path: Option<PathBuf>) -> Self {
        self.script_path = script_path.map(norm_case);
        self
    }

    pub fn startup_command(mut self, startup_command: Option<String>) -> Self {
        self.startup_command = startup_command;
        self
    }

    pub fn environment_variables(
        mut self,
        environment_variables: Option<HashMap<String, String>>,
    ) -> Self {
        self.environment_variables = environment_variables;
        self
    }

    pub fn orthogonal(mut self, orthogonal: Option<bool>) -> Self {
        self.orthogonal = orthogonal;
        self
    }

    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    pub fn build(self) -> RInstallation {
        let mut symlinks = self.symlinks.unwrap_or_default();
        symlinks = normalize_paths(symlinks);

        let mut known_executables = self.known_executables.unwrap_or_default();
        if let Some(executable) = &self.executable {
            known_executables.push(executable.clone());
        }
        known_executables = normalize_paths(known_executables);

        let script_path = self
            .script_path
            .clone()
            .or_else(|| {
                infer_script_path(
                    self.executable.as_ref(),
                    self.home.as_ref(),
                    self.arch.as_ref(),
                )
            })
            .map(norm_case);
        let orthogonal = self
            .orthogonal
            .or_else(|| infer_orthogonal(self.home.as_ref()));
        let environment_variables = self.environment_variables.filter(|vars| !vars.is_empty());
        let mut discovered_by = self.discovered_by;
        if discovered_by.is_empty()
            && self.kind != Some(RInstallationKind::GlobalPaths)
            && self.kind.is_some()
        {
            discovered_by.push(DiscoverySource::Locator);
        }
        discovered_by.sort();
        discovered_by.dedup();

        RInstallation {
            display_name: self.display_name,
            name: self.name,
            executable: self.executable,
            kind: self.kind,
            version: self.version,
            home: self.home,
            manager: self.manager,
            arch: self.arch,
            known_executables: if known_executables.is_empty() {
                None
            } else {
                Some(known_executables)
            },
            symlinks: if symlinks.is_empty() {
                None
            } else {
                Some(symlinks)
            },
            discovered_by,
            locator_metadata: self.locator_metadata,
            rversions_overlay: self.rversions_overlay,
            script_path,
            startup_command: self.startup_command,
            environment_variables,
            orthogonal,
            error: self.error,
        }
    }
}

fn normalize_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut paths = paths.into_iter().map(norm_case).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn infer_script_path(
    executable: Option<&PathBuf>,
    home: Option<&PathBuf>,
    arch: Option<&Architecture>,
) -> Option<PathBuf> {
    if let Some(executable) = executable {
        let file_name = executable
            .file_name()?
            .to_string_lossy()
            .to_ascii_lowercase();
        let mut path = executable.clone();
        if file_name == "rscript" || file_name == "rscript.exe" {
            return Some(path);
        }
        if file_name == "r" {
            path.set_file_name("Rscript");
            return Some(path);
        }
        if file_name == "r.exe" {
            path.set_file_name("Rscript.exe");
            return Some(path);
        }
    }

    let home = home?;
    if cfg!(windows) {
        let mut base = home.join("bin");
        if arch == Some(&Architecture::X64) {
            base = base.join("x64");
        }
        Some(base.join("Rscript.exe"))
    } else {
        Some(home.join("bin").join("Rscript"))
    }
}

fn infer_orthogonal(home: Option<&PathBuf>) -> Option<bool> {
    let home = home?;
    let home_str = home.to_string_lossy();
    if home_str.contains("R.framework") {
        Some(home_str.contains("R.framework/Versions/"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn builder_derives_script_path_from_r_binary() {
        let installation = RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
            .executable(Some(PathBuf::from("/opt/R/4.4.1/bin/R")))
            .build();

        assert_eq!(
            installation.script_path,
            Some(ret_fs::path::norm_case("/opt/R/4.4.1/bin/Rscript"))
        );
    }

    #[test]
    fn builder_derives_orthogonal_from_framework_home() {
        let orthogonal = RInstallationBuilder::new(Some(RInstallationKind::MacFramework))
            .home(Some(PathBuf::from(
                "/Library/Frameworks/R.framework/Versions/4.4-arm64/Resources",
            )))
            .build();
        let non_orthogonal = RInstallationBuilder::new(Some(RInstallationKind::MacFramework))
            .home(Some(PathBuf::from(
                "/Library/Frameworks/R.framework/Resources",
            )))
            .build();

        assert_eq!(orthogonal.orthogonal, Some(true));
        assert_eq!(non_orthogonal.orthogonal, Some(false));
    }

    #[test]
    fn builder_defaults_locator_discovery_for_locator_kinds() {
        let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda)).build();
        let global_paths = RInstallationBuilder::new(Some(RInstallationKind::GlobalPaths)).build();

        assert_eq!(installation.discovered_by, vec![DiscoverySource::Locator]);
        assert!(global_paths.discovered_by.is_empty());
    }

    #[test]
    fn builder_tracks_executable_as_known_executable_but_not_symlink() {
        let installation = RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
            .executable(Some(PathBuf::from("/opt/R/4.4.1/bin/R")))
            .build();

        assert_eq!(
            installation.known_executables,
            Some(vec![ret_fs::path::norm_case("/opt/R/4.4.1/bin/R")])
        );
        assert_eq!(installation.symlinks, None);
    }

    #[test]
    fn builder_preserves_startup_command_and_environment_variables() {
        let mut environment_variables = HashMap::new();
        environment_variables.insert("R_LIBS".to_string(), "/opt/r-libs".to_string());

        let installation = RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
            .startup_command(Some(". /opt/scripts/load-r.sh".to_string()))
            .environment_variables(Some(environment_variables.clone()))
            .build();

        assert_eq!(
            installation.startup_command,
            Some(". /opt/scripts/load-r.sh".to_string())
        );
        assert_eq!(
            installation.environment_variables,
            Some(environment_variables)
        );
    }
}
