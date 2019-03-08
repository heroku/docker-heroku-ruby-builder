#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output \
           -v $CACHE_DIR:/tmp/cache \
           -e VERSION=2.5.1 \
           -e STACK=cedar-14 \
           -e PATCH_URL=https://gist.githubusercontent.com/schneems/374157aff12e92babb1c6a3c6b744392/raw/c70aceeb7a97b8d8858234104afad0b7daccb8a7/ruby-25x-bundler2-rubygems-secure \
           hone/ruby-builder:cedar-14
