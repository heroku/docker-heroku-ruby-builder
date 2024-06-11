use bullet_stream::{style, Print};
use clap::Parser;
use fs_err::PathExt;
use fun_run::CommandWithName;
use inside_docker::{
    download_tar, output_tar_path, tar_dir_to_file, untar_to_dir, update_shebangs_in_dir,
    validate_version_for_stack, BaseImage, CpuArch, Error, RubyDownloadVersion, TarDownloadPath,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: RubyDownloadVersion,

    #[arg(long)]
    cache: PathBuf,

    #[arg(long)]
    output: PathBuf,

    #[arg(long = "base-image")]
    base_image: BaseImage,
}

fn main() {
    let args = Args::parse();
    if let Err(error) = make_ruby(args) {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}

fn make_ruby(args: Args) -> Result<(), Error> {
    let Args {
        base_image,
        cache,
        output,
        version,
    } = args;
    let arch = CpuArch::from_system().map_err(Error::UnknownArchitecture)?;

    let mut log = Print::new(std::io::stderr()).h1(format!(
        "Running make_ruby.rs (Ruby {version} linux/{arch}) for {base_image}",
    ));

    fs_err::create_dir_all(&cache).map_err(Error::FsError)?;
    let tempdir = tempfile::tempdir().map_err(Error::FsError)?;
    let source_dir = tempdir.path().join("source");
    let compiled_dir = tempdir.path().join("compiled");
    let download_tar_path = TarDownloadPath(cache.join(format!("ruby-source-{version}.tgz")));

    let output_tar_file = {
        let path = output_tar_path(&output, &version, &base_image, &arch);

        fs_err::create_dir_all(path.parent().expect("Tar file in a dir"))
            .map_err(Error::FsError)?;

        fs_err::File::create(path).map_err(Error::FsError)
    }?;

    validate_version_for_stack(&version, &base_image)?;

    log = if Path::fs_err_try_exists(download_tar_path.as_ref()).map_err(Error::FsError)? {
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
    log = log
        .bullet(format!("Untar to {}", source_dir.display()))
        .done();
    untar_to_dir(&download_tar_path, &source_dir)?;

    log = {
        let path = source_dir.join(version.dir_name_format());
        let bullet = log.bullet(format!(
            "Change directory to {}",
            style::value(path.to_str().expect("Path to str"))
        ));

        std::env::set_current_dir(path).map_err(Error::FsError)?;

        bullet.done()
    };

    log = {
        let mut bullet = log.bullet("Configure");

        let mut cmd = Command::new("./configure");
        cmd.args(configure_args(&compiled_dir, &version));
        cmd.env("debugflags", "-g");

        let envs = cmd
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
            .collect::<HashMap<_, _>>();
        let cmd_name_with_envs =
            fun_run::display_with_env_keys(cmd.mut_cmd(), envs, ["debugflags"]);

        bullet
            .stream_with(
                format!("Running {}", style::command(cmd_name_with_envs)),
                |stdout, stderr| cmd.stream_output(stdout, stderr),
            )
            .map_err(Error::CmdError)?;
        bullet.done()
    };

    log = {
        let mut bullet = log.bullet("Make");

        let mut cmd = Command::new("make");
        cmd.arg(format!("-j{}", num_cpus::get()));
        bullet
            .stream_with(
                format!("Running {}", style::command(cmd.name())),
                |stdout, stderr| cmd.stream_output(stdout, stderr),
            )
            .map_err(Error::CmdError)?;
        bullet.done()
    };

    log = {
        let mut bullet = log.bullet("Make install");

        let mut cmd = Command::new("make");
        cmd.arg("install");

        bullet
            .stream_with(
                format!("Running {}", style::command(cmd.name())),
                |stdout, stderr| cmd.stream_output(stdout, stderr),
            )
            .map_err(Error::CmdError)?;

        bullet.done()
    };

    log = update_shebangs_in_dir(log.bullet("Update shebangs"), &compiled_dir.join("bin"))?.done();

    log = log
        .bullet(format!(
            "Write tarball {}",
            output_tar_file.path().display()
        ))
        .done();

    tar_dir_to_file(&compiled_dir, &output_tar_file)?;
    log.done();

    Ok(())
}

fn configure_args<'a>(compiled_dir: &'a Path, version: &'a RubyDownloadVersion) -> Vec<&'a str> {
    let mut configure_args = vec![
        "--disable-install-doc",
        "--enable-load-relative",
        "--enable-shared",
        // Tell make where to put the compiled files with the --prefix option
        "--prefix",
        compiled_dir.to_str().expect("path to string"),
    ];

    if version.major > 3 || (version.major == 3 && version.minor >= 2) {
        configure_args.push("--enable-yjit");
    }
    configure_args
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_configure_args_yjit() {
        let version = RubyDownloadVersion::new("3.3.0").unwrap();
        let compiled_dir = Path::new("/tmp/ruby");
        let args = configure_args(compiled_dir, &version);
        assert_eq!(
            args,
            vec![
                "--disable-install-doc",
                "--enable-load-relative",
                "--enable-shared",
                "--prefix",
                "/tmp/ruby",
                "--enable-yjit"
            ]
        );
    }

    #[test]
    fn test_configure_args_no_yjit() {
        let version = RubyDownloadVersion::new("3.0.0").unwrap();
        let compiled_dir = Path::new("/tmp/ruby");
        let args = configure_args(compiled_dir, &version);
        assert_eq!(
            args,
            vec![
                "--disable-install-doc",
                "--enable-load-relative",
                "--enable-shared",
                "--prefix",
                "/tmp/ruby"
            ]
        );
    }
}
