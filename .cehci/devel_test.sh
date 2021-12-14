#!/bin/sh
cargo fmt --all -- --check && cargo check && cargo test -- --nocapture
echo $?
