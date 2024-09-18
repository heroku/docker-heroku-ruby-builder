use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

static KNOWN_BASE_IMAGES: &[(&str, &str)] = &[
    ("heroku-20", "20"),
    ("heroku-22", "22"),
    ("heroku-24", "24"),
];
static MULTI_ARCH_BASE_IMAGES: [&str; 1] = ["heroku-24"];

#[derive(Debug, thiserror::Error)]
#[error("Invalid base image {0} must be one of {}", KNOWN_BASE_IMAGES.iter().map(|(name, _)| format!("'{name}'")).collect::<Vec<_>>().join(", "))]
pub struct BaseImageError(String);

#[derive(Debug, Clone)]
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
            .find(|&&(name, _)| name == s)
            .map(|&(name, version)| Self {
                name: name.to_owned(),
                distro_number: version.to_owned(),
            })
            .ok_or_else(|| BaseImageError(s.to_owned()))
    }

    pub fn distro_version(&self) -> DistroVersion {
        DistroVersion(format!("{}.04", self.distro_number))
    }

    pub fn is_arch_aware(&self) -> bool {
        MULTI_ARCH_BASE_IMAGES.contains(&self.name.as_str())
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
