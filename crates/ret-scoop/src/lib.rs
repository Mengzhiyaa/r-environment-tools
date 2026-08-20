use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Scoop {
    install_roots: Vec<PathBuf>,
    manager: Option<EnvManager>,
}

impl Scoop {
    pub fn from(environment: &dyn Environment) -> Scoop {
        let install_roots = scoop_install_roots(environment);
        let manager = scoop_manager(environment);
        Scoop {
            install_roots,
            manager,
        }
    }
}

impl Locator for Scoop {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Scoop
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Scoop]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        let home = env.home.clone()?;
        if !looks_like_scoop_path(&env.executable)
            && !looks_like_scoop_path(&home)
            && !self
                .install_roots
                .iter()
                .any(|root| env.executable.starts_with(root) || home.starts_with(root))
        {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Scoop))
                .display_name(Some("Scoop R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .manager(self.manager.clone())
                .known_executables(env.known_executables.clone())
                .symlinks(env.symlinks.clone())
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(windows) {
            return;
        }

        for root in &self.install_roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };

            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path
                    .file_name()
                    .map(|name| name == "current")
                    .unwrap_or(false)
                    || path.is_dir()
                {
                    let Some(executable) = find_executable(&path) else {
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
    }
}

fn scoop_install_roots(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut roots = vec![];
    for base in scoop_base_roots(environment) {
        let apps = base.join("apps");
        roots.push(apps.join("r"));
        roots.push(apps.join("r-base"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn scoop_base_roots(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(root) = environment.get_env_var("SCOOP".to_string()) {
        roots.push(PathBuf::from(root));
    }
    if let Some(root) = environment.get_env_var("SCOOP_GLOBAL".to_string()) {
        roots.push(PathBuf::from(root));
    }
    if let Some(home) = environment.get_user_home() {
        roots.push(home.join("scoop"));

        let config_home = environment
            .get_env_var("XDG_CONFIG_HOME".to_string())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let config_path = config_home.join("scoop").join("config.json");
        if let Ok(content) = fs::read_to_string(config_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
                for key in ["root_path", "global_path"] {
                    if let Some(path) = config.get(key).and_then(|value| value.as_str()) {
                        roots.push(PathBuf::from(path));
                    }
                }
            }
        }
    }
    if let Some(program_data) = environment.get_env_var("ProgramData".to_string()) {
        roots.push(PathBuf::from(program_data).join("scoop"));
    }

    roots.sort();
    roots.dedup();
    roots
}

fn scoop_manager(environment: &dyn Environment) -> Option<EnvManager> {
    for root in scoop_base_roots(environment) {
        for name in ["scoop.cmd", "scoop.ps1", "scoop"] {
            let candidate = root.join("shims").join(name);
            if candidate.exists() {
                return Some(EnvManager::new(candidate, EnvManagerType::Scoop, None));
            }
        }
    }

    None
}

fn looks_like_scoop_path(path: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.contains("\\scoop\\apps\\r\\")
        || path.contains("\\scoop\\apps\\r-base\\")
        || path.contains("\\scoop\\shims\\r")
}

#[cfg(test)]
mod tests {
    use super::{looks_like_scoop_path, scoop_base_roots, scoop_install_roots};
    use ret_core::os_environment::Environment;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    struct TestEnvironment {
        home: PathBuf,
        variables: HashMap<String, String>,
    }

    impl Environment for TestEnvironment {
        fn get_user_home(&self) -> Option<PathBuf> {
            Some(self.home.clone())
        }

        fn get_root(&self) -> Option<PathBuf> {
            None
        }

        fn get_env_var(&self, key: String) -> Option<String> {
            self.variables.get(&key).cloned()
        }

        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    #[test]
    fn recognizes_scoop_paths() {
        assert!(looks_like_scoop_path(Path::new(
            r"C:\Users\me\scoop\apps\r\current\bin\R.exe"
        )));
        assert!(looks_like_scoop_path(Path::new(
            r"C:\Users\me\scoop\apps\r-base\4.4.1\bin\R.exe"
        )));
        assert!(!looks_like_scoop_path(Path::new(
            r"C:\Program Files\R\R-4.4.1\bin\R.exe"
        )));
    }

    #[test]
    fn includes_user_global_environment_and_configured_roots() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let config_home = temp.path().join("config");
        let config_dir = config_home.join("scoop");
        std::fs::create_dir_all(&config_dir).expect("failed to create config directory");
        std::fs::write(
            config_dir.join("config.json"),
            r#"{"root_path":"/configured/user","global_path":"/configured/global"}"#,
        )
        .expect("failed to write Scoop config");

        let environment = TestEnvironment {
            home: temp.path().join("home"),
            variables: HashMap::from([
                ("SCOOP".to_string(), "/env/user".to_string()),
                ("SCOOP_GLOBAL".to_string(), "/env/global".to_string()),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    config_home.to_string_lossy().into_owned(),
                ),
                ("ProgramData".to_string(), "/program-data".to_string()),
            ]),
        };

        let roots = scoop_base_roots(&environment);
        assert!(roots.contains(&PathBuf::from("/env/user")));
        assert!(roots.contains(&PathBuf::from("/env/global")));
        assert!(roots.contains(&PathBuf::from("/configured/user")));
        assert!(roots.contains(&PathBuf::from("/configured/global")));
        assert!(roots.contains(&PathBuf::from("/program-data/scoop")));

        let install_roots = scoop_install_roots(&environment);
        assert!(install_roots.contains(&PathBuf::from("/configured/global/apps/r")));
        assert!(install_roots.contains(&PathBuf::from("/env/user/apps/r-base")));
    }
}
