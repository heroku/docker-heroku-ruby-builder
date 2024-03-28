require "pathname"
require "tmpdir"
require "fileutils"
require "uri"

require "ruby_version"
require "tar_and_untar"

$stdout.sync = true

# Returns the number of processors available
def nproc
  @nproc ||= run!("nproc").strip
end

# Execute a shell command and fail if it returns a non-zero exit
def run!(cmd)
  result = `#{cmd}`
  raise "Error running #{cmd}, result: #{result}" unless $?.success?
  result
end

# Parallelization factor when running `make`
DEFAULT_JOBS = ENV.fetch("JOBS", nproc)

# Build logic in a method
def run_build_script(
  architecture:,
  io: $stdout,
  workspace_dir: ARGV[0],
  output_dir: ARGV[1],
  cache_dir: ARGV[2],
  stack: ENV.fetch("STACK"),
  ruby_version: ENV.fetch("STACK")
)
  parts = VersionParts.new(ruby_version)
  ruby_version = RubyVersion.new(ruby_version)

  # The directory where ruby source will be downloaded
  ruby_source_dir = Pathname(".")

  # create cache dir if it doesn't exist
  FileUtils.mkdir_p(cache_dir)

  check_version_on_stack(
    stack: stack,
    ruby_version: ruby_version
  )

  tar_file = download_to_cache(
    io: io,
    cache_dir: cache_dir,
    download_url: DownloadRuby.new(parts: parts).url
  )

  untar_to_dir(
    tar_file: tar_file,
    dest_directory: ruby_source_dir
  )

  Dir.mktmpdir do |tmp_dir|
    # The directory where Ruby will be built into
    ruby_binary_dir = Pathname(tmp_dir).join("prefix")

    build(
      io: io,
      ruby_version: ruby_version,
      destination_dir: ruby_binary_dir,
      ruby_source_dir: ruby_source_dir
    )

    fix_binstubs_in_dir(
      io: io,
      dir: ruby_binary_dir.join("bin")
    )

    destination = stack_architecture_tar_file_name(
      stack: stack,
      output_dir: output_dir,
      architecture: architecture,
      tar_file_name_output: ruby_version.tar_file_name_output
    )

    io.puts "Writing #{destination}"
    tar_dir(
      io: io,
      dir_to_tar: ruby_binary_dir,
      destination_file: destination
    )
  end
end

# Returns a Pathname to the destination tar file
#
# The directory structure corresponds to the S3 directory structure directly
def stack_architecture_tar_file_name(stack:, output_dir:, tar_file_name_output:, architecture:)
  output_stack_dir = Pathname(output_dir).join(stack)

  case stack
  when "heroku-24"
    output_stack_dir.join(architecture)
  else
    output_stack_dir
  end.tap(&:mkpath).join(tar_file_name_output)
end

# Runs a command on the command line and streams the results
def pipe(command, io: $stdout)
  output = ""
  IO.popen(command) do |stream|
    until stream.eof?
      buffer = stream.gets
      output << buffer
      io.puts buffer
    end
  end

  raise "Command failed #{command}" unless $?.success?

  output
end

# Guard against known errors
def check_version_on_stack(ruby_version:, stack:)
  # https://bugs.ruby-lang.org/issues/18658
  if stack == "heroku-22" && ruby_version <= Gem::Version.new("3.0")
    raise "Cannot build Ruby 3.0 on heroku-22"
  end
end

# Downloads the given ruby version into the cache direcory
#
# Returns a path to the file just downloaded
def download_to_cache(cache_dir:, download_url:, io: $stdout)
  file = Pathname(cache_dir).join(download_url.split("/").last)

  if file.exist?
    io.puts "Using cached #{file} (instead of downloading #{download_url})"
  else
    io.puts "Fetching #{file} (from #{download_url})"
    run!("curl #{download_url} -s -o #{file}")
  end

  file
end

# Compiles the ruby program and puts it into `prefix`
# input a tar file
def build(ruby_source_dir:, destination_dir:, ruby_version:, jobs: DEFAULT_JOBS, io: $stdout)
  # Move into the directory we just unziped and run `make`
  # We tell make where to put the result with the `prefix` argument
  Dir.chdir(ruby_source_dir.join(ruby_version.ruby_source_dir_name)) do
    command = make_commands(
      jobs: jobs,
      prefix: destination_dir,
      ruby_version: ruby_version
    )
    pipe(command)
  end
end

# Generates the `make` commands that will build ruby
# this is split up from running the commands to make testing easiers
def make_commands(prefix:, ruby_version:, jobs: DEFAULT_JOBS, io: $stdout)
  configure_opts = [
    "--disable-install-doc",
    "--prefix #{prefix}",
    "--enable-load-relative",
    "--enable-shared"
  ]

  if ruby_version >= Gem::Version.new("3.2")
    configure_opts << "--enable-yjit"
  end
  configure_opts = configure_opts.join(" ")

  configure_env = "debugflags=\"-g\""

  io.puts "configure env:  #{configure_env}"
  io.puts "configure opts: #{configure_opts}"
  cmds = [
    "#{configure_env} ./configure #{configure_opts}",
    "make -j#{jobs}",
    "make install"
  ]

  cmds.join(" && ")
end

# Binstubs have a "shebang" on the first line that tells the os
# how to execute the file if it's called directly i.e. `$ ./script.rb` instead
# of `$ ruby ./script.rb`.
#
# We need the shebang to be portable (not use an absolute path) so we check
# for any ruby shebang lines and replace them with `#!/usr/bin/env ruby`
# which translates to telling the os "Use the `ruby` as `which ruby`" to run
# this program
def fix_binstubs_in_dir(dir:, io: $stdout)
  Pathname(dir)
    .children
    .filter { |entry| entry.file? }
    .each do |entry|
    entry = entry.expand_path
    shebang = entry.open.readline

    # Check if binstub is a binary file
    # And has a shebang line https://rubular.com/r/qG63pPmTrX9wVr
    if shebang.force_encoding("UTF-8").valid_encoding? && shebang.match?(/#!.*\/ruby/)
      io.puts "Updating binstub for #{entry}"

      lines = entry.readlines
      lines.shift
      lines.unshift("#!/usr/bin/env ruby\n")
      entry.write(lines.join(""))
    end
  end
end

def get_architecture(system_output: `arch`, success: $?.success?)
  raise "Error running `arch`: #{system_output}" unless success

  case system_output.strip
  when "x86_64"
    "amd64"
  when "aarch64"
    "arm64"
  else
    raise "Unknown architecture: #{system_output}"
  end
end
