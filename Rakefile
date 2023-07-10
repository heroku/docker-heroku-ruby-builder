require 'fileutils'

S3_BUCKET_NAME = "heroku-buildpack-ruby"

desc "Generate a new ruby shell script"
task :new, [:version, :stack, :patch] do |t, args|
  write_file = Proc.new do |version, stack, patch=false|
    file =
      if patch
        patch_name = File.basename(patch, File.extname(patch))
        "rubies/#{args[:stack]}/ruby-#{args[:version]}-#{patch_name}.sh"
      else
        "rubies/#{args[:stack]}/ruby-#{args[:version]}.sh"
      end
    puts "Writing #{file}"
    FileUtils.mkdir_p(File.dirname(file))
    File.open(file, 'w') do |file|
      file.puts <<-FILE
#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=#{args[:version]}#{patch ? " -e PATCH_URL=#{patch}": " "} -e STACK=#{args[:stack]} hone/ruby-builder:#{args[:stack]}
FILE
    end
    File.chmod(0775, file)
  end

  write_file.call(args[:version], args[:stack], args[:patch])
end

desc "Output the Rubygems version for a given binary"
task :rubygems_version, [:version, :stack] do |t, args|
  stack = args[:stack]
  ruby_version = args[:version]

  # Extract the binary in a docker image and run `bin/gem -v` to output it's Rubygems version
  command = "docker run -v $(PWD)/builds/#{stack}:/tmp/output hone/ruby-builder:#{stack} bash -c \"cd /tmp/output && tar xzf ruby-#{ruby_version}.tgz && echo 'Rubygems version is: ' && bin/gem -v\""
  puts "Running: #{command}"
  pipe(command)
end

desc "Emits a changelog message"
task :changelog, [:version] do |_, args|
  ruby_version = args[:version]

  puts "Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new"

  puts <<~EOM

    ## Ruby version #{ruby_version} is now available

    [Ruby v#{ruby_version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
    your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

    ```ruby
    ruby "3.3.0"
    ```

    For more information on [Ruby #{ruby_version}, you can view the release announcement](https://www.ruby-lang.org/en/news/).

  EOM

end

desc "Upload a ruby to S3"
task :upload, [:version, :stack, :staging] do |t, args|
  require 'aws-sdk-s3'

  profile_name = "#{S3_BUCKET_NAME}#{args[:staging] ? "-staging" : ""}"

  filename     = "ruby-#{args[:version]}.tgz"
  s3_key       = "#{args[:stack]}/#{filename.sub(/-((preview|rc)\d+)/, '\1')}"

  s3 = Aws::S3::Resource.new(
    region: "us-east-1",
    access_key_id: ENV.fetch("AWS_ACCESS_KEY_ID"),
    secret_access_key: ENV.fetch("AWS_SECRET_ACCESS_KEY"),
    session_token: ENV.fetch("AWS_SESSION_TOKEN")
  )
  bucket       = s3.bucket(profile_name)
  s3_object    = bucket.object(s3_key)
  output_file  = "builds/#{args[:stack]}/#{filename}"

  puts "Uploading #{output_file} to s3://#{profile_name}/#{s3_key}"
  File.open(output_file, 'rb') do |file|
    s3_object.put(body: file, acl: "public-read")
  end
end

desc "Make this patchlevel the default for that version"
task :default, [:version, :stack, :build] do |t, args|
  require 'aws-sdk'

  file     = "ruby-#{args[:build] ? "build-" : ""}#{args[:version]}.tgz"
  s3_key   = "#{args[:stack]}/#{file}"
  dest_key = "#{args[:stack]}/ruby-#{args[:build] ? "build-" : ""}#{args[:version].split("-").first}.tgz"
  s3       = AWS::S3.new
  bucket   = s3.buckets['heroku-buildpack-ruby']
  object   = bucket.objects[s3_key]

  puts "Copying #{s3_key} to #{dest_key}"
  object.copy_to(dest_key, acl: :public_read)
end

desc "Build docker image for stack"
task :generate_image, [:stack] do |t, args|
  require 'fileutils'
  FileUtils.cp("dockerfiles/Dockerfile.#{args[:stack]}", "Dockerfile")
  system("docker build -t hone/ruby-builder:#{args[:stack]} .")
  FileUtils.rm("Dockerfile")
end

namespace :batch do
  desc "Batch build"
  task :build, [:stack, :pattern] do |t, args|
    rubies = Dir.glob("./rubies/#{args[:stack]}/#{args[:pattern]}")

    if rubies.empty?
      puts "No rubies detected: #{args[:pattern]}"
      exit 0
    end

    puts "Building the following rubies:\n* #{rubies.join("\n* ")}"

    rubies.each do |file|
      puts "\n\n-- Running #{file} --"
      IO.popen(file) do |io|
        Signal.trap("QUIT") { io.pid.kill }
        begin
          while data = io.readpartial(1024)
            print(data)
          end
        rescue EOFError
        end
      end
    end
  end

  desc "Batch upload"
  task :upload, [:stack, :pattern] do |t, args|
    rubies = Dir.glob("./builds/#{args[:stack]}/#{args[:pattern]}")

    if rubies.empty?
      puts "No rubies detected: #{args[:pattern]}"
      exit 0
    end

    puts "Uploading the following rubies:\n* #{rubies.join("\n* ")}"

    rubies.each do |ruby_path|
      version = ruby_path.gsub("ruby-")
      puts "Uploading #{ruby_path} to s3://#{S3_BUCKET_NAME}/{s3_key}"
      Rake::Task[:upload].invoke(version, stack)
    end
  end
end

desc "Test images"
task :test, [:version, :stack, :staging] do |t, args|
  require 'hatchet'

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
      ruby_string = %Q{ruby "#{ruby_version}"}
      ruby_string << %Q{, :patchlevel => "#{patchlevel}"} if patchlevel
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
