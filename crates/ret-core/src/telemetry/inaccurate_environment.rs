// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Deserialize, Serialize};

use crate::r_installation::RInstallationKind;

/// Telemetry information about inaccurate environment detection.
///
/// Emitted when a locator's initial (unresolved) discovery differs
/// from the resolved installation details.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct InaccurateEnvironmentInfo {
    pub kind: Option<RInstallationKind>,
    pub invalid_executable: Option<bool>,
    pub executable_not_in_symlinks: Option<bool>,
    pub invalid_prefix: Option<bool>,
    pub invalid_version: Option<bool>,
    pub invalid_arch: Option<bool>,
}
