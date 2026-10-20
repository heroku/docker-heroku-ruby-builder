//! Refuses to publish a table that shows less than the article already does.
//!
//! Builds are never deleted from S3 and a Ruby version is never downgraded, so a cell that moves to an older version
//! or to unavailable points at a bad read, such as an S3 outage answering `403`, rather than a real change.

use crate::MajorMinor;
use shared::BaseImage;
use std::collections::BTreeMap;
use std::fmt;

/// The version one table row shows on each base image. A base image without a version shows nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowVersions<V> {
    pub major_minor: MajorMinor,
    pub versions: BTreeMap<BaseImage, V>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Regression<V> {
    RowRemoved {
        major_minor: MajorMinor,
    },
    CellRemoved {
        major_minor: MajorMinor,
        base_image: BaseImage,
        was: V,
    },
    CellDowngraded {
        major_minor: MajorMinor,
        base_image: BaseImage,
        was: V,
        now: V,
    },
}

impl<V: fmt::Display> fmt::Display for Regression<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Regression::RowRemoved { major_minor } => write!(f, "{major_minor}: row removed"),
            Regression::CellRemoved {
                major_minor,
                base_image,
                was,
            } => write!(f, "{major_minor} on {base_image}: {was} became unavailable"),
            Regression::CellDowngraded {
                major_minor,
                base_image,
                was,
                now,
            } => write!(f, "{major_minor} on {base_image}: {was} became {now}"),
        }
    }
}

/// Every way `new` shows less than `old`.
///
/// Rows below `floor` may disappear. A base image missing from `new_base_images` is ignored, since dropping a column
/// is a code change rather than a bad read.
pub fn regressions<V: Ord + Clone>(
    old: &[RowVersions<V>],
    new: &[RowVersions<V>],
    new_base_images: &[BaseImage],
    floor: MajorMinor,
) -> Vec<Regression<V>> {
    let mut found = Vec::new();
    for old_row in old.iter().filter(|row| row.major_minor >= floor) {
        let major_minor = old_row.major_minor;
        let Some(new_row) = new.iter().find(|row| row.major_minor == major_minor) else {
            found.push(Regression::RowRemoved { major_minor });
            continue;
        };
        for (base_image, was) in old_row
            .versions
            .iter()
            .filter(|(base_image, _)| new_base_images.contains(base_image))
        {
            match new_row.versions.get(base_image) {
                None => found.push(Regression::CellRemoved {
                    major_minor,
                    base_image: base_image.clone(),
                    was: was.clone(),
                }),
                Some(now) if now < was => found.push(Regression::CellDowngraded {
                    major_minor,
                    base_image: base_image.clone(),
                    was: was.clone(),
                    now: now.clone(),
                }),
                Some(_) => {}
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use shared::RubyDownloadVersion;

    fn major_minor(s: &str) -> MajorMinor {
        s.parse().unwrap()
    }

    fn version(s: &str) -> RubyDownloadVersion {
        RubyDownloadVersion::new(s).unwrap()
    }

    fn base_images() -> Vec<BaseImage> {
        ["heroku-24", "heroku-26"]
            .map(|name| BaseImage::new(name).unwrap())
            .to_vec()
    }

    /// `cells` holds the version shown on each of [`base_images`], in order.
    fn row(name: &str, cells: &[Option<&str>]) -> RowVersions<RubyDownloadVersion> {
        RowVersions {
            major_minor: major_minor(name),
            versions: base_images()
                .into_iter()
                .zip(cells)
                .filter_map(|(base_image, cell)| Some((base_image, version((*cell)?))))
                .collect(),
        }
    }

    #[test]
    fn newer_and_newly_available_cells_pass() {
        let old = [row("8.1", &[Some("8.1.9"), None])];
        let new = [
            row("8.1", &[Some("8.1.10"), Some("8.1.10")]),
            row("8.2", &[Some("8.2.0"), None]),
        ];
        assert_eq!(
            regressions(&old, &new, &base_images(), major_minor("8.0")),
            vec![]
        );
    }

    #[test]
    fn an_older_cell_is_a_regression() {
        let old = [row("8.1", &[Some("8.1.10"), Some("8.1.10")])];
        let new = [row("8.1", &[Some("8.1.9"), Some("8.1.10")])];
        assert_eq!(
            regressions(&old, &new, &base_images(), major_minor("8.0")),
            vec![Regression::CellDowngraded {
                major_minor: major_minor("8.1"),
                base_image: BaseImage::new("heroku-24").unwrap(),
                was: version("8.1.10"),
                now: version("8.1.9"),
            }]
        );
    }

    #[test]
    fn a_cell_becoming_unavailable_is_a_regression() {
        let old = [row("8.1", &[Some("8.1.1"), Some("8.1.1")])];
        let new = [row("8.1", &[Some("8.1.1"), None])];
        assert_eq!(
            regressions(&old, &new, &base_images(), major_minor("8.0")),
            vec![Regression::CellRemoved {
                major_minor: major_minor("8.1"),
                base_image: BaseImage::new("heroku-26").unwrap(),
                was: version("8.1.1"),
            }]
        );
    }

    #[test]
    fn a_dropped_row_at_or_above_the_floor_is_a_regression() {
        let old = [row("8.1", &[Some("8.1.1")]), row("8.0", &[Some("8.0.7")])];
        let new = [row("8.1", &[Some("8.1.1")])];
        assert_eq!(
            regressions(&old, &new, &base_images(), major_minor("8.0")),
            vec![Regression::RowRemoved {
                major_minor: major_minor("8.0")
            }]
        );
    }

    #[test]
    fn a_row_dropped_by_a_raised_floor_passes() {
        let old = [row("8.1", &[Some("8.1.1")]), row("8.0", &[Some("8.0.7")])];
        let new = [row("8.1", &[Some("8.1.1")])];
        assert_eq!(
            regressions(&old, &new, &base_images(), major_minor("8.1")),
            vec![]
        );
    }

    #[test]
    fn a_removed_base_image_column_passes() {
        let old = [row("8.1", &[Some("8.1.1"), Some("8.1.1")])];
        let new = [row("8.1", &[Some("8.1.1")])];
        assert_eq!(
            regressions(&old, &new, &base_images()[..1], major_minor("8.0")),
            vec![]
        );
    }
}
