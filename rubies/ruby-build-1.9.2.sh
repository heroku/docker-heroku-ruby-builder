#!/bin/bash

source `dirname $0`/common.sh

docker run -v `pwd`/cache:/tmp/cache -v `pwd`/builds:/tmp/output -e VERSION=1.9.2-p328 -e SVN_URL="http://svn.ruby-lang.org/repos/ruby/trunk" -e RELNAME="branches/ruby_1_9_2" -e BUILD:true hone/ruby-builder:cedar
