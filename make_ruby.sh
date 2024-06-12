#!/usr/bin/env bash

set -euo pipefail

SOURCE_TAR=$1
OUT_TAR=$2

set -o xtrace

mkdir -p "$(dirname "$OUT_TAR")"
mkdir /tmp/source
mkdir /tmp/compiled

tar -xzf "$SOURCE_TAR" -C /tmp/source --strip-components=1
cd /tmp/source

if [[ -z "${ENABLE_YJIT:-}" ]]; then
    debugflags=-g ./configure \
        --disable-install-doc \
        --enable-load-relative \
        --enable-shared \
        --enable-yjit \
        --prefix /tmp/compiled
else
    debugflags=-g ./configure \
        --disable-install-doc \
        --enable-load-relative \
        --enable-shared \
        --prefix /tmp/compiled
fi

make -j"$(nproc)"
make install

cd /tmp/compiled
tar -czf "$OUT_TAR" .
