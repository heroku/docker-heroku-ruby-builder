#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.4.3 -e PATCH_URL=https://bugs.ruby-lang.org/attachments/download/7028/rubygems-276-for-ruby24.patch -e STACK=heroku-16 hone/ruby-builder:heroku-16
