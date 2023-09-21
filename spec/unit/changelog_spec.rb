require "spec_helper"
require "changelog"

describe RubyVersion do
  it "prints changelog info for regular releases" do
    io = StringIO.new
    Changelog.new(
      io: io,
      ruby_version: RubyVersion.new("3.1.2")
    ).call

    expect(io.string).to eq(<<~'EOF')
      Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new

      ## Ruby version 3.1.2 is now available

      [Ruby v3.1.2](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
      your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

      ```ruby
      ruby "3.1.2"
      ```

      For more information on [Ruby 3.1.2, you can view the release announcement](https://www.ruby-lang.org/en/news/).
    EOF
  end

  it "prints changelog info for preview release" do
    io = StringIO.new
    Changelog.new(
      io: io,
      ruby_version: RubyVersion.new("3.3.0-preview2")
    ).call

    expect(io.string).to eq(<<~'EOF')
      Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new

      ## Ruby version 3.3.0-preview2 is now available

      [Ruby v3.3.0-preview2](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
      your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

      ```ruby
      ruby "3.3.0"
      ```

      For more information on [Ruby 3.3.0-preview2, you can view the release announcement](https://www.ruby-lang.org/en/news/).

      Note: This version of Ruby is not suitable for production applications.
            However, it can be used to test that your application is ready for
            the December release of Ruby 3.3.0 and
            to provide feedback to the Ruby core team.
    EOF
  end
end
