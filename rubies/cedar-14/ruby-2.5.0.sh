#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output \
           -v $CACHE_DIR:/tmp/cache \
           -e VERSION=2.5.0 \
           -e STACK=cedar-14 \
           -e PATCH_URL=https://gist.githubusercontent.com/schneems/8c1c2eae9b255e9e1c18d95bffbb9d9f/raw/31b830252f277646a14ddc73d8e0601a847615f1/rubygems-276-for-ruby25-and-rubygems-cve-and-bundler2 \
           hone/ruby-builder:cedar-14
