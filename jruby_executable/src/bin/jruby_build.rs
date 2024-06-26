use bullet_stream::{style, Print};
use clap::Parser;
use fs_err::PathExt;
use indoc::formatdoc;
use jruby_executable::jruby_build_properties;
use shared::{download_tar, tar_dir_to_file, untar_to_dir, BaseImage, TarDownloadPath};
use std::{error::Error, path::PathBuf};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: String,

    #[arg(long)]
    base_image: BaseImage,
}

fn source_dir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");

    fs_err::canonicalize(path).expect("Canonicalize source dir")
}

fn jruby_build(args: &Args) -> Result<(), Box<dyn Error>> {
    let Args {
        version,
        base_image,
    } = args;

    let mut log = Print::new(std::io::stderr()).h1("Building JRuby");
    let volume_cache_dir = source_dir().join("cache");
    let volume_output_dir = source_dir().join("output");

    fs_err::create_dir_all(&volume_cache_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let extracted_path = temp_dir.path().join("extracted");

    let ruby_stdlib_version = jruby_build_properties(version)?.ruby_stdlib_version()?;

    let download_path =
        TarDownloadPath(volume_cache_dir.join(format!("jruby-dist-{version}-bin.tar.gz")));

    if download_path.as_ref().fs_err_try_exists()? {
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
            for path in glob::glob(&jruby_dir.join("bin").join(pattern).to_string_lossy())?
                .collect::<Result<Vec<_>, _>>()?
            {
                bullet = bullet.sub_bullet(format!("Remove {}", path.display()));
                fs_err::remove_file(path)?;
            }
        }

        let path = jruby_dir.join("lib").join("target");
        if path.fs_err_try_exists()? {
            bullet = bullet.sub_bullet(format!("Remove recursive {}", path.display()));
            fs_err::remove_dir_all(&path)?;
        }

        bullet.done()
    };

    log = {
        let bullet = log.bullet("Create ruby symlink to jruby");
        fs_err::os::unix::fs::symlink("jruby", jruby_dir.join("bin/ruby"))?;

        bullet.done()
    };

    let tgz_name = format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz");

    log = {
        let mut bullet = log.bullet("Creating tgz archives");
        let tar_dir = volume_output_dir.join(base_image.to_string());

        fs_err::create_dir_all(&tar_dir)?;

        let tar_file = fs_err::File::create(tar_dir.join(&tgz_name))?;

        let timer = bullet.start_timer(format!("Write {}", tar_file.path().display()));
        tar_dir_to_file(&jruby_dir, &tar_file)?;
        bullet = timer.done();

        for arch in &["amd64", "arm64"] {
            let dir = volume_output_dir.join(base_image.to_string()).join(arch);
            fs_err::create_dir_all(&dir)?;

            let path = dir.join(&tgz_name);
            bullet = bullet.sub_bullet(format!("Write {}", path.display()));
            fs_err::copy(tar_file.path(), &path)?;
        }

        bullet.done()
    };
    log.done();

    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_build(&args) {
        Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
        std::process::exit(1);
    }
}
