// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
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

impl Architecture {
    pub fn as_serialized_str(&self) -> &'static str {
        match self {
            Architecture::Arm64 => "arm64",
            Architecture::X64 => "x86_64",
            Architecture::X86 => "x86",
        }
    }

    pub fn from_serialized_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "arm64" | "aarch64" => Some(Architecture::Arm64),
            "x64" | "x86_64" | "amd64" => Some(Architecture::X64),
            "x86" | "i386" | "i686" => Some(Architecture::X86),
            _ => None,
        }
    }
}

impl Serialize for Architecture {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_serialized_str())
    }
}

impl<'de> Deserialize<'de> for Architecture {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Architecture::from_serialized_str(&value).ok_or_else(|| {
            serde::de::Error::unknown_variant(
                &value,
                &[
                    "arm64", "aarch64", "x86_64", "x64", "amd64", "x86", "i386", "i686",
                ],
            )
        })
    }
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.as_serialized_str())
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
        assert_eq!(format!("{}", Architecture::X64), "x86_64");
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
            "\"x86_64\""
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
            serde_json::from_str::<Architecture>("\"x86_64\"").unwrap(),
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
        assert_eq!(
            serde_json::from_str::<Architecture>("\"aarch64\"").unwrap(),
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
