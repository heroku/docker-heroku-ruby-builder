#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.3.6 -e PATCH_URL=https://bugs.ruby-lang.org/attachments/download/7029/rubygems-276-for-ruby23.patch -e STACK=cedar-14 hone/ruby-builder:cedar-14
