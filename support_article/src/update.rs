//! Tables the article is updated with.

use crate::ReadError;
use crate::guard::RowVersions;
use shared::BaseImage;
use std::fmt;

/// A table this tool generates into a region of the article.
pub trait VersionTable: Sized {
    /// Names the region's markers.
    const NAME: &'static str;

    type Version: Ord + Clone + fmt::Debug + fmt::Display;

    /// HTML with `\n` line endings.
    fn render(&self) -> String;

    /// Reads HTML produced by [`VersionTable::render`].
    fn read(html: &str) -> Result<Self, ReadError>;

    fn base_images(&self) -> &[BaseImage];

    fn row_versions(&self) -> Vec<RowVersions<Self::Version>>;
}
