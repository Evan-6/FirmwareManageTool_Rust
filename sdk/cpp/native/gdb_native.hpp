// gdb_native.hpp - header-only C++17 GDBOX SDK that speaks vendor HID v3 itself.
//
// The other C++ binding (`sdk/cpp/gdb.hpp`) loads gdb.dll and forwards to the
// Rust SDK. This one has no library behind it: it opens the vendor collection
// with CreateFileW, frames reports on its own, keeps the session lease alive and
// exposes the same operations. Diagnostics (Feature 14, device strings, the
// absolute cursor) come from hid.dll / cfgmgr32.dll, loaded at runtime so the
// header needs no import library.
//
//     #include "gdb_native.hpp"
//     auto device = gdb::native::Device::open();           // first GDBOX device
//     device.keyboard().type_text("hello");
//     device.mouse().move_to(200, 300, 250);
//     auto movement = device.mouse().start_move_to(640, 480);   // background
//     movement.retarget(100, 100);                              // mid-flight
//     movement.wait(2000);
//     auto fake = std::make_shared<gdb::native::FakeTransport>();
//     auto mock = gdb::native::Device::open_fake(fake);     // headless, no hardware
//
// Three layers, the same split as the Rust SDK:
//
//   * `Device`/`Mouse`/`Keyboard`/`Scheduler` speak the protocol (this header);
//   * `Movement` runs one move on its own thread so a caller can cancel,
//     retarget or wait for it (port of sdk/rust/src/motion.rs);
//   * `flash` builds the firmware, packs ELF -> UF2, copies it to the BOOTSEL
//     drive and verifies the rebooted board (port of sdk/rust/src/flash.rs).
//
// Diagnostics go through `set_log_level`/`set_log_sink`; a Trace line is the
// same text the Rust SDK writes (`out opcode=… session=… sequence=… payload=…`).
//
// Every call throws `gdb::native::Error` (code + message) on failure; nothing
// returns a silent error code. Everything here is one translation unit's worth
// of inline functions, so include it from a single .cpp when possible.
//
// Threading: one `Device` owns one session and one heartbeat thread. Commands
// are serialised by a mutex; the heartbeat takes the same mutex, so a caller
// that blocks inside a command delays (never breaks) the heartbeat. Event
// handlers run on whichever thread noticed the change and may run while the
// device lock is held, so they must not call back into the same Device.

#pragma once

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <exception>
#include <fstream>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <set>
#include <stdexcept>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#ifndef WINVER
#define WINVER 0x0A00
#endif
#include <windows.h>
#endif

namespace gdb {
namespace native {

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

// The numeric values are shared with the C ABI (sdk/ffi/include/gdb.h) and with
// every other binding, so they must never be reordered.
enum class ErrorCode : std::int32_t {
    Ok = 0,
    Invalid = 1,
    Timeout = 2,
    Disconnected = 3,
    Busy = 4,
    Lease = 5,
    Framing = 6,
    Overflow = 7,
    Unsupported = 8,
    Cancelled = 9,
    Io = 10,
};

class Error : public std::runtime_error {
public:
    Error(ErrorCode code, const std::string& message)
        : std::runtime_error(message), code_(code) {}

    ErrorCode code() const noexcept { return code_; }

private:
    ErrorCode code_;
};

inline const char* error_text(ErrorCode code) {
    switch (code) {
        case ErrorCode::Ok: return "ok";
        case ErrorCode::Invalid: return "invalid argument";
        case ErrorCode::Timeout: return "device did not answer in time";
        case ErrorCode::Disconnected: return "device disconnected";
        case ErrorCode::Busy: return "device busy: same session id or all eight owners taken";
        case ErrorCode::Lease: return "session lease expired";
        case ErrorCode::Framing: return "device rejected the frame";
        case ErrorCode::Overflow: return "device queue overflow or stale output";
        case ErrorCode::Unsupported: return "not supported on this platform or firmware";
        case ErrorCode::Cancelled: return "cancelled";
        case ErrorCode::Io: return "HID transfer failed";
    }
    return "unknown";
}

// Maps the protocol result byte (`firmware/protocol-v3.md`, 控制回應) onto the
// SDK code space. Codes 6 and 7 both mean "the USB link is gone".
inline Error error_from_result(std::uint8_t code) {
    switch (code) {
        case 0:
            return Error(ErrorCode::Ok, error_text(ErrorCode::Ok));
        case 1:
            return Error(ErrorCode::Framing, "device reported a framing error");
        case 2:
            return Error(ErrorCode::Busy, error_text(ErrorCode::Busy));
        case 3:
            return Error(ErrorCode::Invalid, "device rejected the input");
        case 5:
            return Error(ErrorCode::Lease, error_text(ErrorCode::Lease));
        case 6:
            return Error(ErrorCode::Disconnected, "device failed to send the HID report");
        case 7:
            return Error(ErrorCode::Disconnected, error_text(ErrorCode::Disconnected));
        case 8:
            return Error(ErrorCode::Overflow, error_text(ErrorCode::Overflow));
        case 9:
            return Error(ErrorCode::Framing, "duplicate sequence");
        case 10:
            return Error(ErrorCode::Framing, "sequence gap");
        default:
            return Error(ErrorCode::Io, "device error code " + std::to_string(code));
    }
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

// Same five levels and the same process-wide switch as `gdb::log` in the Rust
// SDK: off by default, one sink per process, filtered by level before anything
// is formatted.
namespace log {

constexpr std::int32_t kOff = 0;
constexpr std::int32_t kError = 1;
constexpr std::int32_t kWarn = 2;
constexpr std::int32_t kInfo = 3;
constexpr std::int32_t kDebug = 4;
constexpr std::int32_t kTrace = 5;

enum class Level : std::int32_t {
    Off = kOff,
    Error = kError,
    Warn = kWarn,
    Info = kInfo,
    Debug = kDebug,
    Trace = kTrace,
};

using Sink = std::function<void(std::int32_t, std::string_view)>;

namespace state {

struct Logger {
    // Read on every packet, so the level is a plain atomic.
    std::atomic<std::int32_t> level{kOff};
    std::mutex sink_mutex;
    Sink sink;
};

// One instance per process even with several translation units including this
// header: inline functions share their local statics.
inline Logger& logger() {
    static Logger instance;
    return instance;
}

inline std::int32_t clamp_level(std::int32_t level) {
    return std::clamp<std::int32_t>(level, kOff, kTrace);
}

}  // namespace state

inline void set_level(std::int32_t level) {
    state::logger().level.store(state::clamp_level(level));
}

inline std::int32_t level() { return state::logger().level.load(); }

// An empty function removes the sink.
inline void set_sink(Sink sink) {
    std::lock_guard<std::mutex> lock(state::logger().sink_mutex);
    state::logger().sink = std::move(sink);
}

inline bool enabled(std::int32_t wanted) { return wanted <= state::logger().level.load(); }

inline bool enabled(Level wanted) { return enabled(static_cast<std::int32_t>(wanted)); }

// The sink is copied out before it runs, so it may call back into this API.
inline void write(std::int32_t wanted, std::string_view message) {
    if (!enabled(wanted)) {
        return;
    }
    Sink sink;
    {
        std::lock_guard<std::mutex> lock(state::logger().sink_mutex);
        sink = state::logger().sink;
    }
    if (!sink) {
        return;
    }
    try {
        sink(wanted, message);
    } catch (...) {
        // A throwing sink must not take a protocol call down with it.
    }
}

inline void write(Level wanted, std::string_view message) {
    write(static_cast<std::int32_t>(wanted), message);
}

}  // namespace log

// Level and sink as the other bindings spell them (`gdb_set_log_level`).
inline void set_log_level(std::int32_t level) { log::set_level(level); }
inline std::int32_t log_level() { return log::level(); }
inline void set_log_sink(log::Sink sink) { log::set_sink(std::move(sink)); }

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

constexpr std::size_t REPORT_SIZE = 64;
constexpr std::size_t MAX_PAYLOAD = 51;
constexpr std::size_t SNAPSHOT_BYTES = 31;
constexpr std::size_t STATUS_BYTES = 51;
constexpr std::size_t STATUS_EXT_BYTES = 23;
constexpr std::size_t SESSION_ENTRY = 6;
constexpr std::size_t SESSION_SLOTS = 8;
constexpr std::size_t SESSION_LIST_BYTES = SESSION_ENTRY * SESSION_SLOTS;

constexpr std::uint8_t OUT_ID = 12;
constexpr std::uint8_t IN_ID = 13;
constexpr std::uint8_t FEATURE_ID = 14;
constexpr std::uint8_t WIRE_VERSION = 3;

constexpr std::uint8_t OPEN = 0x01;
constexpr std::uint8_t CLOSE = 0x02;
constexpr std::uint8_t HEARTBEAT = 0x03;
constexpr std::uint8_t BARRIER = 0x04;
constexpr std::uint8_t RELEASE_ALL = 0x05;
constexpr std::uint8_t STATUS = 0x06;
constexpr std::uint8_t STATUS_EXT = 0x07;
constexpr std::uint8_t SESSION_LIST = 0x08;
constexpr std::uint8_t BOOTLOADER = 0x09;
constexpr std::uint8_t KEY = 0x10;
constexpr std::uint8_t SNAPSHOT = 0x11;
constexpr std::uint8_t MOUSE_MOVE = 0x12;
constexpr std::uint8_t MOUSE_BUTTONS = 0x13;
constexpr std::uint8_t WHEEL = 0x14;
constexpr std::uint8_t EVENT = 0x7F;

// Firmware 3.2.0: 2000 ms lease, renewed by everything except STATUS and
// BARRIER, so a host polling STATUS still has to heartbeat every 500 ms.
constexpr std::uint32_t DEVICE_LEASE_MS = 2000;
constexpr std::uint32_t HEARTBEAT_MS = 500;

inline std::uint16_t read_u16(const std::uint8_t* p) {
    return static_cast<std::uint16_t>(p[0] | (static_cast<std::uint16_t>(p[1]) << 8));
}

inline std::uint32_t read_u32(const std::uint8_t* p) {
    return static_cast<std::uint32_t>(p[0]) | (static_cast<std::uint32_t>(p[1]) << 8) |
           (static_cast<std::uint32_t>(p[2]) << 16) | (static_cast<std::uint32_t>(p[3]) << 24);
}

inline void write_u16(std::uint8_t* p, std::uint16_t value) {
    p[0] = static_cast<std::uint8_t>(value & 0xFF);
    p[1] = static_cast<std::uint8_t>(value >> 8);
}

inline void write_u32(std::uint8_t* p, std::uint32_t value) {
    p[0] = static_cast<std::uint8_t>(value & 0xFF);
    p[1] = static_cast<std::uint8_t>((value >> 8) & 0xFF);
    p[2] = static_cast<std::uint8_t>((value >> 16) & 0xFF);
    p[3] = static_cast<std::uint8_t>((value >> 24) & 0xFF);
}

// A control or input packet. `encode` mirrors `Packet::encode` in
// shared/input-protocol/src/v3.rs byte for byte; `decode` rejects anything the
// firmware would never send (wrong report id, wrong version, flags, padding).
struct Packet {
    std::uint8_t opcode = 0;
    std::uint8_t flags = 0;
    std::uint32_t session = 0;
    std::uint32_t sequence = 0;
    std::vector<std::uint8_t> payload;

    std::array<std::uint8_t, REPORT_SIZE> encode(std::uint8_t id) const {
        if (id != OUT_ID && id != IN_ID) {
            throw Error(ErrorCode::Framing, "v3 reports use id 12 or 13");
        }
        if (payload.size() > MAX_PAYLOAD) {
            throw Error(ErrorCode::Framing, "v3 payload is at most 51 bytes");
        }
        if (flags != 0) {
            throw Error(ErrorCode::Framing, "v3 flags must be zero");
        }
        std::array<std::uint8_t, REPORT_SIZE> report{};
        report[0] = id;
        report[1] = WIRE_VERSION;
        report[2] = opcode;
        report[3] = static_cast<std::uint8_t>(payload.size());
        report[4] = flags;
        write_u32(&report[5], session);
        write_u32(&report[9], sequence);
        std::copy(payload.begin(), payload.end(), report.begin() + 13);
        return report;
    }

    static Packet decode(const std::array<std::uint8_t, REPORT_SIZE>& report, std::uint8_t id) {
        if (report[0] != id || report[1] != WIRE_VERSION || report[4] != 0) {
            throw Error(ErrorCode::Invalid, "v3 report id, version or flags are wrong");
        }
        const std::size_t length = report[3];
        if (length > MAX_PAYLOAD) {
            throw Error(ErrorCode::Invalid, "v3 payload is longer than 51 bytes");
        }
        for (std::size_t index = 13 + length; index < REPORT_SIZE; ++index) {
            if (report[index] != 0) {
                throw Error(ErrorCode::Invalid, "v3 report padding is not zero");
            }
        }
        Packet packet;
        packet.opcode = report[2];
        packet.session = read_u32(&report[5]);
        packet.sequence = read_u32(&report[9]);
        packet.payload.assign(report.begin() + 13, report.begin() + 13 + static_cast<std::ptrdiff_t>(length));
        return packet;
    }
};

// Consumer usages the firmware drives, in bit order 0..6.
constexpr std::uint16_t CONSUMERS[7] = {0xE2, 0xE9, 0xEA, 0xCD, 0xB5, 0xB6, 0xB7};

// `Vec<u8>`'s Debug shape — that is what the Rust SDK writes into a Trace line,
// so a sink written against one binding reads the other unchanged.
inline std::string trace_payload(const std::vector<std::uint8_t>& payload) {
    std::string out = "[";
    for (std::size_t index = 0; index < payload.size(); ++index) {
        if (index != 0) {
            out += ", ";
        }
        out += std::to_string(static_cast<unsigned>(payload[index]));
    }
    out += "]";
    return out;
}

inline bool key_valid(std::uint16_t page, std::uint16_t usage);

// 31-byte union state: modifiers, 28-byte NKRO bitmap, consumer bits, buttons.
struct InputState {
    std::uint8_t modifiers = 0;
    std::array<std::uint8_t, 28> keys{};
    std::uint8_t consumer = 0;
    std::uint8_t buttons = 0;

    void set(std::uint16_t page, std::uint16_t usage, bool down) {
        if (!key_valid(page, usage)) {
            throw Error(ErrorCode::Invalid,
                        "usage " + std::to_string(page) + ":" + std::to_string(usage) +
                            " is not in the key catalog");
        }
        std::uint8_t* byte = nullptr;
        std::uint8_t bit = 0;
        if (page == 7 && usage >= 0xE0) {
            byte = &modifiers;
            bit = static_cast<std::uint8_t>(1u << (usage - 0xE0));
        } else if (page == 7) {
            byte = &keys[usage / 8];
            bit = static_cast<std::uint8_t>(1u << (usage % 8));
        } else {
            std::size_t index = 0;
            for (; index < 7; ++index) {
                if (CONSUMERS[index] == usage) {
                    break;
                }
            }
            byte = &consumer;
            bit = static_cast<std::uint8_t>(1u << index);
        }
        if (down) {
            *byte = static_cast<std::uint8_t>(*byte | bit);
        } else {
            *byte = static_cast<std::uint8_t>(*byte & ~bit);
        }
    }

    bool contains(std::uint16_t page, std::uint16_t usage) const {
        if (!key_valid(page, usage)) {
            return false;
        }
        if (page == 7 && usage >= 0xE0) {
            return (modifiers & (1u << (usage - 0xE0))) != 0;
        }
        if (page == 7) {
            return (keys[usage / 8] & (1u << (usage % 8))) != 0;
        }
        for (std::size_t index = 0; index < 7; ++index) {
            if (CONSUMERS[index] == usage) {
                return (consumer & (1u << index)) != 0;
            }
        }
        return false;
    }

    std::array<std::uint8_t, SNAPSHOT_BYTES> encode() const {
        std::array<std::uint8_t, SNAPSHOT_BYTES> out{};
        out[0] = modifiers;
        std::copy(keys.begin(), keys.end(), out.begin() + 1);
        out[29] = consumer;
        out[30] = buttons;
        return out;
    }

    static InputState decode(const std::uint8_t* p, std::size_t length) {
        if (length != SNAPSHOT_BYTES) {
            throw Error(ErrorCode::Invalid, "input snapshots are 31 bytes");
        }
        if ((p[29] & 0x80) != 0 || (p[30] & 0xE0) != 0) {
            throw Error(ErrorCode::Invalid, "input snapshot reserved bits are set");
        }
        InputState state;
        state.modifiers = p[0];
        std::copy(p + 1, p + 29, state.keys.begin());
        state.consumer = p[29];
        state.buttons = p[30];
        for (std::uint16_t usage = 0; usage < 224; ++usage) {
            if ((state.keys[usage / 8] & (1u << (usage % 8))) != 0 && !key_valid(7, usage)) {
                throw Error(ErrorCode::Invalid, "input snapshot holds a reserved usage");
            }
        }
        return state;
    }
};

// Feature 14: the identity block the firmware answers without taking ownership.
struct Capabilities {
    std::uint8_t board = 0;
    std::string firmware;
    std::uint16_t lease_ms = 0;
    std::uint8_t poll_ms = 0;
    std::uint32_t flags = 0;
    std::array<std::uint8_t, 28> keyboard{};
    std::uint8_t modifiers = 0;
    std::uint8_t consumer = 0;
    std::array<std::uint8_t, 8> device_id{};
    std::uint32_t build = 0;

    static Capabilities parse(const std::array<std::uint8_t, REPORT_SIZE>& r) {
        if (r[0] != FEATURE_ID || std::memcmp(&r[1], "FMT3", 4) != 0 || r[5] != WIRE_VERSION) {
            throw Error(ErrorCode::Invalid, "Feature 14 is not protocol 3");
        }
        for (std::size_t index = 61; index < REPORT_SIZE; ++index) {
            if (r[index] != 0) {
                throw Error(ErrorCode::Invalid, "capability reserved fields are not zero");
            }
        }
        Capabilities caps;
        caps.board = r[6];
        caps.firmware = std::to_string(r[7]) + "." + std::to_string(r[8]) + "." + std::to_string(r[9]);
        caps.lease_ms = read_u16(&r[10]);
        caps.poll_ms = r[14];
        caps.flags = read_u32(&r[15]);
        std::copy(r.begin() + 19, r.begin() + 47, caps.keyboard.begin());
        caps.modifiers = r[47];
        caps.consumer = r[48];
        std::copy(r.begin() + 49, r.begin() + 57, caps.device_id.begin());
        caps.build = read_u32(&r[57]);
        if (caps.lease_ms < 1000 || caps.poll_ms == 0) {
            throw Error(ErrorCode::Invalid, "Feature 14 lease or poll interval is invalid");
        }
        return caps;
    }

    bool supports(std::uint16_t page, std::uint16_t usage) const {
        if (!key_valid(page, usage)) {
            return false;
        }
        if (page == 7 && usage >= 0xE0) {
            return (modifiers & (1u << (usage - 0xE0))) != 0;
        }
        if (page == 7) {
            return (keyboard[usage / 8] & (1u << (usage % 8))) != 0;
        }
        for (std::size_t index = 0; index < 7; ++index) {
            if (CONSUMERS[index] == usage) {
                return (consumer & (1u << index)) != 0;
            }
        }
        return false;
    }
};

// STATUS (06, 51 bytes): per-session counters, shared fault counters, snapshot.
struct Status {
    std::uint32_t received = 0;
    std::uint32_t completed = 0;
    std::uint8_t pending = 0;
    std::uint16_t lease_ms = 0;
    std::uint16_t rx = 0;
    std::uint16_t tx = 0;
    std::uint16_t hid = 0;
    std::uint16_t failsafe = 0;
    InputState state;

    static Status parse(const std::vector<std::uint8_t>& p) {
        if (p.size() != STATUS_BYTES) {
            throw Error(ErrorCode::Framing, "STATUS is 51 bytes");
        }
        if (p[0] != 0) {
            throw error_from_result(p[0]);
        }
        Status status;
        status.received = read_u32(&p[1]);
        status.completed = read_u32(&p[5]);
        status.pending = p[9];
        status.lease_ms = read_u16(&p[10]);
        status.rx = read_u16(&p[12]);
        status.tx = read_u16(&p[14]);
        status.hid = read_u16(&p[16]);
        status.failsafe = read_u16(&p[18]);
        status.state = InputState::decode(&p[20], SNAPSHOT_BYTES);
        return status;
    }
};

// STATUS_EXT (07, 23 bytes): scheduling and reset diagnostics.
struct StatusExt {
    std::uint32_t uptime_ms = 0;
    std::uint16_t late_max_us = 0;
    std::uint8_t out_depth = 0;
    std::uint8_t rx_depth = 0;
    std::uint8_t tx_depth = 0;
    std::uint8_t sessions = 0;
    std::uint8_t last_fault = 0;
    bool has_fault = false;
    std::uint32_t last_fault_age_ms = 0;
    std::uint8_t reset_reason = 0;
    std::uint16_t boot_count = 0;
    std::uint32_t build = 0;

    static StatusExt parse(const std::vector<std::uint8_t>& p) {
        if (p.size() != STATUS_EXT_BYTES) {
            throw Error(ErrorCode::Framing, "STATUS_EXT is 23 bytes");
        }
        if (p[0] != 0) {
            throw error_from_result(p[0]);
        }
        StatusExt ext;
        ext.uptime_ms = read_u32(&p[1]);
        ext.late_max_us = read_u16(&p[5]);
        ext.out_depth = p[7];
        ext.rx_depth = p[8];
        ext.tx_depth = p[9];
        ext.sessions = p[10];
        ext.last_fault = p[11];
        ext.last_fault_age_ms = read_u32(&p[12]);
        ext.has_fault = ext.last_fault_age_ms != 0xFFFFFFFFu;
        ext.reset_reason = p[16];
        ext.boot_count = read_u16(&p[17]);
        ext.build = read_u32(&p[19]);
        return ext;
    }
};

// One owner in the SESSION_LIST table. SESSION_LIST has no result byte: the
// first entry's id sits in byte 0, so an empty table reads as 48 zero bytes.
struct SessionInfo {
    std::uint16_t id = 0;
    std::uint16_t lease_ms = 0;
    std::uint8_t held = 0;
    std::uint8_t flags = 0;
};

inline std::vector<SessionInfo> parse_session_list(const std::vector<std::uint8_t>& p) {
    if (p.size() != SESSION_LIST_BYTES) {
        throw Error(ErrorCode::Framing, "SESSION_LIST is 48 bytes");
    }
    std::vector<SessionInfo> sessions;
    for (std::size_t index = 0; index < SESSION_SLOTS; ++index) {
        const std::uint8_t* entry = &p[index * SESSION_ENTRY];
        const std::uint16_t id = read_u16(entry);
        if (id == 0) {
            continue;
        }
        SessionInfo info;
        info.id = id;
        info.lease_ms = read_u16(entry + 2);
        info.held = entry[4];
        info.flags = entry[5];
        sessions.push_back(info);
    }
    return sessions;
}

// ---------------------------------------------------------------------------
// Key catalog and the US typing table
// ---------------------------------------------------------------------------

// keys.json defines the whole catalog; page 7 collapses to these ranges and page
// 12 to the seven consumer usages above.
inline bool key_valid(std::uint16_t page, std::uint16_t usage) {
    if (page == 7) {
        return (usage >= 0x04 && usage <= 0x31) || (usage >= 0x33 && usage <= 0x65) ||
               (usage >= 0x68 && usage <= 0x73) || (usage >= 0x87 && usage <= 0x8B) ||
               (usage >= 0xE0 && usage <= 0xE7);
    }
    if (page == 12) {
        for (std::size_t index = 0; index < 7; ++index) {
            if (CONSUMERS[index] == usage) {
                return true;
            }
        }
    }
    return false;
}

struct KeyStroke {
    std::uint16_t page = 7;
    std::uint16_t usage = 0;
    bool shift = false;
};

// Ported from sdk/rust/src/keyboard.rs: a US layout, characters the firmware has
// no usage for are dropped, so the stroke count is not the character count.
inline std::vector<KeyStroke> strokes(const std::string& text) {
    std::vector<KeyStroke> out;
    for (const char raw : text) {
        const unsigned char c = static_cast<unsigned char>(raw);
        if (c == '\r') {
            continue;
        }
        std::uint16_t usage = 0;
        bool shift = false;
        if (c >= 'a' && c <= 'z') {
            usage = static_cast<std::uint16_t>(0x04 + (c - 'a'));
        } else if (c >= 'A' && c <= 'Z') {
            usage = static_cast<std::uint16_t>(0x04 + (c - 'A'));
            shift = true;
        } else if (c >= '1' && c <= '9') {
            usage = static_cast<std::uint16_t>(0x1E + (c - '1'));
        } else if (c >= 0x80) {
            continue;  // not a US layout character
        } else {
            switch (c) {
                case '0': usage = 0x27; break;
                case '-': usage = 0x2D; break;
                case '=': usage = 0x2E; break;
                case '[': usage = 0x2F; break;
                case ']': usage = 0x30; break;
                case '\\': usage = 0x31; break;
                case ';': usage = 0x33; break;
                case '\'': usage = 0x34; break;
                case '`': usage = 0x35; break;
                case ',': usage = 0x36; break;
                case '.': usage = 0x37; break;
                case '/': usage = 0x38; break;
                case '!': usage = 0x1E; shift = true; break;
                case '@': usage = 0x1F; shift = true; break;
                case '#': usage = 0x20; shift = true; break;
                case '$': usage = 0x21; shift = true; break;
                case '%': usage = 0x22; shift = true; break;
                case '^': usage = 0x23; shift = true; break;
                case '&': usage = 0x24; shift = true; break;
                case '*': usage = 0x25; shift = true; break;
                case '(': usage = 0x26; shift = true; break;
                case ')': usage = 0x27; shift = true; break;
                case '_': usage = 0x2D; shift = true; break;
                case '+': usage = 0x2E; shift = true; break;
                case '{': usage = 0x2F; shift = true; break;
                case '}': usage = 0x30; shift = true; break;
                case '|': usage = 0x31; shift = true; break;
                case ':': usage = 0x33; shift = true; break;
                case '"': usage = 0x34; shift = true; break;
                case '~': usage = 0x35; shift = true; break;
                case '<': usage = 0x36; shift = true; break;
                case '>': usage = 0x37; shift = true; break;
                case '?': usage = 0x38; shift = true; break;
                case '\n': usage = 0x28; break;
                case '\t': usage = 0x2B; break;
                case ' ': usage = 0x2C; break;
                default: continue;
            }
        }
        KeyStroke stroke;
        stroke.page = 7;
        stroke.usage = usage;
        stroke.shift = shift;
        out.push_back(stroke);
    }
    return out;
}

// ---------------------------------------------------------------------------
// Motion maths (port of sdk/motion/src/lib.rs, identical arithmetic)
// ---------------------------------------------------------------------------

class Rng {
public:
    virtual ~Rng() = default;
    // Must return [0, 1).
    virtual double unit() = 0;
};

// SplitMix64: tiny, seedable, the same numbers on every platform.
class SplitMix64 : public Rng {
public:
    explicit SplitMix64(std::uint64_t seed) : state_(seed) {}

    static SplitMix64 from_entropy() {
        const auto now = std::chrono::steady_clock::now().time_since_epoch();
        const std::uint64_t nanos =
            static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now).count());
        const std::uint64_t pid = static_cast<std::uint64_t>(
            std::hash<std::thread::id>{}(std::this_thread::get_id()));
        return SplitMix64(nanos ^ pid ^ 0x9E3779B97F4A7C15ull);
    }

    double unit() override {
        // 53 significant bits, the same value every language reproduces.
        return static_cast<double>(next() >> 11) / static_cast<double>(1ull << 53);
    }

private:
    std::uint64_t next() {
        state_ += 0x9E3779B97F4A7C15ull;
        std::uint64_t z = state_;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
        return z ^ (z >> 31);
    }

    std::uint64_t state_;
};

struct Screen {
    std::int32_t width = 1920;
    std::int32_t height = 1080;
};

struct Node {
    std::uint64_t at_ms = 0;
    std::int32_t x = 0;
    std::int32_t y = 0;
};

// Same values as gdb.hpp's Ease, so a profile translates one to one.
enum class Ease : std::int32_t {
    Jerk = 0,
    InOut = 1,
    Linear = 2,
    In = 3,
    Out = 4,
    CubicBezier = 5,
};

inline double bezier(double a, double b, double c, double d, double t) {
    const double u = 1.0 - t;
    return u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
}

// Undo the x axis of a cubic bezier (Newton, fixed iteration count so the result
// does not depend on floating-point convergence luck), then read y.
inline double cubic_bezier_ease(double t, double x1, double y1, double x2, double y2) {
    x1 = std::clamp(x1, 0.0, 1.0);
    x2 = std::clamp(x2, 0.0, 1.0);
    const auto curve_x = [x1, x2](double u) { return bezier(0.0, x1, x2, 1.0, u); };
    const auto curve_y = [y1, y2](double u) { return bezier(0.0, y1, y2, 1.0, u); };
    double u = t;
    for (int iteration = 0; iteration < 8; ++iteration) {
        const double x = curve_x(u) - t;
        if (std::fabs(x) < 1e-9) {
            break;
        }
        const double slope = (curve_x(std::min(u + 1e-6, 1.0)) - curve_x(std::max(u - 1e-6, 0.0))) / 2e-6;
        if (std::fabs(slope) < 1e-9) {
            break;
        }
        u = std::clamp(u - x / slope, 0.0, 1.0);
    }
    return std::clamp(curve_y(u), 0.0, 1.0);
}

inline double ease_apply(Ease ease, double t, const double bezier_control[4]) {
    t = std::clamp(t, 0.0, 1.0);
    switch (ease) {
        case Ease::Linear: return t;
        // The original "jerk" curve: smooth at both ends, no overshoot.
        case Ease::Jerk: return t * t * t * (10.0 + t * (-15.0 + 6.0 * t));
        case Ease::In: return t * t * t;
        case Ease::Out: {
            const double u = 1.0 - t;
            return 1.0 - u * u * u;
        }
        case Ease::InOut: return t * t * (3.0 - 2.0 * t);
        case Ease::CubicBezier:
            return cubic_bezier_ease(t, bezier_control[0], bezier_control[1], bezier_control[2],
                                     bezier_control[3]);
    }
    return t;
}

// Human-ish movement profile; field names and defaults match gdb.hpp's Profile.
struct HumanProfile {
    Ease ease = Ease::Jerk;
    double bezier[4] = {0.42, 0.0, 0.58, 1.0};
    double curve = 1.0;
    double jitter_px = 1.0;
    double overshoot_pct = 0.0;
    std::uint32_t settle_ms = 0;
    std::int32_t min_step_px = 1;

    static HumanProfile linear() {
        HumanProfile profile;
        profile.ease = Ease::Linear;
        profile.curve = 0.0;
        profile.jitter_px = 0.0;
        return profile;
    }
};

// Splits one relative move into HID-sized chunks (+/-1024 per axis).
inline std::vector<std::pair<std::int32_t, std::int32_t>> split_delta(std::int32_t x, std::int32_t y) {
    std::vector<std::pair<std::int32_t, std::int32_t>> parts;
    while (x != 0 || y != 0) {
        const std::int32_t dx = std::clamp(x, -1024, 1024);
        const std::int32_t dy = std::clamp(y, -1024, 1024);
        parts.emplace_back(dx, dy);
        x -= dx;
        y -= dy;
    }
    return parts;
}

// Distance-adaptive duration used when the caller passes duration_ms = 0.
inline std::uint32_t natural_duration(double distance, Rng& rng) {
    const double value = 150.0 + distance * 0.42 + rng.unit() * 70.0 - 25.0;
    return static_cast<std::uint32_t>(std::clamp(value, 170.0, 900.0));
}

// Samples a human-ish path from `start` to `target` on `screen`. The returned
// nodes are host coordinates; `at_ms` is relative to the movement start.
inline std::vector<Node> trajectory_with(std::pair<std::int32_t, std::int32_t> start,
                                         std::pair<std::int32_t, std::int32_t> target, Screen screen,
                                         std::uint32_t duration_ms, const HumanProfile& profile,
                                         Rng& rng) {
    if (screen.width <= 1 || screen.height <= 1) {
        throw Error(ErrorCode::Invalid, "screen size is invalid");
    }
    target.first = std::clamp(target.first, 0, screen.width - 1);
    target.second = std::clamp(target.second, 0, screen.height - 1);
    const double dx = static_cast<double>(target.first) - static_cast<double>(start.first);
    const double dy = static_cast<double>(target.second) - static_cast<double>(start.second);
    const double distance = std::hypot(dx, dy);
    const std::uint32_t duration = duration_ms == 0
                                       ? natural_duration(distance, rng)
                                       : std::clamp(duration_ms, 80u, 2000u);
    if (distance < 1.0) {
        return {Node{0, target.first, target.second}};
    }
    const double nx = -dy / distance;
    const double ny = dx / distance;
    const double over = std::clamp(distance * profile.overshoot_pct / 100.0, 0.0, 160.0);
    std::pair<double, double> aim{static_cast<double>(target.first), static_cast<double>(target.second)};
    if (over > 0.5) {
        aim.first = std::clamp(static_cast<double>(target.first) + dx / distance * over, 0.0,
                               static_cast<double>(screen.width - 1));
        aim.second = std::clamp(static_cast<double>(target.second) + dy / distance * over, 0.0,
                                static_cast<double>(screen.height - 1));
    }
    const double limit =
        std::clamp(distance * 0.13, 4.0, 110.0) * std::clamp(profile.curve, 0.0, 2.0);
    const double bend = (rng.unit() * 2.0 - 1.0) * limit;
    const double a1 = 0.20 + rng.unit() * 0.18;
    const double a2 = 0.62 + rng.unit() * 0.22;
    const double bend2 = bend * 0.55 + (rng.unit() * 2.0 - 1.0) * limit * 0.20;
    const double c1x = static_cast<double>(start.first) + dx * a1 + nx * bend;
    const double c1y = static_cast<double>(start.second) + dy * a1 + ny * bend;
    const double c2x = static_cast<double>(start.first) + dx * a2 + nx * bend2;
    const double c2y = static_cast<double>(start.second) + dy * a2 + ny * bend2;
    const double tau = 6.283185307179586476925286766559;
    const double phase = rng.unit() * tau;
    const double frequency = 1.1 + rng.unit() * 1.2;
    const double amplitude = std::clamp(profile.jitter_px, 0.0, 24.0);
    const std::uint32_t steps = std::clamp<std::uint32_t>(duration / 8, 10, 180);
    std::vector<Node> nodes;
    std::pair<std::int32_t, std::int32_t> last = start;
    for (std::uint32_t step = 1; step <= steps; ++step) {
        const double p = static_cast<double>(step) / static_cast<double>(steps);
        const double t = ease_apply(profile.ease, p, profile.bezier);
        const double jitter =
            amplitude * std::sin(3.14159265358979323846 * p) * std::sin(phase + p * tau * frequency);
        std::int32_t px = 0;
        std::int32_t py = 0;
        if (step == steps) {
            px = static_cast<std::int32_t>(std::llround(aim.first));
            py = static_cast<std::int32_t>(std::llround(aim.second));
        } else {
            px = static_cast<std::int32_t>(
                std::llround(bezier(static_cast<double>(start.first), c1x, c2x, aim.first, t) + nx * jitter));
            py = static_cast<std::int32_t>(
                std::llround(bezier(static_cast<double>(start.second), c1y, c2y, aim.second, t) + ny * jitter));
        }
        px = std::clamp(px, 0, screen.width - 1);
        py = std::clamp(py, 0, screen.height - 1);
        const std::int32_t gap = std::abs(px - last.first) + std::abs(py - last.second);
        if (gap >= std::max(profile.min_step_px, 1) || step == steps) {
            Node node;
            node.at_ms = static_cast<std::uint64_t>(duration) * step / steps;
            node.x = px;
            node.y = py;
            nodes.push_back(node);
            last = {px, py};
        }
    }
    if (over > 0.5) {
        Node node;
        node.at_ms = (nodes.empty() ? 0 : nodes.back().at_ms) + 90;
        node.x = target.first;
        node.y = target.second;
        nodes.push_back(node);
    }
    if (profile.settle_ms > 0) {
        Node node;
        node.at_ms = (nodes.empty() ? 0 : nodes.back().at_ms) + profile.settle_ms;
        node.x = target.first;
        node.y = target.second;
        nodes.push_back(node);
    }
    return nodes;
}

// Picks the node that should be active at `elapsed_ms`, never moving backwards.
inline std::size_t current_node(const std::vector<Node>& nodes, std::size_t next,
                                std::uint64_t elapsed_ms) {
    std::size_t found = 0;
    while (found < nodes.size() && nodes[found].at_ms <= elapsed_ms) {
        ++found;
    }
    std::size_t index = std::max(found == 0 ? 0 : found - 1, next);
    if (!nodes.empty()) {
        index = std::min(index, nodes.size() - 1);
    }
    return index;
}

// ---------------------------------------------------------------------------
// Transport seam
// ---------------------------------------------------------------------------

// The three calls the protocol needs. A transport owns framing at the OS level
// only: 64-byte reports, report id in byte 0.
class Transport {
public:
    virtual ~Transport() = default;
    // Feature 14 Get_Report; false when the device did not answer.
    virtual bool feature(std::array<std::uint8_t, REPORT_SIZE>& report) = 0;
    // OUT report id 12.
    virtual void write(const std::array<std::uint8_t, REPORT_SIZE>& report) = 0;
    // IN report id 13; false on timeout. `timeout_ms` is a hint, not a promise.
    virtual bool read(std::array<std::uint8_t, REPORT_SIZE>& report, int timeout_ms) = 0;
    virtual std::string serial() const { return std::string(); }
    virtual std::string product() const { return std::string(); }
};

// In-memory device that speaks the same wire protocol as the firmware, plus
// scripted faults for tests: `inject` puts a raw IN frame in front of the next
// real reply, `set_silent` stops answering entirely.
class FakeTransport final : public Transport {
public:
    FakeTransport() {
        build_ = 0x0DB00001u;
        uptime_ms_ = 1000;
    }

    bool feature(std::array<std::uint8_t, REPORT_SIZE>& report) override {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!feature_present_) {
            return false;
        }
        report = feature_report();
        return true;
    }

    void write(const std::array<std::uint8_t, REPORT_SIZE>& report) override {
        Packet packet;
        try {
            packet = Packet::decode(report, OUT_ID);
        } catch (const Error&) {
            std::lock_guard<std::mutex> lock(mutex_);
            ++malformed_writes_;
            return;
        }
        std::lock_guard<std::mutex> lock(mutex_);
        writes_.push_back(packet);
        handle(packet);
    }

    bool read(std::array<std::uint8_t, REPORT_SIZE>& report, int timeout_ms) override {
        if (timeout_ms <= 0) {
            return false;
        }
        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (!replies_.empty()) {
                report = replies_.front();
                replies_.pop_front();
                return true;
            }
        }
        // Behave like a polled device rather than a hot spin.
        std::this_thread::sleep_for(std::chrono::microseconds(300));
        return false;
    }

    std::string serial() const override { return "MOCK"; }
    std::string product() const override { return "GDBOX mock"; }

    // --- inspection -------------------------------------------------------
    std::vector<Packet> writes() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return writes_;
    }

    std::vector<std::array<std::uint8_t, REPORT_SIZE>> raw_writes() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return raw_writes_;
    }

    int count_opcode(std::uint8_t opcode) const {
        std::lock_guard<std::mutex> lock(mutex_);
        int count = 0;
        for (const Packet& packet : writes_) {
            if (packet.opcode == opcode) {
                ++count;
            }
        }
        return count;
    }

    int malformed_writes() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return malformed_writes_;
    }

    std::uint32_t session() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return session_;
    }

    std::vector<std::uint32_t> sessions() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return sessions_;
    }

    InputState held() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return state_;
    }

    bool closed() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return closed_;
    }

    bool bootloader_requested() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return bootloader_;
    }

    std::pair<std::int32_t, std::int32_t> last_move() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return last_move_;
    }

    std::pair<std::int32_t, std::int32_t> move_total() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return move_total_;
    }

    // --- knobs ------------------------------------------------------------
    void set_silent(bool silent) {
        std::lock_guard<std::mutex> lock(mutex_);
        silent_ = silent;
    }

    void set_feature_present(bool present) {
        std::lock_guard<std::mutex> lock(mutex_);
        feature_present_ = present;
    }

    void set_lease_ms(std::uint16_t lease_ms) {
        std::lock_guard<std::mutex> lock(mutex_);
        lease_ms_ = lease_ms;
    }

    void set_build(std::uint32_t build) {
        std::lock_guard<std::mutex> lock(mutex_);
        build_ = build;
    }

    // Feature 14 board byte: 2 is the XIAO, 1 a Pico, 3 an AVR build.
    void set_board(std::uint8_t board) {
        std::lock_guard<std::mutex> lock(mutex_);
        board_ = board;
    }

    void set_uptime_ms(std::uint32_t uptime_ms) {
        std::lock_guard<std::mutex> lock(mutex_);
        uptime_ms_ = uptime_ms;
    }

    // A raw IN frame, delivered before whatever the model replies next.
    void inject(const std::array<std::uint8_t, REPORT_SIZE>& frame) {
        std::lock_guard<std::mutex> lock(mutex_);
        replies_.push_back(frame);
    }

    void inject_reply(std::uint8_t opcode, std::uint32_t session, std::uint32_t sequence,
                      const std::vector<std::uint8_t>& payload) {
        std::lock_guard<std::mutex> lock(mutex_);
        push_reply(opcode, session, sequence, payload);
    }

    // An EVENT frame for `session`, the way the firmware reports a failed
    // command (result byte = the fault code).
    void inject_event(std::uint32_t session, std::uint8_t code) {
        std::lock_guard<std::mutex> lock(mutex_);
        push_reply(EVENT, session, 0, {code});
    }

private:
    std::array<std::uint8_t, REPORT_SIZE> feature_report() const {
        std::array<std::uint8_t, REPORT_SIZE> report{};
        report[0] = FEATURE_ID;
        std::memcpy(&report[1], "FMT3", 4);
        report[5] = WIRE_VERSION;
        report[6] = board_;  // XIAO RP2040
        report[7] = 3;
        report[8] = 2;
        report[9] = 0;
        write_u16(&report[10], lease_ms_);
        report[14] = 1;
        write_u32(&report[15], 15);
        for (std::uint16_t usage = 0; usage < 224; ++usage) {
            if (key_valid(7, usage)) {
                report[19 + usage / 8] |= static_cast<std::uint8_t>(1u << (usage % 8));
            }
        }
        report[47] = 0xFF;
        report[48] = 0x7F;
        const std::uint8_t device_id[8] = {0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33};
        std::memcpy(&report[49], device_id, 8);
        write_u32(&report[57], build_);
        return report;
    }

    void push_reply(std::uint8_t opcode, std::uint32_t session, std::uint32_t sequence,
                    const std::vector<std::uint8_t>& payload) {
        Packet packet;
        // EVENT is its own opcode (7F), not a control reply with bit 7 set.
        packet.opcode = opcode == EVENT ? EVENT : static_cast<std::uint8_t>(opcode | 0x80);
        packet.session = session;
        packet.sequence = sequence;
        packet.payload = payload;
        replies_.push_back(packet.encode(IN_ID));
    }

    void handle(const Packet& packet) {
        raw_writes_.push_back(packet.encode(OUT_ID));
        if (silent_) {
            return;
        }
        switch (packet.opcode) {
            case OPEN: {
                if (packet.session == 0 || packet.sequence != 1) {
                    push_reply(OPEN, packet.session, packet.sequence, {1});
                    return;
                }
                if (session_ == packet.session || sessions_.size() >= SESSION_SLOTS) {
                    push_reply(OPEN, packet.session, packet.sequence, {2});
                    return;
                }
                session_ = packet.session;
                sessions_.push_back(packet.session);
                closed_ = false;
                push_reply(OPEN, packet.session, packet.sequence, {0});
                return;
            }
            case CLOSE: {
                sessions_.erase(std::remove(sessions_.begin(), sessions_.end(), packet.session),
                                sessions_.end());
                if (session_ == packet.session) {
                    session_ = 0;
                    closed_ = true;
                }
                state_ = InputState{};
                push_reply(CLOSE, packet.session, packet.sequence, {0});
                return;
            }
            case HEARTBEAT:
                return;
            default:
                break;
        }
        if (session_ != packet.session) {
            // Unknown session: the firmware drops input silently.
            return;
        }
        switch (packet.opcode) {
            case KEY: {
                if (packet.payload.size() != 5) {
                    push_reply(EVENT, packet.session, packet.sequence, {1});
                    return;
                }
                const std::uint16_t page = read_u16(&packet.payload[0]);
                const std::uint16_t usage = read_u16(&packet.payload[2]);
                try {
                    state_.set(page, usage, packet.payload[4] != 0);
                } catch (const Error&) {
                    push_reply(EVENT, packet.session, packet.sequence, {3});
                }
                return;
            }
            case SNAPSHOT: {
                try {
                    state_ = InputState::decode(packet.payload.data(), packet.payload.size());
                } catch (const Error&) {
                    push_reply(EVENT, packet.session, packet.sequence, {3});
                }
                return;
            }
            case MOUSE_MOVE: {
                if (packet.payload.size() == 4) {
                    last_move_ = {static_cast<std::int16_t>(read_u16(&packet.payload[0])),
                                  static_cast<std::int16_t>(read_u16(&packet.payload[2]))};
                    move_total_.first += last_move_.first;
                    move_total_.second += last_move_.second;
                }
                return;
            }
            case WHEEL:
                return;
            case MOUSE_BUTTONS:
                if (packet.payload.size() == 1) {
                    state_.buttons = packet.payload[0];
                }
                return;
            case RELEASE_ALL:
                state_ = InputState{};
                push_reply(RELEASE_ALL, packet.session, packet.sequence, {0});
                return;
            case BOOTLOADER:
                bootloader_ = true;
                push_reply(BOOTLOADER, packet.session, packet.sequence, {0});
                return;
            case BARRIER:
                push_reply(BARRIER, packet.session, packet.sequence, {0});
                return;
            case STATUS: {
                std::vector<std::uint8_t> body(STATUS_BYTES, 0);
                write_u32(&body[1], packet.sequence);
                write_u32(&body[5], packet.sequence);
                write_u16(&body[10], lease_ms_);
                const auto snapshot = state_.encode();
                std::copy(snapshot.begin(), snapshot.end(), body.begin() + 20);
                push_reply(STATUS, packet.session, packet.sequence, body);
                return;
            }
            case STATUS_EXT: {
                std::vector<std::uint8_t> body(STATUS_EXT_BYTES, 0);
                write_u32(&body[1], uptime_ms_);
                body[10] = static_cast<std::uint8_t>(sessions_.size());
                write_u32(&body[12], 0xFFFFFFFFu);
                body[16] = 3;
                write_u16(&body[17], 1);
                write_u32(&body[19], build_);
                push_reply(STATUS_EXT, packet.session, packet.sequence, body);
                return;
            }
            case SESSION_LIST: {
                std::vector<std::uint8_t> body(SESSION_LIST_BYTES, 0);
                for (std::size_t index = 0; index < sessions_.size() && index < SESSION_SLOTS; ++index) {
                    const std::size_t base = index * SESSION_ENTRY;
                    write_u16(&body[base], static_cast<std::uint16_t>(sessions_[index] & 0xFFFF));
                    write_u16(&body[base + 2], lease_ms_);
                    body[base + 4] = static_cast<std::uint8_t>(
                        (state_.modifiers != 0 ? 1 : 0) + (state_.consumer != 0 ? 1 : 0));
                }
                push_reply(SESSION_LIST, packet.session, packet.sequence, body);
                return;
            }
            default:
                push_reply(EVENT, packet.session, packet.sequence, {3});
                return;
        }
    }

    mutable std::mutex mutex_;
    std::deque<std::array<std::uint8_t, REPORT_SIZE>> replies_;
    std::vector<Packet> writes_;
    std::vector<std::array<std::uint8_t, REPORT_SIZE>> raw_writes_;
    InputState state_;
    std::uint32_t session_ = 0;
    std::vector<std::uint32_t> sessions_;
    std::pair<std::int32_t, std::int32_t> last_move_{0, 0};
    std::pair<std::int32_t, std::int32_t> move_total_{0, 0};
    std::uint32_t build_ = 0;
    std::uint32_t uptime_ms_ = 0;
    std::uint16_t lease_ms_ = static_cast<std::uint16_t>(DEVICE_LEASE_MS);
    std::uint8_t board_ = 2;
    bool closed_ = false;
    bool silent_ = false;
    bool feature_present_ = true;
    bool bootloader_ = false;
    int malformed_writes_ = 0;
};

// ---------------------------------------------------------------------------
// Windows transport
// ---------------------------------------------------------------------------

struct DeviceEntry {
    std::string path;
    std::string serial;
    std::string product;
    std::string manufacturer;
    std::uint16_t vid = 0;
    std::uint16_t pid = 0;
};

namespace detail {

inline std::string json_escape(const std::string& value) {
    std::string out;
    out.reserve(value.size() + 8);
    for (const char raw : value) {
        const unsigned char c = static_cast<unsigned char>(raw);
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20) {
                    char buffer[8] = {};
                    std::snprintf(buffer, sizeof(buffer), "\\u%04x", c);
                    out += buffer;
                } else {
                    out += raw;
                }
        }
    }
    return out;
}

inline std::string json_string(const std::string& value) { return "\"" + json_escape(value) + "\""; }

template <typename Container>
inline std::string json_bytes(const Container& bytes) {
    std::string out = "[";
    for (std::size_t index = 0; index < bytes.size(); ++index) {
        if (index != 0) {
            out += ",";
        }
        out += std::to_string(static_cast<unsigned>(bytes[index]));
    }
    out += "]";
    return out;
}

inline std::uint32_t new_session_id() {
    static std::atomic<std::uint32_t> next{1};
    const auto now = std::chrono::steady_clock::now().time_since_epoch();
    const std::uint64_t nanos =
        static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now).count());
    const std::uint32_t mixed = static_cast<std::uint32_t>(nanos) ^
                                static_cast<std::uint32_t>(nanos >> 32) ^
                                next.fetch_add(0x9E3779B9u);
    return mixed == 0 ? 1u : mixed;
}

}  // namespace detail

#if defined(_WIN32)

namespace detail {

// hid.dll, loaded at runtime so this header needs no import library.
struct HidApi {
    BOOLEAN(WINAPI* get_feature)(HANDLE, PVOID, ULONG) = nullptr;
    BOOLEAN(WINAPI* get_attributes)(HANDLE, PVOID) = nullptr;
    BOOLEAN(WINAPI* get_product_string)(HANDLE, PWSTR, ULONG) = nullptr;
    BOOLEAN(WINAPI* get_serial_number_string)(HANDLE, PWSTR, ULONG) = nullptr;
    BOOLEAN(WINAPI* get_manufacturer_string)(HANDLE, PWSTR, ULONG) = nullptr;
    BOOLEAN(WINAPI* get_preparsed_data)(HANDLE, PVOID*) = nullptr;
    BOOLEAN(WINAPI* free_preparsed_data)(PVOID) = nullptr;
    // NTSTATUS HidP_GetCaps(PHIDP_PREPARSED_DATA, PHIDP_CAPS): 0 is success.
    long(WINAPI* get_caps)(PVOID, PVOID) = nullptr;

    bool loaded = false;
};

struct HidAttributes {
    unsigned long size;
    unsigned short vendor;
    unsigned short product;
    unsigned short version;
};

// GetProcAddress returns FARPROC; going through void* keeps -Wcast-function-type
// quiet without pretending the two signatures match.
template <typename Fn>
inline Fn symbol(HMODULE module, const char* name) {
    return reinterpret_cast<Fn>(reinterpret_cast<void*>(GetProcAddress(module, name)));
}

inline HidApi& hid_api() {
    static HidApi api = [] {
        HidApi loaded;
        HMODULE module = LoadLibraryA("hid.dll");
        if (module == nullptr) {
            return loaded;
        }
        loaded.get_feature = symbol<decltype(loaded.get_feature)>(module, "HidD_GetFeature");
        loaded.get_attributes = symbol<decltype(loaded.get_attributes)>(module, "HidD_GetAttributes");
        loaded.get_product_string =
            symbol<decltype(loaded.get_product_string)>(module, "HidD_GetProductString");
        loaded.get_serial_number_string = symbol<decltype(loaded.get_serial_number_string)>(
            module, "HidD_GetSerialNumberString");
        loaded.get_manufacturer_string = symbol<decltype(loaded.get_manufacturer_string)>(
            module, "HidD_GetManufacturerString");
        loaded.get_preparsed_data =
            symbol<decltype(loaded.get_preparsed_data)>(module, "HidD_GetPreparsedData");
        loaded.free_preparsed_data =
            symbol<decltype(loaded.free_preparsed_data)>(module, "HidD_FreePreparsedData");
        loaded.get_caps = symbol<decltype(loaded.get_caps)>(module, "HidP_GetCaps");
        loaded.loaded = loaded.get_feature != nullptr && loaded.get_attributes != nullptr;
        return loaded;
    }();
    return api;
}

using CmListSizeFn = unsigned long(WINAPI*)(unsigned long*, const GUID*, const wchar_t*, unsigned long);
using CmListFn = unsigned long(WINAPI*)(const GUID*, const wchar_t*, wchar_t*, unsigned long,
                                        unsigned long);

// The HID class interface GUID; vendor collection paths end with it.
inline const GUID& hid_interface_guid() {
    static const GUID guid = {0x4D1E55B2, 0xF16F, 0x11CF, {0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30}};
    return guid;
}

inline std::string narrow(const std::wstring& value) {
    if (value.empty()) {
        return std::string();
    }
    const int needed = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()),
                                           nullptr, 0, nullptr, nullptr);
    if (needed <= 0) {
        return std::string();
    }
    std::string out(static_cast<std::size_t>(needed), '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), out.data(), needed,
                        nullptr, nullptr);
    return out;
}

inline std::string lower_ascii(std::string value) {
    for (char& c : value) {
        if (c >= 'A' && c <= 'Z') {
            c = static_cast<char>(c - 'A' + 'a');
        }
    }
    return value;
}

inline std::wstring widen(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }
    const int needed = MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()),
                                           nullptr, 0);
    if (needed <= 0) {
        return std::wstring();
    }
    std::wstring out(static_cast<std::size_t>(needed), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), out.data(), needed);
    return out;
}

// One argument as CreateProcessW wants it: always quoted, quotes escaped.
inline std::wstring quote_argument(const std::string& value) {
    std::wstring out = L"\"";
    for (const wchar_t c : widen(value)) {
        if (c == L'"') {
            out += L'\\';
        }
        out += c;
    }
    out += L'"';
    return out;
}

// Paths come straight from cfgmgr32; no SetupAPI walk is needed.
inline std::vector<std::string> hid_interface_paths() {
    std::vector<std::string> paths;
    HMODULE module = LoadLibraryA("cfgmgr32.dll");
    if (module == nullptr) {
        return paths;
    }
    const auto list_size = symbol<CmListSizeFn>(module, "CM_Get_Device_Interface_List_SizeW");
    const auto list = symbol<CmListFn>(module, "CM_Get_Device_Interface_ListW");
    if (list_size == nullptr || list == nullptr) {
        return paths;
    }
    constexpr unsigned long present_only = 0x00000001;  // CM_GET_DEVICE_INTERFACE_LIST_PRESENT
    unsigned long characters = 0;
    if (list_size(&characters, &hid_interface_guid(), nullptr, present_only) != 0 || characters == 0) {
        return paths;
    }
    std::vector<wchar_t> buffer(characters, L'\0');
    if (list(&hid_interface_guid(), nullptr, buffer.data(), characters, present_only) != 0) {
        return paths;
    }
    for (std::size_t index = 0; index < buffer.size() && buffer[index] != L'\0';) {
        std::wstring entry(&buffer[index]);
        index += entry.size() + 1;
        paths.push_back(narrow(entry));
    }
    return paths;
}

// boot9.py's transform: HID\VID_..&MI_01\7&..&0&0000 becomes the interface path.
inline std::string path_from_instance_id(const std::string& instance_id) {
    std::vector<std::string> parts;
    std::string current;
    for (const char c : instance_id) {
        if (c == '\\') {
            parts.push_back(current);
            current.clear();
        } else {
            current += c;
        }
    }
    parts.push_back(current);
    if (parts.size() != 3 || lower_ascii(parts[0]) != "hid") {
        return std::string();
    }
    return "\\\\?\\hid#" + lower_ascii(parts[1]) + "#" + lower_ascii(parts[2]) + "#{4d1e55b2-f16f-11cf-88cb-001111000030}";
}

inline std::string wide_string(HANDLE handle, BOOLEAN(WINAPI* fn)(HANDLE, PWSTR, ULONG)) {
    if (fn == nullptr) {
        return std::string();
    }
    wchar_t buffer[256] = {};
    if (!fn(handle, buffer, sizeof(buffer))) {
        return std::string();
    }
    return narrow(buffer);
}

inline bool looks_like_gdbox(const std::string& path) {
    const std::string lower = lower_ascii(path);
    return lower.find("vid_03f0") != std::string::npos && lower.find("pid_0024") != std::string::npos;
}

// The composite device also publishes its boot keyboard (MI_00 Col02); only the
// vendor collection (usage page FF60, usage 0061) speaks this protocol.
inline bool is_vendor_collection(HANDLE handle) {
    const HidApi& api = hid_api();
    if (api.get_preparsed_data == nullptr || api.get_caps == nullptr ||
        api.free_preparsed_data == nullptr) {
        return true;  // cannot inspect: the caller filtered by path
    }
    PVOID preparsed = nullptr;
    if (api.get_preparsed_data(handle, &preparsed) == 0 || preparsed == nullptr) {
        return false;
    }
    // HIDP_CAPS begins with Usage, UsagePage, then the three report lengths.
    unsigned short caps[32] = {};
    const long status = api.get_caps(preparsed, caps);
    api.free_preparsed_data(preparsed);
    // HIDP_STATUS_SUCCESS is 0x00110000, not zero.
    if (static_cast<unsigned long>(status) != 0x00110000UL) {
        return false;
    }
    return caps[0] == 0x0061 && caps[1] == 0xFF60;
}

}  // namespace detail

// Real hardware over CreateFileW/WriteFile/ReadFile with overlapped I/O so a
// read can time out without cancelling the handle.
class WindowsTransport final : public Transport {
public:
    static std::shared_ptr<WindowsTransport> open(const std::string& selector,
                                                 std::uint32_t timeout_ms) {
        const std::string path = resolve(selector);
        HANDLE handle = CreateFileW(detail::widen(path).c_str(),
                                    GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                    nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
        if (handle == INVALID_HANDLE_VALUE) {
            throw Error(ErrorCode::Disconnected, "cannot open the vendor collection (" +
                                                     std::to_string(GetLastError()) + ")");
        }
        auto transport = std::shared_ptr<WindowsTransport>(new WindowsTransport(handle, timeout_ms));
        transport->read_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        transport->write_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (transport->read_event_ == nullptr || transport->write_event_ == nullptr) {
            throw Error(ErrorCode::Io, "cannot create overlapped events");
        }
        transport->serial_ = detail::wide_string(handle, detail::hid_api().get_serial_number_string);
        transport->product_ = detail::wide_string(handle, detail::hid_api().get_product_string);
        return transport;
    }

    ~WindowsTransport() override {
        if (handle_ != INVALID_HANDLE_VALUE) {
            CloseHandle(handle_);
            handle_ = INVALID_HANDLE_VALUE;
        }
        if (read_event_ != nullptr) {
            CloseHandle(read_event_);
        }
        if (write_event_ != nullptr) {
            CloseHandle(write_event_);
        }
    }

    WindowsTransport(const WindowsTransport&) = delete;
    WindowsTransport& operator=(const WindowsTransport&) = delete;

    bool feature(std::array<std::uint8_t, REPORT_SIZE>& report) override {
        const auto& api = detail::hid_api();
        if (!api.loaded) {
            throw Error(ErrorCode::Unsupported, "hid.dll is not available");
        }
        report.fill(0);
        // Get_Report selects the report by the id in byte 0; report 0 does not
        // exist, and asking for it fails with ERROR_INVALID_PARAMETER.
        report[0] = FEATURE_ID;
        return api.get_feature(handle_, report.data(), static_cast<ULONG>(REPORT_SIZE)) != 0;
    }

    void write(const std::array<std::uint8_t, REPORT_SIZE>& report) override {
        OVERLAPPED overlap{};
        overlap.hEvent = write_event_;
        ResetEvent(write_event_);
        DWORD written = 0;
        const BOOL started = WriteFile(handle_, report.data(), static_cast<DWORD>(REPORT_SIZE), &written,
                                       &overlap);
        if (started == 0) {
            const DWORD error = GetLastError();
            if (error != ERROR_IO_PENDING) {
                throw Error(ErrorCode::Disconnected, "WriteFile failed (" + std::to_string(error) + ")");
            }
            if (WaitForSingleObject(write_event_, timeout_ms_) != WAIT_OBJECT_0) {
                CancelIoEx(handle_, &overlap);
                WaitForSingleObject(write_event_, 100);
                throw Error(ErrorCode::Timeout, "the device did not accept the report");
            }
            if (GetOverlappedResult(handle_, &overlap, &written, FALSE) == 0) {
                throw Error(ErrorCode::Disconnected,
                            "WriteFile did not complete (" + std::to_string(GetLastError()) + ")");
            }
        }
        if (written != REPORT_SIZE) {
            throw Error(ErrorCode::Io, "the device accepted a short OUT report");
        }
    }

    bool read(std::array<std::uint8_t, REPORT_SIZE>& report, int timeout_ms) override {
        if (timeout_ms <= 0) {
            return false;
        }
        OVERLAPPED overlap{};
        overlap.hEvent = read_event_;
        ResetEvent(read_event_);
        DWORD got = 0;
        const BOOL started = ReadFile(handle_, report.data(), static_cast<DWORD>(REPORT_SIZE), &got,
                                      &overlap);
        if (started == 0) {
            const DWORD error = GetLastError();
            if (error == ERROR_OPERATION_ABORTED) {
                return false;
            }
            if (error != ERROR_IO_PENDING) {
                if (error == ERROR_DEVICE_NOT_CONNECTED || error == ERROR_HANDLE_EOF) {
                    throw Error(ErrorCode::Disconnected, "USB reconnected or reset");
                }
                throw Error(ErrorCode::Io, "ReadFile failed (" + std::to_string(error) + ")");
            }
            if (WaitForSingleObject(read_event_, static_cast<DWORD>(timeout_ms)) != WAIT_OBJECT_0) {
                CancelIoEx(handle_, &overlap);
                WaitForSingleObject(read_event_, 100);
                return false;
            }
            if (GetOverlappedResult(handle_, &overlap, &got, FALSE) == 0) {
                const DWORD failure = GetLastError();
                if (failure == ERROR_OPERATION_ABORTED) {
                    return false;
                }
                if (failure == ERROR_DEVICE_NOT_CONNECTED) {
                    throw Error(ErrorCode::Disconnected, "USB reconnected or reset");
                }
                throw Error(ErrorCode::Io, "read did not complete (" + std::to_string(failure) + ")");
            }
        }
        if (got == 0) {
            return false;
        }
        for (std::size_t index = got; index < REPORT_SIZE; ++index) {
            report[index] = 0;
        }
        return true;
    }

    std::string serial() const override { return serial_; }
    std::string product() const override { return product_; }

    // Path, serial, strings, ids for one device; the caller filters.
    static std::vector<DeviceEntry> entries() {
        std::vector<DeviceEntry> list;
        for (const std::string& path : detail::hid_interface_paths()) {
            if (!detail::looks_like_gdbox(path)) {
                continue;
            }
            HANDLE handle = CreateFileW(detail::widen(path).c_str(),
                                        GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                        nullptr, OPEN_EXISTING, 0, nullptr);
            if (handle == INVALID_HANDLE_VALUE) {
                continue;
            }
            if (!detail::is_vendor_collection(handle)) {
                CloseHandle(handle);
                continue;
            }
            DeviceEntry entry;
            entry.path = path;
            detail::HidAttributes attributes{};
            attributes.size = sizeof(attributes);
            if (detail::hid_api().loaded &&
                detail::hid_api().get_attributes(handle, &attributes) != 0) {
                entry.vid = attributes.vendor;
                entry.pid = attributes.product;
            }
            entry.serial = detail::wide_string(handle, detail::hid_api().get_serial_number_string);
            entry.product = detail::wide_string(handle, detail::hid_api().get_product_string);
            entry.manufacturer =
                detail::wide_string(handle, detail::hid_api().get_manufacturer_string);
            CloseHandle(handle);
            list.push_back(entry);
        }
        return list;
    }

private:
    WindowsTransport(HANDLE handle, std::uint32_t timeout_ms)
        : handle_(handle), timeout_ms_(timeout_ms == 0 ? 1000 : timeout_ms) {}

    static std::string resolve(const std::string& selector) {
        if (!selector.empty()) {
            if (selector.rfind("\\\\?\\", 0) == 0 || selector.rfind("\\\\.\\", 0) == 0) {
                return selector;
            }
            if (selector.rfind("hid#", 0) == 0) {
                return "\\\\?\\" + selector;
            }
            if (selector.rfind("HID\\", 0) == 0) {
                const std::string path = detail::path_from_instance_id(selector);
                if (!path.empty()) {
                    return path;
                }
            }
        }
        const std::vector<DeviceEntry> list = entries();
        if (selector.empty()) {
            if (list.empty()) {
                throw Error(ErrorCode::Disconnected, "no GDBOX device found");
            }
            return list.front().path;
        }
        for (const DeviceEntry& entry : list) {
            if (entry.serial == selector || entry.path == selector) {
                return entry.path;
            }
        }
        for (const DeviceEntry& entry : list) {
            if (!entry.serial.empty() && entry.serial.find(selector) != std::string::npos) {
                return entry.path;
            }
        }
        throw Error(ErrorCode::Disconnected, "no GDBOX device matches \"" + selector + "\"");
    }

    HANDLE handle_ = INVALID_HANDLE_VALUE;
    HANDLE read_event_ = nullptr;
    HANDLE write_event_ = nullptr;
    std::uint32_t timeout_ms_ = 1000;
    std::string serial_;
    std::string product_;
};

inline std::vector<DeviceEntry> device_list() { return WindowsTransport::entries(); }

#else  // !_WIN32

// Off Windows there is no vendor-HID transport here; a caller supplies its own.
class WindowsTransport final : public Transport {
public:
    bool feature(std::array<std::uint8_t, REPORT_SIZE>&) override {
        throw Error(ErrorCode::Unsupported, "the native transport needs Windows HID");
    }
    void write(const std::array<std::uint8_t, REPORT_SIZE>&) override {
        throw Error(ErrorCode::Unsupported, "the native transport needs Windows HID");
    }
    bool read(std::array<std::uint8_t, REPORT_SIZE>&, int) override {
        throw Error(ErrorCode::Unsupported, "the native transport needs Windows HID");
    }
};

inline std::vector<DeviceEntry> device_list() { return {}; }

#endif  // _WIN32

inline std::string device_list_json() {
    std::string out = "[";
    bool first = true;
    for (const DeviceEntry& entry : device_list()) {
        if (!first) {
            out += ",";
        }
        first = false;
        out += "{\"path\":" + detail::json_string(entry.path) +
               ",\"serial\":" + detail::json_string(entry.serial) +
               ",\"product\":" + detail::json_string(entry.product) +
               ",\"manufacturer\":" + detail::json_string(entry.manufacturer) +
               ",\"vid\":" + std::to_string(entry.vid) + ",\"pid\":" + std::to_string(entry.pid) + "}";
    }
    out += "]";
    return out;
}

// ---------------------------------------------------------------------------
// Screen and cursor
// ---------------------------------------------------------------------------

// Primary-monitor rectangle and the cursor inside it. Absolute coordinates are
// relative to that rectangle; the firmware never sees screen coordinates.
inline bool screen_and_cursor(Screen& screen, std::int32_t& x, std::int32_t& y) {
#if defined(_WIN32)
    POINT cursor{0, 0};
    if (GetPhysicalCursorPos(&cursor) == 0) {
        return false;
    }
    const HMONITOR monitor = MonitorFromPoint(POINT{0, 0}, MONITOR_DEFAULTTOPRIMARY);
    MONITORINFO info{};
    info.cbSize = sizeof(info);
    if (GetMonitorInfoW(monitor, &info) == 0) {
        return false;
    }
    screen.width = info.rcMonitor.right - info.rcMonitor.left;
    screen.height = info.rcMonitor.bottom - info.rcMonitor.top;
    x = cursor.x - info.rcMonitor.left;
    y = cursor.y - info.rcMonitor.top;
    return true;
#else
    (void)screen;
    (void)x;
    (void)y;
    return false;
#endif
}

// Makes the process DPI aware so physical pixels are what the sensor sees.
inline void enable_dpi_awareness() {
#if defined(_WIN32)
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
#endif
}

namespace detail {

// One vendor-HID session: Feature 14 handshake, OPEN, sequenced commands and
// reply matching. Nothing above this layer touches sequence numbers.
struct Session {
    std::shared_ptr<Transport> transport;
    std::uint32_t id = 1;
    std::uint32_t sequence = 0;
    Capabilities capabilities;
    std::chrono::milliseconds timeout{1000};
    // When this session last sent something the device counts as a renewal.
    // STATUS, BARRIER, STATUS_EXT and SESSION_LIST do not renew, so a host
    // polling any of them still has to heartbeat (firmware/protocol-v3.md,
    // 完成與租約).
    std::chrono::steady_clock::time_point last_renew = std::chrono::steady_clock::now();

    static Session open(std::shared_ptr<Transport> transport, std::uint32_t timeout_ms,
                        std::uint32_t id) {
        std::array<std::uint8_t, REPORT_SIZE> report{};
        if (!transport->feature(report)) {
            throw Error(ErrorCode::Disconnected, "no Feature 14 reply");
        }
        Session session;
        session.transport = std::move(transport);
        session.id = id == 0 ? 1 : id;
        session.capabilities = Capabilities::parse(report);
        session.timeout = std::chrono::milliseconds(timeout_ms < 100 ? 100 : timeout_ms);
        session.command(OPEN, {}, true);
        return session;
    }

    std::chrono::milliseconds renew_age() const {
        return std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() -
                                                                    last_renew);
    }

    static bool expects_reply(std::uint8_t opcode) {
        switch (opcode) {
            case OPEN:
            case CLOSE:
            case STATUS:
            case BARRIER:
            case RELEASE_ALL:
            case STATUS_EXT:
            case SESSION_LIST:
            case BOOTLOADER: return true;
            default: return false;
        }
    }

    // Sequence exhaustion is a new connection, not a wrap (protocol-v3.md).
    // The firmware writes lease_at only for OPEN, HEARTBEAT, RELEASE_ALL,
    // BOOTLOADER and valid input (firmware/engine/src/engine.rs).
    static bool renews_lease(std::uint8_t opcode) {
        switch (opcode) {
            case OPEN:
            case HEARTBEAT:
            case RELEASE_ALL:
            case BOOTLOADER:
            case KEY:
            case SNAPSHOT:
            case MOUSE_MOVE:
            case MOUSE_BUTTONS:
            case WHEEL: return true;
            default: return false;
        }
    }

    std::optional<Packet> command(std::uint8_t opcode, const std::vector<std::uint8_t>& payload,
                                  bool wait_for_reply) {
        if (sequence == 0xFFFFFFFFu) {
            throw Error(ErrorCode::Invalid, "sequence exhausted; reopen the session");
        }
        const std::uint32_t used = ++sequence;
        Packet packet;
        packet.opcode = opcode;
        packet.session = id;
        packet.sequence = used;
        packet.payload = payload;
        const auto report = packet.encode(OUT_ID);
        if (renews_lease(opcode)) {
            last_renew = std::chrono::steady_clock::now();
        }
        if (log::enabled(log::kTrace)) {
            log::write(log::kTrace, "out opcode=" + std::to_string(opcode) +
                                        " session=" + std::to_string(id) +
                                        " sequence=" + std::to_string(used) +
                                        " payload=" + trace_payload(payload));
        }
        transport->write(report);
        if (!wait_for_reply) {
            return std::nullopt;
        }
        const auto deadline = std::chrono::steady_clock::now() + timeout;
        for (;;) {
            const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
                deadline - std::chrono::steady_clock::now());
            if (remaining.count() <= 0) {
                throw Error(ErrorCode::Timeout, "no reply to opcode " + std::to_string(opcode) +
                                                    " within " + std::to_string(timeout.count()) +
                                                    " ms");
            }
            std::array<std::uint8_t, REPORT_SIZE> reply{};
            if (!transport->read(reply, static_cast<int>(remaining.count()))) {
                continue;
            }
            // Anything that is not an IN report cannot be a reply at all.
            if (reply[0] != IN_ID) {
                continue;
            }
            const Packet frame = Packet::decode(reply, IN_ID);
            if (frame.session != id) {
                if (log::enabled(log::kTrace)) {
                    log::write(log::kTrace, "skip opcode=" + std::to_string(frame.opcode) +
                                                " session=" + std::to_string(frame.session) +
                                                " sequence=" + std::to_string(frame.sequence) +
                                                " (not ours)");
                }
                continue;  // another owner's reply: never ours, never fatal
            }
            if (log::enabled(log::kTrace)) {
                log::write(log::kTrace, "in opcode=" + std::to_string(frame.opcode) +
                                            " session=" + std::to_string(frame.session) +
                                            " sequence=" + std::to_string(frame.sequence) +
                                            " payload=" + trace_payload(frame.payload));
            }
            if (frame.opcode == EVENT) {
                const std::uint8_t code = frame.payload.empty() ? 0 : frame.payload[0];
                throw error_from_result(code);
            }
            if (frame.opcode != static_cast<std::uint8_t>(opcode | 0x80) || frame.sequence != used) {
                continue;
            }
            if (frame.payload.empty()) {
                throw Error(ErrorCode::Framing, "empty control reply");
            }
            // SESSION_LIST has no result byte; the id lives in byte 0.
            if (opcode != SESSION_LIST && frame.payload[0] != 0) {
                throw error_from_result(frame.payload[0]);
            }
            return frame;
        }
    }
};

}  // namespace detail

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

struct DeviceInfo {
    std::string serial;
    std::string product;
    std::uint8_t board = 0;
    std::string firmware;
    std::uint16_t lease_ms = 0;
    std::uint8_t poll_ms = 0;
    std::uint32_t capabilities = 0;
    std::array<std::uint8_t, 8> device_id{};
    std::uint32_t build = 0;
};

enum class ConnectionState : std::int32_t {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
};

struct DeviceOptions {
    bool auto_reconnect = true;
    std::uint32_t timeout_ms = 1000;
    bool heartbeat = true;
};

class Mouse;
class Keyboard;
class Scheduler;

namespace detail {

// Wake-up handle shared with the heartbeat thread so the device object itself
// can be destroyed without the thread holding a reference to it.
struct Beat {
    std::mutex mutex;
    std::condition_variable cv;
    bool stop = false;
};

struct DeviceImpl {
    std::mutex mutex;
    std::unique_ptr<Session> session;
    std::function<std::shared_ptr<Transport>()> factory;
    DeviceOptions options;
    DeviceInfo info;
    std::atomic<bool> auto_reconnect{true};
    std::atomic<bool> closed{false};
    std::atomic<std::uint8_t> buttons{0};
    std::mutex cursor_mutex;
    std::pair<std::int32_t, std::int32_t> cursor{960, 540};
    std::mutex events_mutex;
    std::function<void(std::int32_t, std::int32_t, const std::string&)> events;
    std::shared_ptr<Beat> beat = std::make_shared<Beat>();
    std::thread heartbeat;
    bool mock = false;

    ~DeviceImpl() {
        {
            std::lock_guard<std::mutex> hold(beat->mutex);
            beat->stop = true;
        }
        beat->cv.notify_all();
        if (heartbeat.joinable()) {
            heartbeat.join();
        }
    }
};

inline bool recoverable(ErrorCode code) {
    return code == ErrorCode::Disconnected || code == ErrorCode::Io || code == ErrorCode::Lease;
}

// The event handler is called with no lock held: a handler that calls back into
// the device would otherwise deadlock on the state mutex.
inline void notify_events(const std::shared_ptr<DeviceImpl>& impl, ConnectionState state,
                          std::int32_t code, const std::string& message) {
    std::function<void(std::int32_t, std::int32_t, const std::string&)> handler;
    {
        std::lock_guard<std::mutex> hold(impl->events_mutex);
        handler = impl->events;
    }
    if (!handler) {
        return;
    }
    try {
        handler(static_cast<std::int32_t>(state), code, message);
    } catch (...) {
    }
}

inline void run_heartbeat(std::weak_ptr<DeviceImpl> weak, std::shared_ptr<Beat> beat) {
    for (;;) {
        {
            std::unique_lock<std::mutex> lock(beat->mutex);
            beat->cv.wait_for(lock, std::chrono::milliseconds(50), [&beat] { return beat->stop; });
            if (beat->stop) {
                return;
            }
        }
        std::shared_ptr<DeviceImpl> impl = weak.lock();
        if (!impl) {
            return;
        }
        std::unique_lock<std::mutex> lock(impl->mutex);
        if (impl->closed.load()) {
            return;
        }
        // Ask the session, not the caller: STATUS, BARRIER, STATUS_EXT and
        // SESSION_LIST do not renew the device-side lease, so a STATUS_EXT
        // polling loop must still heartbeat.
        if (!impl->session || impl->session->renew_age() < std::chrono::milliseconds(HEARTBEAT_MS)) {
            continue;
        }
        ErrorCode failure = ErrorCode::Ok;
        std::string message;
        try {
            impl->session->command(HEARTBEAT, {}, false);
        } catch (const Error& error) {
            failure = error.code();
            message = error.what();
        }
        if (failure == ErrorCode::Ok) {
            continue;
        }
        const bool reconnect = impl->auto_reconnect.load() && recoverable(failure);
        lock.unlock();
        notify_events(impl, ConnectionState::Error, static_cast<std::int32_t>(failure), message);
        if (!reconnect) {
            continue;  // the lease is the backstop; keep reporting
        }
        std::lock_guard<std::mutex> relock(impl->mutex);
        if (impl->closed.load()) {
            return;
        }
        try {
            impl->session.reset();
            impl->session = std::make_unique<Session>(
                Session::open(impl->factory(), impl->options.timeout_ms, new_session_id()));
        } catch (const Error&) {
            impl->closed.store(true);
            return;
        }
    }
}

}  // namespace detail

class Device {
public:
    Device() = default;

    static Device open(const std::string& selector = std::string(), std::uint32_t timeout_ms = 1000,
                       bool auto_reconnect = true, bool heartbeat = true) {
        DeviceOptions options;
        options.auto_reconnect = auto_reconnect;
        options.timeout_ms = timeout_ms;
        options.heartbeat = heartbeat;
        std::shared_ptr<Transport> transport;
#if defined(_WIN32)
        transport = WindowsTransport::open(selector, timeout_ms);
#else
        (void)selector;
        throw Error(ErrorCode::Unsupported, "the native transport needs Windows HID");
#endif
        auto device = Device::open_transport(std::move(transport), options);
        device.impl_->mock = false;
        return device;
    }

    // A device backed by the in-memory fake; no hardware, no DLL.
    static Device open_mock(std::uint32_t timeout_ms = 1000, bool auto_reconnect = true,
                            bool heartbeat = true) {
        return open_fake(std::make_shared<FakeTransport>(), timeout_ms, auto_reconnect, heartbeat);
    }

    // Same, with the fake kept alive so a test can inspect what was sent.
    static Device open_fake(std::shared_ptr<FakeTransport> fake, std::uint32_t timeout_ms = 1000,
                            bool auto_reconnect = true, bool heartbeat = true) {
        DeviceOptions options;
        options.auto_reconnect = auto_reconnect;
        options.timeout_ms = timeout_ms;
        options.heartbeat = heartbeat;
        std::shared_ptr<Transport> transport = fake;
        auto device = Device::open_transport(std::move(transport), options);
        device.impl_->mock = true;
        return device;
    }

    static Device open_transport(std::shared_ptr<Transport> transport, const DeviceOptions& options) {
        auto impl = std::make_shared<detail::DeviceImpl>();
        impl->options = options;
        impl->auto_reconnect.store(options.auto_reconnect);
        const std::string serial = transport->serial();
        const std::string product = transport->product();
        std::shared_ptr<Transport> kept = transport;
        impl->factory = [kept]() -> std::shared_ptr<Transport> { return kept; };
        impl->session = std::make_unique<detail::Session>(
            detail::Session::open(std::move(transport), options.timeout_ms, detail::new_session_id()));
        const Capabilities& caps = impl->session->capabilities;
        impl->info.serial = serial;
        impl->info.product = product;
        impl->info.board = caps.board;
        impl->info.firmware = caps.firmware;
        impl->info.lease_ms = caps.lease_ms;
        impl->info.poll_ms = caps.poll_ms;
        impl->info.capabilities = caps.flags;
        impl->info.device_id = caps.device_id;
        impl->info.build = caps.build;
        Device device;
        device.impl_ = impl;
        if (options.heartbeat) {
            std::weak_ptr<detail::DeviceImpl> weak = impl;
            impl->heartbeat = std::thread(detail::run_heartbeat, weak, impl->beat);
        }
        log::write(log::kInfo, "connected serial=" + impl->info.serial + " firmware=" +
                                   impl->info.firmware + " board=" +
                                   std::to_string(static_cast<unsigned>(impl->info.board)));
        return device;
    }

    static std::string list_json() { return device_list_json(); }

    bool valid() const { return impl_ != nullptr; }
    bool is_mock() const { return impl_ && impl_->mock; }

    ConnectionState state() const {
        if (!impl_ || impl_->closed.load()) {
            return ConnectionState::Disconnected;
        }
        std::lock_guard<std::mutex> lock(impl_->mutex);
        return impl_->session ? ConnectionState::Connected : ConnectionState::Disconnected;
    }

    std::uint32_t session_id() const {
        std::lock_guard<std::mutex> lock(impl_->mutex);
        return impl_->session ? impl_->session->id : 0;
    }

    const DeviceInfo& info() const { return impl_->info; }

    std::string info_json() const {
        const DeviceInfo& info = impl_->info;
        return "{\"serial\":" + detail::json_string(info.serial) +
               ",\"product\":" + detail::json_string(info.product) +
               ",\"board\":" + std::to_string(info.board) +
               ",\"firmware\":" + detail::json_string(info.firmware) +
               ",\"leaseMs\":" + std::to_string(info.lease_ms) +
               ",\"pollMs\":" + std::to_string(info.poll_ms) +
               ",\"capabilities\":" + std::to_string(info.capabilities) +
               ",\"deviceId\":" + detail::json_bytes(info.device_id) +
               ",\"build\":" + std::to_string(info.build) + "}";
    }

    Status status() const {
        return with_session([](detail::Session& session) {
            const auto reply = session.command(STATUS, {}, true);
            if (!reply) {
                throw Error(ErrorCode::Framing, "STATUS had no reply");
            }
            return Status::parse(reply->payload);
        });
    }

    StatusExt status_ext() const {
        return with_session([](detail::Session& session) {
            const auto reply = session.command(STATUS_EXT, {}, true);
            if (!reply) {
                throw Error(ErrorCode::Framing, "STATUS_EXT had no reply");
            }
            return StatusExt::parse(reply->payload);
        });
    }

    std::vector<SessionInfo> sessions() const {
        return with_session([](detail::Session& session) {
            const auto reply = session.command(SESSION_LIST, {}, true);
            if (!reply) {
                throw Error(ErrorCode::Framing, "SESSION_LIST had no reply");
            }
            return parse_session_list(reply->payload);
        });
    }

    std::string status_json() const {
        const Status status = this->status();
        return "{\"received\":" + std::to_string(status.received) +
               ",\"completed\":" + std::to_string(status.completed) +
               ",\"pending\":" + std::to_string(status.pending) +
               ",\"leaseMs\":" + std::to_string(status.lease_ms) +
               ",\"rx\":" + std::to_string(status.rx) + ",\"tx\":" + std::to_string(status.tx) +
               ",\"hid\":" + std::to_string(status.hid) +
               ",\"failsafe\":" + std::to_string(status.failsafe) +
               ",\"state\":{\"modifiers\":" + std::to_string(status.state.modifiers) +
               ",\"keys\":" + detail::json_bytes(status.state.keys) +
               ",\"consumer\":" + std::to_string(status.state.consumer) +
               ",\"buttons\":" + std::to_string(status.state.buttons) + "}}";
    }

    std::string status_ext_json() const {
        const StatusExt ext = status_ext();
        return "{\"uptimeMs\":" + std::to_string(ext.uptime_ms) +
               ",\"lateMaxUs\":" + std::to_string(ext.late_max_us) +
               ",\"outDepth\":" + std::to_string(ext.out_depth) +
               ",\"rxDepth\":" + std::to_string(ext.rx_depth) +
               ",\"txDepth\":" + std::to_string(ext.tx_depth) +
               ",\"sessions\":" + std::to_string(ext.sessions) +
               ",\"lastFault\":" + std::to_string(ext.last_fault) +
               ",\"lastFaultAgeMs\":" +
               (ext.has_fault ? std::to_string(ext.last_fault_age_ms) : std::string("null")) +
               ",\"resetReason\":" + std::to_string(ext.reset_reason) +
               ",\"bootCount\":" + std::to_string(ext.boot_count) +
               ",\"build\":" + std::to_string(ext.build) + "}";
    }

    std::string sessions_json() const {
        const std::vector<SessionInfo> list = sessions();
        std::string out = "[";
        for (std::size_t index = 0; index < list.size(); ++index) {
            if (index != 0) {
                out += ",";
            }
            out += "{\"id\":" + std::to_string(list[index].id) +
                   ",\"leaseMs\":" + std::to_string(list[index].lease_ms) +
                   ",\"held\":" + std::to_string(list[index].held) +
                   ",\"flags\":" + std::to_string(list[index].flags) + "}";
        }
        out += "]";
        return out;
    }

    void barrier() const { with_session([](detail::Session& session) { session.command(BARRIER, {}, true); }); }
    void release_all() const {
        impl_->buttons.store(0);
        with_session([](detail::Session& session) { session.command(RELEASE_ALL, {}, true); });
    }
    void bootloader() const {
        with_session([](detail::Session& session) { session.command(BOOTLOADER, {}, true); });
    }

    // Raw escape hatch: the caller already speaks the protocol and gets the
    // reply packet. SDK bookkeeping (held buttons, mock cursor) is not updated.
    std::optional<Packet> command(std::uint8_t opcode, const std::vector<std::uint8_t>& payload,
                                  bool wait_for_reply) const {
        return with_session([&](detail::Session& session) {
            return session.command(opcode, payload, wait_for_reply);
        });
    }

    void set_auto_reconnect(bool enabled) { impl_->auto_reconnect.store(enabled); }

    void set_event_handler(std::function<void(std::int32_t, std::int32_t, const std::string&)> handler) {
        std::lock_guard<std::mutex> lock(impl_->events_mutex);
        impl_->events = std::move(handler);
    }

    void close() const {
        impl_->closed.store(true);
        {
            std::lock_guard<std::mutex> hold(impl_->beat->mutex);
            impl_->beat->stop = true;
        }
        impl_->beat->cv.notify_all();
        if (impl_->heartbeat.joinable()) {
            impl_->heartbeat.join();
        }
        impl_->buttons.store(0);
        std::unique_ptr<detail::Session> session;
        {
            std::lock_guard<std::mutex> lock(impl_->mutex);
            session = std::move(impl_->session);
        }
        log::write(log::kInfo, "closed serial=" + impl_->info.serial);
        detail::notify_events(impl_, ConnectionState::Disconnected,
                          static_cast<std::int32_t>(ErrorCode::Ok), std::string());
        if (session) {
            try {
                session->command(CLOSE, {}, true);
            } catch (const Error&) {
                // The lease is the backstop when CLOSE does not arrive.
            }
        }
    }

    // Re-opens the transport and a fresh session; held keys are not restored.
    void reconnect() const {
        std::lock_guard<std::mutex> lock(impl_->mutex);
        reopen();
    }

    Mouse mouse() const;
    Keyboard keyboard() const;
    Scheduler scheduler() const;

    // --- used by Mouse/Keyboard/Scheduler ---------------------------------
    void send_input(std::uint8_t opcode, const std::vector<std::uint8_t>& payload) const {
        with_session([&](detail::Session& session) { session.command(opcode, payload, false); });
    }

    void send_mouse_move(std::int32_t dx, std::int32_t dy) const {
        if (dx < -1024 || dx > 1024 || dy < -1024 || dy > 1024) {
            throw Error(ErrorCode::Invalid, "mouse moves are +/-1024 per axis");
        }
        const auto ux = static_cast<std::uint16_t>(static_cast<std::int16_t>(dx));
        const auto uy = static_cast<std::uint16_t>(static_cast<std::int16_t>(dy));
        std::vector<std::uint8_t> payload(4);
        write_u16(&payload[0], ux);
        write_u16(&payload[2], uy);
        send_input(MOUSE_MOVE, payload);
        if (impl_->mock) {
            std::lock_guard<std::mutex> lock(impl_->cursor_mutex);
            impl_->cursor.first = std::clamp(impl_->cursor.first + dx, 0, 1919);
            impl_->cursor.second = std::clamp(impl_->cursor.second + dy, 0, 1079);
        }
    }

    std::uint8_t buttons() const { return impl_->buttons.load(); }

    void set_buttons(std::uint8_t mask) const {
        if ((mask & 0xE0u) != 0) {
            throw Error(ErrorCode::Invalid, "buttons are bits 0-4");
        }
        impl_->buttons.store(mask);
    }

    // Real cursor on hardware, simulated pointer for the mock/fake device.
    void cursor_state(Screen& screen, std::int32_t& x, std::int32_t& y) const {
        if (impl_->mock) {
            screen.width = 1920;
            screen.height = 1080;
            std::lock_guard<std::mutex> lock(impl_->cursor_mutex);
            x = impl_->cursor.first;
            y = impl_->cursor.second;
            return;
        }
        if (!screen_and_cursor(screen, x, y)) {
            throw Error(ErrorCode::Unsupported,
                        "absolute mouse coordinates need Windows cursor APIs");
        }
    }

private:
    template <typename Fn>
    auto with_session(Fn&& operation) const -> decltype(operation(std::declval<detail::Session&>())) {
        if (!impl_ || impl_->closed.load()) {
            throw Error(ErrorCode::Disconnected, "device is closed");
        }
        std::unique_lock<std::mutex> lock(impl_->mutex);
        if (impl_->session) {
            try {
                return operation(*impl_->session);
            } catch (const Error& error) {
                log::write(log::kWarn, "command failed (" +
                                           std::to_string(static_cast<std::int32_t>(error.code())) +
                                           "): " + error.what());
                if (!detail::recoverable(error.code()) || !impl_->auto_reconnect.load()) {
                    detail::notify_events(impl_, ConnectionState::Error,
                                          static_cast<std::int32_t>(error.code()), error.what());
                    throw;
                }
            }
        }
        try {
            reopen();
        } catch (const Error& error) {
            impl_->closed.store(true);
            detail::notify_events(impl_, ConnectionState::Error,
                                  static_cast<std::int32_t>(error.code()), error.what());
            throw;
        }
        try {
            return operation(*impl_->session);
        } catch (const Error& error) {
            detail::notify_events(impl_, ConnectionState::Error,
                                  static_cast<std::int32_t>(error.code()), error.what());
            throw;
        }
    }

    // Called with the mutex held.
    void reopen() const {
        impl_->session.reset();
        impl_->session = std::make_unique<detail::Session>(
            detail::Session::open(impl_->factory(), impl_->options.timeout_ms, detail::new_session_id()));
        log::write(log::kInfo, "reconnected serial=" + impl_->info.serial);
        detail::notify_events(impl_, ConnectionState::Connected,
                          static_cast<std::int32_t>(ErrorCode::Ok), std::string());
    }

    std::shared_ptr<detail::DeviceImpl> impl_;
};

// ---------------------------------------------------------------------------
// Motion: options, the shared cell and the background Movement handle
// ---------------------------------------------------------------------------

// Port of sdk/rust/src/motion.rs. A movement runs on its own thread; the handle
// the caller keeps can cancel it, retarget it or wait for it.
struct MotionOptions {
    // 0 = pick a natural duration from the distance.
    std::uint32_t duration_ms = 0;
    HumanProfile profile;

    static MotionOptions linear(std::uint32_t duration_ms) {
        MotionOptions options;
        options.duration_ms = duration_ms;
        options.profile = HumanProfile::linear();
        return options;
    }
};

// What the worker and the handle share: the cancel flag, the pending retarget
// and the result. No lock is held while the pointer moves.
class MovementCell {
public:
    void stop() { cancel_.store(true); cv_.notify_all(); }

    void retarget(std::int32_t x, std::int32_t y) {
        std::lock_guard<std::mutex> lock(mutex_);
        target_ = std::make_pair(x, y);
        has_target_ = true;
    }

    bool cancelled() const { return cancel_.load(); }

    // A retarget is consumed once: the worker rebuilds the path from wherever
    // the pointer is now to the newest target.
    bool take_target(std::pair<std::int32_t, std::int32_t>& out) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!has_target_) {
            return false;
        }
        out = target_;
        has_target_ = false;
        return true;
    }

    bool finished() const { return finished_.load(); }

    void complete(std::exception_ptr error) {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            error_ = std::move(error);
        }
        finished_.store(true);
        cv_.notify_all();
    }

    std::exception_ptr error() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return error_;
    }

    bool wait_finished(std::uint32_t timeout_ms) {
        std::unique_lock<std::mutex> lock(mutex_);
        return cv_.wait_for(lock, std::chrono::milliseconds(timeout_ms),
                            [this] { return finished_.load(); });
    }

private:
    std::atomic<bool> cancel_{false};
    std::atomic<bool> finished_{false};
    mutable std::mutex mutex_;
    std::condition_variable cv_;
    std::pair<std::int32_t, std::int32_t> target_{0, 0};
    bool has_target_ = false;
    std::exception_ptr error_;
};

inline std::pair<std::int32_t, std::int32_t> clamp_to_screen(std::int32_t x, std::int32_t y,
                                                             Screen screen) {
    return {std::clamp(x, 0, screen.width > 0 ? screen.width - 1 : 0),
            std::clamp(y, 0, screen.height > 0 ? screen.height - 1 : 0)};
}

// Relative move, paced over the chunks the HID report can carry. Cheap enough
// to stay inline; a background handle simply wraps it.
inline void run_move_by(const Device& device, std::int32_t dx, std::int32_t dy,
                        const MotionOptions& options, MovementCell& shared) {
    const auto chunks = split_delta(dx, dy);
    if (chunks.empty()) {
        return;
    }
    const std::uint32_t pause_ms =
        options.duration_ms == 0 ? 0 : options.duration_ms / static_cast<std::uint32_t>(chunks.size());
    for (std::size_t index = 0; index < chunks.size(); ++index) {
        if (shared.cancelled()) {
            throw Error(ErrorCode::Cancelled, "movement cancelled");
        }
        device.send_mouse_move(chunks[index].first, chunks[index].second);
        device.barrier();
        if (index + 1 != chunks.size() && pause_ms > 0) {
            std::this_thread::sleep_for(std::chrono::milliseconds(pause_ms));
        }
    }
}

// Absolute move: read the real cursor, plan a trajectory, feed relative deltas
// until the pointer arrives. A retarget replans from the current position, so
// the same loop serves both.
inline void run_move_to(const Device& device, std::int32_t x, std::int32_t y,
                        const MotionOptions& options, MovementCell& shared) {
    enable_dpi_awareness();
    Screen screen;
    std::int32_t cursor_x = 0;
    std::int32_t cursor_y = 0;
    device.cursor_state(screen, cursor_x, cursor_y);
    if (cursor_x < 0 || cursor_y < 0 || cursor_x >= screen.width || cursor_y >= screen.height) {
        throw Error(ErrorCode::Unsupported,
                    "absolute coordinates are relative to the primary monitor (" +
                        std::to_string(screen.width) + "x" + std::to_string(screen.height) +
                        " at 0,0) and the cursor is at (" + std::to_string(cursor_x) + ", " +
                        std::to_string(cursor_y) + "), so the move would start off that screen");
    }
    auto target = clamp_to_screen(x, y, screen);
    SplitMix64 rng = SplitMix64::from_entropy();
    auto nodes = trajectory_with({cursor_x, cursor_y}, target, screen, options.duration_ms,
                                 options.profile, rng);
    auto started = std::chrono::steady_clock::now();
    auto deadline = started + std::chrono::milliseconds((nodes.empty() ? 0 : nodes.back().at_ms) + 500);
    std::size_t index = 0;
    for (;;) {
        if (shared.cancelled()) {
            throw Error(ErrorCode::Cancelled, "movement cancelled");
        }
        std::pair<std::int32_t, std::int32_t> next;
        if (shared.take_target(next)) {
            target = clamp_to_screen(next.first, next.second, screen);
            Screen current_screen;
            std::int32_t now_x = 0;
            std::int32_t now_y = 0;
            device.cursor_state(current_screen, now_x, now_y);
            if (current_screen.width != screen.width || current_screen.height != screen.height) {
                throw Error(ErrorCode::Unsupported,
                            "monitor layout changed during the movement (" +
                                std::to_string(screen.width) + "x" + std::to_string(screen.height) +
                                " -> " + std::to_string(current_screen.width) + "x" +
                                std::to_string(current_screen.height) + ")");
            }
            nodes = trajectory_with({now_x, now_y}, target, screen, options.duration_ms,
                                    options.profile, rng);
            started = std::chrono::steady_clock::now();
            index = 0;
            deadline = started +
                       std::chrono::milliseconds((nodes.empty() ? 0 : nodes.back().at_ms) + 500);
        }
        if (index >= nodes.size()) {
            return;
        }
        const Node node = nodes[index];
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now() - started);
        if (elapsed.count() < static_cast<std::int64_t>(node.at_ms)) {
            const auto wait = std::min<std::int64_t>(
                2, static_cast<std::int64_t>(node.at_ms) - elapsed.count());
            std::this_thread::sleep_for(std::chrono::milliseconds(std::max<std::int64_t>(1, wait)));
            continue;
        }
        Screen ignored_screen;
        std::int32_t now_x = 0;
        std::int32_t now_y = 0;
        device.cursor_state(ignored_screen, now_x, now_y);
        const std::int32_t dx = node.x - now_x;
        const std::int32_t dy = node.y - now_y;
        if (dx == 0 && dy == 0) {
            if (index + 1 == nodes.size()) {
                device.barrier();
                return;
            }
            ++index;
            continue;
        }
        for (const auto& part : split_delta(dx, dy)) {
            device.send_mouse_move(part.first, part.second);
        }
        device.barrier();
        if (std::chrono::steady_clock::now() > deadline) {
            throw Error(ErrorCode::Timeout, "pointer did not reach the target");
        }
    }
}

// Visits each point in order; every segment uses the same options.
inline void run_path(const Device& device,
                     const std::vector<std::pair<std::int32_t, std::int32_t>>& points,
                     const MotionOptions& options, MovementCell& shared) {
    for (const auto& point : points) {
        run_move_to(device, point.first, point.second, options, shared);
    }
}

// A running movement. Dropping it cancels the worker and joins it, so no thread
// outlives the handle and the device is never touched after the handle dies.
class Movement {
public:
    Movement() = default;

    Movement(Movement&& other) noexcept = default;

    Movement& operator=(Movement&& other) noexcept {
        if (this != &other) {
            stop_and_join();
            cell_ = std::move(other.cell_);
            thread_ = std::move(other.thread_);
        }
        return *this;
    }

    Movement(const Movement&) = delete;
    Movement& operator=(const Movement&) = delete;

    ~Movement() { stop_and_join(); }

    bool valid() const { return cell_ != nullptr; }
    bool is_finished() const { return !cell_ || cell_->finished(); }

    void cancel() {
        if (cell_) {
            cell_->stop();
        }
    }

    // Takes effect at the next node of a running absolute move; a relative move
    // only checks the cancel flag, so it is accepted and ignored there.
    void retarget(std::int32_t x, std::int32_t y) {
        if (!cell_) {
            throw Error(ErrorCode::Invalid, "movement is not running");
        }
        cell_->retarget(x, y);
    }

    // Waits at most `timeout_ms`. On timeout the worker keeps running and the
    // handle stays usable (cancel it or wait again); when the worker finished,
    // its error — including `Cancelled` after a cancel — is rethrown here.
    void wait(std::uint32_t timeout_ms = 10000) {
        if (!cell_) {
            return;
        }
        if (!cell_->wait_finished(timeout_ms)) {
            throw Error(ErrorCode::Timeout, "movement did not finish in time");
        }
        join();
        const std::exception_ptr failure = cell_->error();
        if (failure) {
            std::rethrow_exception(failure);
        }
    }

    // One worker thread per movement; the body runs it and reports its outcome.
    static Movement spawn(Device device,
                          std::function<void(const Device&, MovementCell&)> body) {
        auto cell = std::make_shared<MovementCell>();
        std::shared_ptr<MovementCell> kept = cell;
        std::thread worker([kept, device = std::move(device), body = std::move(body)]() mutable {
            std::exception_ptr failure;
            try {
                body(device, *kept);
            } catch (...) {
                failure = std::current_exception();
            }
            kept->complete(std::move(failure));
        });
        return Movement(std::move(cell), std::move(worker));
    }

private:
    Movement(std::shared_ptr<MovementCell> cell, std::thread worker)
        : cell_(std::move(cell)), thread_(std::move(worker)) {}

    void join() {
        if (thread_.joinable()) {
            thread_.join();
        }
    }

    void stop_and_join() {
        if (cell_) {
            cell_->stop();
        }
        join();
        cell_.reset();
    }

    std::shared_ptr<MovementCell> cell_;
    std::thread thread_;
};

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

// HID button bits: left = bit 0 ... forward = bit 4.
enum class Button : std::uint8_t {
    Left = 1,
    Right = 2,
    Middle = 3,
    Back = 4,
    Forward = 5,
};

inline std::uint8_t button_bit(Button button) {
    return static_cast<std::uint8_t>(1u << (static_cast<std::uint8_t>(button) - 1));
}

class Mouse {
public:
    explicit Mouse(Device device) : device_(std::move(device)) {}

    // One relative move with no barrier and no interpolation: the deltas go out
    // as they are, split only at the +/-1024 HID limit.
    void move_raw(std::int32_t dx, std::int32_t dy) const {
        for (const auto& part : split_delta(dx, dy)) {
            device_.send_mouse_move(part.first, part.second);
        }
    }

    void move_by(std::int32_t dx, std::int32_t dy, std::uint32_t duration_ms = 0) const {
        MovementCell shared;
        run_move_by(device_, dx, dy, motion_options(duration_ms, nullptr), shared);
    }

    // Absolute move: the host reads the real cursor, generates a trajectory and
    // feeds the device relative deltas until the pointer arrives.
    void move_to(std::int32_t x, std::int32_t y, std::uint32_t duration_ms = 0,
                 const HumanProfile* profile = nullptr) const {
        MovementCell shared;
        run_move_to(device_, x, y, motion_options(duration_ms, profile), shared);
    }

    void move_path(const std::vector<std::pair<std::int32_t, std::int32_t>>& points,
                   std::uint32_t duration_ms = 0, const HumanProfile* profile = nullptr) const {
        MovementCell shared;
        run_path(device_, points, motion_options(duration_ms, profile), shared);
    }

    void move_by(std::int32_t dx, std::int32_t dy, const MotionOptions& options) const {
        MovementCell shared;
        run_move_by(device_, dx, dy, options, shared);
    }

    void move_to(std::int32_t x, std::int32_t y, const MotionOptions& options) const {
        MovementCell shared;
        run_move_to(device_, x, y, options, shared);
    }

    void move_path(const std::vector<std::pair<std::int32_t, std::int32_t>>& points,
                   const MotionOptions& options) const {
        MovementCell shared;
        run_path(device_, points, options, shared);
    }

    // The same three moves on a worker thread: cancel, retarget or wait on the
    // returned handle while the pointer is still travelling.
    Movement start_move_to(std::int32_t x, std::int32_t y,
                           MotionOptions options = MotionOptions()) const {
        return Movement::spawn(device_, [x, y, options](const Device& device, MovementCell& shared) {
            run_move_to(device, x, y, options, shared);
        });
    }

    Movement start_move_by(std::int32_t dx, std::int32_t dy,
                           MotionOptions options = MotionOptions()) const {
        return Movement::spawn(device_,
                               [dx, dy, options](const Device& device, MovementCell& shared) {
                                   run_move_by(device, dx, dy, options, shared);
                               });
    }

    Movement start_move_path(const std::vector<std::pair<std::int32_t, std::int32_t>>& points,
                             MotionOptions options = MotionOptions()) const {
        return Movement::spawn(device_, [points, options](const Device& device, MovementCell& shared) {
            run_path(device, points, options, shared);
        });
    }

    void button(Button button, bool down) const {
        // Each session owns its own button mask; the firmware unions the owners.
        const std::uint8_t current = device_.buttons();
        const std::uint8_t bit = button_bit(button);
        const std::uint8_t next =
            down ? static_cast<std::uint8_t>(current | bit) : static_cast<std::uint8_t>(current & ~bit);
        device_.set_buttons(next);
        device_.send_input(MOUSE_BUTTONS, {next});
    }

    void click(Button button = Button::Left, std::uint8_t count = 1) const {
        if (count == 0) {
            throw Error(ErrorCode::Invalid, "click count must be at least 1");
        }
        for (std::uint8_t index = 0; index < count; ++index) {
            this->button(button, true);
            std::this_thread::sleep_for(std::chrono::milliseconds(25));
            this->button(button, false);
            std::this_thread::sleep_for(std::chrono::milliseconds(35));
        }
    }

    void double_click(Button button = Button::Left) const { click(button, 2); }

    void wheel(std::int32_t vertical, std::int32_t horizontal = 0) const {
        if (vertical < -1024 || vertical > 1024 || horizontal < -1024 || horizontal > 1024) {
            throw Error(ErrorCode::Invalid, "wheel is limited to +/-1024 per axis");
        }
        const auto uv = static_cast<std::uint16_t>(static_cast<std::int16_t>(vertical));
        const auto uh = static_cast<std::uint16_t>(static_cast<std::int16_t>(horizontal));
        std::vector<std::uint8_t> payload(4);
        write_u16(&payload[0], uv);
        write_u16(&payload[2], uh);
        device_.send_input(WHEEL, payload);
    }

    // Presses the button, moves, then releases: the button is never left down,
    // even when the move fails (the Rust SDK does the same with a guard).
    void drag(Button button, std::int32_t x, std::int32_t y, std::uint32_t duration_ms = 0,
              const HumanProfile* profile = nullptr) const {
        this->button(button, true);
        try {
            move_to(x, y, duration_ms, profile);
        } catch (...) {
            try {
                device_.release_all();
            } catch (...) {
            }
            throw;
        }
        device_.release_all();
    }

    void release_all() const { device_.release_all(); }

private:
    static MotionOptions motion_options(std::uint32_t duration_ms, const HumanProfile* profile) {
        MotionOptions options;
        options.duration_ms = duration_ms;
        if (profile != nullptr) {
            options.profile = *profile;
        }
        return options;
    }

    Device device_;
};

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

struct KeyStep {
    std::uint16_t page = 7;
    std::uint16_t usage = 0;
    std::uint32_t hold_ms = 18;
    std::uint32_t gap_ms = 0;
};

class Keyboard {
public:
    explicit Keyboard(Device device) : device_(std::move(device)) {}

    void key_down(std::uint16_t page, std::uint16_t usage) const { emit(page, usage, true); }
    void key_up(std::uint16_t page, std::uint16_t usage) const { emit(page, usage, false); }
    void press(std::uint16_t page, std::uint16_t usage) const { long_press(page, usage, 18); }

    void long_press(std::uint16_t page, std::uint16_t usage, std::uint32_t hold_ms) const {
        key_down(page, usage);
        std::this_thread::sleep_for(std::chrono::milliseconds(std::max<std::uint32_t>(hold_ms, 1)));
        key_up(page, usage);
    }

    // Presses every usage, holds, then releases in reverse order.
    void chord(const std::vector<std::uint16_t>& usages, std::uint32_t hold_ms = 60) const {
        if (usages.empty()) {
            throw Error(ErrorCode::Invalid, "a chord needs at least one usage");
        }
        for (const std::uint16_t usage : usages) {
            key_down(7, usage);
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(std::max<std::uint32_t>(hold_ms, 1)));
        for (auto it = usages.rbegin(); it != usages.rend(); ++it) {
            key_up(7, *it);
        }
    }

    void sequence(const std::vector<KeyStep>& steps) const {
        for (const KeyStep& step : steps) {
            long_press(step.page, step.usage, step.hold_ms);
            if (step.gap_ms > 0) {
                std::this_thread::sleep_for(std::chrono::milliseconds(step.gap_ms));
            }
        }
    }

    void type_text(const std::string& text, std::uint32_t interval_ms = 45,
                   std::uint32_t hold_ms = 18, std::uint32_t jitter_ms = 30) const {
        SplitMix64 rng = SplitMix64::from_entropy();
        for (const KeyStroke& stroke : strokes(text)) {
            if (stroke.shift) {
                key_down(7, 0xE1);
            }
            key_down(stroke.page, stroke.usage);
            std::this_thread::sleep_for(std::chrono::milliseconds(std::max<std::uint32_t>(hold_ms, 1)));
            key_up(stroke.page, stroke.usage);
            if (stroke.shift) {
                key_up(7, 0xE1);
            }
            const std::uint32_t jitter =
                jitter_ms == 0 ? 0 : static_cast<std::uint32_t>(rng.unit() * jitter_ms);
            std::this_thread::sleep_for(std::chrono::milliseconds(interval_ms + jitter));
        }
    }

    void release_all() const { device_.release_all(); }

private:
    void emit(std::uint16_t page, std::uint16_t usage, bool down) const {
        if (!key_valid(page, usage)) {
            throw Error(ErrorCode::Invalid,
                        "usage " + std::to_string(page) + ":" + std::to_string(usage) +
                            " is not in the key catalog");
        }
        std::vector<std::uint8_t> payload(5);
        write_u16(&payload[0], page);
        write_u16(&payload[2], usage);
        payload[4] = static_cast<std::uint8_t>(down ? 1 : 0);
        device_.send_input(KEY, payload);
    }

    Device device_;
};

inline Mouse Device::mouse() const { return Mouse(*this); }
inline Keyboard Device::keyboard() const { return Keyboard(*this); }

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

// One worker thread; jobs run one at a time in (due time, priority, submission
// order). Callbacks throw `Error` to report a failure, which is collected in
// `take_errors` instead of stopping the queue.
class Scheduler {
public:
    using Job = std::function<void()>;

    explicit Scheduler(Device device) : state_(std::make_shared<State>()), device_(std::move(device)) {
        state_->started = std::chrono::steady_clock::now();
        std::shared_ptr<State> state = state_;
        worker_ = std::thread([state] { run(state); });
    }

    ~Scheduler() { shutdown(); }

    Scheduler(const Scheduler&) = delete;
    Scheduler& operator=(const Scheduler&) = delete;

    std::uint64_t submit(std::uint32_t delay_ms, std::int32_t priority, Job job) {
        Entry entry;
        entry.id = state_->next_id++;
        entry.seq = state_->next_seq++;
        entry.priority = priority;
        entry.due = elapsed() + delay_ms;
        entry.job = std::move(job);
        {
            std::lock_guard<std::mutex> lock(state_->mutex);
            state_->queue.push_back(entry);
        }
        state_->cv.notify_all();
        return entry.id;
    }

    // Repeats until cancelled. The first run happens after `interval_ms`.
    std::uint64_t every(std::uint32_t interval_ms, std::int32_t priority, Job job) {
        if (interval_ms == 0) {
            throw Error(ErrorCode::Invalid, "interval must be positive");
        }
        Entry entry;
        entry.id = state_->next_id++;
        entry.seq = state_->next_seq++;
        entry.priority = priority;
        entry.due = elapsed() + interval_ms;
        entry.every = interval_ms;
        entry.job = std::move(job);
        {
            std::lock_guard<std::mutex> lock(state_->mutex);
            state_->queue.push_back(entry);
        }
        state_->cv.notify_all();
        return entry.id;
    }

    // A cancel that races a due job still wins as long as the callback has not
    // started.
    void cancel(std::uint64_t id) {
        {
            std::lock_guard<std::mutex> lock(state_->mutex);
            state_->cancelled.insert(id);
        }
        state_->cv.notify_all();
    }

    void pause() {
        state_->paused.store(true);
        state_->cv.notify_all();
    }

    void resume() {
        state_->paused.store(false);
        state_->cv.notify_all();
    }

    // Blocks until the queue is empty and no job is running.
    void wait_idle(std::uint32_t timeout_ms = 5000) {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeout_ms);
        std::unique_lock<std::mutex> lock(state_->mutex);
        while (!state_->queue.empty() || state_->running) {
            if (std::chrono::steady_clock::now() >= deadline) {
                throw Error(ErrorCode::Timeout, "scheduler did not become idle");
            }
            state_->cv.wait_for(lock, std::chrono::milliseconds(2));
        }
    }

    std::vector<std::pair<std::uint64_t, Error>> take_errors() {
        std::lock_guard<std::mutex> lock(state_->mutex);
        std::vector<std::pair<std::uint64_t, Error>> errors;
        errors.swap(state_->errors);
        return errors;
    }

    // Stops the worker; pending jobs do not run. Idempotent.
    void shutdown() {
        {
            std::lock_guard<std::mutex> lock(state_->mutex);
            state_->stop = true;
        }
        state_->cv.notify_all();
        if (worker_.joinable()) {
            worker_.join();
        }
    }

    const Device& device() const { return device_; }

private:
    static constexpr std::size_t MAX_ERRORS = 32;

    struct Entry {
        std::uint64_t id = 0;
        std::uint64_t seq = 0;
        std::uint64_t due = 0;
        std::uint64_t every = 0;
        std::int32_t priority = 0;
        Job job;
    };

    struct State {
        std::mutex mutex;
        std::condition_variable cv;
        std::vector<Entry> queue;
        std::set<std::uint64_t> cancelled;
        std::vector<std::pair<std::uint64_t, Error>> errors;
        std::atomic<std::uint64_t> next_id{1};
        std::atomic<std::uint64_t> next_seq{1};
        std::atomic<bool> paused{false};
        bool running = false;
        bool stop = false;
        std::chrono::steady_clock::time_point started{};
    };

    static std::uint64_t elapsed_of(const State& state) {
        return static_cast<std::uint64_t>(
            std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() -
                                                                 state.started)
                .count());
    }

    std::uint64_t elapsed() const { return elapsed_of(*state_); }

    // Earlier due first, then higher priority, then earlier submission.
    static bool before(const Entry& a, const Entry& b) {
        if (a.due != b.due) {
            return a.due < b.due;
        }
        if (a.priority != b.priority) {
            return a.priority > b.priority;
        }
        return a.seq < b.seq;
    }

    static void run(const std::shared_ptr<State>& state) {
        for (;;) {
            Entry job;
            bool have = false;
            {
                std::unique_lock<std::mutex> lock(state->mutex);
                for (;;) {
                    if (state->stop) {
                        return;
                    }
                    const std::uint64_t now = elapsed_of(*state);
                    std::int64_t wait_ms = 20;
                    if (!state->paused.load()) {
                        std::size_t best = state->queue.size();
                        for (std::size_t index = 0; index < state->queue.size(); ++index) {
                            if (state->queue[index].due > now) {
                                continue;
                            }
                            if (best == state->queue.size() ||
                                before(state->queue[index], state->queue[best])) {
                                best = index;
                            }
                        }
                        if (best < state->queue.size()) {
                            job = state->queue[best];
                            state->queue.erase(state->queue.begin() + static_cast<std::ptrdiff_t>(best));
                            if (state->cancelled.erase(job.id) != 0) {
                                continue;  // cancelled before it started
                            }
                            if (job.every > 0) {
                                Entry again = job;
                                std::uint64_t next = again.due + again.every;
                                while (next <= now) {
                                    next += again.every;
                                }
                                again.due = next;
                                state->queue.push_back(again);
                            }
                            state->running = true;
                            have = true;
                            break;
                        }
                        std::uint64_t soonest = UINT64_MAX;
                        for (const Entry& entry : state->queue) {
                            soonest = std::min(soonest, entry.due);
                        }
                        if (soonest != UINT64_MAX) {
                            const std::uint64_t delta = soonest > now ? soonest - now : 0;
                            wait_ms = static_cast<std::int64_t>(std::clamp<std::uint64_t>(delta, 1, 20));
                        }
                    }
                    state->cv.wait_for(lock, std::chrono::milliseconds(wait_ms));
                }
            }
            if (!have) {
                continue;
            }
            try {
                job.job();
            } catch (const Error& error) {
                std::lock_guard<std::mutex> lock(state->mutex);
                if (state->errors.size() >= MAX_ERRORS) {
                    state->errors.erase(state->errors.begin());
                }
                state->errors.emplace_back(job.id, error);
            } catch (const std::exception& error) {
                std::lock_guard<std::mutex> lock(state->mutex);
                if (state->errors.size() >= MAX_ERRORS) {
                    state->errors.erase(state->errors.begin());
                }
                state->errors.emplace_back(job.id, Error(ErrorCode::Io, error.what()));
            }
            {
                std::lock_guard<std::mutex> lock(state->mutex);
                state->running = false;
            }
            state->cv.notify_all();
        }
    }

    std::shared_ptr<State> state_;
    Device device_;
    std::thread worker_;
};

inline Scheduler Device::scheduler() const { return Scheduler(*this); }

// ---------------------------------------------------------------------------
// Flashing: cargo build, ELF -> UF2, BOOTSEL copy
// ---------------------------------------------------------------------------

// Port of sdk/rust/src/flash.rs and sdk/rust/src/flash/uf2.rs. Three things
// learned on hardware decide the shape:
//
//  * the board refuses BOOTLOADER while a host still polls it, so the runtime
//    session is closed *before* the drive is waited for;
//  * the UF2 drive only appears after that reboot, so "was it mounted before
//    this command" — not its letter — is what identifies the right one;
//  * verification is a second enumeration plus a Feature 14 read, not the reply
//    of the copy: the old handle is gone by the time the board is back.
//
// cargo comes in through `Runner`, progress through `Reporter` and cancellation
// through `Cancel`, so the same sequence runs from any host program. The
// messages are the ones a person reads while flashing, hence Chinese.
namespace flash {

// Crate name of the firmware binary this module builds.
constexpr const char* APP_CRATE = "xiao-firmware";
// Only supported firmware target: Seeed XIAO RP2040 (Cortex-M0+).
constexpr const char* TARGET = "thumbv6m-none-eabi";
// Feature that turns on the embassy USB device; without it the crate is a lib.
constexpr const char* FEATURES = "hardware";
// Feature `board` id reported by the XIAO firmware build.
constexpr std::uint8_t BOARD_ID = 2;

// How long the UF2 drive may take to appear after the reboot, how long the
// runtime HID may take to come back and answer Feature 14, and how often both
// loops look.
constexpr std::uint32_t DRIVE_WAIT_MS = 30000;
constexpr std::uint32_t HANDSHAKE_WAIT_MS = 15000;
constexpr std::uint32_t POLL_MS = 250;
// Copy chunk: a cancelled transfer stops between chunks, not mid-block.
constexpr std::size_t COPY_CHUNK = 16 * 1024;

inline std::string join_path(const std::string& base, const std::string& leaf) {
    if (base.empty()) {
        return leaf;
    }
    const char last = base.back();
    if (last == '/' || last == '\\') {
        return base + leaf;
    }
    return base + "/" + leaf;
}

inline bool is_regular_file(const std::string& path) {
#if defined(_WIN32)
    const DWORD attributes = GetFileAttributesW(detail::widen(path).c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES) {
        return false;
    }
    return (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
#else
    std::ifstream probe(path, std::ios::binary);
    return probe.good();
#endif
}

inline std::string read_text_file(const std::string& path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) {
        return std::string();
    }
    return std::string(std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>());
}

inline std::vector<std::uint8_t> read_binary_file(const std::string& path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) {
        throw Error(ErrorCode::Io, "讀取 ELF 失敗：" + path + "：檔案無法開啟");
    }
    input.seekg(0, std::ios::end);
    const std::streamoff size = input.tellg();
    if (size < 0) {
        throw Error(ErrorCode::Io, "讀取 ELF 失敗：" + path + "：檔案大小不明");
    }
    input.seekg(0, std::ios::beg);
    std::vector<std::uint8_t> bytes(static_cast<std::size_t>(size));
    if (size > 0 && !input.read(reinterpret_cast<char*>(bytes.data()), size)) {
        throw Error(ErrorCode::Io, "讀取 ELF 失敗：" + path + "：讀取中斷");
    }
    return bytes;
}

// Where the firmware lives and how the board should be put into BOOTSEL.
struct Config {
    // Explicit cargo binary; empty looks it up on PATH.
    std::string cargo;
    // Firmware root: the folder that contains `app/`.
    std::string firmware = "firmware";
    // Skip `rustup target add` when the target is known to be installed.
    bool skip_dependencies = false;
    // Send BOOTLOADER to the selected board instead of asking for BOOTSEL.
    bool auto_bootloader = true;
    // An explicit UF2 drive; empty waits for the one this command created.
    std::string boot_drive;
    std::uint32_t drive_wait_ms = DRIVE_WAIT_MS;
    std::uint32_t handshake_wait_ms = HANDSHAKE_WAIT_MS;
    std::uint32_t poll_ms = POLL_MS;

    static Config with_firmware(const std::string& firmware) {
        Config config;
        config.firmware = firmware;
        return config;
    }

    std::string app() const { return join_path(firmware, "app"); }

    std::string elf() const {
        return join_path(join_path(app(), std::string("target/") + TARGET + "/release"), APP_CRATE);
    }
};

// A packed image: the bytes the bootloader drive expects and their block count.
struct Image {
    std::vector<std::uint8_t> bytes;
    std::size_t blocks = 0;
};

// Cancellation as the flashing sequence needs it: one question, asked often.
class Cancel {
public:
    virtual ~Cancel() = default;
    virtual bool cancelled() const = 0;
};

// The obvious token; a GUI passes its own Cancel instead.
class Token final : public Cancel {
public:
    bool cancelled() const override { return flag_.load(); }
    void cancel() { flag_.store(true); }

private:
    std::atomic<bool> flag_{false};
};

// Progress, in the order the user sees it: stages 1-5 and a log of what ran.
class Reporter {
public:
    virtual ~Reporter() = default;
    virtual void log(const std::string& line) = 0;
    virtual void stage(std::uint8_t stage, const std::string& message) = 0;
    // Several UF2 drives are mounted; the answer must be one of `candidates`.
    // Refusing by default is deliberate: guessing here flashes the wrong board.
    virtual std::string choose_drive(const std::vector<std::string>& candidates) {
        std::string joined;
        for (std::size_t index = 0; index < candidates.size(); ++index) {
            if (index != 0) {
                joined += "、";
            }
            joined += candidates[index];
        }
        throw Error(ErrorCode::Invalid, "同時偵測到多個 UF2 磁碟（" + joined + "），請指定 boot 磁碟");
    }
};

// Drops every line; handy for a caller that only cares about the outcome.
class QuietReporter final : public Reporter {
public:
    void log(const std::string&) override {}
    void stage(std::uint8_t, const std::string&) override {}
};

inline void check_cancelled(const Cancel& cancel) {
    if (cancel.cancelled()) {
        throw Error(ErrorCode::Cancelled, "操作已取消");
    }
}

// Sleeps in slices so cancellation is honoured inside a wait.
inline void pause_for(const Cancel& cancel, std::uint32_t ms) {
    const auto end = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    while (std::chrono::steady_clock::now() < end) {
        check_cancelled(cancel);
        const auto left = std::chrono::duration_cast<std::chrono::milliseconds>(
            end - std::chrono::steady_clock::now());
        if (left.count() <= 0) {
            break;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(std::min<std::int64_t>(left.count(), 10)));
    }
    check_cancelled(cancel);
}

// One line of child output.
enum class Stream : std::int32_t {
    Stdout = 0,
    Stderr = 1,
};

inline const char* stream_text(Stream stream) {
    return stream == Stream::Stdout ? "Stdout" : "Stderr";
}

// What a finished child program reported.
struct Outcome {
    std::string out;
    std::string err;
    std::optional<int> exit_code;
    bool timed_out = false;
    bool cancelled = false;

    // Returns stdout when the program succeeded, a usable message when not.
    std::string check() const {
        if (cancelled) {
            throw Error(ErrorCode::Cancelled, "操作已取消");
        }
        if (timed_out) {
            throw Error(ErrorCode::Timeout, "子程序逾時");
        }
        if (!exit_code.has_value() || *exit_code != 0) {
            throw Error(ErrorCode::Io,
                        "子程序失敗，exit=" +
                            (exit_code.has_value() ? std::to_string(*exit_code)
                                                   : std::string("None")) +
                            "\n" + err + "\n" + out);
        }
        return out;
    }
};

// Runs an external program; the host supplies its own when it already has one
// (the desktop tool kills a whole process tree, which a library cannot assume).
class Runner {
public:
    virtual ~Runner() = default;
    virtual Outcome run(const std::string& program, const std::vector<std::string>& args,
                        std::uint32_t timeout_ms, const Cancel& cancel,
                        const std::function<void(Stream, const std::string&)>& line) = 0;
};

namespace io {

struct Line {
    Stream stream = Stream::Stdout;
    std::string text;
};

struct PipeLines {
    std::mutex mutex;
    std::condition_variable cv;
    std::deque<Line> queue;
    int readers = 2;
};

// Defined below; declared here so the runner reads top to bottom.
#if defined(_WIN32)
inline void read_lines(HANDLE handle, Stream stream, const std::shared_ptr<PipeLines>& shared);
#endif
inline bool drain(const std::shared_ptr<PipeLines>& shared, std::uint32_t wait_ms, Outcome& outcome,
                  const std::function<void(Stream, const std::string&)>& line);
inline bool wait_readers(const std::shared_ptr<PipeLines>& shared, std::uint32_t timeout_ms);

}  // namespace io

// The plain implementation: spawn with CreateProcessW, stream both pipes line by
// line, kill on cancel or timeout.
class SystemRunner final : public Runner {
public:
    Outcome run(const std::string& program, const std::vector<std::string>& args,
                std::uint32_t timeout_ms, const Cancel& cancel,
                const std::function<void(Stream, const std::string&)>& line) override {
#if defined(_WIN32)
        SECURITY_ATTRIBUTES inherit{};
        inherit.nLength = sizeof(inherit);
        inherit.bInheritHandle = TRUE;

        HANDLE out_read = nullptr;
        HANDLE out_write = nullptr;
        HANDLE err_read = nullptr;
        HANDLE err_write = nullptr;
        const auto make_pipe = [&inherit](HANDLE& read_end, HANDLE& write_end) {
            if (CreatePipe(&read_end, &write_end, &inherit, 0) == 0) {
                throw Error(ErrorCode::Io, "無法建立子程序管線");
            }
            SetHandleInformation(read_end, HANDLE_FLAG_INHERIT, 0);
        };
        make_pipe(out_read, out_write);
        try {
            make_pipe(err_read, err_write);
        } catch (...) {
            CloseHandle(out_read);
            CloseHandle(out_write);
            throw;
        }
        HANDLE null_in = CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                     &inherit, OPEN_EXISTING, 0, nullptr);

        STARTUPINFOW startup{};
        startup.cb = sizeof(startup);
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = null_in == INVALID_HANDLE_VALUE ? nullptr : null_in;
        startup.hStdOutput = out_write;
        startup.hStdError = err_write;

        std::wstring command = detail::quote_argument(program);
        for (const std::string& arg : args) {
            command += L" " + detail::quote_argument(arg);
        }
        PROCESS_INFORMATION process{};
        std::vector<wchar_t> mutable_command(command.begin(), command.end());
        mutable_command.push_back(L'\0');
        const BOOL started = CreateProcessW(nullptr, mutable_command.data(), nullptr, nullptr, TRUE,
                                            CREATE_NO_WINDOW, nullptr, nullptr, &startup, &process);
        CloseHandle(out_write);
        CloseHandle(err_write);
        if (null_in != INVALID_HANDLE_VALUE) {
            CloseHandle(null_in);
        }
        if (!started) {
            const DWORD failure = GetLastError();
            CloseHandle(out_read);
            CloseHandle(err_read);
            throw Error(ErrorCode::Io,
                        "無法啟動 " + program + "：" + std::to_string(failure));
        }

        auto lines = std::make_shared<io::PipeLines>();
        std::thread reader_out(io::read_lines, out_read, Stream::Stdout, lines);
        std::thread reader_err(io::read_lines, err_read, Stream::Stderr, lines);

        Outcome outcome;
        const auto started_at = std::chrono::steady_clock::now();
        for (;;) {
            io::drain(lines, 20, outcome, line);
            const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::steady_clock::now() - started_at);
            const bool stop = cancel.cancelled();
            if (stop || elapsed.count() >= static_cast<std::int64_t>(timeout_ms)) {
                outcome.cancelled = stop;
                outcome.timed_out = !stop;
                TerminateProcess(process.hProcess, 1);
                break;
            }
            if (WaitForSingleObject(process.hProcess, 20) == WAIT_OBJECT_0) {
                break;
            }
        }
        WaitForSingleObject(process.hProcess, 5000);
        DWORD code = 0;
        if (GetExitCodeProcess(process.hProcess, &code) != 0) {
            outcome.exit_code = static_cast<int>(code);
        }
        // A grandchild can keep the pipes open after the child is gone, so the
        // readers get a moment and are then left to finish on their own.
        const auto deadline =
            std::chrono::steady_clock::now() + std::chrono::milliseconds(1000);
        while (std::chrono::steady_clock::now() < deadline) {
            if (!io::drain(lines, 20, outcome, line)) {
                break;
            }
        }
        if (io::wait_readers(lines, 2000)) {
            reader_out.join();
            reader_err.join();
        } else {
            reader_out.detach();
            reader_err.detach();
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        return outcome;
#else
        (void)program;
        (void)args;
        (void)timeout_ms;
        (void)cancel;
        (void)line;
        throw Error(ErrorCode::Unsupported, "子程序需要 Windows CreateProcessW");
#endif
    }
};

namespace io {

#if defined(_WIN32)
inline void read_lines(HANDLE handle, Stream stream, const std::shared_ptr<PipeLines>& shared) {
    std::string pending;
    std::vector<char> buffer(4096);
    for (;;) {
        DWORD got = 0;
        if (ReadFile(handle, buffer.data(), static_cast<DWORD>(buffer.size()), &got, nullptr) == 0 ||
            got == 0) {
            break;
        }
        pending.append(buffer.data(), got);
        std::size_t at = 0;
        while ((at = pending.find('\n')) != std::string::npos) {
            std::string text = pending.substr(0, at);
            pending.erase(0, at + 1);
            if (!text.empty() && text.back() == '\r') {
                text.pop_back();
            }
            std::lock_guard<std::mutex> lock(shared->mutex);
            shared->queue.push_back(Line{stream, std::move(text)});
            shared->cv.notify_all();
        }
    }
    if (!pending.empty()) {
        if (!pending.empty() && pending.back() == '\r') {
            pending.pop_back();
        }
        std::lock_guard<std::mutex> lock(shared->mutex);
        shared->queue.push_back(Line{stream, std::move(pending)});
        shared->cv.notify_all();
    }
    CloseHandle(handle);
    {
        std::lock_guard<std::mutex> lock(shared->mutex);
        --shared->readers;
    }
    shared->cv.notify_all();
}
#endif  // _WIN32

// Moves whatever the readers produced into `outcome`; `false` once they are all
// gone, so a caller can stop waiting.
inline bool drain(const std::shared_ptr<PipeLines>& shared, std::uint32_t wait_ms, Outcome& outcome,
                  const std::function<void(Stream, const std::string&)>& line) {
    std::deque<Line> batch;
    {
        std::unique_lock<std::mutex> lock(shared->mutex);
        if (shared->queue.empty()) {
            shared->cv.wait_for(lock, std::chrono::milliseconds(wait_ms), [&shared] {
                return !shared->queue.empty() || shared->readers == 0;
            });
        }
        if (shared->queue.empty()) {
            return shared->readers > 0;
        }
        batch.swap(shared->queue);
    }
    for (const Line& item : batch) {
        std::string& sink = item.stream == Stream::Stdout ? outcome.out : outcome.err;
        // Bound what is retained; every line still reaches the log.
        if (sink.size() + item.text.size() <= 10 * 1024 * 1024) {
            sink += item.text;
            sink += '\n';
        }
        line(item.stream, item.text);
    }
    return true;
}

inline bool wait_readers(const std::shared_ptr<PipeLines>& shared, std::uint32_t timeout_ms) {
    std::unique_lock<std::mutex> lock(shared->mutex);
    return shared->cv.wait_for(lock, std::chrono::milliseconds(timeout_ms),
                               [&shared] { return shared->readers <= 0; });
}

}  // namespace io

// Every mounted RP2040 bootloader drive, by volume label and by INFO_UF2.TXT.
inline std::vector<std::string> drives() {
#if defined(_WIN32)
    std::vector<std::string> found;
    std::vector<wchar_t> buffer(512, L'\0');
    DWORD length = GetLogicalDriveStringsW(static_cast<DWORD>(buffer.size() - 1), buffer.data());
    if (length == 0 || length > buffer.size()) {
        return found;
    }
    for (std::size_t index = 0; index < length && buffer[index] != L'\0';) {
        const std::wstring root(&buffer[index]);
        index += root.size() + 1;
        if (root.size() < 2) {
            continue;
        }
        // Label or INFO_UF2.TXT, exactly as the Rust SDK decides: some hosts
        // mount the bootloader volume as fixed media, so the drive type is not
        // part of the test.
        wchar_t label[64] = {};
        const bool have_label =
            GetVolumeInformationW(root.c_str(), label, 64, nullptr, nullptr, nullptr, nullptr, 0) != 0;
        const std::string root_text = detail::narrow(root);
        const std::string info = read_text_file(join_path(root_text, "INFO_UF2.TXT"));
        if ((have_label && detail::lower_ascii(detail::narrow(label)) == "rpi-rp2") ||
            info.find("RP2040") != std::string::npos ||
            info.find("RPI-RP2") != std::string::npos) {
            found.push_back(root_text);
        }
    }
    return found;
#else
    throw Error(ErrorCode::Unsupported, "UF2 磁碟偵測僅支援 Windows");
#endif
}

// The selection seam: a test swaps the enumeration for a temp folder, the same
// way a host could point at a mounted image.
struct Hooks {
    std::function<std::vector<std::string>()> drives;             // mounted UF2 roots
    std::function<std::vector<DeviceEntry>()> devices;            // runtime HID entries
    std::function<Device(const std::string&)> open_device;        // one device, opened
};

inline std::vector<std::string> mounted_drives(const Hooks& hooks) {
    return hooks.drives ? hooks.drives() : drives();
}

inline std::vector<DeviceEntry> runtime_devices(const Hooks& hooks) {
    return hooks.devices ? hooks.devices() : device_list();
}

inline Device open_runtime_device(const Hooks& hooks, const std::string& path) {
    if (hooks.open_device) {
        return hooks.open_device(path);
    }
    return Device::open(path, 1000, false, false);
}

// What the runtime board answered after the reboot.
struct Verified {
    std::uint8_t board = 0;
    std::string firmware;
    std::uint16_t lease_ms = 0;

    std::string text() const {
        return "board=" + std::to_string(static_cast<unsigned>(board)) + ", fw=" + firmware +
               ", lease=" + std::to_string(lease_ms) + " ms";
    }
};

// The two outcomes after a successful copy: the board came back and answered,
// or it did not. Either way the image is on the flash.
struct Handshake {
    bool ok = false;
    Verified verified;
    std::string failure;
};

inline void check_board(std::uint8_t reported) {
    if (reported == BOARD_ID) {
        return;
    }
    throw Error(ErrorCode::Invalid, "重新列舉的板型不符：" +
                                        std::to_string(static_cast<unsigned>(reported)) +
                                        "（預期 " + std::to_string(BOARD_ID) + "）");
}

// RP2040 ELF -> UF2. Every block must be 256-byte aligned (the BOOTSEL
// bootloader silently ignores a file that is not) and the flash segments have to
// be flattened into one image, because a segment's file order is not its address
// order — `.boot2` regularly comes last while it must sit at 0x10000000.
constexpr std::uint32_t FAMILY_RP2040 = 0xE48BFF56u;
constexpr std::uint32_t XIP_BASE = 0x10000000u;
constexpr std::uint32_t XIP_LIMIT = 0x11000000u;
constexpr std::size_t UF2_BLOCK = 512;
constexpr std::size_t UF2_PAYLOAD = 256;
constexpr std::uint32_t UF2_MAGIC_START_0 = 0x0A324655u;
constexpr std::uint32_t UF2_MAGIC_START_1 = 0x9E5D5157u;
constexpr std::uint32_t UF2_MAGIC_END = 0x0AB16F30u;
constexpr std::uint32_t UF2_FLAG_FAMILY = 0x00002000u;

namespace elf {

inline std::uint16_t u16_at(const std::vector<std::uint8_t>& data, std::size_t at) {
    if (at + 2 > data.size()) {
        throw Error(ErrorCode::Invalid, "ELF 太短");
    }
    return static_cast<std::uint16_t>(data[at] | (static_cast<std::uint16_t>(data[at + 1]) << 8));
}

inline std::uint32_t u32_at(const std::vector<std::uint8_t>& data, std::size_t at) {
    if (at + 4 > data.size()) {
        throw Error(ErrorCode::Invalid, "ELF 太短");
    }
    return read_u32(&data[at]);
}

}  // namespace elf

// Packs the flash image of an RP2040 ELF into a UF2 file.
inline Image uf2_pack(const std::vector<std::uint8_t>& elf) {
    if (elf.size() <= 52 || elf[0] != 0x7F || elf[1] != 'E' || elf[2] != 'L' || elf[3] != 'F') {
        throw Error(ErrorCode::Invalid, "不是 ELF 檔");
    }
    if (elf[4] != 1 || elf[5] != 1) {
        throw Error(ErrorCode::Invalid, "只支援 32-bit little-endian ELF");
    }
    const std::size_t phoff = elf::u32_at(elf, 28);
    const std::size_t phentsize = elf::u16_at(elf, 42);
    const std::size_t phnum = elf::u16_at(elf, 44);

    struct Segment {
        std::uint32_t address = 0;
        std::size_t offset = 0;
        std::size_t size = 0;
    };
    std::vector<Segment> segments;
    for (std::size_t index = 0; index < phnum; ++index) {
        const std::size_t base = phoff + index * phentsize;
        if (base + 32 > elf.size()) {
            throw Error(ErrorCode::Invalid, "program header 超出檔案");
        }
        const std::uint32_t kind = elf::u32_at(elf, base);
        const std::size_t offset = elf::u32_at(elf, base + 4);
        const std::uint32_t address = elf::u32_at(elf, base + 12);
        const std::size_t size = elf::u32_at(elf, base + 16);
        if (kind != 1 || size == 0 || address < XIP_BASE || address >= XIP_LIMIT) {
            continue;
        }
        if (offset > elf.size() || size > elf.size() - offset) {
            throw Error(ErrorCode::Invalid, "segment 超出檔案");
        }
        Segment segment;
        segment.address = address;
        segment.offset = offset;
        segment.size = size;
        segments.push_back(segment);
    }
    if (segments.empty()) {
        throw Error(ErrorCode::Invalid, "ELF 沒有任何 flash segment；這不是 RP2040 建置？");
    }
    std::sort(segments.begin(), segments.end(),
              [](const Segment& a, const Segment& b) { return a.address < b.address; });

    // Flatten to one 256-byte aligned image; gaps stay erased (0xFF).
    const std::uint32_t start = segments.front().address & ~(UF2_PAYLOAD - 1u);
    std::uint32_t end = start;
    for (const Segment& segment : segments) {
        end = std::max(end, segment.address + static_cast<std::uint32_t>(segment.size));
    }
    end = (end + static_cast<std::uint32_t>(UF2_PAYLOAD) - 1u) &
          ~static_cast<std::uint32_t>(UF2_PAYLOAD - 1u);
    std::vector<std::uint8_t> image(static_cast<std::size_t>(end - start), 0xFF);
    for (const Segment& segment : segments) {
        const std::size_t at = static_cast<std::size_t>(segment.address - start);
        std::copy(elf.begin() + static_cast<std::ptrdiff_t>(segment.offset),
                  elf.begin() + static_cast<std::ptrdiff_t>(segment.offset + segment.size),
                  image.begin() + static_cast<std::ptrdiff_t>(at));
    }

    const std::size_t total = image.size() / UF2_PAYLOAD;
    Image packed;
    packed.bytes.reserve(total * UF2_BLOCK);
    for (std::size_t number = 0; number < total; ++number) {
        std::vector<std::uint8_t> block(UF2_BLOCK, 0);
        const std::uint32_t fields[8] = {
            UF2_MAGIC_START_0,
            UF2_MAGIC_START_1,
            UF2_FLAG_FAMILY,
            start + static_cast<std::uint32_t>(number * UF2_PAYLOAD),
            static_cast<std::uint32_t>(UF2_PAYLOAD),
            static_cast<std::uint32_t>(number),
            static_cast<std::uint32_t>(total),
            FAMILY_RP2040,
        };
        for (std::size_t index = 0; index < 8; ++index) {
            write_u32(&block[index * 4], fields[index]);
        }
        std::copy(image.begin() + static_cast<std::ptrdiff_t>(number * UF2_PAYLOAD),
                  image.begin() + static_cast<std::ptrdiff_t>((number + 1) * UF2_PAYLOAD),
                  block.begin() + 32);
        write_u32(&block[UF2_BLOCK - 4], UF2_MAGIC_END);
        packed.bytes.insert(packed.bytes.end(), block.begin(), block.end());
    }
    packed.blocks = total;
    return packed;
}

// Block count of a packed image; the caller logs it, the tests assert on it.
inline std::size_t uf2_blocks(const std::vector<std::uint8_t>& uf2) {
    return uf2.size() / UF2_BLOCK;
}

// Writes the image to `target` in chunks so a cancelled copy stops early.
inline void write_image(const std::string& target, const std::vector<std::uint8_t>& bytes,
                        const Cancel& cancel) {
    check_cancelled(cancel);
    std::ofstream output(target, std::ios::binary | std::ios::trunc);
    if (!output) {
        throw Error(ErrorCode::Io, "建立 UF2 目的檔失敗：" + target);
    }
    for (std::size_t at = 0; at < bytes.size(); at += COPY_CHUNK) {
        check_cancelled(cancel);
        const std::size_t size = std::min(COPY_CHUNK, bytes.size() - at);
        output.write(reinterpret_cast<const char*>(&bytes[at]), static_cast<std::streamsize>(size));
        if (!output) {
            output.close();
            std::remove(target.c_str());
            throw Error(ErrorCode::Io, "UF2 複製失敗（保留 OS 錯誤碼）：" + target);
        }
    }
    output.flush();
    if (!output) {
        output.close();
        std::remove(target.c_str());
        throw Error(ErrorCode::Io, "UF2 重新整理失敗：" + target);
    }
    output.close();
}

inline std::string wait_drive(const Config& config, const Cancel& cancel, Reporter& progress,
                              const std::vector<std::string>& previous, const Hooks& hooks) {
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(config.drive_wait_ms);
    while (std::chrono::steady_clock::now() < deadline) {
        check_cancelled(cancel);
        std::vector<std::string> fresh;
        for (const std::string& path : mounted_drives(hooks)) {
            if (std::find(previous.begin(), previous.end(), path) == previous.end()) {
                fresh.push_back(path);
            }
        }
        if (fresh.size() == 1) {
            return fresh.front();
        }
        if (fresh.size() > 1) {
            return progress.choose_drive(fresh);
        }
        pause_for(cancel, config.poll_ms);
    }
    throw Error(ErrorCode::Timeout, std::to_string(config.drive_wait_ms / 1000) +
                                        " 秒內未偵測到 RP2040 UF2 磁碟；請 BOOTSEL 上電後重試");
}

inline Verified verify_runtime(const Config& config, const Hooks& hooks, const Cancel& cancel,
                               const std::string& serial,
                               const std::vector<std::string>& existing) {
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(config.handshake_wait_ms);
    std::string last = "尚未列舉";
    while (std::chrono::steady_clock::now() < deadline) {
        check_cancelled(cancel);
        std::vector<DeviceEntry> matched;
        for (const DeviceEntry& entry : runtime_devices(hooks)) {
            // An already-running board cannot stand in for the one just flashed.
            if (std::find(existing.begin(), existing.end(), entry.path) != existing.end()) {
                continue;
            }
            if (!serial.empty() && entry.serial != serial) {
                continue;
            }
            matched.push_back(entry);
        }
        // Duplicate firmware serials cannot prove identity: never pick the first.
        if (matched.size() == 1) {
            try {
                Device fresh = open_runtime_device(hooks, matched.front().path);
                const DeviceInfo info = fresh.info();
                check_board(info.board);
                Verified verified;
                verified.board = info.board;
                verified.firmware = info.firmware;
                verified.lease_ms = info.lease_ms;
                try {
                    fresh.close();
                } catch (const Error&) {
                }
                return verified;
            } catch (const Error& error) {
                last = error.what();
            }
        } else if (matched.size() > 1) {
            last = "多個符合裝置，無法確認燒錄目標；請僅連接目標板後驗證";
        }
        pause_for(cancel, config.poll_ms);
    }
    throw Error(ErrorCode::Timeout, "runtime hello 驗證逾時：" + last);
}

// Sends BOOTLOADER, copies the image onto the UF2 drive and waits for the board
// to come back. A thrown error means the transfer itself failed; `ok == false`
// on the result means the copy finished but the board never answered — the
// distinction the caller reports to the user.
inline Handshake push(const Config& config, Device* device, const Image& image,
                      const Cancel& cancel, Reporter& progress, const Hooks& hooks) {
    check_cancelled(cancel);
    progress.stage(3, "進入並選擇 bootloader");
    // Never auto-select a drive that was already mounted before this command:
    // it belongs to some other board.
    std::vector<std::string> previous;
    if (config.auto_bootloader && config.boot_drive.empty() && device != nullptr) {
        previous = mounted_drives(hooks);
    }
    if (config.auto_bootloader && config.boot_drive.empty()) {
        if (device != nullptr) {
            try {
                device->bootloader();
                progress.log("已確認 enter_bootloader；等待裝置重啟");
            } catch (const Error& error) {
                check_cancelled(cancel);
                progress.log("bootloader 命令未收到完整確認，繼續檢查列舉結果：" +
                             std::string(error.what()));
            }
            // The board refuses BOOTLOADER while a host keeps polling it, and the
            // runtime session has to be gone before the drive shows up.
            try {
                device->close();
            } catch (const Error&) {
            }
        } else {
            progress.log("未選取 runtime HID，請使用實體 BOOTSEL 進入下載模式");
        }
    }
    const std::string drive =
        !config.boot_drive.empty() ? config.boot_drive
                                   : wait_drive(config, cancel, progress, previous, hooks);
    check_cancelled(cancel);
    // Boards already in runtime here are unrelated to this transfer: excluding
    // them stops an existing board's hello from verifying another board.
    std::vector<std::string> existing;
    for (const DeviceEntry& entry : runtime_devices(hooks)) {
        existing.push_back(entry.path);
    }
    progress.stage(4, "傳輸韌體");
    if (!is_regular_file(join_path(drive, "INFO_UF2.TXT"))) {
        throw Error(ErrorCode::Invalid, "指定位置不是 UF2 bootloader 磁碟");
    }
    write_image(join_path(drive, "Firmware.uf2"), image.bytes, cancel);
    progress.log("UF2 複製完成，等待 runtime 驗證");
    progress.stage(5, "等待 runtime HID 重新列舉與 hello 驗證");
    std::string serial;
    if (device != nullptr) {
        serial = device->info().serial;
    }
    Handshake handshake;
    try {
        handshake.verified = verify_runtime(config, hooks, cancel, serial, existing);
        handshake.ok = true;
    } catch (const Error& error) {
        if (error.code() == ErrorCode::Cancelled) {
            throw;
        }
        handshake.ok = false;
        handshake.failure = error.what();
    }
    return handshake;
}

// The config-free form: defaults except for the device and the image.
inline Handshake push(Device* device, const Image& image, Reporter& progress,
                      const Cancel& cancel, const Hooks& hooks = Hooks()) {
    return push(Config{}, device, image, cancel, progress, hooks);
}

namespace tool {

inline std::string locate_on_path(const std::string& name) {
#if defined(_WIN32)
    std::vector<wchar_t> buffer(4096, L'\0');
    for (;;) {
        const std::wstring wide = detail::widen(name);
        const DWORD length = SearchPathW(nullptr, wide.c_str(), nullptr,
                                         static_cast<DWORD>(buffer.size()), buffer.data(), nullptr);
        if (length == 0) {
            return std::string();
        }
        if (length < buffer.size()) {
            return detail::narrow(std::wstring(buffer.data(), length));
        }
        buffer.resize(static_cast<std::size_t>(length) + 1);
    }
#else
    return std::string();
#endif
}

}  // namespace tool

// `cargo`, preferring an explicit setting.
inline std::string cargo_path(const Config& config) {
    if (!config.cargo.empty()) {
        if (!is_regular_file(config.cargo)) {
            throw Error(ErrorCode::Invalid, "指定的 cargo 不存在：" + config.cargo);
        }
        return config.cargo;
    }
    const std::string found = tool::locate_on_path("cargo.exe");
    if (found.empty()) {
        throw Error(ErrorCode::Disconnected,
                    "找不到 cargo；請安裝 Rust 工具鏈，或在設定指定 cargo 路徑");
    }
    return found;
}

// rustup normally sits beside cargo; without it the target cannot be added.
inline std::string rustup_beside(const std::string& cargo) {
    const std::size_t slash = cargo.find_last_of("/\\");
    const std::string directory = slash == std::string::npos ? std::string() : cargo.substr(0, slash);
    const std::string candidate = join_path(directory.empty() ? std::string(".") : directory,
                                            "rustup.exe");
    return is_regular_file(candidate) ? candidate : std::string();
}

// Runs one child with progress and cancellation; returns its stdout.
inline std::string run_child(Runner& runner, const std::string& program,
                             const std::vector<std::string>& args, std::uint32_t timeout_ms,
                             const Cancel& cancel, Reporter& progress) {
    check_cancelled(cancel);
    std::string command = program;
    for (const std::string& arg : args) {
        command += " " + arg;
    }
    progress.log(command);
    const Outcome outcome = runner.run(program, args, timeout_ms, cancel,
                                       [&progress](Stream stream, const std::string& text) {
                                           progress.log(std::string(stream_text(stream)) + ": " + text);
                                       });
    check_cancelled(cancel);
    return outcome.check();
}

// Builds `app/` for the RP2040 and packs the result into a UF2 image.
inline Image build(const Config& config, Runner& runner, const Cancel& cancel,
                   Reporter& progress) {
    check_cancelled(cancel);
    const std::string manifest = join_path(config.app(), "Cargo.toml");
    if (!is_regular_file(manifest)) {
        throw Error(ErrorCode::Invalid,
                    "韌體資料夾不完整（缺 app/Cargo.toml）：" + config.firmware);
    }
    progress.stage(1, "檢查 Rust 工具鏈");
    const std::string cargo = cargo_path(config);
    if (!config.skip_dependencies) {
        const std::string rustup = rustup_beside(cargo);
        if (!rustup.empty()) {
            run_child(runner, rustup, {"target", "add", TARGET}, 600000, cancel, progress);
        } else {
            progress.log(std::string("找不到 rustup，跳過 target 安裝（假設 ") + TARGET + " 已安裝）");
        }
    }
    progress.stage(2, "編譯 Rust 韌體");
    run_child(runner, cargo,
              {"build", "--release", "--target", TARGET, "--manifest-path", manifest,
               "--features", FEATURES},
              900000, cancel, progress);

    const std::string elf = config.elf();
    if (!is_regular_file(elf)) {
        throw Error(ErrorCode::Invalid, "編譯成功但找不到 ELF：" + elf);
    }
    const std::vector<std::uint8_t> bytes = read_binary_file(elf);
    const Image packed = uf2_pack(bytes);
    progress.log("UF2：" + std::to_string(packed.blocks) + " 個區塊");
    return packed;
}

}  // namespace flash

}  // namespace native
}  // namespace gdb
