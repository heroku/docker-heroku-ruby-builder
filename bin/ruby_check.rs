#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2021"

[dependencies]
java-properties = "2.0.0"
glob = "0.3.1"
clap = { version = "4.5.4", features = ["derive"] }
shared = { path = "../shared" }
indoc = "2.0.5"
lazy_static = "1.4.0"
flate2 = "1.0.30"
fs-err = "2.11.0"
fun_run = "0.2.0"
nom = "7.1.3"
regex = "1.10.4"
reqwest = { version = "0.12.4", features = ["blocking"] }
tar = "0.4.40"
tempfile = "3.10.1"
thiserror = "1.0.61"
bullet_stream = "0.2.0"
---
use bullet_stream::{style, Print};
use clap::Parser;
use fun_run::CommandWithName;
use indoc::formatdoc;
use shared::{output_tar_path, source_dir, BaseImage, CpuArch, RubyDownloadVersion};
use std::{error::Error, path::PathBuf, process::Command};

static INNER_OUTPUT: &str = "/tmp/output";

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: CpuArch,

    #[arg(long)]
    version: RubyDownloadVersion,

    #[arg(long = "base-image")]
    base_image: BaseImage,
}

fn ruby_check(args: &RubyArgs) -> Result<(), Box<dyn Error>> {
    let RubyArgs {
        arch,
        version,
        base_image,
    } = args;
    let log = Print::new(std::io::stderr()).h1(format!(
        "Checking Ruby version ({version} linux/{arch}) for {base_image}",
    ));
    let path = output_tar_path(&PathBuf::from(INNER_OUTPUT), version, base_image, arch);
    let distro_number = base_image.distro_number();

    let image_name = format!("heroku/heroku:{distro_number}-build");
    let outside_output = source_dir().join("output");

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
        &[
            "mkdir /tmp/unzipped",
            &format!("tar xzf {} -C /tmp/unzipped", path.display()),
            "echo -n '- Rubygems version: '",
            "tmp/unzipped/bin/gem -v",
            "echo -n '- Ruby version: '",
            "/tmp/unzipped/bin/ruby -v",
        ]
        .join(" && "),
    );

    let mut cmd_stream = log.bullet("Versions");

    let result = cmd_stream.stream_with(
        format!("Running {}", style::command(cmd.name())),
        |stdout, stderr| cmd.stream_output(stdout, stderr),
    )?;

    cmd_stream.done().done();
    eprintln!();

    // Print results to STDOUT for github summary
    println!("## Ruby {version} linux/{arch} for {base_image}");
    println!();
    println!("{}", result.stdout_lossy());

    Ok(())
}

fn main() {
    let args = RubyArgs::parse();
    if let Err(error) = ruby_check(&args) {
        Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
        std::process::exit(1);
    }
}
