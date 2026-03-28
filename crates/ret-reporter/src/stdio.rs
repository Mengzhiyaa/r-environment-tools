// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use env_logger::Builder;
use log::LevelFilter;
use ret_core::{
    manager::{EnvManager, EnvManagerType},
    r_installation::{RInstallation, RInstallationKind},
    reporter::Reporter,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct StdioReporter {
    print_list: bool,
    managers: Arc<Mutex<HashMap<EnvManagerType, u16>>>,
    installations: Arc<Mutex<HashMap<Option<RInstallationKind>, u16>>>,
    installation_paths: Arc<Mutex<HashMap<Option<RInstallationKind>, Vec<RInstallation>>>>,
    kind: Option<RInstallationKind>,
}

pub struct Summary {
    pub managers: HashMap<EnvManagerType, u16>,
    pub installations: HashMap<Option<RInstallationKind>, u16>,
    pub installation_paths: HashMap<Option<RInstallationKind>, Vec<RInstallation>>,
}

impl StdioReporter {
    pub fn get_summary(&self) -> Summary {
        Summary {
            managers: self
                .managers
                .lock()
                .expect("managers mutex poisoned")
                .clone(),
            installations: self
                .installations
                .lock()
                .expect("installations mutex poisoned")
                .clone(),
            installation_paths: self
                .installation_paths
                .lock()
                .expect("installation_paths mutex poisoned")
                .clone(),
        }
    }
}

impl Reporter for StdioReporter {
    fn report_manager(&self, manager: &EnvManager) {
        let mut managers = self.managers.lock().expect("managers mutex poisoned");
        let count = managers.get(&manager.tool).unwrap_or(&0) + 1;
        managers.insert(manager.tool, count);
        if self.print_list {
            println!("{manager}");
        }
    }

    fn report_installation(&self, installation: &RInstallation) {
        if self.kind.is_some() && installation.kind != self.kind {
            return;
        }

        let mut installations = self
            .installations
            .lock()
            .expect("installations mutex poisoned");
        let count = installations.get(&installation.kind).unwrap_or(&0) + 1;
        installations.insert(installation.kind, count);

        let mut installation_paths = self
            .installation_paths
            .lock()
            .expect("installation_paths mutex poisoned");
        installation_paths
            .entry(installation.kind)
            .or_default()
            .push(installation.clone());

        if self.print_list {
            println!("{installation}");
        }
    }
}

pub fn create_reporter(print_list: bool, kind: Option<RInstallationKind>) -> StdioReporter {
    StdioReporter {
        print_list,
        managers: Arc::new(Mutex::new(HashMap::new())),
        installations: Arc::new(Mutex::new(HashMap::new())),
        installation_paths: Arc::new(Mutex::new(HashMap::new())),
        kind,
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Eq, Clone)]
pub enum LogLevel {
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub message: String,
    pub level: LogLevel,
}

pub fn initialize_logger(log_level: LevelFilter) {
    Builder::new().filter(None, log_level).init();
}
