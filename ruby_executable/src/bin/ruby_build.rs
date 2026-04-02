use bullet_stream::global::print;
use clap::Parser;
use fs_err::{self as fs, PathExt};
use indoc::{formatdoc, indoc};
use libherokubuildpack::inventory::artifact::Arch;
use reqwest::Url;
use shared::{
    BaseImage, BuildStatus, RubyDownloadVersion, S3_BASE_URL, TarDownloadPath,
    append_filename_with, download_tar, output_ruby_tar_path, s3_url_exists, sha256_from_path,
    source_dir, write_job_metadata,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

static INNER_OUTPUT: &str = "/tmp/output";
static INNER_CACHE: &str = "/tmp/cache";

#[derive(clap::ValueEnum, Clone, Debug)]
enum OnConflict {
    Skip,
    Overwrite,
}

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: Arch,

    #[arg(long)]
    version: RubyDownloadVersion,

    #[arg(long = "base-image")]
    base_image: BaseImage,

    #[arg(long)]
    on_conflict: OnConflict,

    #[arg(long = "job-metadata")]
    job_metadata: Option<PathBuf>,
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
        RUN rustup update

        WORKDIR /tmp/workdir/
        COPY make_ruby.sh /tmp/workdir/make_ruby.sh
    "#});

    dockerfile
}

fn ruby_build(args: &RubyArgs) -> Result<BuildStatus, Box<dyn std::error::Error>> {
    let RubyArgs {
        arch,
        version,
        base_image,
        on_conflict,
        job_metadata: _,
    } = args;

    let start = Instant::now();
    print::h2("Building Ruby");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    fs::create_dir_all(&volume_cache_dir)?;
    fs::create_dir_all(&volume_output_dir)?;

    let expected_output = output_ruby_tar_path(&volume_output_dir, version, base_image, Some(arch));

    match on_conflict {
        OnConflict::Skip => {
            if expected_output.fs_err_try_exists()? {
                print::bullet(format!(
                    "Output already exists locally: {}, skipping",
                    expected_output.display()
                ));
                return Ok(BuildStatus::Skipped);
            }

            let s3_path = expected_output.strip_prefix(&volume_output_dir)?;
            let url = {
                let mut url = Url::parse(S3_BASE_URL)?;
                url.path_segments_mut()
                    .expect("valid base URL")
                    .extend(s3_path.iter().map(|s| s.to_string_lossy()));
                url
            };

            print::bullet(format!("Checking if already uploaded: {url}"));
            if s3_url_exists(url.clone())? {
                print::bullet(format!("Already exists: {url}, skipping"));
                return Ok(BuildStatus::Skipped);
            }
        }
        OnConflict::Overwrite => {}
    }

    let temp_dir = tempfile::tempdir()?;
    let image_name = format!("heroku/ruby-builder:{base_image}");
    let dockerfile = ruby_dockerfile_contents(base_image);
    let dockerfile_path = temp_dir.path().join("Dockerfile");

    print::bullet("Dockerfile");
    print::sub_stream_with("Writing contents to tmpdir", |mut stream, _| {
        write!(stream, "{dockerfile}").and_then(|_| fs::write(&dockerfile_path, &dockerfile))
    })?;

    print::bullet(format!("Docker image {image_name}"));
    let mut docker_build = Command::new("docker");
    docker_build.arg("build");
    docker_build.args(["--platform", &format!("linux/{arch}")]);
    docker_build.args(["--progress", "plain"]);
    docker_build.args(["--tag", &image_name]);
    docker_build.args(["--file", &dockerfile_path.display().to_string()]);
    docker_build.arg(source_dir());
    print::sub_stream_cmd(docker_build)?;

    let download_tar_path =
        TarDownloadPath(volume_cache_dir.join(format!("ruby-source-{version}.tgz")));

    if Path::fs_err_try_exists(download_tar_path.as_ref())? {
        print::bullet(format!(
            "Using cached tarball {}",
            download_tar_path.as_ref().display()
        ))
    } else {
        print::bullet(format!(
            "Downloading {version} to {}",
            download_tar_path.as_ref().display()
        ));
        download_tar(&version.download_url(), &download_tar_path)?;
    };

    print::bullet("Make Ruby");
    let input_tar = PathBuf::from(INNER_CACHE).join(format!("ruby-source-{version}.tgz"));
    let output_tar = output_ruby_tar_path(Path::new(INNER_OUTPUT), version, base_image, Some(arch));
    let volume_cache = volume_cache_dir.display();
    let volume_output = volume_output_dir.display();

    let mut docker_run = Command::new("docker");
    docker_run.arg("run");
    docker_run.arg("--rm");
    docker_run.args(["--platform", &format!("linux/{arch}")]);
    docker_run.args(["--volume", &format!("{volume_output}:{INNER_OUTPUT}")]);
    docker_run.args(["--volume", &format!("{volume_cache}:{INNER_CACHE}")]);

    docker_run.arg(&image_name);
    docker_run.args(["bash", "-c"]);
    docker_run.arg(format!(
        "./make_ruby.sh {} {}",
        input_tar.display(),
        output_tar.display()
    ));

    print::sub_stream_cmd(docker_run)?;

    let output_tar = output_ruby_tar_path(&volume_output_dir, version, base_image, Some(arch));

    let sha_seven_path = cp_file_sha_seven_same_dir(&output_tar)?;

    print::sub_bullet(format!("Copied SHA tgz {}", sha_seven_path.display(),));

    if base_image.has_legacy_path() {
        let legacy_output = output_ruby_tar_path(&volume_output_dir, version, base_image, None);
        fs::copy(expected_output, &legacy_output)?;
        cp_file_sha_seven_same_dir(&legacy_output)?;
    }

    print::all_done(&Some(start));

    Ok(BuildStatus::Success)
}

fn cp_file_sha_seven_same_dir(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let sha = sha256_from_path(path)?;
    let sha_seven = sha.chars().take(7).collect::<String>();
    let sha_seven_path = append_filename_with(path, &format!("-{sha_seven}"), ".tgz")?;
    fs::copy(path, &sha_seven_path)?;
    Ok(sha_seven_path)
}

fn main() {
    let args = RubyArgs::parse();
    let metadata = args.job_metadata.as_deref();
    match ruby_build(&args) {
        Ok(status) => {
            if let Err(e) = write_job_metadata(metadata, "status", status.as_str()) {
                print::error(format!("Failed to write job metadata: {e}"));
            }
        }
        Err(error) => {
            if let Err(e) = write_job_metadata(metadata, "status", "error") {
                print::error(format!("Failed to write job metadata: {e}"));
            }
            print::error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
            std::process::exit(1);
        }
    }
}
