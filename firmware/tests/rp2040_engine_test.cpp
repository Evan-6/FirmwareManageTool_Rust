// Link the production engine without Arduino, TinyUSB or Pico SDK headers.
#include "../common/input/InputEngine.h"
#include "../common/input/BootloaderGate.h"
#include "../boards/rp2040/src/platform/StatusIndicator.h"
#include <assert.h>
#include <stdio.h>
#include <vector>
using namespace hidfw;
namespace w = hidfw::wire;
struct Harness {
    InputEngine engine;
    uint32_t now = 0;
    bool mounted = true, acknowledge = true;
    uint8_t pending = 0;
    bool vendor_pending = false;
    uint32_t vendor_generation = 0;
    std::vector<InputReport> input;
    std::vector<w::Report> replies;
    explicit Harness(uint32_t start = 0) : now(start) { tick(12); }
    void step(uint32_t delta = 1) {
        now += delta;
        const auto done = acknowledge ? pending : 0;
        if (acknowledge)
            pending = 0;
        InputReport report;
        engine.receiveFault(0, 0, now);
        if (engine.advance(now, mounted, done, false, !pending, report)) {
            input.push_back(report);
            pending = report.id;
        }
        if (vendor_pending && acknowledge) {
            engine.vendorCompleted(vendor_generation);
            vendor_pending = false;
        }
        engine.serviceControl(now);
        if (const auto *reply = engine.vendorReply()) {
            replies.push_back(*reply);
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
    void send(uint8_t op, uint32_t seq, std::vector<uint8_t> data = {}, uint32_t id = 42) {
        w::Report r{};
        r.bytes[0] = 3;
        r.bytes[1] = op;
        r.bytes[2] = data.size();
        w::write32(r.bytes + 4, id);
        w::write32(r.bytes + 8, seq);
        if (!data.empty())
            memcpy(r.bytes + 12, data.data(), data.size());
        engine.handle(r, now);
    }
    void open() {
        send(w::Open, 1);
        tick(12);
        assert(engine.session(42));
    }
};
void testIndependentEnginesAndClockWrap() {
    Harness first, second;
    first.open();
    second.open();
    first.send(w::Key, 2, {7, 0, 4, 0, 1});
    first.tick(8);
    assert(first.engine.completedState().keys[0] == 16);
    assert(!second.engine.completedState().held());
    first.engine.receiveFault(0, 65535, first.now);
    first.engine.receiveFault(0, 1, first.now);
    assert(first.engine.counters().rx == 0 && second.engine.counters().failsafe == 0);

    Harness wrap(UINT32_MAX - 1000);
    wrap.open();
    const uint32_t leased = wrap.now - 12;
    wrap.now = leased + 1999;
    wrap.engine.serviceControl(wrap.now);
    assert(wrap.engine.session(42));
    wrap.send(w::Status, 2);
    const auto *status = wrap.engine.vendorReply();
    assert(status);
    assert(w::read16(status->bytes + 22) == 1); // STATUS payload lease field.
    wrap.engine.vendorSubmitted();
    wrap.now = leased + 2000;
    wrap.engine.serviceControl(wrap.now);
    assert(!wrap.engine.session(42) && wrap.engine.counters().failsafe == 1);
    assert(wrap.engine.vendorReply()->bytes[12] == 5);
}
void testStatusAndBarrierDoNotRenew() {
    Harness h;
    h.open();
    const uint32_t leased = h.engine.session(42)->lease_at;
    h.now = leased + 1000;
    h.send(w::Status, 2);
    h.send(w::Barrier, 3);
    h.tick(5);
    h.now = leased + 1999;
    h.engine.serviceControl(h.now);
    assert(h.engine.session(42));
    h.now = leased + 2000;
    h.engine.serviceControl(h.now);
    assert(!h.engine.session(42));
    h.send(w::Heartbeat, 4);
    assert(!h.engine.session(42));
}
void testStaleAndTxOverflow() {
    Harness h;
    h.open();
    h.send(w::Key, 2, {7, 0, 4, 0, 1});
    const auto queued = h.now;
    h.acknowledge = false;
    h.step();
    h.now = queued + 50;
    h.step(0);
    assert(h.engine.session(42));
    h.now = queued + 51;
    h.step(0);
    assert(!h.engine.session(42));
    assert(h.engine.counters().failsafe == 1);
    bool stale = false;
    for (const auto &r : h.replies)
        if (r.bytes[1] == w::Event && r.bytes[12] == 8)
            stale = true;
    assert(stale);

    Harness tx;
    tx.open();
    for (uint32_t seq = 2; seq <= uint32_t(config::TxDepth) + 2; ++seq)
        tx.send(w::Status, seq);
    assert(tx.engine.counters().tx == 1);
    tx.engine.receiveFault(0, 0, tx.now);
    assert(!tx.engine.session(42));
    assert(tx.engine.vendorReply()->bytes[1] == w::Event &&
           tx.engine.vendorReply()->bytes[12] == 8);
}
void testMotionOverflowAndBootCompletion() {
    Harness motion;
    motion.open();
    for (uint32_t seq = 2; seq <= 10; ++seq)
        motion.send(w::Move, seq, {0, 4, 0, 0});
    assert(motion.engine.session(42));
    motion.send(w::Move, 11, {1, 0, 0, 0});
    assert(!motion.engine.session(42));

    Harness boot;
    boot.open();
    boot.send(w::Bootloader, 2);
    while (!boot.vendor_pending)
        boot.tick(1);
    assert(boot.engine.bootRequested() && !boot.engine.bootConfirmed());
    boot.send(w::Status, 3);
    boot.engine.vendorSubmitted(); // A later control must not erase the boot ACK frontier.
    boot.engine.vendorCompleted(boot.vendor_generation + 1);
    boot.engine.serviceControl(boot.now);
    assert(!boot.engine.bootConfirmed());
    boot.engine.vendorCompleted(boot.vendor_generation);
    boot.engine.serviceControl(boot.now);
    assert(boot.engine.bootConfirmed());
    boot.engine.receiveFault(7, 0, boot.now);
    assert(!boot.engine.bootConfirmed());
    boot.engine.vendorCompleted(boot.vendor_generation);
    boot.engine.serviceControl(boot.now);
    assert(!boot.engine.bootConfirmed());
}
void testPeerReleaseAndHistoricalCapture() {
    Harness h;
    h.open();
    h.send(w::Open, 1, {}, 43);
    h.tick(12);
    h.send(w::Release, 2);
    h.tick(12);
    const uint32_t lease = h.engine.session(42)->lease_at;
    h.now = lease + 1900;
    h.send(w::Heartbeat, 2, {}, 43);
    h.now = lease + 2000;
    h.engine.serviceControl(h.now);
    assert(!h.engine.session(42) && h.engine.session(43));
    h.tick(12);
    h.send(w::Key, 3, {7, 0, 4, 0, 1}, 43);
    h.send(w::Key, 4, {7, 0, 4, 0, 0}, 43);
    h.send(w::Barrier, 5, {}, 43);
    h.tick(12);
    bool down = false, barrier = false;
    for (const auto &r : h.input)
        if (r.id == w::KeyboardReportId && (r.data[1] & 16))
            down = true;
    for (const auto &r : h.replies)
        if (r.bytes[1] == (w::Barrier | 128) && w::read32(r.bytes + 4) == 43)
            barrier = true;
    assert(down && barrier && !h.engine.completedState().held());
    assert(h.engine.counters().failsafe == 1);
}
void testSharedBootGate() {
    BootloaderGate gate;
    const uint32_t at = UINT32_MAX - 60;
    assert(!gate.advance(at, false, true, true, true));
    assert(!gate.advance(at, true, true, true, true));
    assert(gate.pending());
    assert(!gate.advance(at + 119, true, true, true, true));
    assert(!gate.advance(at + 120, true, false, true, true));
    assert(!gate.advance(at + 120, true, true, false, true));
    assert(!gate.advance(at + 120, true, true, true, false));
    assert(gate.advance(at + 120, true, true, true, true));
    assert(!gate.advance(at + 120, false, true, true, true) && !gate.pending());
    assert(!gate.advance(at + 121, true, true, true, true));
}
void testLedPriorityAndWrap() {
    StatusIndicator led;
    led.update(0, false, false);
    assert(led.state() == LedState::Off);
    led.update(0, true, false);
    assert(led.state() == LedState::Idle);
    led.update(0, true, true);
    assert(led.state() == LedState::Held);
    const uint32_t start = UINT32_MAX - 50;
    led.signalFailsafe(start);
    led.signalError(start);
    led.update(start + 139, true, true);
    assert(led.state() == LedState::Error);
    led.update(start + 140, true, true);
    assert(led.state() == LedState::Failsafe);
    led.update(start + 2999, true, true);
    assert(led.state() == LedState::Failsafe);
    led.update(start + 3000, true, true);
    assert(led.state() == LedState::Held);
    led.signalBootloader();
    led.update(0, false, false);
    assert(led.state() == LedState::Bootloader);
}
int main() {
    testIndependentEnginesAndClockWrap();
    testStatusAndBarrierDoNotRenew();
    testStaleAndTxOverflow();
    testMotionOverflowAndBootCompletion();
    testLedPriorityAndWrap();
    testSharedBootGate();
    testPeerReleaseAndHistoricalCapture();
    puts("RP2040 portable engine: independent instances, lease/stale boundaries, clock/counter "
         "wrap, TX/motion overflow, boot generation and LED priority passed");
}
