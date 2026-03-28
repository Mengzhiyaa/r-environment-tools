// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::PathBuf;

#[allow(dead_code)]
pub fn resolve_test_path(paths: &[&str]) -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    paths.iter().for_each(|p| root.push(p));
    root
}

/// Returns true when the detected version starts with the expected version
/// (allowing the expected to be a prefix, e.g. "4.4" matches "4.4.1").
#[allow(dead_code)]
pub fn does_version_match(version: &str, expected_version: &str) -> bool {
    expected_version.starts_with(version) || version.starts_with(expected_version)
}
