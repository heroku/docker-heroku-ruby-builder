#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.5.0  -e STACK=heroku-16 -e PATCH_URL=https://bugs.ruby-lang.org/attachments/download/7027/rubygems-276-for-ruby25.patch hone/ruby-builder:heroku-16
