#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.2.9 -e PATCH_URL=https://bugs.ruby-lang.org/attachments/download/7030/rubygems-276-for-ruby22.patch -e STACK=heroku-16 hone/ruby-builder:heroku-16
