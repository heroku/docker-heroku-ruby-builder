use bullet_stream::state::SubBullet;
use bullet_stream::Print;
use fs_err::{File, PathExt};
use fun_run::CommandWithName;
use inventory::artifact::Arch;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

mod base_image;
mod download_ruby_version;
mod inventory_help;

pub use base_image::BaseImage;
pub use download_ruby_version::RubyDownloadVersion;
pub use inventory_help::{
    artifact_is_different, artifact_same_url_different_checksum, atomic_inventory_update,
    inventory_check, sha256_from_path, ArtifactMetadata,
};

/// Appends the given string after the filename and before the `ends_with`
///
/// ```
/// use std::path::Path;
/// use shared::append_filename_with;
///
/// let path = Path::new("/tmp/file.txt");
/// let out = append_filename_with(path, "-lol", ".txt").unwrap();
/// assert_eq!(Path::new("/tmp/file-lol.txt"), out);
/// ```
///
/// Raises an error if the files doesn't exist or if the file name doesn't end with `ends_with`
pub fn append_filename_with(path: &Path, append: &str, ends_with: &str) -> Result<PathBuf, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Other(format!("Cannot determine parent from {}", path.display())))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            Error::Other(format!(
                "Cannot determine file name from {}",
                path.display()
            ))
        })?
        .to_string_lossy();

    if !file_name.ends_with(ends_with) {
        Err(Error::Other(format!(
            "File name {} does not end with {}",
            file_name, ends_with
        )))?;
    }
    let file_base = file_name.trim_end_matches(ends_with);

    Ok(parent.join(format!("{file_base}{append}{ends_with}")))
}

#[derive(Debug, Clone)]
pub struct TarDownloadPath(pub PathBuf);

impl AsRef<Path> for TarDownloadPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

pub fn untar_to_dir(tar_path: &TarDownloadPath, workspace: &Path) -> Result<(), Error> {
    fs_err::create_dir_all(workspace).map_err(Error::FsError)?;

    // Shelling out due to https://github.com/alexcrichton/tar-rs/issues/369
    let mut cmd = Command::new("bash");
    cmd.arg("-c");
    cmd.arg(format!(
        "tar xzf {tar_file} -C {out_dir}",
        tar_file = tar_path.as_ref().display(),
        out_dir = workspace.display()
    ));
    cmd.named_output().map_err(Error::CmdError)?;

    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Command failed {0}")]
    CmdError(fun_run::CmdError),

    #[error("Invalid version {0} for stack {1}")]
    InvalidVersionForStack(String, String),

    #[error("Cannot convert to integer {0}")]
    ParseIntError(std::num::ParseIntError),

    #[error("Failed to download {0}")]
    FailedRequest(reqwest::Error),

    #[error("Invalid ruby version {version} reason: {reason}")]
    InvalidVersion { version: String, reason: String },

    #[error("Error {0}")]
    FsError(std::io::Error),

    #[error("Could not copy body from {url} to {file} due to error {source}")]
    UrlToFileError {
        url: String,
        file: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Error {0}")]
    Other(String),
}

pub fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .fs_err_canonicalize()
        .expect("Canonicalize source dir")
}

pub fn validate_version_for_stack(
    ruby_version: &RubyDownloadVersion,
    base_image: &BaseImage,
) -> Result<(), Error> {
    // https://bugs.ruby-lang.org/issues/18658
    if base_image.name() == "heroku-22" && ruby_version.major >= 3 && ruby_version.minor == 0 {
        return Err(Error::InvalidVersionForStack(
            ruby_version.bundler_format(),
            base_image.to_string(),
        ));
    }

    Ok(())
}

pub fn download_tar(url: &str, path: &TarDownloadPath) -> Result<(), Error> {
    let mut dest = fs_err::File::create(path.as_ref()).map_err(Error::FsError)?;

    let client = reqwest::blocking::Client::new();
    let mut response = client.get(url).send().map_err(Error::FailedRequest)?;
    std::io::copy(&mut response, &mut dest).map_err(|err| Error::UrlToFileError {
        url: url.to_string(),
        file: path.as_ref().to_path_buf(),
        source: err,
    })?;

    response.error_for_status().map_err(Error::FailedRequest)?;
    Ok(())
}

pub fn output_tar_path(
    output: &Path,
    version: &RubyDownloadVersion,
    base_image: &BaseImage,
    cpu_architecture: &Arch,
) -> PathBuf {
    let directory = if base_image.is_arch_aware() {
        PathBuf::from(base_image.to_string()).join(cpu_architecture.to_string())
    } else {
        PathBuf::from(base_image.to_string())
    };

    output
        .join(directory)
        .join(format!("ruby-{}.tgz", version.bundler_format()))
}

pub fn tar_dir_to_file(compiled_dir: &Path, tar_file: &File) -> Result<(), Error> {
    let enc = flate2::write::GzEncoder::new(tar_file, flate2::Compression::best());

    let mut tar = tar::Builder::new(enc);
    // When set to true,  `follow_symlinks` will duplicate internal symlinks which increases the resulting file size
    tar.follow_symlinks(false);
    tar.append_dir_all("", compiled_dir)
        .map_err(Error::FsError)?;
    tar.finish().map_err(Error::FsError)?;

    Ok(())
}

// # Binstubs have a "shebang" on the first line that tells the OS
// # how to execute the file if it's called directly i.e. `$ ./script.rb` instead
// # of `$ ruby ./script.rb`.
// #
// # We need the shebang to be portable (not use an absolute path) so we check
// # for any ruby shebang lines and replace them with `#!/usr/bin/env ruby`
// # which translates to telling the os "Use the `ruby` executable from the same
// location as `which ruby`" to run this program.
pub fn update_shebangs_in_dir<W>(
    mut log: Print<SubBullet<W>>,
    path: &Path,
) -> Result<Print<SubBullet<W>>, Error>
where
    W: Send + Write + Sync + 'static,
{
    let dir = fs_err::read_dir(path).map_err(Error::FsError)?;
    for entry in dir {
        let entry = entry.map_err(Error::FsError)?;
        let entry_path = entry.path();
        if entry_path.is_file() {
            let mut file = fs_err::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&entry_path)
                .map_err(Error::FsError)?;
            let mut contents = String::new();

            log = log.sub_bullet(format!("Reading {}", entry_path.display()));
            if file.read_to_string(&mut contents).is_ok() {
                if let Some(contents) = update_shebang(contents) {
                    log = log.sub_bullet(format!("Updating shebang in {}", entry_path.display()));
                    file.seek(SeekFrom::Start(0)).map_err(Error::FsError)?;
                    file.write_all(contents.as_bytes())
                        .map_err(Error::FsError)?;
                } else {
                    log = log.sub_bullet("Skipping (no ruby shebang found)");
                }
            } else {
                log = log.sub_bullet("Skipping (possibly binary file)");
            }
        }
    }
    Ok(log)
}

pub fn update_shebang(contents: String) -> Option<String> {
    if let Some(shebang) = contents.lines().next() {
        if shebang.starts_with("#!") && shebang.contains("/ruby") {
            Some(contents.replacen(shebang, "#!/usr/bin/env ruby", 1))
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;
    use std::thread;
    use tempfile::tempdir;
    use tiny_http::{Response, Server};

    #[test]
    fn test_update_shebang_with_ruby_shebang() {
        let contents = "#!/path/to/ruby\nprint 'Hello, world!'\n";
        let updated_contents = update_shebang(contents.to_string());
        assert_eq!(
            updated_contents,
            Some("#!/usr/bin/env ruby\nprint 'Hello, world!'\n".to_string())
        );
    }

    #[test]
    fn test_update_shebang_without_ruby_shebang() {
        let contents = "#!/path/to/python\nprint('Hello, world!')\n";
        let updated_contents = update_shebang(contents.to_string());
        assert_eq!(updated_contents, None);
    }

    #[test]
    fn test_update_shebang_with_empty_contents() {
        let contents = "";
        let updated_contents = update_shebang(contents.to_string());
        assert_eq!(updated_contents, None);
    }

    #[test]
    fn test_update_shebangs_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.rb");

        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "#!/path/to/ruby\nprint 'Hello, world!'").unwrap();

        let log = Print::new(std::io::stdout())
            .without_header()
            .bullet("shebangs");
        _ = update_shebangs_in_dir(log, dir.path()).unwrap();

        let contents = fs_err::read_to_string(&file_path).unwrap();
        assert_eq!(contents, "#!/usr/bin/env ruby\nprint 'Hello, world!'\n");
    }

    #[test]
    fn test_validate_version_for_stack() {
        assert!(validate_version_for_stack(
            &RubyDownloadVersion::from_str("2.7.3").unwrap(),
            &BaseImage::new("heroku-22").unwrap()
        )
        .is_ok());

        assert!(validate_version_for_stack(
            &RubyDownloadVersion::from_str("3.0.0").unwrap(),
            &BaseImage::new("heroku-22").unwrap()
        )
        .is_err());
    }

    #[test]
    fn test_output_tar_path_pre_24() {
        let output = PathBuf::from("/tmp");
        let version = RubyDownloadVersion::from_str("2.7.3").unwrap();
        let base_image = BaseImage::new("heroku-20").unwrap();
        let cpu_architecture = Arch::Amd64;

        let tar_path = output_tar_path(&output, &version, &base_image, &cpu_architecture);

        // assert!(tar_path.is_absolute());
        assert_eq!(PathBuf::from("/tmp/heroku-20/ruby-2.7.3.tgz"), tar_path);
    }

    #[test]
    fn test_output_tar_path_post_24() {
        let output = PathBuf::from("/tmp");
        let version = RubyDownloadVersion::from_str("2.7.3").unwrap();
        let base_image = BaseImage::new("heroku-24").unwrap();
        let cpu_architecture = Arch::Amd64;

        let tar_path = output_tar_path(&output, &version, &base_image, &cpu_architecture);

        assert_eq!(
            PathBuf::from("/tmp/heroku-24/amd64/ruby-2.7.3.tgz"),
            tar_path
        );
    }

    #[test]
    fn test_download_tar() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", server.server_addr());

        let response = Response::from_string("Hello, world!");
        thread::spawn(move || {
            let _ = server.recv().unwrap().respond(response);
        });

        let dir = tempdir().unwrap();
        let tar_path = TarDownloadPath(dir.path().join("file.tar"));

        download_tar(&addr, &tar_path).unwrap();

        let mut file = fs_err::File::open(tar_path.as_ref()).unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();

        assert_eq!(contents, "Hello, world!");
    }

    #[test]
    fn test_download_tar_404() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", server.server_addr());

        let response = Response::empty(tiny_http::StatusCode(404));
        thread::spawn(move || {
            let _ = server.recv().unwrap().respond(response);
        });

        let dir = tempdir().unwrap();
        let tar_path = TarDownloadPath(dir.path().join("file.tar"));

        let result = download_tar(&addr, &tar_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_ruby_version_bundler_format() {
        assert_eq!(
            "3.1.2".to_string(),
            RubyDownloadVersion::new("3.1.2").unwrap().bundler_format()
        );

        assert_eq!(
            "3.1.2.preview1".to_string(),
            RubyDownloadVersion::new("3.1.2-preview1")
                .unwrap()
                .bundler_format()
        );
    }

    #[test]
    fn test_ruby_dir_format() {
        assert_eq!(
            "ruby-3.1.2".to_string(),
            RubyDownloadVersion::new("3.1.2").unwrap().dir_name_format()
        );

        assert_eq!(
            "ruby-3.1.2-preview1".to_string(),
            RubyDownloadVersion::new("3.1.2-preview1")
                .unwrap()
                .dir_name_format()
        );
    }

    #[test]
    fn bad_ruby_version() {
        assert!(RubyDownloadVersion::new("3.-1.2-preview1").is_err());
        assert!(RubyDownloadVersion::new("3.1").is_err());
        assert!(RubyDownloadVersion::new("3").is_err());
        assert!(RubyDownloadVersion::new("3-1").is_err());
    }

    #[test]
    fn test_ruby_download_url() {
        assert_eq!(
            "https://cache.ruby-lang.org/pub/ruby/3.0/ruby-3.0.2.tar.gz".to_string(),
            RubyDownloadVersion::new("3.0.2").unwrap().download_url()
        );

        assert_eq!(
            "https://cache.ruby-lang.org/pub/ruby/3.1/ruby-3.1.2.tar.gz".to_string(),
            RubyDownloadVersion::new("3.1.2").unwrap().download_url()
        );

        assert_eq!(
            "https://cache.ruby-lang.org/pub/ruby/3.1/ruby-3.1.2-preview1.tar.gz".to_string(),
            RubyDownloadVersion::new("3.1.2-preview1")
                .unwrap()
                .download_url()
        );
    }

    #[test]
    fn test_untar_to_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let temp_path = tempdir.path().join("ruby-3.3.1");
        fs_err::create_dir_all(&temp_path).unwrap();
        fs_err::write(temp_path.join("array.c"), "").unwrap();

        let bin_path = temp_path.join("bin");
        fs_err::create_dir_all(&bin_path).unwrap();
        fs_err::write(bin_path.join("gem"), "").unwrap();

        let temptar_dir = tempfile::tempdir().unwrap();
        let tar_path = temptar_dir.path().join("ruby-source-3.3.1.tgz");

        tar_dir_to_file(tempdir.path(), &fs_err::File::create(&tar_path).unwrap()).unwrap();

        let temp_out = tempfile::tempdir().unwrap();
        untar_to_dir(&TarDownloadPath(tar_path), temp_out.path()).unwrap();

        let filenames = filenames_in_path(temp_out.as_ref());
        assert_eq!(vec!["ruby-3.3.1".to_string()], filenames);

        let filenames = filenames_in_path(&temp_out.path().join("ruby-3.3.1"));
        assert!(filenames.iter().any(|name| *name == "array.c"));
    }

    fn filenames_in_path(path: &Path) -> Vec<String> {
        let mut filenames = fs_err::read_dir(path)
            .unwrap()
            .filter_map(|entry| {
                entry.ok().and_then(|e| {
                    e.path()
                        .file_name()
                        .and_then(|n| n.to_str().map(String::from))
                })
            })
            .collect::<Vec<String>>();

        filenames.sort();
        filenames
    }
}
