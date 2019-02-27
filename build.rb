#!/usr/bin/env ruby

require 'tmpdir'
require 'fileutils'
require 'uri'

def pipe(command)
  output = ""
  IO.popen(command) do |io|
    until io.eof?
      buffer = io.gets
      output << buffer
      puts buffer
    end
  end

  output
end

def fetch(url, name = nil)
  uri    = URI.parse(url)
  binary = uri.to_s.split("/").last
  if File.exists?(binary)
    puts "Using #{binary}"
  else
    puts "Fetching #{binary}"
    if name
      `curl #{uri} -s -o #{name}`
    else
      `curl #{uri} -s -O`
    end
  end
end

workspace_dir = ARGV[0]
output_dir    = ARGV[1]
cache_dir     = ARGV[2]

LIBYAML_VERSION = "0.1.7"
LIBFFI_VERSION  = "3.2.1"

vendor_url   = "https://s3.amazonaws.com/#{ENV['S3_BUCKET_NAME'] ? ENV['S3_BUCKET_NAME'] : 'heroku-buildpack-ruby'}"
full_version = ENV['VERSION']
full_name    = "ruby-#{full_version}"
version      = full_version.split('-').first
name         = "ruby-#{version}"
major_ruby   = version.match(/\d\.\d/)[0]
build        = false
build        = true if ENV["BUILD"]
debug        = nil
debug        = true if ENV['DEBUG']
jobs         = ENV['JOBS'] || 2
rubygems     = ENV['RUBYGEMS_VERSION'] ? ENV['RUBYGEMS_VERSION'] : nil
git_url      = ENV["GIT_URL"]
svn_url      = ENV["SVN_URL"]
relname      = ENV["RELNAME"]
stack        = ENV["STACK"]
treeish      = nil
if ENV['PATCH_URL']
  patch = ENV['PATCH_URL']
  patch_name = patch.split("/").last
else
  patch = nil
end

# create cache dir if it doesn't exist
FileUtils.mkdir_p(cache_dir)

# fetch deps
Dir.chdir(cache_dir) do
  tarball = "#{full_name}.tar.gz"

  if git_url
    uri          = URI.parse(git_url)
    treeish      = uri.fragment
    uri.fragment = nil
    full_name    = uri.to_s.split('/').last.sub(".git", "")

    if File.exists?(full_name)
      Dir.chdir(full_name) do
        puts "Updating git repo"
        pipe "git pull"
      end
    else
      puts "Fetching #{git_url}"
      pipe "git clone #{uri}"
    end

  elsif svn_url
    uri = URI.parse(svn_url)

    if File.exists?(full_name)
      puts "Using existing svn checkout: #{full_name}"
      pipe "svn update"
    else
      pipe "svn co #{svn_url} #{full_name}"
    end

    Dir.chdir(full_name) do
      cmd = "ruby tool/make-snapshot -archname=#{full_name} build #{relname}"
      puts cmd
      pipe cmd
    end

    FileUtils.mv("#{full_name}/build/#{tarball}", ".")
  else
    fetch("http://ftp.ruby-lang.org/pub/ruby/#{major_ruby}/#{tarball}")
  end


  if stack.match(/cedar/)
    ["libyaml-#{LIBYAML_VERSION}.tgz", "libffi-#{LIBFFI_VERSION}.tgz"].each do |binary|
      fetch("#{vendor_url}/#{stack}/#{binary}")
    end
  end
  if rubygems
    rubygems_binary = "rubygems-#{rubygems}"
    fetch("http://production.cf.rubygems.org/rubygems/#{rubygems_binary}.tgz")
  end
  fetch(patch, patch_name) if patch
end

Dir.mktmpdir("ruby-vendor-") do |vendor_dir|
  if git_url
    FileUtils.cp_r("#{cache_dir}/#{full_name}", ".")
  else
    `tar zxf #{cache_dir}/#{full_name}.tar.gz`
  end
  Dir.chdir(vendor_dir) do
    if stack.match(/cedar/)
      `tar zxf #{cache_dir}/libyaml-#{LIBYAML_VERSION}.tgz`
      `tar zxf #{cache_dir}/libffi-#{LIBFFI_VERSION}.tgz`
    end
    `tar zxf #{cache_dir}/rubygems-#{rubygems}.tgz` if rubygems
  end

  prefix = "/app/vendor/#{name}"
  prefix = "/tmp/#{name}" if build

  puts "prefix: #{prefix}"

  Dir.chdir(full_name) do
    pipe "git checkout #{treeish}" if treeish

    pipe "patch -p0 < #{cache_dir}/#{patch_name}" if patch

    if debug
      configure_env = "debugflags=\"-ggdb3\""
    else
      configure_env = "debugflags=\"-g\""
    end

    configure_opts = "--disable-install-doc --prefix #{prefix}"
    configure_opts += " --enable-load-relative" if major_ruby != "1.8" && version != "1.9.2"
    if stack != "cedar-14"
      configure_opts += " --enable-shared"
    end
    puts "configure env:  #{configure_env}"
    puts "configure opts: #{configure_opts}"
    cmds = [
      "#{configure_env} ./configure #{configure_opts}",
      "env CPATH=#{vendor_dir}/include:\\$CPATH CPPATH=#{vendor_dir}/include:\\$CPPATH LIBRARY_PATH=#{vendor_dir}/lib:\\$LIBRARY_PATH make -j#{jobs}",
      "make install"
    ]
    cmds.unshift("#{configure_env} autoconf") if git_url
    cmds.unshift("chmod +x ./tool/*") if git_url
    pipe(cmds.join(" && "))
  end
  if rubygems
    Dir.chdir("#{vendor_dir}/rubygems-#{rubygems}") do
      pipe("#{prefix}/bin/ruby setup.rb")
    end
    gem_bin_file = "#{prefix}/bin/gem"
    gem = File.read(gem_bin_file)
    File.open(gem_bin_file, 'w') do |file|
      file.puts "#!/usr/bin/env ruby"
      lines = gem.split("\n")
      lines.shift
      lines.each {|line| file.puts line }
    end
  end
  Dir.chdir(prefix) do
    filename =
      if build
        "ruby-build-#{full_version}.tgz"
      else
        "#{full_name}.tgz"
      end

    pipe "ls"
    puts "Writing #{filename}"
    FileUtils.mkdir_p("#{output_dir}/#{stack}")
    pipe("tar czf #{output_dir}/#{stack}/#{filename} *")
  end
end
