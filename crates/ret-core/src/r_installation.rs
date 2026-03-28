// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use clap::{Parser, ValueEnum};
use log::error;
use ret_fs::path::norm_case;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub symlinks: Option<Vec<PathBuf>>,
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
        if let Some(arch) = &self.arch {
            writeln!(f, "   Architecture: {arch}")?;
        }
        if let Some(manager) = &self.manager {
            writeln!(
                f,
                "   Manager     : {:?}, {}",
                manager.tool,
                manager.executable.display()
            )?;
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
    symlinks: Option<Vec<PathBuf>>,
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
            symlinks: None,
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
            symlinks: installation.symlinks,
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

    pub fn symlinks(mut self, symlinks: Option<Vec<PathBuf>>) -> Self {
        self.symlinks = symlinks.map(|items| {
            let mut items = items.into_iter().map(norm_case).collect::<Vec<_>>();
            items.sort();
            items.dedup();
            items
        });
        self
    }

    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    pub fn build(self) -> RInstallation {
        let mut symlinks = self.symlinks.unwrap_or_default();
        if let Some(executable) = &self.executable {
            symlinks.push(executable.clone());
        }
        symlinks.sort();
        symlinks.dedup();

        RInstallation {
            display_name: self.display_name,
            name: self.name,
            executable: self.executable,
            kind: self.kind,
            version: self.version,
            home: self.home,
            manager: self.manager,
            arch: self.arch,
            symlinks: if symlinks.is_empty() {
                None
            } else {
                Some(symlinks)
            },
            error: self.error,
        }
    }
}
