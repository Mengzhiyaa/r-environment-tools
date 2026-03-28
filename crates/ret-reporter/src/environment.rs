// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::r_installation::RInstallation;
use std::path::PathBuf;

/// Returns a deduplication key for an R installation.
///
/// Delegates to `RInstallation::get_environment_key()`.
pub fn get_environment_key(installation: &RInstallation) -> Option<PathBuf> {
    installation.get_environment_key()
}
