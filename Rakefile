$LOAD_PATH.unshift File.expand_path("../lib", __FILE__)
require "ruby_version"
require "changelog"
require "docker_command"

require "fileutils"

S3_BUCKET_NAME = "heroku-buildpack-ruby"

desc "Generate a new ruby shell script"
task :new, [:version, :stack, :patch] do |t, args|
  write_file = proc do |version, stack, patch = false|
    file =
      if patch
        patch_name = File.basename(patch, File.extname(patch))
        "rubies/#{args[:stack]}/ruby-#{args[:version]}-#{patch_name}.sh"
      else
        "rubies/#{args[:stack]}/ruby-#{args[:version]}.sh"
      end
    puts "Writing #{file}"
    FileUtils.mkdir_p(File.dirname(file))
    File.open(file, "w") do |file|
      file.puts <<~FILE
        #!/bin/bash

        source `dirname $0`/../common.sh

        docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=#{args[:version]}#{patch ? " -e PATCH_URL=#{patch}" : " "} -e STACK=#{args[:stack]} hone/ruby-builder:#{args[:stack]}
      FILE
    end
    File.chmod(0o775, file)
  end

  write_file.call(args[:version], args[:stack], args[:patch])
end

desc "Output the Rubygems version for a given binary"
task :rubygems_version, [:version, :stack] do |t, args|
  command = DockerCommand.gem_version_from_tar(
    stack: args[:stack],
    ruby_version: RubyVersion.new(args[:version])
  )
  puts "Running: #{command}"
  pipe(command)
end

desc "Emits a changelog message"
task :changelog, [:version] do |_, args|
  Changelog.new(
    parts: VersionParts.new(args[:version])
  ).call
end

desc "Upload a ruby to S3"
task :upload, [:version, :stack, :staging] do |t, args|
  require "aws-sdk-s3"

  profile_name = "#{S3_BUCKET_NAME}#{args[:staging] ? "-staging" : ""}"

  filename = "ruby-#{args[:version]}.tgz"
  s3_key = "#{args[:stack]}/#{filename.sub(/-((preview|rc)\d+)/, '\1')}"

  s3 = Aws::S3::Resource.new(
    region: "us-east-1",
    access_key_id: ENV.fetch("AWS_ACCESS_KEY_ID"),
    secret_access_key: ENV.fetch("AWS_SECRET_ACCESS_KEY"),
    session_token: ENV.fetch("AWS_SESSION_TOKEN")
  )
  bucket = s3.bucket(profile_name)
  s3_object = bucket.object(s3_key)
  output_file = "builds/#{args[:stack]}/#{filename}"

  puts "Uploading #{output_file} to s3://#{profile_name}/#{s3_key}"
  File.open(output_file, "rb") do |file|
    s3_object.put(body: file, acl: "public-read")
  end
end

desc "Build docker image for stack"
task :generate_image, [:stack] do |t, args|
  require "fileutils"
  stack = args[:stack]
  FileUtils.cp("dockerfiles/Dockerfile.#{stack}", "Dockerfile")
  image = "hone/ruby-builder:#{stack}"
  arguments = ["-t #{image}"]

  # rubocop:disable Lint/EmptyWhen
  # TODO: Local cross compile story?
  # case stack
  # when "heroku-24"
  #   arguments.push("--platform='linux/amd64,linux/arm64'")
  # when "heroku-20", "heroku-22"
  # else
  #   raise "Unknown stack: #{stack}"
  # end
  # rubocop:enable Lint/EmptyWhen

  command = "docker build #{arguments.join(" ")} ."
  puts "Running: `#{command}`"
  system(command)
  FileUtils.rm("Dockerfile")
end

desc "Test images"
task :test, [:version, :stack, :staging] do |t, args|
  require "hatchet"

  ruby_version, patchlevel = args[:version].split("-p")
  stack = args[:stack]
  staging = args[:staging]

  if staging
    buildpacks = ["https://github.com/sharpstone/sudo_set_config_var_buildpack", "heroku/ruby"]
    config = {"__SUDO_BUILDPACK_VENDOR_URL" => "https://heroku-buildpack-ruby-staging.s3.us-east-1.amazonaws.com"}
  else
    buildpacks = ["heroku/ruby"]
    config = {}
  end

  Hatchet::Runner.new("default_ruby", stack: stack, buildpacks: buildpacks, config: config).tap do |app|
    app.before_deploy do
      ruby_string = %(ruby "#{ruby_version}")
      ruby_string << %(, :patchlevel => "#{patchlevel}") if patchlevel
      out = `echo "#{ruby_string.inspect}" >> Gemfile`
      raise "Could not modify Gemfile: #{out}" unless $?.success?
    end
    app.deploy do
      out = app.run("echo 'Ruby version: $(ruby -v), Gem version: $(gem -v)'", raw: true).chomp
      puts "Stack: #{stack}, #{out}, s3_bucket: #{staging ? "staging" : "production"}"
    end
  end
end

def pipe(command)
  output = ""
  IO.popen(command) do |io|
    until io.eof?
      buffer = io.gets
      output << buffer
      puts buffer
    end
  end

  raise "Command failed #{command}" unless $?.success?

  output
end
