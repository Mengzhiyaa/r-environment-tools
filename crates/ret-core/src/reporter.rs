// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{manager::EnvManager, r_installation::RInstallation, telemetry::TelemetryEvent};

pub trait Reporter: Send + Sync {
    fn report_manager(&self, manager: &EnvManager);
    fn report_installation(&self, installation: &RInstallation);
    fn report_telemetry(&self, _event: &TelemetryEvent) {
        //
    }
}
