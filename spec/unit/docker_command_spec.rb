require "spec_helper"
require "docker_command"

describe DockerCommand do
  it "Generates docker command for outputting rubygems versions" do
    actual = DockerCommand.gem_version_from_tar(
      ruby_version: RubyVersion.new("3.1.4"),
      stack: "heroku-22",
      system_output: "x86_64",
      success: true
    )
    expected = %{docker run -v $(pwd)/builds:/tmp/output hone/ruby-builder:heroku-22 bash -c "mkdir /tmp/unzipped && tar xzf /tmp/output/heroku-22/ruby-3.1.4.tgz -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v"}
    expect(actual).to eq(expected)
  end

  it "Works with amd and arm" do
    actual = DockerCommand.gem_version_from_tar(
      ruby_version: RubyVersion.new("3.1.4"),
      stack: "heroku-24",
      system_output: "x86_64",
      success: true
    )

    expected = %{docker run -v $(pwd)/builds:/tmp/output hone/ruby-builder:heroku-24 bash -c "mkdir /tmp/unzipped && tar xzf /tmp/output/heroku-24/amd64/ruby-3.1.4.tgz -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v"}
    expect(actual).to eq(expected)

    actual = DockerCommand.gem_version_from_tar(
      ruby_version: RubyVersion.new("3.1.4"),
      stack: "heroku-24",
      system_output: "aarch64",
      success: true
    )

    expected = %{docker run -v $(pwd)/builds:/tmp/output hone/ruby-builder:heroku-24 bash -c "mkdir /tmp/unzipped && tar xzf /tmp/output/heroku-24/arm64/ruby-3.1.4.tgz -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v"}
    expect(actual).to eq(expected)
  end

  it "works with preview releases" do
    actual = DockerCommand.gem_version_from_tar(
      ruby_version: RubyVersion.new("3.3.0-preview2"),
      stack: "heroku-22",
      system_output: "x86_64",
      success: true
    )
    expected = %{docker run -v $(pwd)/builds:/tmp/output hone/ruby-builder:heroku-22 bash -c "mkdir /tmp/unzipped && tar xzf /tmp/output/heroku-22/ruby-3.3.0.preview2.tgz -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v"}
    expect(actual).to eq(expected)
  end
end
