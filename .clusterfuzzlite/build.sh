#!/bin/bash -eu
# SPDX-License-Identifier: MPL-2.0

cd $SRC/project
cargo +nightly fuzz build --release
cp fuzz/target/*/release/fuzz_* $OUT/
