class Changelog
  private attr_reader :io, :parts

  def initialize(parts:, io: $stdout)
    @io = io
    @parts = parts
  end

  def call
    io.puts "Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new"

    io.puts <<~EOM

      ## Ruby version #{parts.download_format} is now available

      [Ruby v#{parts.download_format}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
      your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

      ```ruby
      ruby "#{parts.bundler_format}"
      ```

      For more information on [Ruby #{parts.download_format}, you can view the release announcement](https://www.ruby-lang.org/en/news/).
    EOM

    if parts.pre.length > 0
      io.puts <<~EOF

        Note: This version of Ruby is not suitable for production applications.
              However, it can be used to test that your application is ready for
              the official release of Ruby #{parts.major}.#{parts.minor}.#{parts.patch} and
              to provide feedback to the Ruby core team.
      EOF
    end
  end
end
