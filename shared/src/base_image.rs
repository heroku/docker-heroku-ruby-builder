use libherokubuildpack::inventory::artifact::Arch;
use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

static KNOWN_BASE_IMAGES: &[(&str, &str, &[Arch])] = &[
    ("heroku-22", "22", &[Arch::Amd64]),
    ("heroku-24", "24", &[Arch::Amd64, Arch::Arm64]),
    ("heroku-26", "26", &[Arch::Amd64, Arch::Arm64]),
];

#[derive(Debug, thiserror::Error)]
#[error("Invalid base image {0} must be one of {known}", known = KNOWN_BASE_IMAGES.iter().map(|(name, _, _)| format!("'{name}'")).collect::<Vec<_>>().join(", "))]
pub struct BaseImageError(String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BaseImage {
    name: String,
    distro_number: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DistroVersion(String);

impl BaseImage {
    pub fn new(s: &str) -> Result<Self, BaseImageError> {
        KNOWN_BASE_IMAGES
            .iter()
            .find(|&&(name, _, _)| name == s)
            .map(|&(name, version, _)| Self {
                name: name.to_owned(),
                distro_number: version.to_owned(),
            })
            .ok_or_else(|| BaseImageError(s.to_owned()))
    }

    pub fn distro_version(&self) -> DistroVersion {
        DistroVersion(format!("{}.04", self.distro_number))
    }

    pub fn has_legacy_path(&self) -> bool {
        self.name == "heroku-22"
    }

    /// Every supported base image, oldest first.
    pub fn all() -> Vec<BaseImage> {
        KNOWN_BASE_IMAGES
            .iter()
            .map(|&(name, _, _)| BaseImage::new(name).expect("known base image"))
            .collect()
    }

    /// The architectures Ruby binaries are built for on this base image.
    pub fn archs(&self) -> &'static [Arch] {
        KNOWN_BASE_IMAGES
            .iter()
            .find(|&&(name, _, _)| name == self.name)
            .map(|&(_, _, archs)| archs)
            .expect("base images are only constructed from KNOWN_BASE_IMAGES")
    }
}

impl Display for BaseImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl BaseImage {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn distro_number(&self) -> &str {
        &self.distro_number
    }
}

impl FromStr for BaseImage {
    type Err = BaseImageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BaseImage::new(s)
    }
}

/// Returns all valid (BaseImage, Arch) pairs for building Ruby binaries.
pub fn build_matrix() -> Vec<(BaseImage, Arch)> {
    BaseImage::all()
        .into_iter()
        .flat_map(|base_image| {
            base_image
                .archs()
                .iter()
                .map(move |arch| (base_image.clone(), *arch))
        })
        .collect()
}
