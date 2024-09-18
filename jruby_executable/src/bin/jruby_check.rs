use bullet_stream::{style, Print};
use clap::Parser;
use fun_run::CommandWithName;
use indoc::formatdoc;
use inventory::artifact::Arch;
use jruby_executable::jruby_build_properties;
use shared::{source_dir, BaseImage};
use std::error::Error;
use std::io::Write;
use std::{path::PathBuf, process::Command};

static INNER_OUTPUT: &str = "/tmp/output";

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: Arch,

    #[arg(long)]
    version: String,

    #[arg(long = "base-image")]
    base_image: BaseImage,
}

fn jruby_check(args: &RubyArgs) -> Result<(), Box<dyn Error>> {
    let RubyArgs {
        arch,
        version,
        base_image,
    } = args;

    let jruby_stdlib_version = jruby_build_properties(version)?.ruby_stdlib_version()?;

    // Log progress to STDERR, print results to STDOUT directly
    let mut log = Print::new(std::io::stderr()).h1(format!(
        "Prepare: Checking JRuby version ({version} linux/{arch} stdlib {jruby_stdlib_version}) for {base_image}",
    ));
    let distro_number = base_image.distro_number();

    let tempdir = tempfile::tempdir()?;
    let dockerfile_path = tempdir.path().join("Dockerfile");

    let image_name = format!("heroku/jruby-builder:{base_image}");

    let mut stream = log
        .bullet(format!("Dockerfile for {}", image_name))
        .start_stream("Contents");

    let dockerfile = formatdoc! {"
        FROM heroku/heroku:{distro_number}-build

        USER root
        RUN apt-get update -y; apt-get install default-jre default-jdk -y
    "};

    write!(stream, "{}", dockerfile)?;

    fs_err::write(&dockerfile_path, dockerfile)?;

    log = stream.done().done();

    let outside_output = source_dir().join("output");

    log = {
        let mut bullet = log.bullet(format!("Docker image {image_name}"));
        let mut docker_build = Command::new("docker");
        docker_build.arg("build");
        docker_build.args(["--platform", &format!("linux/{arch}")]);
        docker_build.args(["--progress", "plain"]);
        docker_build.args(["--tag", &image_name]);
        docker_build.args(["--file", &dockerfile_path.display().to_string()]);
        docker_build.arg(source_dir().to_str().expect("Path to str"));
        let _ = bullet.stream_with(
            format!("Building via {}", style::command(docker_build.name())),
            |stdout, stderr| docker_build.stream_output(stdout, stderr),
        )?;

        bullet.done()
    };

    let (log, result) = {
        let inner_jruby_path = PathBuf::from(INNER_OUTPUT)
            .join(base_image.to_string())
            .join(format!("ruby-{jruby_stdlib_version}-jruby-{version}.tgz"));

        let mut cmd = Command::new("docker");
        cmd.arg("run");
        cmd.arg("--rm");
        cmd.args(["--platform", &format!("linux/{arch}")]);
        cmd.args([
            "--volume",
            &format!(
                "{outside_output}:{INNER_OUTPUT}",
                outside_output = outside_output.display()
            ),
        ]);
        cmd.arg(image_name);
        cmd.args(["bash", "-c"]);
        cmd.arg(
            [
                "mkdir /tmp/unzipped",
                &format!("tar xzf {} -C /tmp/unzipped", inner_jruby_path.display()),
                "export PATH=\"tmp/unzipped/bin:$PATH\"",
                "echo -n '- Rubygems version: '",
                "gem -v",
                "echo -n '- Ruby version: '",
                "ruby -v",
            ]
            .join(" && "),
        );

        let mut cmd_stream = log.bullet("Versions");

        let result = cmd_stream.stream_with(
            format!("Running {}", style::command(cmd.name())),
            |stdout, stderr| cmd.stream_output(stdout, stderr),
        )?;

        (cmd_stream.done(), result)
    };

    log.done();
    eprintln!();

    // Print results to STDOUT for github summary
    println!("## JRuby {version} stdlib {jruby_stdlib_version} linux/{arch} for {base_image}");
    println!();
    println!("{}", result.stdout_lossy());
    Ok(())
}

fn main() {
    let args = RubyArgs::parse();
    if let Err(error) = jruby_check(&args) {
        Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
        std::process::exit(1);
    }
}
