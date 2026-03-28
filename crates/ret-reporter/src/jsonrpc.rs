// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use env_logger::Builder;
use log::{trace, LevelFilter};
use ret_core::{
    manager::EnvManager,
    output::OutputSchema,
    r_installation::{RInstallation, RInstallationKind},
    reporter::Reporter,
    telemetry::{get_telemetry_event_name, TelemetryEvent},
};
use ret_jsonrpc::send_message;
use serde::{Deserialize, Serialize};

pub struct JsonRpcReporter {
    report_only: Option<RInstallationKind>,
    output_schema: OutputSchema,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TelemetryData {
    event: String,
    data: TelemetryEvent,
}

impl Reporter for JsonRpcReporter {
    fn report_telemetry(&self, event: &TelemetryEvent) {
        let event = TelemetryData {
            event: get_telemetry_event_name(event).to_string(),
            data: event.clone(),
        };
        trace!("Reporting telemetry {:?}", event.event);
        send_message("telemetry", Some(event));
    }

    fn report_manager(&self, manager: &EnvManager) {
        trace!("Reporting manager {:?}", manager);
        send_message("manager", manager.into())
    }

    fn report_installation(&self, installation: &RInstallation) {
        if let Some(report_only) = &self.report_only {
            if installation.kind != Some(*report_only) {
                trace!(
                    "Skip reporting installation ({:?}) due to filter {:?}",
                    installation.kind,
                    report_only
                );
                return;
            }
        }
        trace!("Reporting installation {:?}", installation);
        match self.output_schema {
            OutputSchema::Ret => send_message("installation", installation.into()),
            OutputSchema::Pet => {
                send_message("environment", Some(installation.to_pet_json()));
            }
            OutputSchema::Dual => {
                send_message("environment", Some(installation.to_pet_json()));
                send_message("installation", installation.into());
            }
        }
    }
}

pub fn create_reporter(
    report_only: Option<RInstallationKind>,
    output_schema: OutputSchema,
) -> impl Reporter {
    JsonRpcReporter {
        report_only,
        output_schema,
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
    Builder::new()
        .format(|_, record| {
            let level = match record.level() {
                log::Level::Debug => LogLevel::Debug,
                log::Level::Error => LogLevel::Error,
                log::Level::Info => LogLevel::Info,
                log::Level::Warn => LogLevel::Warning,
                _ => LogLevel::Debug,
            };
            let payload = Log {
                message: format!("{}", record.args()),
                level,
            };
            send_message("log", payload.into());
            Ok(())
        })
        .filter(None, log_level)
        .init();
}
