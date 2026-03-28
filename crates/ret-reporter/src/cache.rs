// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::installation::get_installation_key;
use ret_core::{
    manager::EnvManager, r_installation::RInstallation, reporter::Reporter,
    telemetry::TelemetryEvent,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

/// Decorator that suppresses duplicate installations and managers.
pub struct CacheReporter {
    reporter: Arc<dyn Reporter>,
    reported_managers: Arc<RwLock<HashMap<PathBuf, EnvManager>>>,
    reported_installations: Arc<RwLock<HashMap<PathBuf, RInstallation>>>,
}

impl CacheReporter {
    pub fn new(reporter: Arc<dyn Reporter>) -> Self {
        Self {
            reporter,
            reported_managers: Arc::new(RwLock::new(HashMap::new())),
            reported_installations: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Reporter for CacheReporter {
    fn report_manager(&self, manager: &EnvManager) {
        {
            let reported_managers = self.reported_managers.read().unwrap();
            if reported_managers.contains_key(&manager.executable) {
                return;
            }
        }

        let mut reported_managers = self.reported_managers.write().unwrap();
        if !reported_managers.contains_key(&manager.executable) {
            reported_managers.insert(manager.executable.clone(), manager.clone());
            self.reporter.report_manager(manager);
        }
    }

    fn report_installation(&self, installation: &RInstallation) {
        if let Some(key) = get_installation_key(installation) {
            {
                let reported_installations = self.reported_installations.read().unwrap();
                if reported_installations.contains_key(&key) {
                    return;
                }
            }

            let mut reported_installations = self.reported_installations.write().unwrap();
            if !reported_installations.contains_key(&key) {
                reported_installations.insert(key.clone(), installation.clone());
                self.reporter.report_installation(installation);
            }
        }
    }

    fn report_telemetry(&self, event: &TelemetryEvent) {
        self.reporter.report_telemetry(event);
    }
}
