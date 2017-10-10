#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.2.7  -e STACK=cedar-14 -e PATCH_URL=https://bugs.ruby-lang.org/attachments/download/6690/rubygems-2613-ruby22.patch hone/ruby-builder:cedar-14
