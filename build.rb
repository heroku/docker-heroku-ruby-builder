#!/usr/bin/env ruby

puts File.expand_path("../lib", __FILE__)
$LOAD_PATH.unshift File.expand_path("../lib", __FILE__)

require "build_script"

run_build_script(
  stack: ENV.fetch("STACK"),
  architecture: get_architecture,
  ruby_version: ENV.fetch("VERSION"),
  workspace_dir: ARGV[0],
  output_dir: ARGV[1],
  cache_dir: ARGV[2]
)
