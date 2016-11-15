#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.4.0-preview2  -e STACK=cedar-14 hone/ruby-builder:cedar-14
