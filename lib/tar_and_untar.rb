# We are handling two different kinds of file compression schemes
#
# Ruby's source puts everything in a folder with the name of the ruby version being downloaded  (notice it is `ruby-3.0.2/.bundle` and not `.bundle`):
#
# ```
# # From https://ftp.ruby-lang.org/pub/ruby/3.0/ruby-3.0.2.tar.gz
# $ tar -tvf /Users/rschneeman/Downloads/ruby-3.0.2.tar.gz
# drwxr-xr-x  0 ruby   ruby        0 Jul  7  2021 ruby-3.0.2/.bundle/
# drwxr-xr-x  0 ruby   ruby        0 Jul  7  2021 ruby-3.0.2/.bundle/gems/
# drwxr-xr-x  0 ruby   ruby        0 Jul  7  2021 ruby-3.0.2/.bundle/gems/minitest-5.14.2/
# ```
#
# After we build this into a binary heroku stores the compressed file it in a different format.
# The heroku archives omit this top level directory (notice it is bin/ and not `ruby-3.1.2/bin`):
#
# ```
# # From https://s3-external-1.amazonaws.com/heroku-buildpack-ruby/heroku-22/ruby-3.1.2.tgz
# $ tar -tvf /Users/rschneeman/Downloads/ruby-3.1.2.tgz
# drwxr-xr-x  0 root   root        0 Apr 12  2022 bin/
# -rwxr-xr-x  0 root   root      619 Apr 12  2022 bin/ri
# -rwxr-xr-x  0 root   root    18544 Apr 12  2022 bin/ruby
# -rwxr-xr-x  0 root   root      658 Apr 12  2022 bin/bundler
# -rwxr-xr-x  0 root   root      617 Apr 12  2022 bin/erb
# -rwxr-xr-x  0 root   root      656 Apr 12  2022 bin/bundle
# -rwxr-xr-x  0 root   root      623 Apr 12  2022 bin/rake
# ```
#
# When we download and unzip a file from Ruby core we need to make sure we move into the top level directory.
#
# When we produce a zipped ruby executable, we need to make sure that the top level contains multiple directories/files
# and is not a singular placeholder directory

# Uncompresses the **contents** of a file into a directory
#
# The `tar_file` should be a path with a file extension like `/output/example/ruby-3.1.2.tgz`
# The `dest_directory` is a path where contents from files inside of the tar file will go
#
# Example:
#
# Given a tar archive:
#
# ```
#   # Inspect the archive
#   $ tar -tvf archive.tgz
#     bin/gem
#     bin/rdoc
# ```
#
# This function will extract it:
#
# ```
# tar_dir(
#   tar_file: "archive.tgz",
#   dest_directory: "/tmp/output"
# )
# ```
#
# Into the same format
#
#   $ ls /tmp/output
#     bin/gem
#     bin/rdoc
#
def untar_to_dir(tar_file:, dest_directory:, io: $stdout)
  tar_file = Pathname(tar_file).expand_path
  dest_directory = Pathname(dest_directory).expand_path

  pipe("tar xzf #{tar_file} -C #{dest_directory}", io: io)
end

# Compress the **contents** a directory into a tar file (to Heroku archive format)
#
# The `dir_to_tar` is a path that contains the files you care about. The name of the directory being tar-d does not affect the outcome
# The `destination_file` should be a path with a file extension like `/output/example/ruby-3.1.2.tgz`
#
# Example:
#
# A directory with files
#
#   $ ls /tmp/input
#     bin/gem
#     bin/rdoc
#
# Compressed by this function:
#
# ```
# tar_dir(
#   dir_to_tar: "/tmp/input",
#   destination_file: "archive.tgz"
# )
# ```
#
# Contains the same files with the same structure:
#
# ```
#   # Inspect the archive
#   $ tar -tvf archive.tgz
#     bin/gem
#     bin/rdoc
# ```
def tar_dir(dir_to_tar:, destination_file:, io: $stdout)
  destination_file = Pathname(destination_file).expand_path

  pipe("cd #{dir_to_tar} && tar czf #{destination_file} * && cd -", io: io)
end
