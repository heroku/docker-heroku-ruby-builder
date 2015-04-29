#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=1.9.3-p550 -e STACK=cedar-14 hone/ruby-builder:cedar-14
