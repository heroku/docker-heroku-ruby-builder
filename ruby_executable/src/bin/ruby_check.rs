use bullet_stream::{Print, global::print, style};
use clap::Parser;
use fun_run::CommandWithName;
use indoc::formatdoc;
use libherokubuildpack::inventory::artifact::Arch;
use shared::{BaseImage, RubyDownloadVersion, output_tar_path, source_dir};
use std::{error::Error, path::PathBuf, process::Command, time::Instant};

static INNER_OUTPUT: &str = "/tmp/output";

#[derive(Parser, Debug)]
struct RubyArgs {
    #[arg(long)]
    arch: Arch,

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
    let start = Instant::now();
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
        [
            "mkdir /tmp/unzipped",
            &format!("tar xzf {} -C /tmp/unzipped", path.display()),
            "echo -n '- Rubygems version: '",
            "tmp/unzipped/bin/gem -v",
            "echo -n '- Ruby version: '",
            "/tmp/unzipped/bin/ruby -v",
        ]
        .join(" && "),
    );

    print::bullet("Versions");
    let output = print::sub_stream_cmd(cmd)?;

    print::all_done(&Some(start));
    print::plain("");

    // Print results to STDOUT for github summary
    println!("## Ruby {version} linux/{arch} for {base_image}");
    println!();
    println!("{}", output.stdout_lossy());

    Ok(())
}

fn main() {
    let args = RubyArgs::parse();
    if let Err(error) = ruby_check(&args) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
        std::process::exit(1);
    }
}
