#pragma once
#include <deque>
#include <string.h>
#include <vector>
struct queue_t {
    std::deque<std::vector<uint8_t>> items;
    size_t size = 0, depth = 0;
};
inline void queue_init(queue_t *q, size_t size, size_t depth) {
    q->items.clear();
    q->size = size;
    q->depth = depth;
}
inline bool queue_try_add(queue_t *q, const void *p) {
    if (q->items.size() == q->depth)
        return false;
    auto b = static_cast<const uint8_t *>(p);
    q->items.emplace_back(b, b + q->size);
    return true;
}
inline bool queue_try_peek(queue_t *q, void *p) {
    if (q->items.empty())
        return false;
    if (p) memcpy(p, q->items.front().data(), q->size);
    return true;
}
inline bool queue_try_remove(queue_t *q, void *p) {
    if (!queue_try_peek(q, p))
        return false;
    q->items.pop_front();
    return true;
}

inline unsigned queue_get_level(queue_t *q) { return q->items.size(); }
