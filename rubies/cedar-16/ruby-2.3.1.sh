#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.3.1  -e STACK=cedar-16 hone/ruby-builder:cedar-16
