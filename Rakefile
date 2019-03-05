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

desc "Upload a ruby to S3"
task :upload, [:version, :stack, :staging] do |t, args|
  require 'aws-sdk'

  profile_name = "#{S3_BUCKET_NAME}#{args[:staging] ? "-staging" : ""}"
  credentials  = AWS::Core::CredentialProviders::SharedCredentialFileProvider.new(profile_name: profile_name)
  filename     = "ruby-#{args[:version]}.tgz"
  s3_key       = "#{args[:stack]}/#{filename.sub(/-(preview|rc)\d+/, '')}"
  s3           = AWS::S3.new(access_key_id: ENV.fetch("AWS_ACCESS_KEY_ID"), secret_access_key: ENV.fetch("AWS_SECRET_ACCESS_KEY"))
  bucket       = s3.buckets[profile_name]
  object       = bucket.objects[s3_key]
  output_file  = "builds/#{args[:stack]}/#{filename}"

  puts "Uploading #{output_file} to s3://#{profile_name}/#{s3_key}"
  object.write(file: output_file)
  object.acl = :public_read
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

    require 'aws-sdk'
    s3     = AWS::S3.new
    bucket = s3.buckets[S3_BUCKET_NAME]

    rubies.each do |ruby_path|
      s3_key = "#{args[:stack]}/#{File.basename(ruby_path)}"
      object = bucket.objects[s3_key]

      puts "Uploading #{ruby_path} to s3://#{S3_BUCKET_NAME}/{s3_key}"

      object.write(file: ruby_path)
      object.acl = :public_read
    end
  end
end

desc "Test images"
task :test, [:version, :stack] do |t, args|
  require 'tmpdir'
  require 'okyakusan'
  require 'rubygems/package'
  require 'zlib'
  require 'net/http'

  def system_pipe(command)
    IO.popen(command) do |io|
      while data = io.read(16) do
        print data
      end
    end
  end

  def gemfile_ruby(version, patchlevel = nil)
    string = %Q{ruby "#{version}"}
    string << %Q{, :patchlevel => "#{patchlevel}"} if patchlevel

    string
  end

  def network_retry(max_retries, retry_count = 0)
    yield
  rescue Errno::ECONNRESET, EOFError
    if retry_count < max_retries
      $stderr.puts "Retry Count: #{retry_count}"
      sleep(0.01 * retry_count)
      retry_count += 1
      retry
    end
  end

  tmp_dir  = Dir.mktmpdir
  app_dir  = "#{tmp_dir}/app"
  app_tar  = "#{tmp_dir}/app.tgz"
  app_name = nil
  web_url  = nil
  FileUtils.mkdir_p("#{tmp_dir}/app")

  begin
    system_pipe("git clone --depth 1 https://github.com/sharpstone/default_ruby #{app_dir}")
    exit 1 unless $?.success?

    ruby_version, patchlevel = args[:version].split("-p")
    ruby_line = gemfile_ruby(ruby_version, patchlevel)
    puts "Setting ruby version: #{ruby_line}"
    text = File.read("#{app_dir}/Gemfile")
    subbed = text.sub!(/^\s*ruby.*$/, ruby_line)
    File.open("#{app_dir}/Gemfile", 'w') do |file|
      file.puts ruby_line unless subbed
      file.print(text)
    end

    Dir.chdir(app_dir) do
      puts "Packaging app"
      system_pipe("tar czf #{app_tar} *")
      exit 1 unless $?.success?
    end

    Okyakusan.start do |heroku|
      # create new app
      response = heroku.post("/apps", data: {
        stack: args[:stack]
      })

      if response.code != "201"
        $stderr.puts "Error Creating Heroku App (#{response.code}): #{response.body}"
        exit 1
      end
      json     = JSON.parse(response.body)
      app_name = json["name"]
      web_url  = json["web_url"]

      if (build_number = ENV['CIRCLE_PREVIOUS_BUILD_NUM']) && (circle_token = ENV['CIRCLE_TOKEN'])
        response = Net::HTTP.get(URI("https://circleci.com/api/v1.1/project/github/hone/docker-heroku-ruby-builder/#{build_number}/artifacts?circle-token=#{circle_token}"))
        artifacts = JSON.parse(response)

        if artifacts.any?
          response = heroku.patch("/apps/#{app_name}/config-vars", data: {
            HEROKU_RUBY_BINARY_OVERRIDE: artifacts.first["url"]
          })
          if response.code != "200"
            $stderr.puts "Error could not set HEROKU_RUBY_BINARY_OVERRIDE env var"
            exit 1
          end

          response = heroku.put("/apps/#{app_name}/buildpack-installations", data: {
            updates: [
              buildpack: "https://github.com/heroku/heroku-buildpack-ruby#3rd-party-ruby"
            ]
          })
        end
      end

      # upload source
      response = heroku.post("/apps/#{app_name}/sources")
      if response.code != "201"
        $stderr.puts "Couldn't get sources to upload code."
        exit 1
      end

      json = JSON.parse(response.body)
      source_get_url = json["source_blob"]["get_url"]
      source_put_url = json["source_blob"]["put_url"]

      puts "Uploading data to #{source_put_url}"
      uri = URI(source_put_url)
      Net::HTTP.start(uri.host, uri.port, :use_ssl => (uri.scheme == 'https')) do |http|
        request = Net::HTTP::Put.new(uri.request_uri, {
          'Content-Length'   => File.size(app_tar).to_s,
          # This is required, or Net::HTTP will add a default unsigned content-type.
          'Content-Type'      => ''
        })
        begin
          app_tar_io          = File.open(app_tar)
          request.body_stream = app_tar_io
          response            = http.request(request)
          if response.code != "200"
            $stderr.puts "Could not upload code"
            exit 1
          end
        ensure
          app_tar_io.close
        end
      end

      # create build
      response = heroku.post("/apps/#{app_name}/builds", version: "3.streaming-build-output", data: {
        "source_blob" => {
          "url"     => source_get_url,
          "version" => ""
        }
      })
      if response.code != "201"
        $stderr.puts "Couldn't create build: #{response.body}"
        exit 1
      end

      # stream build output
      uri = URI(JSON.parse(response.body)["output_stream_url"])
      Net::HTTP.start(uri.host, uri.port, :use_ssl => (uri.scheme == 'https')) do |http|
        request = Net::HTTP::Get.new uri.request_uri
        http.request(request) do |response|
          response.read_body do |chunk|
            print chunk
          end
        end
      end
    end

    # test app
    puts web_url
    sleep(1)
    response = network_retry(20) do
      Net::HTTP.get_response(URI(web_url))
    end

    if response.code != "200"
      $stderr.puts "App did not return a 200: #{response.code}"
      exit 1
    else
      puts "Successfully returned a 200"
      puts `heroku run ruby -v -a #{app_name}`
      puts `heroku run gem -v -a #{app_name}`
    end

    puts response.body
  ensure
    FileUtils.remove_entry tmp_dir
    puts "Deleting #{app_name}"
    Okyakusan.start {|heroku| heroku.delete("/apps/#{app_name}") if app_name }
  end
end
