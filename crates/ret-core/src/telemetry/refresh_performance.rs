// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RefreshPerformance {
    pub total: u128,
    pub breakdown: BTreeMap<String, u128>,
    pub locators: BTreeMap<String, u128>,
}
