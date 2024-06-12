use bullet_stream::{style, Print};
use clap::Parser;
use fun_run::{CmdError, CommandWithName};
use indoc::{formatdoc, indoc};
use inside_docker::{BaseImage, CpuArch, RubyDownloadVersion};
use std::{io::Write, path::PathBuf, process::Command};

static INNER_OUTPUT: &str = "/tmp/output";
static INNER_CACHE: &str = "/tmp/cache";

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: CpuArch,

    #[arg(long)]
    version: RubyDownloadVersion,

    #[arg(long = "base-image")]
    base_image: BaseImage,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Cannot create temp dir {0}")]
    CreateTmpDir(std::io::Error),

    #[error("Cannot write dockerfile contents")]
    CannotWriteDockerfile(std::io::Error),

    #[error("Command failed {0}")]
    CannotRunCmd(CmdError),
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

        COPY ./inside_docker/Cargo.toml /tmp/workdir/Cargo.toml
        COPY ./inside_docker/Cargo.lock /tmp/workdir/Cargo.lock
        WORKDIR /tmp/workdir/

        # Docker cache for dependencies
        RUN mkdir -p src/bin && echo "fn main() {}" > src/bin/make_ruby.rs
        RUN cargo fetch
        RUN cargo build

        COPY ./inside_docker/ /tmp/workdir/
        RUN cargo build --release
    "#});

    dockerfile
}

fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("Canonicalize path")
}

fn ruby_build(args: &RubyArgs) -> Result<(), Error> {
    let RubyArgs {
        arch,
        version,
        base_image,
    } = args;

    let mut log = Print::new(std::io::stderr()).h1("Building Ruby");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    let temp_dir = tempfile::tempdir().map_err(Error::CreateTmpDir)?;
    let image_name = format!("heroku/ruby-builder:{base_image}");
    let dockerfile = ruby_dockerfile_contents(base_image);
    let dockerfile_path = temp_dir.path().join("Dockerfile");

    log = {
        let mut stream = log
            .bullet("Dockerfile")
            .start_stream("Writing contents to tmpdir");
        write!(stream, "{dockerfile}").expect("Stream write");
        fs_err::write(&dockerfile_path, dockerfile).map_err(Error::CannotWriteDockerfile)?;
        stream.done().done()
    };

    log = {
        let mut bullet = log.bullet(format!("Docker image {image_name}"));
        let mut docker_build = Command::new("docker");
        docker_build.arg("build");
        docker_build.args(["--platform", &format!("linux/{arch}")]);
        docker_build.args(["--progress", "plain"]);
        docker_build.args(["--tag", &image_name]);
        docker_build.args(["--file", &dockerfile_path.display().to_string()]);
        docker_build.arg(source_dir().to_str().expect("Path to str"));
        let _ = bullet
            .stream_with(
                format!("Building via {}", style::command(docker_build.name())),
                |stdout, stderr| docker_build.stream_output(stdout, stderr),
            )
            .map_err(Error::CannotRunCmd)?;

        bullet.done()
    };

    log = {
        let mut bullet = log.bullet("Ruby binaries");
        let outside_cache = volume_cache_dir.display();
        let outside_output = volume_output_dir.display();

        let mut docker_run = Command::new("docker");
        docker_run.arg("run");
        docker_run.arg("--rm");
        docker_run.args(["--platform", &format!("linux/{arch}")]);
        docker_run.args(["--volume", &format!("{outside_output}:{INNER_OUTPUT}")]);
        docker_run.args(["--volume", &format!("{outside_cache}:{INNER_CACHE}")]);
        docker_run.arg(&image_name);
        docker_run.args(["bash", "-c"]);
        docker_run.arg(&format!(
            "cargo run --release --bin make_ruby -- --version {version} --base-image {base_image} --output {INNER_OUTPUT} --cache {INNER_CACHE}",
        ));

        bullet
            .stream_with(
                format!("Running {}", style::command(docker_run.name())),
                |stdout, stderr| docker_run.stream_output(stdout, stderr),
            )
            .map_err(Error::CannotRunCmd)?;

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
