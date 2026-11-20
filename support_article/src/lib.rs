//! Generates the Ruby and JRuby version tables in the Dev Center article "Heroku Ruby Support".

use gem_version::GemVersion;
use minijinja::{Environment, UndefinedBehavior, Value};
use shared::inventory_format::artifact_key;
use shared::{BaseImage, BuildTarget, RubyInventory};
use std::collections::{BTreeMap, BTreeSet};

pub mod jruby_table;
pub mod ruby_table;

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error(transparent)]
    Shared(#[from] shared::Error),

    #[error(transparent)]
    Inventory(#[from] shared::inventory_format::InventoryFormatError),

    #[error(transparent)]
    JRubyVersion(#[from] jruby_executable::jruby_version::ParseError),

    #[error("{0} is not named `ruby-<stdlib version>-jruby-<version>.tgz`")]
    NotAJRubyUrl(url::Url),

    #[error(transparent)]
    InvalidUrl(#[from] shared::UrlParseError),
}

/// Each version in `inventory`, with the base images it has a build of for every architecture.
fn complete_base_images(
    inventory: &RubyInventory,
) -> Result<BTreeMap<GemVersion, BTreeSet<BaseImage>>, CheckError> {
    let mut targets = BTreeMap::<GemVersion, BTreeSet<BuildTarget>>::new();
    for artifact in &inventory.artifacts {
        let key = artifact_key(artifact)?;
        targets.entry(key.version).or_default().insert(key.target);
    }
    Ok(targets
        .into_iter()
        .map(|(version, targets)| {
            let base_images = BaseImage::all()
                .into_iter()
                .filter(|base_image| {
                    base_image.archs().iter().all(|&arch| {
                        targets.contains(&BuildTarget {
                            base_image: base_image.clone(),
                            arch,
                        })
                    })
                })
                .collect();
            (version, base_images)
        })
        .collect())
}

/// Undefined values are errors, so a misspelled variable fails instead of rendering blank.
pub(crate) fn render_template(
    name: &str,
    source: &str,
    context: Value,
) -> Result<String, minijinja::Error> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.add_template(name, source)?;
    environment.get_template(name)?.render(context)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use libherokubuildpack::inventory::artifact::{Arch, Os};
    use minijinja::context;
    use pretty_assertions::assert_eq;
    use shared::{ArtifactMetadata, RubyArtifact, S3_BASE_URL};
    use std::str::FromStr;

    pub(crate) fn inventory(builds: &[(&str, &str, Arch)]) -> RubyInventory {
        RubyInventory {
            artifacts: builds
                .iter()
                .map(|&(version, base_image, arch)| {
                    let base_image = BaseImage::new(base_image).unwrap();
                    RubyArtifact {
                        version: GemVersion::from_str(version).unwrap(),
                        os: Os::Linux,
                        arch,
                        url: format!("{S3_BASE_URL}/{base_image}/{arch}/ruby-{version}.tgz"),
                        checksum: format!("sha256:{}", "0".repeat(64)).parse().unwrap(),
                        metadata: ArtifactMetadata {
                            timestamp: "2026-04-22T16:44:16Z".parse().unwrap(),
                            distro_version: base_image.distro_version(),
                        },
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn render_template_errors_on_a_misspelled_attribute() {
        let error = render_template(
            "t",
            "{{ row.nmae }}",
            context! { row => context! { name => "x" } },
        )
        .unwrap_err();
        assert_eq!(error.kind(), minijinja::ErrorKind::UndefinedError);
    }
}
