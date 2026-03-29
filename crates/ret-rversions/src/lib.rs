// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use log::{debug, warn};
use ret_core::{
    env::REnv,
    os_environment::Environment,
    r_installation::{
        DiscoverySource, RInstallation, RInstallationBuilder, RInstallationKind, RVersionsOverlay,
    },
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_module::{
    build_module_startup_command, find_modulecmd, looks_like_module_path, resolve_r_from_module,
};
use ret_r_utils::env::ResolvedRInstallation;
use std::{
    collections::HashMap,
    env, mem,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RVersionsEntry {
    pub path: Option<PathBuf>,
    pub label: Option<String>,
    pub module: Option<String>,
    pub script: Option<String>,
    pub repo: Option<String>,
    pub library: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedRVersionsEntry {
    resolved: ResolvedRInstallation,
    label: Option<String>,
    startup_command: Option<String>,
    environment_variables: Option<HashMap<String, String>>,
    rversions_overlay: Option<RVersionsOverlay>,
}

pub struct RVersions {
    user_home: Option<PathBuf>,
    modulecmd: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    xdg_config_dirs: Vec<PathBuf>,
}

impl RVersions {
    pub fn from(environment: &dyn Environment) -> Self {
        let xdg_config_home = environment
            .get_env_var("XDG_CONFIG_HOME".to_string())
            .map(PathBuf::from);
        let xdg_config_dirs = environment
            .get_env_var("XDG_CONFIG_DIRS".to_string())
            .map(|value| env::split_paths(&value).collect())
            .unwrap_or_default();

        Self {
            user_home: environment.get_user_home(),
            modulecmd: find_modulecmd(environment),
            xdg_config_home,
            xdg_config_dirs,
        }
    }

    fn load_entries(&self) -> Vec<ResolvedRVersionsEntry> {
        if cfg!(windows) {
            return vec![];
        }

        let Some(file) = find_rversions_file(
            self.user_home.as_deref(),
            self.xdg_config_home.as_deref(),
            &self.xdg_config_dirs,
        ) else {
            return vec![];
        };

        let content = match std::fs::read_to_string(&file) {
            Ok(content) => content,
            Err(error) => {
                warn!("Failed to read r-versions file {}: {error}", file.display());
                return vec![];
            }
        };

        parse_rversions_file(&content)
            .into_iter()
            .filter_map(|entry| self.resolve_entry(entry))
            .collect()
    }

    fn resolve_entry(&self, entry: RVersionsEntry) -> Option<ResolvedRVersionsEntry> {
        let module_startup_command = entry.module.as_ref().and_then(|module| {
            self.modulecmd
                .as_ref()
                .map(|modulecmd| build_module_startup_command(modulecmd, module))
        });
        let rversions_overlay = build_rversions_overlay(&entry, module_startup_command.clone());

        let executable = if let Some(home) = &entry.path {
            let executable = home.join("bin").join("R");
            if !executable.is_file() {
                warn!(
                    "Skipping r-versions entry because {} does not exist",
                    executable.display()
                );
                return None;
            }
            executable
        } else if let (Some(modulecmd), Some(module_name)) = (&self.modulecmd, &entry.module) {
            resolve_r_from_module(modulecmd, module_name)?
        } else {
            debug!("Skipping r-versions entry without a resolvable executable");
            return None;
        };

        let resolved = ResolvedRInstallation::from(&executable)?;
        let startup_command =
            combine_startup_command(module_startup_command.as_deref(), entry.script.as_deref());
        let environment_variables = entry
            .library
            .as_ref()
            .map(|library| HashMap::from([(String::from("R_LIBS"), library.clone())]));

        Some(ResolvedRVersionsEntry {
            resolved,
            label: entry.label,
            startup_command,
            environment_variables,
            rversions_overlay,
        })
    }
}

impl Locator for RVersions {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::LinuxGlobal
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![
            RInstallationKind::Conda,
            RInstallationKind::EnvironmentModule,
            RInstallationKind::Guix,
            RInstallationKind::Homebrew,
            RInstallationKind::LinuxGlobal,
            RInstallationKind::MacFramework,
            RInstallationKind::MacPorts,
            RInstallationKind::Nix,
            RInstallationKind::Pixi,
            RInstallationKind::Rig,
            RInstallationKind::Spack,
        ]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if cfg!(windows) {
            return None;
        }

        let matched = self.load_entries().into_iter().find(|entry| {
            entry.resolved.executable == env.executable
                || env.home.as_ref() == Some(&entry.resolved.home)
        })?;

        Some(build_installation_from_env(env, &matched))
    }

    fn find(&self, reporter: &dyn Reporter) {
        if cfg!(windows) {
            return;
        }

        for entry in self.load_entries() {
            let installation = build_installation_from_resolved(&entry.resolved, &entry);
            entry.resolved.add_to_cache(installation.clone());
            reporter.report_installation(&installation);
        }
    }
}

pub fn parse_rversions_file(content: &str) -> Vec<RVersionsEntry> {
    let mut entries = Vec::new();
    let mut current = RVersionsEntry::default();

    for line in content.lines().chain(std::iter::once("")) {
        let trimmed_line = line.trim();

        if trimmed_line.is_empty() {
            finish_entry(&mut current, &mut entries);
            continue;
        }

        if trimmed_line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = trimmed_line.split_once(':') else {
            warn!("Invalid line in r-versions file (no colon): {trimmed_line}");
            continue;
        };

        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "path" => current.path = Some(PathBuf::from(value)),
            "label" => current.label = Some(value.to_string()),
            "module" => current.module = Some(value.to_string()),
            "script" => current.script = Some(value.to_string()),
            "repo" => current.repo = Some(value.to_string()),
            "library" => current.library = Some(value.to_string()),
            other => debug!("Ignoring unknown r-versions key {other}"),
        }
    }

    entries
}

pub fn find_rversions_file(
    user_home: Option<&Path>,
    xdg_config_home: Option<&Path>,
    xdg_config_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let mut config_dirs = Vec::new();

    if let Some(config_home) = xdg_config_home {
        config_dirs.push(config_home.to_path_buf());
    } else if let Some(home) = user_home {
        config_dirs.push(home.join(".config"));
    }

    if xdg_config_dirs.is_empty() {
        config_dirs.push(PathBuf::from("/etc/xdg"));
    } else {
        config_dirs.extend(xdg_config_dirs.iter().cloned());
    }

    config_dirs.push(PathBuf::from("/etc"));

    let mut unique_config_dirs = Vec::new();
    for config_dir in config_dirs {
        if !unique_config_dirs.iter().any(|seen| seen == &config_dir) {
            unique_config_dirs.push(config_dir);
        }
    }

    unique_config_dirs
        .into_iter()
        .map(|config_dir| config_dir.join("rstudio").join("r-versions"))
        .find(|path| path.is_file())
}

fn build_installation_from_env(env: &REnv, entry: &ResolvedRVersionsEntry) -> RInstallation {
    let home = env
        .home
        .clone()
        .unwrap_or_else(|| entry.resolved.home.clone());
    let kind = infer_kind(&env.executable, &home);

    RInstallationBuilder::new(Some(kind))
        .display_name(entry.label.clone())
        .executable(Some(env.executable.clone()))
        .home(Some(home))
        .version(
            env.version
                .clone()
                .or_else(|| Some(entry.resolved.version.clone())),
        )
        .arch(
            env.arch
                .clone()
                .or_else(|| Some(entry.resolved.arch.clone())),
        )
        .known_executables(merge_optional_paths(
            env.known_executables.as_ref(),
            entry.resolved.known_executables.as_ref(),
        ))
        .symlinks(merge_optional_paths(
            env.symlinks.as_ref(),
            entry.resolved.symlinks.as_ref(),
        ))
        .discovered_by(vec![DiscoverySource::Locator, DiscoverySource::RVersions])
        .rversions_overlay(entry.rversions_overlay.clone())
        .startup_command(entry.startup_command.clone())
        .environment_variables(entry.environment_variables.clone())
        .build()
}

fn build_installation_from_resolved(
    resolved: &ResolvedRInstallation,
    entry: &ResolvedRVersionsEntry,
) -> RInstallation {
    let kind = infer_kind(&resolved.executable, &resolved.home);

    RInstallationBuilder::new(Some(kind))
        .display_name(entry.label.clone())
        .executable(Some(resolved.executable.clone()))
        .home(Some(resolved.home.clone()))
        .version(Some(resolved.version.clone()))
        .arch(Some(resolved.arch.clone()))
        .known_executables(resolved.known_executables.clone())
        .symlinks(resolved.symlinks.clone())
        .discovered_by(vec![DiscoverySource::Locator, DiscoverySource::RVersions])
        .rversions_overlay(entry.rversions_overlay.clone())
        .startup_command(entry.startup_command.clone())
        .environment_variables(entry.environment_variables.clone())
        .build()
}

fn build_rversions_overlay(
    entry: &RVersionsEntry,
    module_startup_command: Option<String>,
) -> Option<RVersionsOverlay> {
    let overlay = RVersionsOverlay {
        label: entry.label.clone(),
        script: entry.script.clone(),
        repo: entry.repo.clone(),
        library: entry.library.clone(),
        module: entry.module.clone(),
        module_startup_command,
    };

    if overlay.label.is_some()
        || overlay.script.is_some()
        || overlay.repo.is_some()
        || overlay.library.is_some()
        || overlay.module.is_some()
        || overlay.module_startup_command.is_some()
    {
        Some(overlay)
    } else {
        None
    }
}

fn finish_entry(current: &mut RVersionsEntry, entries: &mut Vec<RVersionsEntry>) {
    if current.path.is_some() || current.module.is_some() {
        entries.push(mem::take(current));
    } else if has_any_fields(current) {
        warn!("Skipping r-versions entry without Path or Module");
        *current = RVersionsEntry::default();
    }
}

fn has_any_fields(entry: &RVersionsEntry) -> bool {
    entry.path.is_some()
        || entry.label.is_some()
        || entry.module.is_some()
        || entry.script.is_some()
        || entry.repo.is_some()
        || entry.library.is_some()
}

fn infer_kind(executable: &Path, home: &Path) -> RInstallationKind {
    if looks_like_module_path(executable) || looks_like_module_path(home) {
        RInstallationKind::EnvironmentModule
    } else {
        RInstallationKind::LinuxGlobal
    }
}

fn merge_optional_paths(
    primary: Option<&Vec<PathBuf>>,
    secondary: Option<&Vec<PathBuf>>,
) -> Option<Vec<PathBuf>> {
    let mut merged = Vec::new();
    if let Some(primary) = primary {
        merged.extend(primary.iter().cloned());
    }
    if let Some(secondary) = secondary {
        merged.extend(secondary.iter().cloned());
    }
    (!merged.is_empty()).then_some(merged)
}

fn combine_startup_command(
    module_startup_command: Option<&str>,
    script: Option<&str>,
) -> Option<String> {
    match (module_startup_command, script) {
        (Some(module_startup_command), Some(script)) => {
            Some(format!("{module_startup_command} && . {script}"))
        }
        (Some(module_startup_command), None) => Some(module_startup_command.to_string()),
        (None, Some(script)) => Some(format!(". {script}")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{find_rversions_file, parse_rversions_file};
    use std::{fs, path::PathBuf};

    #[test]
    fn parses_path_only_entries() {
        let entries = parse_rversions_file("Path: /opt/R/4.3.0/lib/R");

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path.as_ref().unwrap().to_string_lossy(),
            "/opt/R/4.3.0/lib/R"
        );
        assert_eq!(entries[0].label, None);
    }

    #[test]
    fn parses_multiple_entries_with_multiple_blank_lines() {
        let entries = parse_rversions_file(
            "Path: /opt/R/4.3.0/lib/R\n\n\nPath: /opt/R/4.4.0/lib/R\nLabel: Production\n",
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].path.as_ref().unwrap().to_string_lossy(),
            "/opt/R/4.4.0/lib/R"
        );
        assert_eq!(entries[1].label.as_deref(), Some("Production"));
    }

    #[test]
    fn parses_module_only_entries() {
        let entries = parse_rversions_file("Module: R/4.3.0\nLabel: Production");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].module.as_deref(), Some("R/4.3.0"));
        assert_eq!(entries[0].label.as_deref(), Some("Production"));
    }

    #[test]
    fn parses_all_supported_fields() {
        let entries = parse_rversions_file(
            "Path: /opt/R/4.3.0/lib/R\nLabel: Production\nModule: R/4.3.0\nScript: /opt/load.sh\nRepo: https://ppm.example.com/cran/latest\nLibrary: /opt/R-libs",
        );

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.label.as_deref(), Some("Production"));
        assert_eq!(entry.module.as_deref(), Some("R/4.3.0"));
        assert_eq!(entry.script.as_deref(), Some("/opt/load.sh"));
        assert_eq!(
            entry.repo.as_deref(),
            Some("https://ppm.example.com/cran/latest")
        );
        assert_eq!(entry.library.as_deref(), Some("/opt/R-libs"));
    }

    #[test]
    fn trims_whitespace_and_keeps_values_with_colons() {
        let entries = parse_rversions_file(
            "  Path  :   /opt/R/4.3.0/lib/R\n  Library : /path/one:/path/two:/path/three  ",
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path.as_ref().unwrap().to_string_lossy(),
            "/opt/R/4.3.0/lib/R"
        );
        assert_eq!(
            entries[0].library.as_deref(),
            Some("/path/one:/path/two:/path/three")
        );
    }

    #[test]
    fn skips_entries_without_path_or_module() {
        let entries = parse_rversions_file("Label: orphan\n\nPath: /opt/R/4.3.0/lib/R");

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path.as_ref().unwrap().to_string_lossy(),
            "/opt/R/4.3.0/lib/R"
        );
    }

    #[test]
    fn ignores_comments_and_invalid_lines() {
        let entries = parse_rversions_file(
            "# comment\nPath: /opt/R/4.3.0/lib/R\nThis line has no colon\n\n# another\nModule: R/4.4.0",
        );

        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn finds_rversions_file_using_xdg_search_order() {
        let tmp = tempfile::tempdir().expect("failed to create tempdir");
        let home = tmp.path().join("home");
        let xdg_config_home = tmp.path().join("xdg-home");
        let xdg_config_dir = tmp.path().join("xdg-dir");

        fs::create_dir_all(xdg_config_dir.join("rstudio"))
            .expect("failed to create xdg config dir");
        fs::create_dir_all(home.join(".config").join("rstudio"))
            .expect("failed to create home config dir");

        let expected = xdg_config_home.join("rstudio").join("r-versions");
        fs::create_dir_all(expected.parent().expect("missing parent"))
            .expect("failed to create xdg config home dir");
        fs::write(&expected, "Path: /opt/R/4.4.1/lib/R").expect("failed to write r-versions file");

        let found = find_rversions_file(
            Some(&home),
            Some(&xdg_config_home),
            &[PathBuf::from(&xdg_config_dir)],
        );

        assert_eq!(found, Some(expected));
    }
}
