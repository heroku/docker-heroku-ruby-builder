#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION="1.8.7-p376@44719" -e SVN_URL="http://svn.ruby-lang.org/repos/ruby/trunk" -e RELNAME="branches/ruby_1_8_7@44719" -e RUBYGEMS_VERSION=1.8.24 -e STACK=cedar-14 hone/ruby-builder:cedar-14

