// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Ord, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub enum Architecture {
    Arm64,
    X64,
    X86,
}

impl Architecture {
    /// Infer architecture from a filesystem path.
    ///
    /// Priority: explicit path markers → compile-time `target_arch` → `X86` fallback.
    pub fn infer_from_path(path: &Path) -> Self {
        let haystack = path.display().to_string().to_lowercase();
        if haystack.contains("aarch64") || haystack.contains("arm64") {
            Architecture::Arm64
        } else if haystack.contains("x86_64") || haystack.contains("x64") {
            Architecture::X64
        } else if haystack.contains("i386") || haystack.contains("i686") || haystack.contains("x86")
        {
            Architecture::X86
        } else if cfg!(target_arch = "aarch64") {
            Architecture::Arm64
        } else if cfg!(target_arch = "x86_64") {
            Architecture::X64
        } else {
            Architecture::X86
        }
    }
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Architecture::Arm64 => write!(f, "arm64"),
            Architecture::X64 => write!(f, "x64"),
            Architecture::X86 => write!(f, "x86"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_architecture_display_arm64() {
        assert_eq!(format!("{}", Architecture::Arm64), "arm64");
    }

    #[test]
    fn test_architecture_display_x64() {
        assert_eq!(format!("{}", Architecture::X64), "x64");
    }

    #[test]
    fn test_architecture_display_x86() {
        assert_eq!(format!("{}", Architecture::X86), "x86");
    }

    #[test]
    fn test_architecture_ordering() {
        // Arm64 < X64 < X86 (declaration order)
        assert!(Architecture::Arm64 < Architecture::X64);
        assert!(Architecture::X64 < Architecture::X86);
        assert_eq!(
            Architecture::X64.cmp(&Architecture::X64),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_architecture_partial_ordering() {
        assert_eq!(
            Architecture::X64.partial_cmp(&Architecture::X86),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            Architecture::X86.partial_cmp(&Architecture::X64),
            Some(std::cmp::Ordering::Greater)
        );
    }

    #[test]
    fn test_architecture_equality() {
        assert_eq!(Architecture::X64, Architecture::X64);
        assert_eq!(Architecture::X86, Architecture::X86);
        assert_eq!(Architecture::Arm64, Architecture::Arm64);
        assert_ne!(Architecture::X64, Architecture::Arm64);
    }

    #[test]
    fn test_architecture_clone() {
        let arch = Architecture::Arm64;
        assert_eq!(arch.clone(), Architecture::Arm64);
    }

    #[test]
    fn test_architecture_debug() {
        assert_eq!(format!("{:?}", Architecture::X64), "X64");
        assert_eq!(format!("{:?}", Architecture::X86), "X86");
        assert_eq!(format!("{:?}", Architecture::Arm64), "Arm64");
    }

    #[test]
    fn test_architecture_serialize() {
        assert_eq!(
            serde_json::to_string(&Architecture::X64).unwrap(),
            "\"x64\""
        );
        assert_eq!(
            serde_json::to_string(&Architecture::X86).unwrap(),
            "\"x86\""
        );
        assert_eq!(
            serde_json::to_string(&Architecture::Arm64).unwrap(),
            "\"arm64\""
        );
    }

    #[test]
    fn test_architecture_deserialize() {
        assert_eq!(
            serde_json::from_str::<Architecture>("\"x64\"").unwrap(),
            Architecture::X64
        );
        assert_eq!(
            serde_json::from_str::<Architecture>("\"x86\"").unwrap(),
            Architecture::X86
        );
        assert_eq!(
            serde_json::from_str::<Architecture>("\"arm64\"").unwrap(),
            Architecture::Arm64
        );
    }

    #[test]
    fn infer_from_path_detects_aarch64() {
        assert_eq!(
            Architecture::infer_from_path(Path::new("/nix/store/hash-R-4.4.1-aarch64/lib/R")),
            Architecture::Arm64
        );
    }

    #[test]
    fn infer_from_path_detects_x86_64() {
        assert_eq!(
            Architecture::infer_from_path(Path::new("/nix/store/hash-R-4.4.1-x86_64/lib/R")),
            Architecture::X64
        );
    }

    #[test]
    fn infer_from_path_detects_i386() {
        assert_eq!(
            Architecture::infer_from_path(Path::new("/opt/R/i386/bin/R")),
            Architecture::X86
        );
    }

    #[test]
    fn infer_from_path_falls_back_to_target_arch() {
        let arch = Architecture::infer_from_path(Path::new("/opt/R/bin/R"));
        // On an x86_64 host this will be X64, on aarch64 it will be Arm64
        if cfg!(target_arch = "aarch64") {
            assert_eq!(arch, Architecture::Arm64);
        } else if cfg!(target_arch = "x86_64") {
            assert_eq!(arch, Architecture::X64);
        } else {
            assert_eq!(arch, Architecture::X86);
        }
    }
}
