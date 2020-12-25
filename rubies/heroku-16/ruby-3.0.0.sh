#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=3.0.0  -e STACK=heroku-16 hone/ruby-builder:heroku-16
