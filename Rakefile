desc "Generate a new ruby shell script"
task :new, [:version, :stack] do |t, args|
  file = "rubies/ruby-#{args[:version]}.sh"
  puts "Writing #{file}"
  File.open(file, 'w') do |file|
    file.puts <<FILE
#!/bin/bash

source `dirname $0`/common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=#{args[:version]} hone/ruby-builder:#{args[:stack]}
FILE
  end
end

desc "Upload a ruby to S3"
task :upload, [:version, :stack] do |t, args|
  require 'aws-sdk'
  
  file        = "ruby-#{args[:version]}.tgz"
  s3_key      = "#{args[:stack]}/#{file}"
  bucket_name = "heroku-buildpack-ruby"
  s3          = AWS::S3.new
  bucket      = s3.buckets[bucket_name]
  object      = bucket.objects[s3_key]

  puts "Uploading output/#{file} to s3://#{bucket_name}/#{s3_key}"
  object.write(file: "output/#{file}")
  object.acl = :public_read
end

desc "Make this patchlevel the default for that version"
task :default, [:version, :stack] do |t, args|
  file     = "ruby-#{args[:version]}.tgz"
  s3_key   = "#{args[:stack]}/#{file}"
  dest_key = "#{args[:stack]}/ruby-#{args[:version].split.first}.tgz"
  s3       = AWS::S3.new
  bucket   = s3.buckets['heroku-buildpack-ruby']
  object   = bucket.objects[s3_key]
  object.copy_to(dest_key, acl: :public_read)
end
