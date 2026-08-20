// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{any::Any, path::PathBuf};

use env::REnv;
use manager::EnvManager;
use r_installation::{RInstallation, RInstallationKind};
use reporter::Reporter;

pub mod arch;
pub mod cache;
pub mod env;
pub mod homebrew_utils;
pub mod manager;
pub mod os_environment;

pub mod r_installation;
pub mod reporter;
pub mod telemetry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorResult {
    pub managers: Vec<EnvManager>,
    pub installations: Vec<RInstallation>,
}

#[derive(Debug, Default, Clone)]
pub struct Configuration {
    /// Project or workspace directories that should be searched for R installations.
    pub workspace_directories: Option<Vec<PathBuf>>,
    /// Additional global directories that may contain one or more R installations.
    pub environment_directories: Option<Vec<PathBuf>>,
    /// One-off explicit search directories, primarily used by CLI and refresh search paths.
    pub search_directories: Option<Vec<PathBuf>>,
    pub executables: Option<Vec<PathBuf>>,
    /// Optional cache directory for resolved R installation details.
    pub cache_directory: Option<PathBuf>,
    /// Optional path to the conda, mamba, or micromamba executable.
    pub conda_executable: Option<PathBuf>,
    /// Optional path to the `rig` executable.
    pub rig_executable: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LocatorKind {
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
    RVersions,
    Scoop,
    Spack,
    WindowsHq,
    WindowsRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshStatePersistence {
    /// The locator has no mutable request state.
    Stateless,
    /// The locator stores configuration, which must come from the request snapshot.
    ConfiguredOnly,
    /// The locator has a cache that later requests can rebuild on demand.
    SelfHydratingCache,
    /// Later requests depend on state discovered by refresh.
    SyncedDiscoveryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshStateSyncScope {
    Full,
    GlobalFiltered(RInstallationKind),
    Workspace,
}

pub trait Locator: Any + Send + Sync {
    /// Returns the name of the locator.
    fn get_kind(&self) -> LocatorKind;
    /// Configures the locator with the given configuration.
    ///
    /// Override this method if you need to store configuration in the locator.
    ///
    /// # Why `&self` instead of `&mut self`?
    ///
    /// Locators are shared across threads via `Arc<dyn Locator>` and may be
    /// configured while other operations are in progress. Using `&self` allows
    /// concurrent access without requiring the caller to hold an exclusive lock
    /// on the entire locator.
    ///
    /// Implementations that need to store configuration should use interior
    /// mutability (e.g., `Mutex<T>` or `RwLock<T>`) for the mutable fields only.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::sync::Mutex;
    /// use std::path::PathBuf;
    ///
    /// struct MyLocator {
    ///     search_dirs: Mutex<Vec<PathBuf>>,
    /// }
    ///
    /// impl Locator for MyLocator {
    ///     fn configure(&self, config: &Configuration) {
    ///         if let Some(dirs) = &config.search_directories {
    ///             *self.search_dirs.lock().expect("search_dirs mutex poisoned") = dirs.clone();
    ///         }
    ///     }
    ///     // ... other required methods
    /// }
    /// ```
    fn configure(&self, _config: &Configuration) {
        //
    }
    /// Declares how mutable locator state behaves across a transient refresh.
    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::Stateless
    }
    /// Copies correctness-critical discovery state from a transient locator.
    ///
    /// Only locators classified as `SyncedDiscoveryState` should override this.
    fn sync_refresh_state_from(&self, _source: &dyn Locator, _scope: &RefreshStateSyncScope) {
        //
    }
    /// Returns a list of supported installation kinds for this locator.
    fn supported_categories(&self) -> Vec<RInstallationKind>;
    /// Attempts to classify a raw R executable into a known installation kind.
    fn try_from(&self, env: &REnv) -> Option<RInstallation>;
    /// Finds all installations specific to this locator.
    fn find(&self, reporter: &dyn Reporter);
}

impl dyn Locator {
    pub fn as_any(&self) -> &dyn Any {
        self
    }
}
