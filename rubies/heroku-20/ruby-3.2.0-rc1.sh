#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=3.2.0-rc1  -e STACK=heroku-20 hone/ruby-builder:heroku-20
