#pragma once
#include <stdint.h>
#include <string.h>
struct pico_unique_board_id_t {
    uint8_t id[8];
};
inline void pico_get_unique_board_id(pico_unique_board_id_t *p) { memset(p->id, 0x42, 8); }
inline void pico_get_unique_board_id_string(char *p, unsigned n) {
    if (n >= 17)
        memcpy(p, "4242424242424242", 17);
}
