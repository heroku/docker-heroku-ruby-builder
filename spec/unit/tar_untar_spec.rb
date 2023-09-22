require "spec_helper"

describe "Tar and untar" do
  it "tars and untars contents" do
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

      source_dir.entries.map(&:to_s).each do |filename|
        expect(unzip_dir.entries.map(&:to_s)).to include(filename)
      end
    end
  end
end
