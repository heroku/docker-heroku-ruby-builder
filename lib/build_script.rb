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
  io: $stdout,
  workspace_dir: ARGV[0],
  output_dir: ARGV[1],
  cache_dir: ARGV[2],
  stack: ENV.fetch("STACK"),
  ruby_version: ENV.fetch("STACK")
)

  ruby_version = RubyVersion.new(ruby_version)

  # The destination location of the built ruby version is the `prefix`
  prefix = Pathname("/app/vendor/#{ruby_version.plain_file_name}")
  io.puts "Using prefix: #{prefix}"

  # create cache dir if it doesn't exist
  FileUtils.mkdir_p(cache_dir)

  check_version_on_stack(
    stack: stack,
    ruby_version: ruby_version
  )

  download_to_cache(
    io: io,
    cache_dir: cache_dir,
    ruby_version: ruby_version
  )

  build(
    io: io,
    stack: stack,
    prefix: prefix,
    cache_dir: cache_dir,
    ruby_version: ruby_version
  )

  fix_binstubs_in_dir(
    io: io,
    dir: prefix.join("bin")
  )

  move_to_output(
    io: io,
    stack: stack,
    prefix: prefix,
    output_dir: output_dir,
    ruby_version: ruby_version
  )
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
def download_to_cache(cache_dir:, ruby_version:, io: $stdout)
  Dir.chdir(cache_dir) do
    url = ruby_version.download_url
    uri = URI.parse(url)
    filename = uri.to_s.split("/").last

    io.puts "Downloading #{url}"

    if File.exist?(filename)
      io.puts "Using #{filename}"
    else
      io.puts "Fetching #{filename}"
      run!("curl #{uri} -s -O")
    end
  end
end

# Compiles the ruby program and puts it into `prefix`
def build(stack:, prefix:, cache_dir:, ruby_version:, jobs: DEFAULT_JOBS, io: $stdout)
  build_dir = Pathname(".")
  untar_to_dir(
    tar_file: Pathname(cache_dir).join("#{ruby_version.plain_file_name}.tar.gz"),
    dest_directory: build_dir
  )

  # Move into the directory we just unziped and run `make`
  # We tell make where to put the result with the `prefix` argument
  Dir.chdir(build_dir.join(ruby_version.plain_file_name)) do
    command = make_commands(
      jobs: jobs,
      prefix: prefix,
      ruby_version: ruby_version
    )
    pipe(command)
  end
end

# After a ruby is compiled, this function will move it to the directory
# that docker was given so it's available when the container exits
def move_to_output(output_dir:, stack:, ruby_version:, prefix:, io: $stdout)
  destination = Pathname(output_dir)
    .join(stack)
    .tap { |path| path.mkpath }
    .join(ruby_version.tar_file_name_output)

  io.puts "Writing #{destination}"
  tar_dir(
    io: io,
    dir_to_tar: prefix,
    destination_file: destination
  )
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
