#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION="1.9.2-p327" -e SVN_URL="http://svn.ruby-lang.org/repos/ruby/trunk" -e RELNAME="branches/ruby_1_9_2@46553" -e STACK=cedar hone/ruby-builder:cedar

