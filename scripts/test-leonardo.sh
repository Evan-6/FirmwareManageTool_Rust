#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
sources=("$project_root"/firmware/boards/leonardo_avr/src/input/*.cpp "$project_root"/firmware/boards/leonardo_avr/src/platform/Usb*.cpp "$project_root"/firmware/boards/leonardo_avr/src/platform/Bootloader.cpp "$project_root"/firmware/boards/leonardo_avr/src/runtime/*.cpp "$project_root"/firmware/tests/avr-stubs/HidPackets.cpp)
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} -DARDUINO_ARCH_AVR -I"$project_root/firmware/tests/avr-stubs" "${sources[@]}" "$project_root/firmware/tests/leonardo_test.cpp" -o "$test_dir/leonardo-test"
"$test_dir/leonardo-test"
# Every platform/header is standalone; a second TU may include the same API.
headers=("$project_root"/firmware/boards/leonardo_avr/src/input/*.h "$project_root"/firmware/boards/leonardo_avr/src/platform/*.h "$project_root"/firmware/boards/leonardo_avr/src/runtime/*.h)
for header in "${headers[@]}"; do
  printf '#include "%s"\n' "$header" > "$test_dir/header.cpp"
  "${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -DARDUINO_ARCH_AVR -I"$project_root/firmware/tests/avr-stubs" -fsyntax-only "$test_dir/header.cpp"
done
for header in "${headers[@]}"; do printf '#include "%s"\n' "$header"; done > "$test_dir/other.cpp"
printf 'int other(){return 0;}\n' >> "$test_dir/other.cpp"
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror ${CXXFLAGS:-} -DARDUINO_ARCH_AVR -I"$project_root/firmware/tests/avr-stubs" "${sources[@]}" "$project_root/firmware/tests/leonardo_test.cpp" "$test_dir/other.cpp" -o "$test_dir/headers-test"
"$test_dir/headers-test"
