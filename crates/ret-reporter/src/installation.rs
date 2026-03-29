use log::error;
use ret_core::r_installation::{
    LocatorMetadata, RInstallation, RInstallationBuilder, RVersionsOverlay,
};
use std::{collections::HashMap, path::PathBuf};

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

pub fn merge_installations(existing: &RInstallation, new: &RInstallation) -> RInstallation {
    let (primary, secondary) = preferred_installation(existing, new);

    let mut discovered_by = existing.discovered_by.clone();
    discovered_by.extend(new.discovered_by.iter().copied());
    discovered_by.sort();
    discovered_by.dedup();

    let mut symlinks = existing.symlinks.clone().unwrap_or_default();
    symlinks.extend(new.symlinks.clone().unwrap_or_default());
    symlinks.sort();
    symlinks.dedup();

    let mut known_executables = existing.known_executables.clone().unwrap_or_default();
    known_executables.extend(new.known_executables.clone().unwrap_or_default());
    known_executables.sort();
    known_executables.dedup();

    let locator_metadata = select_locator_metadata(
        existing.locator_metadata.as_ref(),
        new.locator_metadata.as_ref(),
    );
    let rversions_overlay = merge_rversions_overlay(
        existing.rversions_overlay.as_ref(),
        new.rversions_overlay.as_ref(),
    );
    let environment_variables = merge_environment_variables(
        primary.environment_variables.as_ref(),
        secondary.environment_variables.as_ref(),
    );
    let display_name = rversions_overlay
        .as_ref()
        .and_then(|overlay| overlay.label.clone())
        .or_else(|| primary.display_name.clone())
        .or_else(|| secondary.display_name.clone());

    RInstallationBuilder::from_installation(primary.clone())
        .display_name(display_name)
        .name(primary.name.clone().or_else(|| secondary.name.clone()))
        .executable(
            primary
                .executable
                .clone()
                .or_else(|| secondary.executable.clone()),
        )
        .kind(primary.kind.or(secondary.kind))
        .version(
            primary
                .version
                .clone()
                .or_else(|| secondary.version.clone()),
        )
        .home(primary.home.clone().or_else(|| secondary.home.clone()))
        .manager(
            primary
                .manager
                .clone()
                .or_else(|| secondary.manager.clone()),
        )
        .arch(primary.arch.clone().or_else(|| secondary.arch.clone()))
        .known_executables(if known_executables.is_empty() {
            None
        } else {
            Some(known_executables)
        })
        .symlinks(if symlinks.is_empty() {
            None
        } else {
            Some(symlinks)
        })
        .discovered_by(discovered_by)
        .locator_metadata(locator_metadata)
        .rversions_overlay(rversions_overlay)
        .script_path(
            primary
                .script_path
                .clone()
                .or_else(|| secondary.script_path.clone()),
        )
        .startup_command(
            primary
                .startup_command
                .clone()
                .or_else(|| secondary.startup_command.clone()),
        )
        .environment_variables(environment_variables)
        .orthogonal(primary.orthogonal.or(secondary.orthogonal))
        .error(primary.error.clone().or_else(|| secondary.error.clone()))
        .build()
}

fn preferred_installation<'a>(
    existing: &'a RInstallation,
    new: &'a RInstallation,
) -> (&'a RInstallation, &'a RInstallation) {
    if installation_priority(new) > installation_priority(existing)
        || (installation_priority(new) == installation_priority(existing)
            && kind_specificity(new) > kind_specificity(existing))
        || (installation_priority(new) == installation_priority(existing)
            && kind_specificity(new) == kind_specificity(existing)
            && installation_completeness(new) > installation_completeness(existing))
    {
        (new, existing)
    } else {
        (existing, new)
    }
}

fn installation_priority(installation: &RInstallation) -> u8 {
    installation
        .locator_metadata
        .as_ref()
        .map(LocatorMetadata::priority)
        .unwrap_or(0)
}

fn kind_specificity(installation: &RInstallation) -> u8 {
    match installation.kind {
        Some(ret_core::r_installation::RInstallationKind::GlobalPaths) => 1,
        Some(ret_core::r_installation::RInstallationKind::LinuxGlobal) => 2,
        Some(ret_core::r_installation::RInstallationKind::EnvironmentModule) => 3,
        Some(_) => 4,
        None => 0,
    }
}

fn installation_completeness(installation: &RInstallation) -> u8 {
    [
        installation.display_name.is_some(),
        installation.name.is_some(),
        installation.executable.is_some(),
        installation.kind.is_some(),
        installation.version.is_some(),
        installation.home.is_some(),
        installation.manager.is_some(),
        installation.arch.is_some(),
        installation.known_executables.is_some(),
        installation.symlinks.is_some(),
        installation.locator_metadata.is_some(),
        installation.rversions_overlay.is_some(),
        installation.script_path.is_some(),
        installation.startup_command.is_some(),
        installation.environment_variables.is_some(),
        installation.orthogonal.is_some(),
        installation.error.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count() as u8
}

fn merge_environment_variables(
    primary: Option<&HashMap<String, String>>,
    secondary: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    match (primary, secondary) {
        (None, None) => None,
        (Some(primary), None) => Some(primary.clone()),
        (None, Some(secondary)) => Some(secondary.clone()),
        (Some(primary), Some(secondary)) => {
            let mut merged = secondary.clone();
            merged.extend(primary.clone());
            Some(merged)
        }
    }
}

fn merge_rversions_overlay(
    existing: Option<&RVersionsOverlay>,
    new: Option<&RVersionsOverlay>,
) -> Option<RVersionsOverlay> {
    match (existing, new) {
        (None, None) => None,
        (Some(existing), None) => Some(existing.clone()),
        (None, Some(new)) => Some(new.clone()),
        (Some(existing), Some(new)) => Some(RVersionsOverlay {
            label: existing.label.clone().or_else(|| new.label.clone()),
            script: existing.script.clone().or_else(|| new.script.clone()),
            repo: existing.repo.clone().or_else(|| new.repo.clone()),
            library: existing.library.clone().or_else(|| new.library.clone()),
            module: existing.module.clone().or_else(|| new.module.clone()),
            module_startup_command: existing
                .module_startup_command
                .clone()
                .or_else(|| new.module_startup_command.clone()),
        }),
    }
}

pub fn select_locator_metadata(
    existing: Option<&LocatorMetadata>,
    new: Option<&LocatorMetadata>,
) -> Option<LocatorMetadata> {
    match (existing, new) {
        (Some(existing), Some(new)) => {
            if new.priority() > existing.priority() {
                Some(new.clone())
            } else {
                Some(existing.clone())
            }
        }
        (Some(existing), None) => Some(existing.clone()),
        (None, Some(new)) => Some(new.clone()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::merge_installations;
    use ret_core::r_installation::{
        DiscoverySource, LocatorMetadata, RInstallationBuilder, RInstallationKind, RVersionsOverlay,
    };
    use std::{collections::HashMap, path::PathBuf};

    #[test]
    fn merge_installations_unions_sources_and_prefers_higher_priority_metadata() {
        let existing = RInstallationBuilder::new(Some(RInstallationKind::Conda))
            .executable(Some(PathBuf::from("/usr/bin/R")))
            .known_executables(Some(vec![PathBuf::from("/usr/bin/R")]))
            .discovered_by(vec![DiscoverySource::Locator])
            .locator_metadata(Some(LocatorMetadata::Conda {
                environment_path: PathBuf::from("/tmp/conda"),
            }))
            .symlinks(Some(vec![PathBuf::from("/usr/bin/R")]))
            .build();
        let merged = merge_installations(
            &existing,
            &RInstallationBuilder::new(Some(RInstallationKind::Pixi))
                .executable(Some(PathBuf::from("/usr/local/bin/R")))
                .known_executables(Some(vec![PathBuf::from("/usr/local/bin/Rscript")]))
                .discovered_by(vec![DiscoverySource::ExplicitSearch])
                .locator_metadata(Some(LocatorMetadata::Pixi {
                    environment_path: PathBuf::from("/tmp/pixi"),
                    manifest_path: None,
                    environment_name: Some("default".to_string()),
                }))
                .symlinks(Some(vec![PathBuf::from("/usr/local/bin/R")]))
                .build(),
        );

        assert_eq!(
            merged.discovered_by,
            vec![DiscoverySource::Locator, DiscoverySource::ExplicitSearch]
        );
        assert_eq!(merged.known_executables.unwrap().len(), 3);
        assert_eq!(merged.symlinks.unwrap().len(), 2);
        assert!(matches!(
            merged.locator_metadata,
            Some(LocatorMetadata::Pixi { .. })
        ));
    }

    #[test]
    fn merge_installations_keeps_rversions_overlay_and_prefers_overlay_label() {
        let mut existing_env = HashMap::new();
        existing_env.insert("BASE".to_string(), "1".to_string());

        let existing = RInstallationBuilder::new(Some(RInstallationKind::Conda))
            .display_name(Some("Conda R".to_string()))
            .executable(Some(PathBuf::from("/tmp/conda/bin/R")))
            .home(Some(PathBuf::from("/tmp/conda/lib/R")))
            .startup_command(Some("conda activate /tmp/conda".to_string()))
            .environment_variables(Some(existing_env))
            .discovered_by(vec![DiscoverySource::Locator])
            .locator_metadata(Some(LocatorMetadata::Conda {
                environment_path: PathBuf::from("/tmp/conda"),
            }))
            .build();

        let mut overlay_env = HashMap::new();
        overlay_env.insert("R_LIBS".to_string(), "/opt/R-libs".to_string());

        let merged = merge_installations(
            &existing,
            &RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
                .display_name(Some("Production R".to_string()))
                .executable(Some(PathBuf::from("/tmp/conda/bin/R")))
                .home(Some(PathBuf::from("/tmp/conda/lib/R")))
                .startup_command(Some(". /opt/r/setup.sh".to_string()))
                .environment_variables(Some(overlay_env))
                .discovered_by(vec![DiscoverySource::Locator, DiscoverySource::RVersions])
                .rversions_overlay(Some(RVersionsOverlay {
                    label: Some("Production R".to_string()),
                    script: Some("/opt/r/setup.sh".to_string()),
                    repo: Some("https://ppm.example.com/cran/latest".to_string()),
                    library: Some("/opt/R-libs".to_string()),
                    module: None,
                    module_startup_command: None,
                }))
                .build(),
        );

        assert_eq!(merged.display_name.as_deref(), Some("Production R"));
        assert_eq!(
            merged.startup_command.as_deref(),
            Some("conda activate /tmp/conda")
        );
        assert!(matches!(
            merged.locator_metadata,
            Some(LocatorMetadata::Conda { .. })
        ));
        assert_eq!(
            merged
                .rversions_overlay
                .as_ref()
                .and_then(|overlay| overlay.repo.as_deref()),
            Some("https://ppm.example.com/cran/latest")
        );
        assert_eq!(
            merged
                .environment_variables
                .as_ref()
                .and_then(|vars| vars.get("BASE"))
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            merged
                .environment_variables
                .as_ref()
                .and_then(|vars| vars.get("R_LIBS"))
                .map(String::as_str),
            Some("/opt/R-libs")
        );
        assert_eq!(
            merged.discovered_by,
            vec![DiscoverySource::Locator, DiscoverySource::RVersions]
        );
    }
}
