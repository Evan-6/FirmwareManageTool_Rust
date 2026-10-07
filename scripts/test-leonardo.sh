#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -I"$project_root/firmware/tests/avr-stubs" "$project_root/firmware/tests/leonardo_test.cpp" -o "$test_dir/leonardo-test"
"$test_dir/leonardo-test"
