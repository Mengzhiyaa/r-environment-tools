// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::{error, trace};
use ret_fs::path::norm_case;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::env::ResolvedRInstallation;

type FilePathWithMTimeCTime = (PathBuf, SystemTime, Option<SystemTime>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    pub installation: ResolvedRInstallation,
    pub executables: Vec<FilePathWithMTimeCTime>,
}

pub fn generate_cache_file(cache_directory: &Path, executable: &PathBuf) -> PathBuf {
    cache_directory.join(format!("{}.5.json", generate_hash(executable)))
}

pub fn delete_cache_file(cache_directory: &Path, executable: &PathBuf) {
    let cache_file = generate_cache_file(cache_directory, executable);
    let _ = fs::remove_file(cache_file);
}

pub fn get_cache_from_file(
    cache_directory: &Path,
    executable: &PathBuf,
) -> Option<(ResolvedRInstallation, Vec<FilePathWithMTimeCTime>)> {
    let cache_file = generate_cache_file(cache_directory, executable);
    let file = File::open(cache_file.clone()).ok()?;
    let reader = BufReader::new(file);
    let cache: CacheEntry = serde_json::from_reader(reader).ok()?;
    if !cache
        .installation
        .clone()
        .known_executables
        .unwrap_or_default()
        .contains(executable)
    {
        trace!(
            "Cache file {:?} {:?}, does not match executable {:?}",
            cache_file,
            cache.installation,
            executable
        );
        return None;
    }

    let cache_is_valid = cache.executables.iter().all(|executable| {
        if let Ok(metadata) = executable.0.metadata() {
            let mtime_valid = metadata.modified().ok() == Some(executable.1);
            let ctime_valid = match executable.2 {
                Some(stored_ctime) => metadata.created().ok() == Some(stored_ctime),
                None => true,
            };
            mtime_valid && ctime_valid
        } else {
            false
        }
    });

    if cache_is_valid {
        trace!("Using cache from {:?} for {:?}", cache_file, executable);
        Some((cache.installation, cache.executables))
    } else {
        let _ = fs::remove_file(cache_file);
        None
    }
}

pub fn store_cache_in_file(
    cache_directory: &Path,
    executable: &PathBuf,
    installation: &ResolvedRInstallation,
    executables_with_times: Vec<FilePathWithMTimeCTime>,
) {
    let cache_file = generate_cache_file(cache_directory, executable);
    match std::fs::create_dir_all(cache_directory) {
        Ok(_) => {
            let cache = CacheEntry {
                installation: installation.clone(),
                executables: executables_with_times,
            };
            match std::fs::File::create(cache_file.clone()) {
                Ok(file) => {
                    trace!("Caching {:?} in {:?}", executable, cache_file);
                    if let Err(err) = serde_json::to_writer_pretty(file, &cache) {
                        error!("Error writing cache file {:?}: {:?}", cache_file, err);
                    }
                }
                Err(err) => error!("Error creating cache file {:?}: {:?}", cache_file, err),
            }
        }
        Err(err) => error!(
            "Error creating cache directory {:?}: {:?}",
            cache_directory, err
        ),
    }
}

fn generate_hash(executable: &PathBuf) -> String {
    let mut hasher = Sha256::new();
    hasher.update(norm_case(executable).to_string_lossy().as_bytes());
    let h_bytes = hasher.finalize();
    format!("{h_bytes:x}")[..16].to_string()
}
