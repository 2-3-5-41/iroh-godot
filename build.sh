#! /bin/bash

# Build all binaries for debug and release.
cargo build
cargo build --release

# create symlink to binaries in the addons directory.
# Developers building from source can then symlink the addons directory into their project.
ln -s ./target/debug ./addons/debug
ln -s ./target/release ./addons/release
