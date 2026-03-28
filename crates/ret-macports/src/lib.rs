use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct MacPorts {
    manager: Option<EnvManager>,
}

impl MacPorts {
    pub fn new() -> MacPorts {
        MacPorts {
            manager: find_port_manager(),
        }
    }
}

impl Default for MacPorts {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for MacPorts {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::MacPorts
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::MacPorts]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if std::env::consts::OS != "macos" {
            return None;
        }

        env.version.as_ref()?;

        let home = env.home.clone()?;
        if !looks_like_macports_path(&env.executable) && !looks_like_macports_path(&home) {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::MacPorts))
                .display_name(Some("MacPorts R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .manager(self.manager.clone())
                .symlinks(env.symlinks.clone())
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if std::env::consts::OS != "macos" {
            return;
        }

        for install_root in candidate_install_roots() {
            let executable = find_executable(&install_root).or_else(|| {
                let direct = install_root.join("bin").join("R");
                if direct.exists() {
                    Some(direct)
                } else {
                    None
                }
            });
            let Some(executable) = executable else {
                continue;
            };

            if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                let env = resolved.to_r_env();
                if let Some(installation) = self.try_from(&env) {
                    resolved.add_to_cache(installation.clone());
                    if let Some(manager) = &installation.manager {
                        reporter.report_manager(manager);
                    }
                    reporter.report_installation(&installation);
                }
            }
        }
    }
}

fn looks_like_macports_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.starts_with("/opt/local/lib/R")
        || path.starts_with("/opt/local/bin/")
        || path.starts_with("/opt/local/Library/Frameworks/R.framework/Versions/")
}

fn find_port_manager() -> Option<EnvManager> {
    let manager = PathBuf::from("/opt/local/bin/port");
    if manager.exists() {
        Some(EnvManager::new(manager, EnvManagerType::MacPorts, None))
    } else {
        None
    }
}

fn candidate_install_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/opt/local"),
        PathBuf::from("/opt/local/lib/R"),
    ];

    let framework_versions = PathBuf::from("/opt/local/Library/Frameworks/R.framework/Versions");
    if let Ok(entries) = fs::read_dir(&framework_versions) {
        for entry in entries.filter_map(Result::ok) {
            let version_dir = entry.path();
            if version_dir.ends_with("Current") {
                continue;
            }
            roots.push(version_dir.join("Resources"));
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::looks_like_macports_path;
    use std::path::Path;

    #[test]
    fn recognizes_macports_paths() {
        assert!(looks_like_macports_path(Path::new("/opt/local/lib/R")));
        assert!(looks_like_macports_path(Path::new(
            "/opt/local/Library/Frameworks/R.framework/Versions/4.4/Resources"
        )));
        assert!(!looks_like_macports_path(Path::new("/usr/local/lib/R")));
    }
}
