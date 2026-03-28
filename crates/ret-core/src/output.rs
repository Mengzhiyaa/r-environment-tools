use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(ValueEnum, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum OutputSchema {
    #[default]
    Ret,
    Pet,
    Dual,
}
