require "build_script"

module DockerCommand
  def self.gem_version_from_tar(ruby_version:, stack:, system_output: `docker run --rm hone/ruby-builder:#{stack} arch`, success: $?.success?)
    ruby_tar_file = stack_architecture_tar_file_name(
      stack: stack,
      output_dir: "/tmp/output",
      architecture: get_architecture(system_output: system_output, success: success),
      tar_file_name_output: ruby_version.tar_file_name_output
    )
    "docker run -v $(pwd)/builds:/tmp/output hone/ruby-builder:#{stack} bash -c \"mkdir /tmp/unzipped && tar xzf #{ruby_tar_file} -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v\""
  end
end
