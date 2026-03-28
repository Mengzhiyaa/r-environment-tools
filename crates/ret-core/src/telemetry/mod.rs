// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use inaccurate_environment::InaccurateEnvironmentInfo;
use refresh_performance::RefreshPerformance;
use serde::{Deserialize, Serialize};

pub mod inaccurate_environment;
pub mod refresh_performance;

pub type NumberOfCustomSearchPaths = u32;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum TelemetryEvent {
    GlobalEnvironmentsSearchCompleted(std::time::Duration),
    GlobalPathVariableEnvironmentsSearchCompleted(std::time::Duration),
    AllSearchPathsEnvironmentsSearchCompleted(std::time::Duration, NumberOfCustomSearchPaths),
    SearchCompleted(std::time::Duration),
    InaccurateEnvironmentInfo(InaccurateEnvironmentInfo),
    RefreshPerformance(RefreshPerformance),
}

pub fn get_telemetry_event_name(event: &TelemetryEvent) -> &'static str {
    match event {
        TelemetryEvent::GlobalEnvironmentsSearchCompleted(_) => "GlobalEnvironmentsSearchCompleted",
        TelemetryEvent::GlobalPathVariableEnvironmentsSearchCompleted(_) => {
            "GlobalPathVariableEnvironmentsSearchCompleted"
        }
        TelemetryEvent::AllSearchPathsEnvironmentsSearchCompleted(_, _) => {
            "AllSearchPathsEnvironmentsSearchCompleted"
        }
        TelemetryEvent::SearchCompleted(_) => "SearchCompleted",
        TelemetryEvent::InaccurateEnvironmentInfo(_) => "InaccurateEnvironmentInfo",
        TelemetryEvent::RefreshPerformance(_) => "RefreshPerformance",
    }
}
