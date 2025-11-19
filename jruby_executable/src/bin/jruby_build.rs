use bullet_stream::{global::print, style};
use clap::Parser;
use fs_err::PathExt;
use gem_version::GemVersion;
use indoc::formatdoc;
use jruby_executable::jruby_build_properties;
use libherokubuildpack::inventory;
use libherokubuildpack::inventory::artifact::{Arch, Artifact};
use shared::{
    ArtifactMetadata, BaseImage, TarDownloadPath, append_filename_with, artifact_is_different,
    artifact_same_url_different_checksum, atomic_inventory_update, download_tar, sha256_from_path,
    source_dir, tar_dir_to_file, untar_to_dir,
};
use std::convert::From;
use std::error::Error;
use std::str::FromStr;
use std::time::Instant;

static S3_BASE_URL: &str = "https://heroku-buildpack-ruby.s3.us-east-1.amazonaws.com";

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: String,

    #[arg(long)]
    base_image: BaseImage,
}

fn jruby_build(args: &Args) -> Result<(), Box<dyn Error>> {
    let Args {
        version,
        base_image,
    } = args;

    let start = Instant::now();
    print::h2("Building JRuby");
    let inventory = source_dir().join("jruby_inventory.toml");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    fs_err::create_dir_all(&volume_cache_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let extracted_path = temp_dir.path().join("extracted");

    let ruby_stdlib_version = jruby_build_properties(version)?.ruby_stdlib_version()?;

    let download_path =
        TarDownloadPath(volume_cache_dir.join(format!("jruby-dist-{version}-bin.tar.gz")));

    if download_path.as_ref().fs_err_try_exists()? {
        print::bullet(format!(
            "Using cached JRuby archive {}",
            download_path.as_ref().display()
        ));
    } else {
        let url = format!(
            "https://repo1.maven.org/maven2/org/jruby/jruby-dist/{version}/jruby-dist-{version}-bin.tar.gz"
        );
        print::bullet("Download JRuby");
        print::sub_bullet(format!("To {}", download_path.as_ref().to_string_lossy()));
        print::sub_bullet(format!("From {}", style::url(&url)));

        let timer = print::sub_start_timer("Downloading");
        download_tar(&url, &download_path)?;
        timer.done();
    }

    untar_to_dir(&download_path, &extracted_path)?;

    let jruby_dir = extracted_path.join(format!("jruby-{version}"));

    print::bullet("Removing unnecessary files");
    for pattern in &["*.bat", "*.dll", "*.exe"] {
        for path in glob::glob(&jruby_dir.join("bin").join(pattern).to_string_lossy())?
            .collect::<Result<Vec<_>, _>>()?
        {
            print::sub_bullet(format!("Remove {}", path.display()));
            fs_err::remove_file(path)?;
        }
    }

    let path = jruby_dir.join("lib").join("target");
    if path.fs_err_try_exists()? {
        print::sub_bullet(format!("Remove recursive {}", path.display()));
        fs_err::remove_dir_all(&path)?;
    }

    print::bullet("Checking for `ruby` binstub");
    let ruby_bin = jruby_dir.join("bin").join("ruby");
    if ruby_bin.fs_err_try_exists()? {
        print::sub_bullet("File exists")
    } else {
        print::sub_bullet("Create ruby symlink to jruby");
        fs_err::os::unix::fs::symlink("jruby", ruby_bin)?;
    }

    let tgz_name = format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz");

    print::bullet("Creating tgz archives");
    print::sub_bullet(format!(
        "Inventory file {}",
        style::value(inventory.to_string_lossy())
    ));
    let tar_dir = volume_output_dir.join(base_image.to_string());

    fs_err::create_dir_all(&tar_dir)?;

    let tar_file = fs_err::File::create(tar_dir.join(&tgz_name))?;

    let timer = print::sub_start_timer(format!("Write {}", tar_file.path().display()));
    tar_dir_to_file(&jruby_dir, &tar_file)?;
    timer.done();

    let tar_path = tar_file.path();
    let sha = sha256_from_path(tar_path)?;
    let sha_seven = sha.chars().take(7).collect::<String>();
    let sha_seven_path = append_filename_with(tar_path, &format!("-{sha_seven}"), ".tgz")?;

    print::sub_bullet(format!("Write {}", sha_seven_path.display(),));
    fs_err::copy(tar_file.path(), &sha_seven_path)?;

    let timestamp = chrono::Utc::now();
    for cpu_arch in [Arch::Amd64, Arch::Arm64] {
        let distro_version = base_image.distro_version();
        let artifact = Artifact {
            version: GemVersion::from_str(version)?,
            os: inventory::artifact::Os::Linux,
            arch: cpu_arch,
            url: format!(
                "{S3_BASE_URL}/{}",
                sha_seven_path.strip_prefix(&volume_output_dir)?.display()
            ),
            checksum: format!("sha256:{sha}").parse()?,
            metadata: ArtifactMetadata {
                distro_version,
                timestamp,
            },
        };
        atomic_inventory_update(&inventory, |inventory| {
            for prior in &inventory.artifacts {
                if let Err(error) = artifact_same_url_different_checksum(prior, &artifact) {
                    print::error(format!("Error updating inventory\n\nError: {error}"));

                    fs_err::remove_file(&sha_seven_path)?;
                    return Err(error);
                };
            }

            inventory
                .artifacts
                .retain(|a| artifact_is_different(a, &artifact));

            inventory.push(artifact);
            Ok(())
        })?
    }

    // Can be removed once manifest file support is fully rolled out
    for cpu_arch in [Arch::Amd64, Arch::Arm64] {
        let dir = volume_output_dir
            .join(base_image.to_string())
            .join(cpu_arch.to_string());
        fs_err::create_dir_all(&dir)?;

        let path = dir.join(&tgz_name);
        print::sub_bullet(format!("Write {}", path.display()));
        fs_err::copy(tar_file.path(), &path)?;
    }

    print::all_done(&Some(start));
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_build(&args) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
        std::process::exit(1);
    }
}
