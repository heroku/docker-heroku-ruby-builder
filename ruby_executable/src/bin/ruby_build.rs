use bullet_stream::{style, Print};
use clap::Parser;
use fs_err::PathExt;
use fun_run::CommandWithName;
use gem_version::GemVersion;
use indoc::{formatdoc, indoc};
use libherokubuildpack::inventory::{
    self,
    artifact::{Arch, Artifact},
};
use shared::{
    append_filename_with, artifact_is_different, artifact_same_url_different_checksum,
    atomic_inventory_update, download_tar, output_tar_path, sha256_from_path, source_dir,
    validate_version_for_stack, ArtifactMetadata, BaseImage, RubyDownloadVersion, TarDownloadPath,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

static INNER_OUTPUT: &str = "/tmp/output";
static INNER_CACHE: &str = "/tmp/cache";
static S3_BASE_URL: &str = "https://heroku-buildpack-ruby.s3.us-east-1.amazonaws.com";

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: Arch,

    #[arg(long)]
    version: RubyDownloadVersion,

    #[arg(long = "base-image")]
    base_image: BaseImage,
}

fn ruby_dockerfile_contents(base_image: &BaseImage) -> String {
    let distro_number = base_image.distro_number();
    let mut dockerfile = String::new();
    dockerfile.push_str(&format!("FROM heroku/heroku:{distro_number}-build\n"));
    dockerfile.push_str(indoc! {r#"
        USER root

        RUN apt-get update -y && apt-get install -y libreadline-dev ruby
        RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
        ENV PATH="/root/.cargo/bin:${PATH}"

        # https://bugs.ruby-lang.org/issues/20506
        RUN rustup install 1.77 && rustup default 1.77

        WORKDIR /tmp/workdir/
        COPY make_ruby.sh /tmp/workdir/make_ruby.sh
    "#});

    dockerfile
}

fn ruby_build(args: &RubyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let RubyArgs {
        arch,
        version,
        base_image,
    } = args;

    let mut log = Print::new(std::io::stderr()).h1("Building Ruby");
    let inventory = source_dir().join("ruby_inventory.toml");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    fs_err::create_dir_all(&volume_cache_dir)?;
    fs_err::create_dir_all(&volume_output_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let image_name = format!("heroku/ruby-builder:{base_image}");
    let dockerfile = ruby_dockerfile_contents(base_image);
    let dockerfile_path = temp_dir.path().join("Dockerfile");

    log = {
        let mut bullet = log.bullet("Dockerfile");
        bullet.stream_with("Writing contents to tmpdir", |mut stream, _| {
            write!(stream, "{dockerfile}")?;
            fs_err::write(&dockerfile_path, &dockerfile)
        })?;
        bullet.done()
    };

    log = {
        let mut bullet = log.bullet(format!("Docker image {image_name}"));
        let mut docker_build = Command::new("docker");
        docker_build.arg("build");
        docker_build.args(["--platform", &format!("linux/{arch}")]);
        docker_build.args(["--progress", "plain"]);
        docker_build.args(["--tag", &image_name]);
        docker_build.args(["--file", &dockerfile_path.display().to_string()]);
        docker_build.arg(source_dir());
        let _ = bullet.stream_with(
            format!("Building via {}", style::command(docker_build.name())),
            |stdout, stderr| docker_build.stream_output(stdout, stderr),
        )?;

        bullet.done()
    };

    let download_tar_path =
        TarDownloadPath(volume_cache_dir.join(format!("ruby-source-{version}.tgz")));

    validate_version_for_stack(version, base_image)?;

    log = if Path::fs_err_try_exists(download_tar_path.as_ref())? {
        log.bullet(format!(
            "Using cached tarball {}",
            download_tar_path.as_ref().display()
        ))
        .done()
    } else {
        let bullet = log.bullet(format!(
            "Downloading {version} to {}",
            download_tar_path.as_ref().display()
        ));
        download_tar(&version.download_url(), &download_tar_path)?;
        bullet.done()
    };

    log = {
        let mut bullet = log.bullet("Make Ruby");
        let input_tar = PathBuf::from(INNER_CACHE).join(format!("ruby-source-{version}.tgz"));
        let output_tar = output_tar_path(Path::new(INNER_OUTPUT), version, base_image, arch);
        let volume_cache = volume_cache_dir.display();
        let volume_output = volume_output_dir.display();

        let mut docker_run = Command::new("docker");
        docker_run.arg("run");
        docker_run.arg("--rm");
        docker_run.args(["--platform", &format!("linux/{arch}")]);
        docker_run.args(["--volume", &format!("{volume_output}:{INNER_OUTPUT}")]);
        docker_run.args(["--volume", &format!("{volume_cache}:{INNER_CACHE}")]);

        if version.major > 3 || (version.major == 3 && version.minor >= 2) {
            docker_run.args(["-e", "ENABLE_YJIT=1"]);
        }

        docker_run.arg(&image_name);
        docker_run.args(["bash", "-c"]);
        docker_run.arg(format!(
            "./make_ruby.sh {} {}",
            input_tar.display(),
            output_tar.display()
        ));

        bullet.stream_with(
            format!("Running {}", style::command(docker_run.name())),
            |stdout, stderr| docker_run.stream_output(stdout, stderr),
        )?;
        bullet.done()
    };

    log = {
        let mut bullet = log.bullet(format!(
            "Updating manifest {}",
            style::value(inventory.to_string_lossy())
        ));

        let output_tar = output_tar_path(&volume_output_dir, version, base_image, arch);

        let sha = sha256_from_path(&output_tar)?;
        let sha_seven = sha.chars().take(7).collect::<String>();
        let sha_seven_path = append_filename_with(&output_tar, &format!("-{sha_seven}"), ".tgz")?;
        let url = format!(
            "{S3_BASE_URL}/{}",
            sha_seven_path.strip_prefix(&volume_output_dir)?.display()
        );

        bullet = bullet.sub_bullet(format!("Copying SHA tgz {}", sha_seven_path.display(),));
        fs_err::copy(output_tar, &sha_seven_path)?;

        let artifact = Artifact {
            version: GemVersion::from_str(&version.bundler_format())?,
            os: inventory::artifact::Os::Linux,
            arch: *arch,
            url,
            checksum: format!("sha256:{sha}").parse()?,
            metadata: ArtifactMetadata {
                distro_version: base_image.distro_version(),
                timestamp: chrono::Utc::now(),
            },
        };

        atomic_inventory_update(&inventory, |inventory| {
            for prior in &inventory.artifacts {
                if let Err(error) = artifact_same_url_different_checksum(prior, &artifact) {
                    // TODO: Investigate bullet stream ownership
                    println!(
                        "{}",
                        style::important(format!("!!!!!!!!!! Error updating inventory: {error}"))
                    );

                    fs_err::remove_file(&sha_seven_path)?;
                    return Err(error);
                };
            }

            inventory
                .artifacts
                .retain(|a| artifact_is_different(a, &artifact));

            inventory.push(artifact);

            Ok(())
        })?;

        bullet.done()
    };

    log.done();

    Ok(())
}

fn main() {
    let args = RubyArgs::parse();
    if let Err(error) = ruby_build(&args) {
        Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
        std::process::exit(1);
    }
}
