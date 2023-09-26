module DockerCommand
  def self.gem_version_from_tar(ruby_version: , stack: )
    "docker run -v $(pwd)/builds/#{stack}:/tmp/output hone/ruby-builder:#{stack} bash -c \"mkdir /tmp/unzipped && tar xzf /tmp/output/#{ruby_version.tar_file_name_output} -C /tmp/unzipped && echo 'Rubygems version is: ' && /tmp/unzipped/bin/gem -v\""
  end
end
