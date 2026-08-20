// HPC Environment Modules locator.
//
// Discovers R installations managed by the `module` command (Lmod / Environment Modules).
// On HPC clusters, R is typically available via:
//
//   module load R/4.3.0
//
// This locator runs `modulecmd sh -t avail R` (or `module -t avail R` via
// LMOD_CMD) to list available R modules, then resolves each to an R binary
// by temporarily loading the module and querying the resulting PATH.

use log::trace;
use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{LocatorMetadata, RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::env::ResolvedRInstallation;
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub struct EnvironmentModule {
    modulecmd: Option<PathBuf>,
    manager: Option<EnvManager>,
}

impl EnvironmentModule {
    pub fn from(environment: &dyn Environment) -> EnvironmentModule {
        let modulecmd = find_modulecmd(environment);
        let manager = modulecmd
            .as_ref()
            .map(|cmd| EnvManager::new(cmd.clone(), EnvManagerType::EnvironmentModule, None));
        EnvironmentModule { modulecmd, manager }
    }

    fn installation_from_env(&self, env: &REnv) -> Option<RInstallation> {
        env.version.as_ref()?;
        let home = env.home.clone()?;
        Some(
            RInstallationBuilder::new(Some(RInstallationKind::EnvironmentModule))
                .display_name(Some("Module R".to_string()))
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
}

impl Locator for EnvironmentModule {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::EnvironmentModule
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::EnvironmentModule]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if cfg!(windows) {
            return None;
        }
        env.version.as_ref()?;
        let home = env.home.clone()?;

        // Accept if the executable lives under a typical module tree.
        if !looks_like_module_path(&env.executable) && !looks_like_module_path(&home) {
            return None;
        }

        self.installation_from_env(env)
    }

    fn find(&self, reporter: &dyn Reporter) {
        if cfg!(windows) {
            return;
        }

        let Some(modulecmd) = &self.modulecmd else {
            trace!("modulecmd not found, skipping Environment Modules search");
            return;
        };

        // List available R modules.
        let r_modules = list_r_modules(modulecmd);
        if r_modules.is_empty() {
            trace!("No R modules found via modulecmd");
            return;
        }

        for module_name in &r_modules {
            // Resolve the R binary by loading the module in a subshell and
            // extracting the PATH it adds.
            if let Some(r_binary) = resolve_r_from_module(modulecmd, module_name) {
                if let Some(resolved) = ResolvedRInstallation::from(&r_binary) {
                    let env = resolved.to_r_env();
                    // A successfully loaded module is authoritative; module
                    // trees are not restricted to the common path names above.
                    if let Some(installation) = self.installation_from_env(&env) {
                        let installation = RInstallationBuilder::from_installation(installation)
                            .startup_command(Some(build_module_startup_command(
                                modulecmd,
                                module_name,
                            )))
                            .locator_metadata(Some(LocatorMetadata::Module {
                                module_name: module_name.clone(),
                                startup_command: build_module_startup_command(
                                    modulecmd,
                                    module_name,
                                ),
                            }))
                            .build();
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

pub fn build_module_startup_command(modulecmd: &Path, module_name: &str) -> String {
    format!("eval $({} sh load {})", modulecmd.display(), module_name)
}

/// Paths characteristic of Environment Modules / Lmod installations.
pub fn looks_like_module_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    // Common module installation trees
    s.contains("/modules/")
        || s.contains("/modulefiles/")
        || s.contains("/easybuild/")
        || s.contains("/lmod/")
        || s.contains("/spack/opt/") // spack+modules hybrid
        || s.contains("/apps/")      // common HPC /apps/ tree
        || s.contains("/software/") // EasyBuild default
}

/// Find the modulecmd / module command.
pub fn find_modulecmd(environment: &dyn Environment) -> Option<PathBuf> {
    // LMOD_CMD is set by Lmod when a module is loaded.
    if let Some(lmod_cmd) = environment
        .get_env_var("LMOD_CMD".to_string())
        .or_else(|| env::var("LMOD_CMD").ok())
    {
        let path = PathBuf::from(&lmod_cmd);
        if path.exists() {
            return Some(path);
        }
    }

    // MODULESHOME is set by both Lmod and classic Environment Modules.
    if let Some(modules_home) = environment
        .get_env_var("MODULESHOME".to_string())
        .or_else(|| env::var("MODULESHOME").ok())
    {
        let modulecmd = PathBuf::from(&modules_home).join("bin").join("modulecmd");
        if modulecmd.exists() {
            return Some(modulecmd);
        }
        let lmod = PathBuf::from(&modules_home).join("libexec").join("lmod");
        if lmod.exists() {
            return Some(lmod);
        }
    }

    // Try common paths.
    let candidates = [
        "/usr/share/lmod/lmod/libexec/lmod",
        "/usr/share/Modules/libexec/modulecmd.tcl",
        "/usr/local/Modules/libexec/modulecmd.tcl",
        "/opt/apps/lmod/lmod/libexec/lmod",
    ];
    for candidate in &candidates {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Some(path);
        }
    }

    // Try PATH.
    if let Ok(path_var) = env::var("PATH") {
        for dir in env::split_paths(&path_var) {
            for name in ["lmod", "modulecmd"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// List available R module names by running `modulecmd sh -t avail R`.
fn list_r_modules(modulecmd: &Path) -> Vec<String> {
    // Lmod and classic modulecmd output to stderr with -t flag.
    let output = Command::new(modulecmd)
        .args(["sh", "-t", "avail", "R"])
        .output();

    let Ok(output) = output else {
        trace!("Failed to execute modulecmd");
        return vec![];
    };

    // Both stdout and stderr may contain the listing depending on the tool.
    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).to_string()
    };

    parse_r_modules(&text)
}

fn parse_r_modules(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let name = line
                .split_whitespace()
                .next()?
                .trim_end_matches("(default)")
                .trim_end_matches("(D)");
            is_r_module_name(name).then(|| name.to_string())
        })
        .collect()
}

fn is_r_module_name(name: &str) -> bool {
    let components = name.split('/').collect::<Vec<_>>();
    components.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("r")
            && pair[1]
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
    }) || components.last().is_some_and(|component| {
        component
            .to_ascii_lowercase()
            .strip_prefix("r-")
            .and_then(|version| version.chars().next())
            .is_some_and(|character| character.is_ascii_digit())
    })
}

/// Resolve the R binary from loading a module.
///
/// Runs: `eval $(modulecmd sh load <module>) && which R`
pub fn resolve_r_from_module(modulecmd: &Path, module_name: &str) -> Option<PathBuf> {
    let script = format!(
        "eval $({} sh load {}) 2>/dev/null && which R",
        modulecmd.display(),
        module_name
    );

    let output = Command::new("sh").args(["-c", &script]).output().ok()?;

    if !output.status.success() {
        trace!("Failed to resolve R from module {}", module_name);
        return None;
    }

    let r_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if r_path.is_empty() {
        return None;
    }

    let path = PathBuf::from(&r_path);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::looks_like_module_path;
    #[cfg(unix)]
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    fn recognizes_module_paths() {
        assert!(looks_like_module_path(Path::new("/opt/apps/R/4.3.0/lib/R")));
        assert!(looks_like_module_path(Path::new(
            "/software/R/4.3.0-GCC-12.2.0/lib/R"
        )));
        assert!(looks_like_module_path(Path::new("/modules/R/4.3.0/bin/R")));
        assert!(!looks_like_module_path(Path::new("/usr/bin/R")));
        assert!(!looks_like_module_path(Path::new("/opt/R/4.3.0/bin/R")));
    }

    #[test]
    fn parses_nested_r_module_names() {
        let modules = super::parse_r_modules(
            "R/4.4.1 (default)\nlang/R/4.3.3 (D)\ncompiler/R/4.2\nr-4.1\nr-base/4.4\nr-ggplot2/3.5\nrstudio/2024",
        );

        assert_eq!(
            modules,
            vec!["R/4.4.1", "lang/R/4.3.3", "compiler/R/4.2", "r-4.1"]
        );
    }

    #[test]
    fn loaded_modules_accept_custom_install_paths() {
        let locator = super::EnvironmentModule {
            modulecmd: None,
            manager: None,
        };
        let env = ret_core::env::REnv::new(
            std::path::PathBuf::from("/cluster/toolchains/r/4.4/bin/R"),
            Some(std::path::PathBuf::from("/cluster/toolchains/r/4.4/lib/R")),
            Some("4.4.1".to_string()),
        );

        let installation = locator
            .installation_from_env(&env)
            .expect("loaded module path should be authoritative");
        assert_eq!(
            installation.kind,
            Some(ret_core::r_installation::RInstallationKind::EnvironmentModule)
        );
    }
}
