desc "Generate a new ruby shell script"
task :new, [:version, :stack] do |t, args|
  write_file = Proc.new do |version, stack, build=false|
    file =
     if build
       "rubies/#{args[:stack]}/ruby-build-#{args[:version]}.sh"
      else
       "rubies/#{args[:stack]}/ruby-#{args[:version]}.sh"
      end
    puts "Writing #{file}"
    File.open(file, 'w') do |file|
      file.puts <<FILE
#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=#{args[:version]}#{build ? " -e BUILD=true" : " "} -e STACK=#{args[:stack]} hone/ruby-builder:#{args[:stack]}
FILE
    end
    File.chmod(0775, file)
  end

  write_file.call(args[:version], args[:stack])
  write_file.call(args[:version], args[:stack], true) if args[:version].include?("1.9.2") || args[:version].include?("1.8.7")
end

desc "Upload a ruby to S3"
task :upload, [:version, :stack, :build] do |t, args|
  require 'aws-sdk'
  
  filename    = "ruby-#{args[:build] ? "build-" : ""}#{args[:version]}.tgz"
  s3_key      = "#{args[:stack]}/#{filename}"
  bucket_name = "heroku-buildpack-ruby"
  s3          = AWS::S3.new
  bucket      = s3.buckets[bucket_name]
  object      = bucket.objects[s3_key]
  output_file = "builds/#{args[:stack]}/#{filename}"

  puts "Uploading #{output_file} to s3://#{bucket_name}/#{s3_key}"
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

desc "Batch build"
task :batch_build, [:stack, :pattern] do |t, args|
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
