#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -I"$project_root/firmware/tests/stubs" "$project_root/firmware/tests/rp2040_test.cpp" -o "$test_dir/rp2040-test"
"$test_dir/rp2040-test"
