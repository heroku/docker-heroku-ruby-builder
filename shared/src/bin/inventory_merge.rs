use bullet_stream::global::print;
use clap::Parser;
use fs_err::{self as fs};
use gem_version::GemVersion;
use indoc::formatdoc;
use libherokubuildpack::inventory::Inventory;
use libherokubuildpack::inventory::artifact::Artifact;
use sha2::Sha256;
use shared::ArtifactMetadata;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "inventory_merge",
    about = "Merge multiple inventory TOML files into one"
)]
struct Args {
    /// Base inventory file to merge into (will be read first)
    #[arg(long)]
    base: Option<PathBuf>,

    /// Output file path (defaults to stdout)
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Input inventory files to merge
    #[arg(required = true)]
    input_files: Vec<PathBuf>,
}

type RubyArtifact = Artifact<GemVersion, Sha256, ArtifactMetadata>;
type RubyInventory = Inventory<GemVersion, Sha256, ArtifactMetadata>;

fn parse_inventory(contents: &str) -> Result<RubyInventory, Box<dyn std::error::Error>> {
    if contents.trim().is_empty() {
        Ok(Inventory {
            artifacts: Vec::new(),
        })
    } else {
        contents
            .parse::<RubyInventory>()
            .map_err(|e| format!("Error parsing inventory: {e}").into())
    }
}

/// Check if two artifacts represent the same build target
/// (same version, architecture, and distro)
fn same_build_target(a: &RubyArtifact, b: &RubyArtifact) -> bool {
    a.version == b.version
        && a.arch == b.arch
        && a.metadata.distro_version == b.metadata.distro_version
}

fn merge_inventories(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut all_artifacts: Vec<RubyArtifact> = Vec::new();

    // Load base inventory first if provided
    if let Some(base_path) = &args.base {
        print::bullet(format!("Loading base inventory: {}", base_path.display()));
        let contents = fs::read_to_string(base_path)?;
        let inventory = parse_inventory(&contents)?;
        print::sub_bullet(format!("Found {} artifacts", inventory.artifacts.len()));
        all_artifacts.extend(inventory.artifacts);
    }

    // Load and merge all input files
    for path in &args.input_files {
        print::bullet(format!("Loading: {}", path.display()));
        let contents = fs::read_to_string(path)?;
        let inventory = parse_inventory(&contents)?;
        print::sub_bullet(format!("Found {} artifacts", inventory.artifacts.len()));

        // Add artifacts, replacing any with the same build target
        for new_artifact in inventory.artifacts {
            // Remove any existing artifact with same build target
            all_artifacts.retain(|existing| !same_build_target(existing, &new_artifact));
            all_artifacts.push(new_artifact);
        }
    }

    // Sort artifacts for consistent output (by string representation)
    all_artifacts.sort_by(|a, b| {
        let a_key = (
            a.version.to_string(),
            a.arch.to_string(),
            a.metadata.distro_version.to_string(),
        );
        let b_key = (
            b.version.to_string(),
            b.arch.to_string(),
            b.metadata.distro_version.to_string(),
        );
        a_key.cmp(&b_key)
    });

    let merged = Inventory {
        artifacts: all_artifacts,
    };

    print::bullet(format!(
        "Merged inventory contains {} artifacts",
        merged.artifacts.len()
    ));

    // Write output
    match &args.output {
        Some(output_path) => {
            print::bullet(format!("Writing to: {}", output_path.display()));
            let mut file = fs::File::create(output_path)?;
            write!(file, "{merged}")?;
        }
        None => {
            print!("{merged}");
        }
    }

    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = merge_inventories(&args) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn artifact_heroku22_amd64() -> &'static str {
        indoc! {r#"
            [[artifacts]]
            version = "3.3.0"
            os = "linux"
            arch = "amd64"
            url = "https://example.com/heroku-22/ruby-3.3.0.tgz"
            checksum = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

            [artifacts.metadata]
            timestamp = "2024-01-01T00:00:00Z"
            distro_version = "22.04"
        "#}
    }

    fn artifact_heroku24_amd64() -> &'static str {
        indoc! {r#"
            [[artifacts]]
            version = "3.3.0"
            os = "linux"
            arch = "amd64"
            url = "https://example.com/heroku-24/amd64/ruby-3.3.0.tgz"
            checksum = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

            [artifacts.metadata]
            timestamp = "2024-01-01T00:00:00Z"
            distro_version = "24.04"
        "#}
    }

    fn artifact_heroku24_arm64() -> &'static str {
        indoc! {r#"
            [[artifacts]]
            version = "3.3.0"
            os = "linux"
            arch = "arm64"
            url = "https://example.com/heroku-24/arm64/ruby-3.3.0.tgz"
            checksum = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"

            [artifacts.metadata]
            timestamp = "2024-01-01T00:00:00Z"
            distro_version = "24.04"
        "#}
    }

    fn artifact_ruby320() -> &'static str {
        indoc! {r#"
            [[artifacts]]
            version = "3.2.0"
            os = "linux"
            arch = "amd64"
            url = "https://example.com/heroku-24/amd64/ruby-3.2.0.tgz"
            checksum = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"

            [artifacts.metadata]
            timestamp = "2023-01-01T00:00:00Z"
            distro_version = "24.04"
        "#}
    }

    #[test]
    fn test_parse_empty_inventory() {
        let inventory = parse_inventory("").unwrap();
        assert_eq!(inventory.artifacts.len(), 0);

        let inventory = parse_inventory("   \n  ").unwrap();
        assert_eq!(inventory.artifacts.len(), 0);
    }

    #[test]
    fn test_parse_single_artifact() {
        let inventory = parse_inventory(artifact_heroku24_amd64()).unwrap();
        assert_eq!(inventory.artifacts.len(), 1);
        assert_eq!(inventory.artifacts[0].version.to_string(), "3.3");
    }

    #[test]
    fn test_same_build_target() {
        let inv1 = parse_inventory(artifact_heroku24_amd64()).unwrap();
        let inv2 = parse_inventory(artifact_heroku24_arm64()).unwrap();
        let inv3 = parse_inventory(artifact_heroku22_amd64()).unwrap();

        // Same file = same target
        assert!(same_build_target(
            &inv1.artifacts[0],
            &inv1.artifacts[0]
        ));

        // Different arch = different target
        assert!(!same_build_target(
            &inv1.artifacts[0],
            &inv2.artifacts[0]
        ));

        // Different distro = different target
        assert!(!same_build_target(
            &inv1.artifacts[0],
            &inv3.artifacts[0]
        ));
    }

    #[test]
    fn test_merge_multiple_inventories() {
        let temp = tempfile::tempdir().unwrap();

        // Create test inventory files
        let file1 = temp.path().join("inv1.toml");
        let file2 = temp.path().join("inv2.toml");
        let file3 = temp.path().join("inv3.toml");
        let output = temp.path().join("merged.toml");

        fs::write(&file1, artifact_heroku22_amd64()).unwrap();
        fs::write(&file2, artifact_heroku24_amd64()).unwrap();
        fs::write(&file3, artifact_heroku24_arm64()).unwrap();

        let args = Args {
            base: None,
            output: Some(output.clone()),
            input_files: vec![file1, file2, file3],
        };

        merge_inventories(&args).unwrap();

        let merged = fs::read_to_string(&output).unwrap();
        let inventory = parse_inventory(&merged).unwrap();

        assert_eq!(inventory.artifacts.len(), 3);

        // Verify all URLs are present
        let urls: Vec<_> = inventory.artifacts.iter().map(|a| &a.url).collect();
        assert!(urls.iter().any(|u| u.contains("heroku-22")));
        assert!(urls.iter().any(|u| u.contains("heroku-24/amd64")));
        assert!(urls.iter().any(|u| u.contains("heroku-24/arm64")));
    }

    #[test]
    fn test_merge_with_base_inventory() {
        let temp = tempfile::tempdir().unwrap();

        let base_file = temp.path().join("base.toml");
        let new_file = temp.path().join("new.toml");
        let output = temp.path().join("merged.toml");

        // Base has Ruby 3.2.0
        fs::write(&base_file, artifact_ruby320()).unwrap();
        // New file has Ruby 3.3.0
        fs::write(&new_file, artifact_heroku24_amd64()).unwrap();

        let args = Args {
            base: Some(base_file),
            output: Some(output.clone()),
            input_files: vec![new_file],
        };

        merge_inventories(&args).unwrap();

        let merged = fs::read_to_string(&output).unwrap();
        let inventory = parse_inventory(&merged).unwrap();

        assert_eq!(inventory.artifacts.len(), 2);

        // Verify both versions are present
        let versions: Vec<_> = inventory
            .artifacts
            .iter()
            .map(|a| a.version.to_string())
            .collect();
        assert!(versions.contains(&"3.2".to_string()));
        assert!(versions.contains(&"3.3".to_string()));
    }

    #[test]
    fn test_merge_replaces_same_build_target() {
        let temp = tempfile::tempdir().unwrap();

        let file1 = temp.path().join("old.toml");
        let file2 = temp.path().join("new.toml");
        let output = temp.path().join("merged.toml");

        // Both files have the same version/arch/distro but different URLs
        fs::write(&file1, artifact_heroku24_amd64()).unwrap();

        let updated_artifact = indoc! {r#"
            [[artifacts]]
            version = "3.3.0"
            os = "linux"
            arch = "amd64"
            url = "https://example.com/heroku-24/amd64/ruby-3.3.0-UPDATED.tgz"
            checksum = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

            [artifacts.metadata]
            timestamp = "2024-01-02T00:00:00Z"
            distro_version = "24.04"
        "#};
        fs::write(&file2, updated_artifact).unwrap();

        let args = Args {
            base: None,
            output: Some(output.clone()),
            input_files: vec![file1, file2],
        };

        merge_inventories(&args).unwrap();

        let merged = fs::read_to_string(&output).unwrap();
        let inventory = parse_inventory(&merged).unwrap();

        // Should have only 1 artifact (the newer one replaced the older)
        assert_eq!(inventory.artifacts.len(), 1);
        assert!(inventory.artifacts[0].url.contains("UPDATED"));
    }

    #[test]
    fn test_merge_empty_base_with_inputs() {
        let temp = tempfile::tempdir().unwrap();

        let base_file = temp.path().join("base.toml");
        let new_file = temp.path().join("new.toml");
        let output = temp.path().join("merged.toml");

        // Empty base file
        fs::write(&base_file, "").unwrap();
        fs::write(&new_file, artifact_heroku24_amd64()).unwrap();

        let args = Args {
            base: Some(base_file),
            output: Some(output.clone()),
            input_files: vec![new_file],
        };

        merge_inventories(&args).unwrap();

        let merged = fs::read_to_string(&output).unwrap();
        let inventory = parse_inventory(&merged).unwrap();

        assert_eq!(inventory.artifacts.len(), 1);
    }
}
