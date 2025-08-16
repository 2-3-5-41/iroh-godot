#! /bin/bash

cargo build;
cargo build --release;

debug_libs=(
    "./target/debug/libiroh_godot.so"
    "./target/debug/libiroh_godot.dll"
    "./target/debug/libiroh_godot.dylib"
)
release_libs=(
    "./target/release/libiroh_godot.so"
    "./target/release/libiroh_godot.dll"
    "./target/release/libiroh_godot.dylib"
)

mkdir ./addons/iroh-godot/debug &&
mkdir ./addons/iroh-godot/release;

for i in $debug_libs
do
    cp $i ./addons/iroh-godot/debug
done

for i in $release_libs
do
    cp $i ./addons/iroh-godot/release
done

cp -r ./addons ./examples/iroh-godot-example/;
