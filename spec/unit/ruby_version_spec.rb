require "spec_helper"
require "ruby_version"

describe RubyVersion do
  it "can be compared" do
    expect(RubyVersion.new("3.3.0")).to be >= Gem::Version.new("3.2")
    expect(RubyVersion.new("3.2.0")).to be >= Gem::Version.new("3.2")
    expect(RubyVersion.new("3.1.0")).to be < Gem::Version.new("3.2")
  end

  it "knows the tarball name of a specific version" do
    expect(RubyVersion.new("3.0.2").tar_file_name_output).to eq("ruby-3.0.2.tgz")
    expect(RubyVersion.new("2.5.7").tar_file_name_output).to eq("ruby-2.5.7.tgz")

    expect(RubyVersion.new("3.3.0-preview1").tar_file_name_output).to eq("ruby-3.3.0.preview1.tgz")
  end

  it "knows the full ftp URL" do
    expect(DownloadRuby.new(parts: VersionParts.new("3.0.2")).url).to eq("https://ftp.ruby-lang.org/pub/ruby/3.0/ruby-3.0.2.tar.gz")
    expect(DownloadRuby.new(parts: VersionParts.new("2.5.7")).url).to eq("https://ftp.ruby-lang.org/pub/ruby/2.5/ruby-2.5.7.tar.gz")
    expect(DownloadRuby.new(parts: VersionParts.new("3.3.0-preview2")).url).to eq("https://ftp.ruby-lang.org/pub/ruby/3.3/ruby-3.3.0-preview2.tar.gz")
  end
end
