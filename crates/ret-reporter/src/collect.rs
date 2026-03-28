// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{manager::EnvManager, r_installation::RInstallation, reporter::Reporter};
use std::sync::{Arc, Mutex};

pub struct CollectReporter {
    pub managers: Arc<Mutex<Vec<EnvManager>>>,
    pub installations: Arc<Mutex<Vec<RInstallation>>>,
}

impl Default for CollectReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectReporter {
    pub fn new() -> CollectReporter {
        CollectReporter {
            managers: Arc::new(Mutex::new(vec![])),
            installations: Arc::new(Mutex::new(vec![])),
        }
    }
}

impl Reporter for CollectReporter {
    fn report_manager(&self, manager: &EnvManager) {
        self.managers
            .lock()
            .expect("managers mutex poisoned")
            .push(manager.clone());
    }

    fn report_installation(&self, installation: &RInstallation) {
        self.installations
            .lock()
            .expect("installations mutex poisoned")
            .push(installation.clone());
    }
}

pub fn create_reporter() -> CollectReporter {
    CollectReporter::new()
}
