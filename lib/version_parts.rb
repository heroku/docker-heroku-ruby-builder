# Normalize Ruby versions
#
# Released ruby versions have a "major.minor.patch" and nothing else.
# Prerelease ruby versions have "major.minor.patch" and a trailing identifier
# for example "3.3.0-preview3".
#
# Ruby stores these versions on its download server using a dash for example:
# https://ftp.ruby-lang.org/pub/ruby/3.3/ruby-3.3.0-preview2.tar.gz
#
# However once you install that version and run `ruby -v` you get a different
# representation:
#
# ```
# $ ruby -v
# ruby 3.3.0preview2 (2023-09-14 master e50fcca9a7) [x86_64-linux]
# ```
#
# And it's in yet another representation in bundler:
#
# ```
# $ cat Gemfile.lock | grep RUBY -A 2
# RUBY VERSION
#   ruby 3.3.0.preview2
# ```
#
# This format comes from this logic https://github.com/rubygems/rubygems/blob/85edf547391043ddd9ff21d8426c9dd5903435b2/lib/rubygems.rb#L858-L875
#
# Note that:
#
# - Download ruby has a dash (`-`) seperator
# - Version output from `ruby -v` has no separator
# - Bundler uses a dot (`.`) separator
#
# We need to round trip:
#
# - Download a ruby source tarball
# - Build it into a binary (`make install` etc.)
# - Zip/tar that binary up and upload it to S3 (filename is coupled to buildpack logic)
#
# Then later the buildpack has to:
#
# - Take the output of `bundle platform` and turn that into an S3 url
# - Download and unzip that tarball and place it on the path
#
# For this to function we care about:
#
# - Download format (because we need to get the source from the ftp site)
# - Bundler format (because `bundle platform` output is how we lookup the donload,
#   therefore it's the format we must use to zip/tar the file).
#
# This class can take in a version string containing:
#
# - Ruby version without pre-release information
# - Ruby version with pre-release in download format
# - Ruby version with pre-release in bundler format
#
# And it will normalize the format to be consistent
class VersionParts
  attr_reader :major, :minor, :patch, :separator, :pre

  # Normalize a version string with an optional pre-release
  def initialize(version)
    # https://rubular.com/r/HgtMk8O0Lscfvv
    parts = version.match(/(?<major>\d+)\.(?<minor>\d+)\.(?<patch>\d+)(?<separator>[-.])?(?<pre>.*)/)

    @major = parts[:major] or raise "Does not contain major #{version}: #{parts}"
    @minor = parts[:minor] or raise "Does not contain minor #{version}: #{parts}"
    @patch = parts[:patch] or raise "Does not contain patch #{version}: #{parts}"
    @separator = parts[:separator] || ""
    @pre = parts[:pre] || ""
  end

  def download_format
    "#{major}.#{minor}.#{patch}#{separator.empty? ? "" : "-"}#{pre}"
  end

  def bundler_format
    "#{major}.#{minor}.#{patch}#{separator.empty? ? "" : "."}#{pre}"
  end
end
