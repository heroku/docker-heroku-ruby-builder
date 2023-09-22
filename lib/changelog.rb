class Changelog
  private attr_reader :io, :ruby_version

  def initialize(ruby_version:, io: $stdout)
    @io = io
    @ruby_version = ruby_version
  end

  def call
    io.puts "Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new"

    io.puts <<~EOM

      ## Ruby version #{ruby_version.raw_version} is now available

      [Ruby v#{ruby_version.raw_version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
      your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

      ```ruby
      ruby "#{ruby_version.major_minor_patch}"
      ```

      For more information on [Ruby #{ruby_version.raw_version}, you can view the release announcement](https://www.ruby-lang.org/en/news/).
    EOM

    if ruby_version.preview?
      io.puts <<~EOF

        Note: This version of Ruby is not suitable for production applications.
              However, it can be used to test that your application is ready for
              the official release of Ruby #{ruby_version.major_minor_patch} and
              to provide feedback to the Ruby core team.
      EOF
    end
  end
end
