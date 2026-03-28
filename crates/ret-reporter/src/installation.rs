use log::error;
use ret_core::r_installation::RInstallation;
use std::path::PathBuf;

pub fn get_installation_key(installation: &RInstallation) -> Option<PathBuf> {
    if let Some(home) = &installation.home {
        Some(home.clone())
    } else if let Some(executable) = &installation.executable {
        Some(executable.clone())
    } else {
        error!(
            "Failed to report installation due to lack of executable and home: {:?}",
            installation
        );
        None
    }
}
