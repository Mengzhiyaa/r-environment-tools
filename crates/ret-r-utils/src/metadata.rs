// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::arch::Architecture;
use ret_fs::path::norm_case;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(crate) struct RMetadata {
    pub version: String,
    pub arch: Architecture,
    pub files: Vec<PathBuf>,
}

pub(crate) enum LauncherHome {
    Literal(PathBuf),
    Query,
}

/// Read only the literal home in a standard R launcher. Dynamic shell logic is
/// left to `R RHOME`, rather than evaluated or approximated here.
pub(crate) fn launcher_home(executable: &Path) -> Option<LauncherHome> {
    let executable = if cfg!(windows) {
        norm_case(executable)
    } else {
        fs::canonicalize(executable).ok()?
    };
    let is_rscript = executable
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with("rscript");
    if is_rscript {
        if let Some(home) = std::env::var_os("RHOME").filter(|home| !home.is_empty()) {
            return Some(LauncherHome::Literal(home.into()));
        }
    }
    if cfg!(windows) {
        return None;
    }
    let sibling_r = executable.with_file_name("R");
    let launcher = if is_rscript && sibling_r.is_file() {
        &sibling_r
    } else {
        &executable
    };
    let mut file = File::open(launcher).ok()?;
    let mut magic = [0; 2];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"#!" {
        return None;
    }
    let mut script = String::new();
    file.read_to_string(&mut script).ok()?;
    if !script.contains("Shell wrapper for R executable") {
        return Some(LauncherHome::Query);
    }
    let mut assignments = script
        .lines()
        .filter_map(|line| line.trim().strip_prefix("R_HOME_DIR="));
    let Some(value) = assignments.next() else {
        return Some(LauncherHome::Query);
    };
    let value = value.trim();
    // Linux's standard launcher can choose lib or lib64 at runtime.
    if !cfg!(target_os = "macos") && assignments.next().is_some() {
        return Some(LauncherHome::Query);
    }
    let value = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(value);
    if value.contains('$') || value.contains('`') {
        return Some(LauncherHome::Query);
    }
    let home = PathBuf::from(value);
    Some(if home.is_absolute() {
        LauncherHome::Literal(home)
    } else {
        LauncherHome::Query
    })
}

pub(crate) fn orthogonal(home: &Path) -> Option<bool> {
    let home = home.to_string_lossy();
    home.contains("R.framework").then(|| {
        home.contains("R.framework/Versions/") && !home.contains("R.framework/Versions/Current/")
    })
}

pub(crate) fn has_r_layout(home: &Path) -> bool {
    home.join("library")
        .join("base")
        .join("DESCRIPTION")
        .is_file()
        || home
            .join("library")
            .join("utils")
            .join("DESCRIPTION")
            .is_file()
}

pub(crate) fn layout_home(executable: &Path) -> Option<PathBuf> {
    let executable = if cfg!(windows) {
        norm_case(executable)
    } else {
        fs::canonicalize(executable).ok()?
    };
    let parent = executable.parent()?;
    let bin = if ["x64", "i386", "aarch64", "arm64"]
        .iter()
        .any(|arch| parent.ends_with(arch))
    {
        parent.parent()?
    } else {
        parent
    };
    if !bin.ends_with("bin") {
        return None;
    }
    let prefix = bin.parent()?;
    [
        prefix.to_path_buf(),
        prefix.join("lib").join("R"),
        prefix.join("lib64").join("R"),
    ]
    .into_iter()
    .find(|home| has_r_layout(home))
}

pub(crate) fn read_metadata(home: &Path, executable: &Path) -> Option<RMetadata> {
    let description = ["utils", "base"]
        .into_iter()
        .map(|package| home.join("library").join(package).join("DESCRIPTION"))
        .find(|path| path.is_file())?;
    let content = fs::read_to_string(&description).ok()?;
    let built = content
        .lines()
        .find_map(|line| line.strip_prefix("Built:"))?;
    let mut fields = built.split(';').map(str::trim);
    let version = fields.next()?.strip_prefix("R ")?.trim().to_string();
    let platform = fields.next().unwrap_or_default();
    if !version.chars().next()?.is_ascii_digit() {
        return None;
    }

    let mut binaries = Vec::new();
    if cfg!(windows) {
        let bin = home.join("bin");
        let parent = executable.parent()?;
        if ["x64", "i386", "aarch64", "arm64"]
            .iter()
            .any(|arch| parent.ends_with(arch))
        {
            binaries.push(parent.join("R.dll"));
            binaries.push(executable.to_path_buf());
        } else {
            binaries.extend([bin.join("x64").join("R.dll"), bin.join("R.dll")]);
        }
    } else {
        let arch = std::env::var("R_ARCH").unwrap_or_default();
        let exec = home.join("bin").join("exec");
        binaries.push(exec.join(arch.trim_start_matches('/')).join("R"));
    }
    let detected = binaries
        .iter()
        .find_map(|binary| binary_architecture(binary).map(|arch| (arch, binary.clone())));
    let mut files = vec![description];
    let arch = if let Some((arch, binary)) = detected {
        files.push(binary);
        arch
    } else {
        architecture(platform)?
    };
    Some(RMetadata {
        version,
        arch,
        files,
    })
}

pub(crate) fn architecture(platform: &str) -> Option<Architecture> {
    let arch = platform.split('-').next()?.to_ascii_lowercase();
    Architecture::from_serialized_str(&arch)
}

fn binary_architecture(path: &Path) -> Option<Architecture> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header).ok()?;
    if &header[..4] == b"\x7fELF" {
        let machine = if header[5] == 2 {
            u16::from_be_bytes([header[18], header[19]])
        } else {
            u16::from_le_bytes([header[18], header[19]])
        };
        return match machine {
            3 => Some(Architecture::X86),
            62 => Some(Architecture::X64),
            183 => Some(Architecture::Arm64),
            _ => None,
        };
    }
    // Thin Mach-O headers cover CRAN and conda-forge macOS installations.
    let cpu = match &header[..4] {
        b"\xcf\xfa\xed\xfe" | b"\xce\xfa\xed\xfe" => {
            Some(u32::from_le_bytes(header[4..8].try_into().ok()?))
        }
        b"\xfe\xed\xfa\xcf" | b"\xfe\xed\xfa\xce" => {
            Some(u32::from_be_bytes(header[4..8].try_into().ok()?))
        }
        _ => None,
    };
    if let Some(cpu) = cpu {
        return match cpu {
            7 => Some(Architecture::X86),
            0x01000007 => Some(Architecture::X64),
            0x0100000c => Some(Architecture::Arm64),
            _ => None,
        };
    }
    if &header[..2] == b"MZ" {
        file.seek(SeekFrom::Start(0x3c)).ok()?;
        let mut offset = [0; 4];
        file.read_exact(&mut offset).ok()?;
        file.seek(SeekFrom::Start(u32::from_le_bytes(offset).into()))
            .ok()?;
        let mut pe = [0; 6];
        file.read_exact(&mut pe).ok()?;
        if &pe[..4] != b"PE\0\0" {
            return None;
        }
        return match u16::from_le_bytes([pe[4], pe[5]]) {
            0x014c => Some(Architecture::X86),
            0x8664 => Some(Architecture::X64),
            0xaa64 => Some(Architecture::Arm64),
            _ => None,
        };
    }
    None
}
