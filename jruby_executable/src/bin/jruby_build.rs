use bullet_stream::{style, Print};
use clap::Parser;
use fs_err::PathExt;
use inside_docker::{download_tar, tar_dir_to_file, untar_to_dir, BaseImage, TarDownloadPath};
use jruby_executable::jruby_build_properties;

use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: String,

    #[arg(long)]
    base_image: BaseImage,
}

#[derive(Debug, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
enum Error {
    #[error("Cannot create temp dir {0}")]
    CreateTmpDir(std::io::Error),

    #[error("{0}")]
    HerokuError(#[from] inside_docker::Error),

    #[error("{0}")]
    LibError(#[from] jruby_executable::Error),

    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    BadPattern(#[from] glob::PatternError),

    #[error("{0}")]
    GlobPathError(#[from] glob::GlobError),
}

fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("Canonicalize path")
}

fn jruby_build(args: &Args) -> Result<(), Error> {
    let Args {
        version,
        base_image,
    } = args;

    let mut log = Print::new(std::io::stderr()).h1("Building JRuby");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    fs_err::create_dir_all(&volume_cache_dir).map_err(Error::IoError)?;

    let temp_dir = tempfile::tempdir().map_err(Error::CreateTmpDir)?;
    let extracted_path = temp_dir.path().join("extracted");

    let ruby_stdlib_version = jruby_build_properties(version)
        .map_err(Error::LibError)?
        .ruby_stdlib_version()
        .map_err(Error::LibError)?;

    let download_path =
        TarDownloadPath(volume_cache_dir.join(format!("jruby-dist-{version}-bin.tar.gz")));

    if download_path
        .as_ref()
        .fs_err_try_exists()
        .map_err(Error::IoError)?
    {
        log = log
            .bullet(format!(
                "Using cached JRuby archive {}",
                download_path.as_ref().display()
            ))
            .done();
    } else {
        let url =  format!("https://repo1.maven.org/maven2/org/jruby/jruby-dist/{version}/jruby-dist-{version}-bin.tar.gz");
        let timer = log
            .bullet("Download JRuby")
            .sub_bullet(format!("To {}", download_path.as_ref().to_string_lossy()))
            .sub_bullet(format!("From {}", style::url(&url)))
            .start_timer("Downloading");
        download_tar(&url, &download_path)?;

        log = timer.done().done();
    }

    untar_to_dir(&download_path, &extracted_path)?;

    let jruby_dir = extracted_path.join(format!("jruby-{version}"));

    log = {
        let mut bullet = log.bullet("Removing unnecessary files");
        for pattern in &["*.bat", "*.dll", "*.exe"] {
            for path in glob::glob(&jruby_dir.join("bin").join(pattern).to_string_lossy())
                .map_err(Error::BadPattern)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::GlobPathError)?
            {
                bullet = bullet.sub_bullet(format!("Remove {}", path.display()));
                fs_err::remove_file(path).map_err(Error::IoError)?;
            }
        }

        let path = jruby_dir.join("lib").join("target");
        if path.fs_err_try_exists().map_err(Error::IoError)? {
            bullet = bullet.sub_bullet(format!("Remove recursive {}", path.display()));
            fs_err::remove_dir_all(&path).map_err(Error::IoError)?;
        }

        bullet.done()
    };

    log = {
        let bullet = log.bullet("Create ruby symlink to jruby");
        fs_err::os::unix::fs::symlink("jruby", jruby_dir.join("bin/ruby")).unwrap();

        bullet.done()
    };

    let tgz_name = format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz");

    log = {
        let mut bullet = log.bullet("Creating tgz archives");
        let tar_dir = volume_output_dir
            .join(base_image.to_string())
            .join(&tgz_name);

        let timer = bullet.start_timer(format!("Write {}", tar_dir.display()));
        tar_dir_to_file(
            &jruby_dir,
            fs_err::File::create(&tar_dir).map_err(Error::IoError)?,
        )?;
        bullet = timer.done();

        for arch in &["amd64", "arm64"] {
            let path = volume_output_dir
                .join(base_image.to_string())
                .join(arch)
                .join(&tgz_name);

            fs_err::create_dir_all(path.parent().expect("Parent dir")).map_err(Error::IoError)?;

            bullet = bullet.sub_bullet(format!("Write {}", path.display()));
            fs_err::copy(&tar_dir, &path).map_err(Error::IoError)?;
        }

        bullet.done()
    };
    log.done();

    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_build(&args) {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}
