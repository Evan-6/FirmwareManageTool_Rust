#pragma once
#define ATOMIC_RESTORESTATE 0
#define ATOMIC_BLOCK(x) for (bool test_once = true; test_once; test_once = false)
