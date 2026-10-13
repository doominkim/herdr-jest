#!/bin/sh
set -eu

repo=$(cd "$(dirname "$0")/../.." && pwd)
work=/tmp/hj-demo

rm -rf "$work"
mkdir -p "$work/acme" "$work/bin" "$work/state"
cp -R "$repo/docs/demo/project" "$work/acme/shop"
ln -s "$repo/tests/fixtures/sample/node_modules" "$work/acme/shop/node_modules"
git -C "$work/acme/shop" init -q -b feat/checkout
cp "$repo/target/release/herdr-jest" "$work/bin/herdr-jest"

cd "$repo/docs/demo"
vhs demo.tape
