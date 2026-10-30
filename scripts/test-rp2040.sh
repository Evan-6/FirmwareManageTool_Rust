#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
# A native (non-MSYS) mingw c++ rejects /d/... paths, so prefer the drive-letter
# form where pwd supports -W. On Linux -W fails and the POSIX path is kept.
project_root=$( (cd "$project_root" && pwd -W 2>/dev/null) || echo "$project_root" )
test_dir=$(mktemp -d)
test_dir=$( (cd "$test_dir" && pwd -W 2>/dev/null) || echo "$test_dir" )
trap 'rm -rf "$test_dir"' EXIT
sources=("$project_root"/firmware/boards/rp2040/src/input/*.cpp "$project_root"/firmware/boards/rp2040/src/platform/*.cpp "$project_root"/firmware/boards/rp2040/src/runtime/*.cpp)
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} -I"$project_root/firmware/tests/stubs" "${sources[@]}" "$project_root/firmware/tests/rp2040_test.cpp" -o "$test_dir/rp2040-test"
"$test_dir/rp2040-test"
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} -DARDUINO_SEEED_XIAO_RP2040 -I"$project_root/firmware/tests/stubs" "${sources[@]}" "$project_root/firmware/tests/rp2040_test.cpp" -o "$test_dir/xiao-test"
"$test_dir/xiao-test"

# The engine alone, without Arduino/TinyUSB/Pico SDK, keeps the protocol and
# scheduling coverage independent of the runtime wiring.
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} "$project_root"/firmware/boards/rp2040/src/input/*.cpp "$project_root/firmware/boards/rp2040/src/platform/StatusIndicator.cpp" "$project_root/firmware/tests/rp2040_engine_test.cpp" -o "$test_dir/engine-test"
"$test_dir/engine-test"

# The shipping C++ engine and the Rust port (firmware/engine) run the same
# script; before the port replaces the C++ engine, their traces must match byte
# for byte. CRLF from a Windows host is normalised away.
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} "$project_root"/firmware/boards/rp2040/src/input/*.cpp "$project_root/firmware/tests/engine_trace.cpp" -o "$test_dir/engine-trace"
"$test_dir/engine-trace" > "$test_dir/engine-trace.txt"
diff --strip-trailing-cr -u "$project_root/firmware/tests/engine_trace.golden" "$test_dir/engine-trace.txt"

# Each header compiles on its own. Two translation units include every header
# and link against the actual modules to catch hidden state/ODR dependencies.
headers=("$project_root"/firmware/boards/rp2040/src/input/*.h "$project_root"/firmware/boards/rp2040/src/platform/*.h "$project_root"/firmware/boards/rp2040/src/runtime/*.h)
for header in "${headers[@]}"; do
  printf '#include "%s"\n' "$header" > "$test_dir/header.cpp"
  "${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -I"$project_root/firmware/tests/stubs" -fsyntax-only "$test_dir/header.cpp"
done
for header in "${headers[@]}"; do printf '#include "%s"\n' "$header"; done > "$test_dir/a.cpp"
printf 'unsigned long test_time=0; int other(); int main(){return other();}\n' >> "$test_dir/a.cpp"
for header in "${headers[@]}"; do printf '#include "%s"\n' "$header"; done > "$test_dir/b.cpp"
printf 'int other(){return 0;}\n' >> "$test_dir/b.cpp"
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} -I"$project_root/firmware/tests/stubs" "${sources[@]}" "$test_dir/a.cpp" "$test_dir/b.cpp" -o "$test_dir/headers-test"
"$test_dir/headers-test"
