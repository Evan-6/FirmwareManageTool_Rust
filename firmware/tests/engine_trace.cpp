// Deterministic A/B trace. The exact same script runs against the shipping C++
// engine here and against the Rust port in
// `firmware/engine/tests/trace.rs`; both must print byte-identical lines.
// Regenerate `engine_trace.golden` only for an intended behaviour change.
#include "../boards/rp2040/src/input/InputEngine.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <string>
#include <vector>
using namespace hidfw;
namespace w = hidfw::wire;

static std::string hex(const uint8_t *p, size_t n) {
    static const char digits[] = "0123456789abcdef";
    std::string out;
    out.reserve(n * 2);
    for (size_t i = 0; i < n; ++i) {
        out += digits[p[i] >> 4];
        out += digits[p[i] & 15];
    }
    return out;
}

struct Harness {
    InputEngine engine;
    uint32_t now = 0;
    bool mounted = true, acknowledge = true;
    uint8_t pending = 0;
    bool vendor_pending = false;
    uint32_t vendor_generation = 0;
    std::string trace;
    explicit Harness(uint32_t start = 0) : now(start) { tick(12); }
    void step(uint32_t delta = 1) {
        now += delta;
        const auto done = acknowledge ? pending : 0;
        if (acknowledge)
            pending = 0;
        InputReport report;
        engine.receiveFault(0, 0, now);
        if (engine.advance(now, mounted, done, false, !pending, report)) {
            pending = report.id;
            char line[64];
            snprintf(line, sizeof line, "I %u %u %u ", now, report.id, report.length);
            trace += line;
            trace += hex(report.data, report.length);
            trace += "\n";
        }
        if (vendor_pending && acknowledge) {
            engine.vendorCompleted(vendor_generation);
            vendor_pending = false;
        }
        engine.serviceControl(now);
        if (const auto *reply = engine.vendorReply()) {
            char line[96];
            snprintf(line, sizeof line, "R %u %u %u %u %u ", now, reply->bytes[1],
                     w::read32(reply->bytes + 4), w::read32(reply->bytes + 8), reply->bytes[2]);
            trace += line;
            trace += hex(reply->bytes + 12, reply->bytes[2]);
            trace += "\n";
            if (reply->bytes[1] == (w::Bootloader | 128) && reply->bytes[12] == 0) {
                vendor_pending = true;
                vendor_generation = engine.bootGeneration();
            }
            engine.vendorSubmitted();
        }
    }
    void tick(unsigned n) {
        for (unsigned i = 0; i < n; ++i)
            step();
    }
    void send(uint8_t op, uint32_t seq, const std::vector<uint8_t> &data, uint32_t id) {
        w::Report r{};
        r.bytes[0] = 3;
        r.bytes[1] = op;
        r.bytes[2] = uint8_t(data.size());
        w::write32(r.bytes + 4, id);
        w::write32(r.bytes + 8, seq);
        if (!data.empty())
            memcpy(r.bytes + 12, data.data(), data.size());
        engine.handle(r, now);
    }
};

int main() {
    const uint32_t A = 0x11111111u, B = 0x22222222u, C = 0x33333333u;
    Harness h(0);
    h.send(w::Open, 1, {}, A);
    h.tick(6);
    h.send(w::Open, 1, {}, B);
    h.tick(6);
    h.send(w::Key, 2, {7, 0, 0x1e, 0, 1}, A);
    h.tick(3);
    h.send(w::Key, 2, {7, 0, 0x1f, 0, 1}, B);
    h.tick(3);
    h.send(w::Move, 3, {7, 0, 0xf9, 0xff}, A);
    h.tick(3);
    h.send(w::Buttons, 3, {1}, B);
    h.tick(3);
    h.send(w::Barrier, 4, {}, A);
    h.tick(6);
    h.send(w::Status, 4, {}, B);
    h.tick(2);
    h.send(w::Release, 5, {}, A);
    h.tick(6);
    h.send(w::Key, 5, {7, 0, 0x1f, 0, 0}, B);
    h.tick(3);
    h.send(w::Barrier, 6, {}, B);
    h.tick(6);
    h.send(w::Heartbeat, 7, {}, B);
    h.tick(2);
    h.send(w::Status, 8, {}, B);
    h.tick(2);

    // Stop acknowledging USB so the head job ages out.
    h.acknowledge = false;
    h.send(w::Key, 9, {7, 0, 0x20, 0, 1}, B);
    h.step();
    // 50 ms is still allowed, 51 ms is stale.
    // step() already advanced 1 ms, so +49 puts the head job at exactly 50 ms.
    h.now += 49;
    h.step(0);
    h.now += 1;
    h.step(0);
    h.acknowledge = true;
    // The global fault queued one all-clear state; OPEN is refused until it drained.
    for (int i = 0; i < 20 && !h.engine.idle(); ++i)
        h.step();

    h.send(w::Open, 1, {}, C);
    h.tick(6);
    h.send(w::Bootloader, 2, {}, C);
    for (int i = 0; i < 20 && !h.vendor_pending; ++i)
        h.step();
    h.tick(8);

    // Lease expiry after the boot request.
    h.now += 2000;
    h.engine.serviceControl(h.now);
    h.tick(2);

    printf("%s", h.trace.c_str());
    const auto &c = h.engine.counters();
    printf("S %u %u %u %u %u %u %u %u\n", h.now, (unsigned)h.engine.sessionCount(),
           (unsigned)c.rx, (unsigned)c.tx, (unsigned)c.hid, (unsigned)c.failsafe,
           (unsigned)h.engine.idle(), (unsigned)h.engine.bootConfirmed());
    return 0;
}
