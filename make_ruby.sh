#!/usr/bin/env bash

set -euo pipefail

SOURCE_TAR=$1
OUT_TAR=$2

# Configure flags, For details call `./configure --help` in the source dir
configure_opts=(
    --disable-install-doc
    --enable-load-relative
    --enable-shared
)

# YJIT is only available in Ruby 3.2+
if [[ -n "${ENABLE_YJIT:-}" ]]; then
    configure_opts+=(--enable-yjit)
fi

# Leader for `set -o xtrace` output <https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html>
PS4='>\e[33m${BASH_SOURCE}:${LINENO} $\e[0m '
# Logs all bash commands after this point
set -o xtrace

mkdir -p "$(dirname "$OUT_TAR")"

# Docker issue with permissions
chmod a+w "$(dirname "$OUT_TAR")"
mkdir -p /tmp/source
mkdir -p /tmp/compiled

# Unzip the ruby source code
tar -xzf "$SOURCE_TAR" -C /tmp/source --strip-components=1
cd /tmp/source

# Build ruby
debugflags=-g ./configure "${configure_opts[@]}" --prefix /tmp/compiled
make -j"$(nproc)"
make install

# Compress and store the compiled ruby
cd /tmp/compiled
tar -czf "$OUT_TAR" .
