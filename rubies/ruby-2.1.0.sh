#!/bin/sh

docker run -v `pwd`/../cache:/tmp/cache -v `pwd`/../builds:/tmp/output -e VERSION=2.1.0 hone/ruby-builder
