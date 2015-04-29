#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=1.9.2-p328 -e SVN_URL="http://svn.ruby-lang.org/repos/ruby/trunk" -e RELNAME="branches/ruby_1_9_2" -e STACK=cedar-14 hone/ruby-builder:cedar-14

