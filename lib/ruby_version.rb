require_relative "version_parts"

class RubyVersion
  # Uses def <=> to implement >=, <=, etc.
  include Comparable

  attr_reader :parts
  private :parts

  def initialize(version = ENV.fetch("VERSION"))
    @parts = VersionParts.new(version)
  end

  # Returns file name with tar extension (but no directory)
  # This is the file name that will be uploaded to Heroku
  #
  # e.g. "ruby-3.1.4.tgz"
  def tar_file_name_output
    "ruby-#{parts.bundler_format}.tgz"
  end

  # Returns a file name without the extension (no directory)
  def download_file_name
    "ruby-#{parts.download_format}"
  end

  # Ruby packages their source with a top level directory matching the name of the download file
  # see the docs in `tar_and_untar.rb` for more details on expected tar formats
  def ruby_source_dir_name
    "ruby-#{parts.download_format}"
  end

  def <=>(other)
    Gem::Version.new(parts.bundler_format) <=> Gem::Version.new(other)
  end
end

class DownloadRuby
  def initialize(parts:)
    @parts = parts
  end

  def url
    "https://ftp.ruby-lang.org/pub/ruby/#{@parts.major}.#{@parts.minor}/ruby-#{@parts.download_format}.tar.gz"
  end
end
