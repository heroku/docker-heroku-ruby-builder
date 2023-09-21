class RubyVersion
  # Uses def <=> to implement >=, <=, etc.
  include Comparable

  # Returns a file name without the extension (no direcory)
  attr_reader :plain_file_name

  # Full URL of the ruby binary on ruby-lang (if it exists)
  attr_reader :download_url

  # Returns file name with tar extension (no directory)
  # This is the file name that will be uploaded to Heroku
  #
  # Preview and release candidates are output as their
  # major.minor.patch (without the `-preview` suffix)
  attr_reader :tar_file_name_output

  # Version without an extra bits at the end
  attr_reader :major_minor_patch

  attr_reader :raw_version

  def initialize(version = ENV.fetch("VERSION"))
    @raw_version = version

    parts = version.split(".")
    major = parts.shift
    minor = parts.shift
    patch = parts.shift.match(/\d+/)[0]

    @major_minor_patch = "#{major}.#{minor}.#{patch}"
    @plain_file_name = "ruby-#{@raw_version}"
    @download_url = "https://ftp.ruby-lang.org/pub/ruby/#{major}.#{minor}/#{@plain_file_name}.tar.gz"

    @tar_file_name_output = "ruby-#{major}.#{minor}.#{patch}.tgz"
    @compare_version = Gem::Version.new(raw_version)
  end

  def preview?
    @raw_version != @major_minor_patch
  end

  def <=>(other)
    @compare_version <=> Gem::Version.new(other)
  end
end
