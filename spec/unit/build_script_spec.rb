require "spec_helper"

describe "Build logic" do
  it "builds a make command" do
    actual = make_commands(
      io: StringIO.new,
      jobs: 16,
      prefix: "iamaprefix",
      ruby_version: RubyVersion.new("3.1.2")
    )

    expected = "debugflags=\"-g\" ./configure --disable-install-doc --prefix iamaprefix --enable-load-relative --enable-shared && make -j16 && make install"
    expect(actual).to eq(expected)
  end

  it "replaces shebang lines" do
    Dir.mktmpdir do |tmp|
      dir = Pathname(tmp)
      file = dir.join("bad")
      file.write(<<~'EOF')
        #!/app/vendor/ruby-3.1.2/bin/ruby

        Rest of the file stays
        the same
      EOF
      fix_binstubs_in_dir(dir: tmp, io: StringIO.new)

      expect(file.read).to eq(<<~'EOF')
        #!/usr/bin/env ruby

        Rest of the file stays
        the same
      EOF
    end
  end

  it "tars dirs" do
    Dir.mktmpdir do |tmp_dir|
      tmp_dir = Pathname(tmp_dir)

      source_dir = tmp_dir.join("source").tap do |path|
        path.mkpath
        path.join("foo.txt").write("foo")
        path.join("bar.txt").write("bar")
      end

      tar_file = tmp_dir.join("destination").tap { |p| p.mkpath }.join("name-does-not-affect-anything.tgz")
      expect(tar_file).to_not exist
      tar_dir(
        io: StringIO.new,
        dir_to_tar: source_dir,
        destination_file: tar_file
      )
      expect(tar_file).to exist

      contents = `tar -tvf #{tar_file}`.strip
      expect(contents).to include(" foo.txt")
      expect(contents).to include(" bar.txt")

      unzip_dir = tmp_dir.join("unzip").tap { |p| p.mkpath }
      untar_to_dir(
        io: StringIO.new,
        tar_file: tar_file,
        dest_directory: unzip_dir
      )

      expect(unzip_dir.entries.map(&:to_s)).to include("foo.txt")
      expect(unzip_dir.entries.map(&:to_s)).to include("bar.txt")
    end
  end
end
