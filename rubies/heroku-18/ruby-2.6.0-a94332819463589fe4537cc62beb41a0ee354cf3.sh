#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.6.0 -e PATCH_URL=https://github.com/ruby/ruby/commit/a94332819463589fe4537cc62beb41a0ee354cf3 -e STACK=heroku-18 hone/ruby-builder:heroku-18
