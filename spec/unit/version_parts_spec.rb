require "spec_helper"
require "version_parts"

describe VersionParts do
  it "converts between bundler and download formats" do
    version = VersionParts.new("3.3.0")
    expect(version.download_format).to eq("3.3.0")
    expect(version.bundler_format).to eq("3.3.0")

    version = VersionParts.new("3.3.0-preview2")
    expect(version.download_format).to eq("3.3.0-preview2")
    expect(version.bundler_format).to eq("3.3.0.preview2")

    version = VersionParts.new("3.3.0.preview2")
    expect(version.download_format).to eq("3.3.0-preview2")
    expect(version.bundler_format).to eq("3.3.0.preview2")
  end
end
