#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=9.3.4.0  -e STACK=heroku-18 hone/ruby-builder:heroku-18
