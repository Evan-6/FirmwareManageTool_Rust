// gdb.hpp - header-only C++17 binding for the GDBOX SDK.
//
// It loads gdb.dll (gdb.so) at runtime, so no import library is needed and the
// same header works with MSVC, clang and MinGW. Set GDBOX_LIBRARY to the full
// path of the library, or put it next to the executable.
//
//     #include "gdb.hpp"
//     auto device = gdb::Device::open_mock();
//     device->keyboard().type_text("hello");
//     device->mouse().move_to(200, 300, 250);
//
// Every call throws gdb::Error (code + message) on failure; nothing returns a
// silent error code.

#pragma once

#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "gdb.h"

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace gdb {

enum class ErrorCode : int32_t {
    Ok = GDB_OK,
    Invalid = GDB_INVALID,
    Timeout = GDB_TIMEOUT,
    Disconnected = GDB_DISCONNECTED,
    Busy = GDB_BUSY,
    Lease = GDB_LEASE,
    Framing = GDB_FRAMING,
    Overflow = GDB_OVERFLOW,
    Unsupported = GDB_UNSUPPORTED,
    Cancelled = GDB_CANCELLED,
    Io = GDB_IO,
    InvalidScreen = GDB_INVALID_SCREEN,
    StaleWindow = GDB_STALE_WINDOW,
};

enum class ConnectionState : int32_t {
    Disconnected = GDB_STATE_DISCONNECTED,
    Connecting = GDB_STATE_CONNECTING,
    Connected = GDB_STATE_CONNECTED,
    Error = GDB_STATE_ERROR,
};

enum class Button : uint8_t {
    Left = GDB_BUTTON_LEFT,
    Right = GDB_BUTTON_RIGHT,
    Middle = GDB_BUTTON_MIDDLE,
    Back = GDB_BUTTON_BACK,
    Forward = GDB_BUTTON_FORWARD,
};

enum class Ease : int32_t {
    Jerk = GDB_EASE_JERK,
    InOut = GDB_EASE_IN_OUT,
    Linear = GDB_EASE_LINEAR,
    In = GDB_EASE_IN,
    Out = GDB_EASE_OUT,
    CubicBezier = GDB_EASE_CUBIC_BEZIER,
};

class Error : public std::runtime_error {
public:
    Error(ErrorCode code, const std::string& message)
        : std::runtime_error(message), code_(code) {}
    ErrorCode code() const noexcept { return code_; }

private:
    ErrorCode code_;
};

struct Point {
    int32_t x = 0;
    int32_t y = 0;
};

struct KeyStep {
    uint16_t page = 7;
    uint16_t usage = 0;
    uint32_t hold_ms = 18;
    uint32_t gap_ms = 0;
};

// One reply packet from command(); opcode 0 means the command had no reply.
struct Packet {
    uint8_t opcode = 0;
    uint8_t flags = 0;
    uint32_t session = 0;
    uint32_t sequence = 0;
    std::vector<uint8_t> payload;
};

struct Profile {
    Ease ease = Ease::Jerk;
    double bezier[4] = {0.42, 0.0, 0.58, 1.0};
    double curve = 1.0;
    double jitter_px = 1.0;
    double overshoot_pct = 0.0;
    uint32_t settle_ms = 0;
    int32_t min_step_px = 1;

    static Profile linear() {
        Profile profile;
        profile.ease = Ease::Linear;
        profile.curve = 0.0;
        profile.jitter_px = 0.0;
        return profile;
    }

    gdb_profile raw() const {
        gdb_profile raw{};
        raw.ease = static_cast<int32_t>(ease);
        for (int index = 0; index < 4; ++index) {
            raw.bezier[index] = bezier[index];
        }
        raw.curve = curve;
        raw.jitter_px = jitter_px;
        raw.overshoot_pct = overshoot_pct;
        raw.settle_ms = settle_ms;
        raw.min_step_px = min_step_px;
        return raw;
    }

    static Profile from_raw(const gdb_profile& raw) {
        Profile profile;
        profile.ease = static_cast<Ease>(raw.ease);
        for (int index = 0; index < 4; ++index) {
            profile.bezier[index] = raw.bezier[index];
        }
        profile.curve = raw.curve;
        profile.jitter_px = raw.jitter_px;
        profile.overshoot_pct = raw.overshoot_pct;
        profile.settle_ms = raw.settle_ms;
        profile.min_step_px = raw.min_step_px;
        return profile;
    }
};

// One snapshot of the session's counters, as gdb_metrics_get fills it. The
// latency figures are microseconds: below 4 ms they are the exact samples, above
// it they are the edges of the bucket the sample fell in.
struct Metrics {
    uint64_t frames_sent = 0;      // reports written, handshake and CLOSE included
    uint64_t frames_received = 0;  // replies accepted for this session
    uint64_t retries = 0;          // reads that had to be looked past
    uint64_t stolen_frames = 0;    // the part of retries that answered someone else
    uint64_t reconnects = 0;
    uint64_t lease_faults = 0;
    uint64_t timeouts = 0;
    uint64_t latency_count = 0;
    uint64_t latency_min_us = 0;
    uint64_t latency_p50_us = 0;
    uint64_t latency_p95_us = 0;
    uint64_t latency_max_us = 0;

    static Metrics from_raw(const gdb_metrics& raw) {
        Metrics metrics;
        metrics.frames_sent = raw.frames_sent;
        metrics.frames_received = raw.frames_received;
        metrics.retries = raw.retries;
        metrics.stolen_frames = raw.stolen_frames;
        metrics.reconnects = raw.reconnects;
        metrics.lease_faults = raw.lease_faults;
        metrics.timeouts = raw.timeouts;
        metrics.latency_count = raw.latency_count;
        metrics.latency_min_us = raw.latency_min_us;
        metrics.latency_p50_us = raw.latency_p50_us;
        metrics.latency_p95_us = raw.latency_p95_us;
        metrics.latency_max_us = raw.latency_max_us;
        return metrics;
    }
};

// Clamped absolute moves, as gdb_metrics_clamps fills them. A move whose
// destination was outside the anchor rectangle is pulled back into it: it still
// converges and the click still lands inside the window, and only these numbers
// say the requested pixel was never reached. The four coordinates are meaningful
// when clamped_total is not zero.
struct Clamps {
    uint64_t clamped_total = 0;
    int32_t requested_x = 0;
    int32_t requested_y = 0;
    int32_t applied_x = 0;
    int32_t applied_y = 0;

    static Clamps from_raw(const gdb_clamps& raw) {
        Clamps clamps;
        clamps.clamped_total = raw.clamped_total;
        clamps.requested_x = raw.requested_x;
        clamps.requested_y = raw.requested_y;
        clamps.applied_x = raw.applied_x;
        clamps.applied_y = raw.applied_y;
        return clamps;
    }
};

// Which rectangle an absolute move is clamped into. Coordinates stay
// virtual-desktop physical pixels: the target is not a new origin.
struct Screen {
    int32_t kind = 0;  // 0 primary, 1 monitor (arg = index), 2 window (arg = HWND),
                       // 3 the monitor containing a packed point
    int64_t arg = 0;

    static Screen primary() { return Screen{0, 0}; }
    // The index-th monitor in enumeration order, which is not the display number
    // Windows settings shows.
    static Screen monitor(int32_t index) { return Screen{1, index}; }
    static Screen window(int64_t hwnd) { return Screen{2, hwnd}; }
    // The area inside the frame: where a game draws and where a capture's
    // coordinates start. `window` is the whole frame, title bar included.
    static Screen window_client(int64_t hwnd) { return Screen{4, hwnd}; }
    // The monitor holding the point, so a caller that starts a move from a
    // coordinate does not have to know which display that is. The point is
    // packed into the same 64-bit argument every target uses.
    static Screen containing(int32_t x, int32_t y) {
        const gdb_screen packed = gdb_screen_containing(x, y);
        return Screen{packed.kind, packed.arg};
    }

    gdb_screen raw() const { return gdb_screen{kind, arg}; }
};

namespace detail {

#if defined(_WIN32)
using Module = HMODULE;
inline Module load_module(const char* path) {
    return LoadLibraryA(path);
}
inline void* symbol(Module module, const char* name) {
    return reinterpret_cast<void*>(GetProcAddress(module, name));
}
#else
using Module = void*;
inline Module load_module(const char* path) {
    return dlopen(path, RTLD_NOW | RTLD_LOCAL);
}
inline void* symbol(Module module, const char* name) {
    return dlsym(module, name);
}
#endif

#if !defined(_WIN32)
inline const char* default_library() {
#if defined(__APPLE__)
    return "libgdb.dylib";
#else
    return "libgdb.so";
#endif
}
#else
inline const char* default_library() { return "gdb.dll"; }
#endif

struct Api {
    Module module = nullptr;

    decltype(&gdb_abi_version) abi_version = nullptr;
    decltype(&gdb_version) version = nullptr;
    decltype(&gdb_last_error) last_error = nullptr;
    decltype(&gdb_string_free) string_free = nullptr;
    decltype(&gdb_set_log_level) set_log_level = nullptr;
    decltype(&gdb_set_log_sink) set_log_sink = nullptr;
    decltype(&gdb_device_list) device_list = nullptr;
    decltype(&gdb_open) open = nullptr;
    decltype(&gdb_open_mock) open_mock = nullptr;
    decltype(&gdb_open_shared) open_shared = nullptr;
    decltype(&gdb_open_mock_shared) open_mock_shared = nullptr;
    decltype(&gdb_stolen_frames) stolen_frames = nullptr;
    decltype(&gdb_device_free) device_free = nullptr;
    decltype(&gdb_close) close = nullptr;
    decltype(&gdb_reconnect) reconnect = nullptr;
    decltype(&gdb_set_auto_reconnect) set_auto_reconnect = nullptr;
    decltype(&gdb_device_state) device_state = nullptr;
    decltype(&gdb_device_info) device_info = nullptr;
    decltype(&gdb_status) status = nullptr;
    decltype(&gdb_status_ext) status_ext = nullptr;
    decltype(&gdb_sessions) sessions = nullptr;
    decltype(&gdb_release_all) release_all = nullptr;
    decltype(&gdb_barrier) barrier = nullptr;
    decltype(&gdb_bootloader) bootloader = nullptr;
    decltype(&gdb_set_event_handler) set_event_handler = nullptr;
    decltype(&gdb_profile_default) profile_default = nullptr;
    decltype(&gdb_command) command = nullptr;
    decltype(&gdb_packet_free) packet_free = nullptr;
    decltype(&gdb_move_raw) move_raw = nullptr;
    decltype(&gdb_mouse_move_by) mouse_move_by = nullptr;
    decltype(&gdb_mouse_move_to) mouse_move_to = nullptr;
    decltype(&gdb_mouse_move_path) mouse_move_path = nullptr;
    decltype(&gdb_mouse_button) mouse_button = nullptr;
    decltype(&gdb_mouse_click) mouse_click = nullptr;
    decltype(&gdb_mouse_click_timed) mouse_click_timed = nullptr;
    decltype(&gdb_mouse_click_at_on) mouse_click_at_on = nullptr;
    decltype(&gdb_mouse_double_click) mouse_double_click = nullptr;
    decltype(&gdb_mouse_wheel) mouse_wheel = nullptr;
    decltype(&gdb_mouse_drag) mouse_drag = nullptr;
    decltype(&gdb_mouse_start_move_to) mouse_start_move_to = nullptr;
    decltype(&gdb_mouse_start_move_by) mouse_start_move_by = nullptr;
    decltype(&gdb_mouse_start_move_path) mouse_start_move_path = nullptr;
    decltype(&gdb_movement_cancel) movement_cancel = nullptr;
    decltype(&gdb_movement_retarget) movement_retarget = nullptr;
    decltype(&gdb_movement_is_finished) movement_is_finished = nullptr;
    decltype(&gdb_movement_wait) movement_wait = nullptr;
    decltype(&gdb_movement_free) movement_free = nullptr;
    decltype(&gdb_key_down) key_down = nullptr;
    decltype(&gdb_key_up) key_up = nullptr;
    decltype(&gdb_key_press) key_press = nullptr;
    decltype(&gdb_key_long_press) key_long_press = nullptr;
    decltype(&gdb_keys_chord) keys_chord = nullptr;
    decltype(&gdb_keys_sequence) keys_sequence = nullptr;
    decltype(&gdb_type_text) type_text = nullptr;
    decltype(&gdb_scheduler_new) scheduler_new = nullptr;
    decltype(&gdb_scheduler_free) scheduler_free = nullptr;
    decltype(&gdb_scheduler_submit) scheduler_submit = nullptr;
        decltype(&gdb_scheduler_submit_at) scheduler_submit_at = nullptr;
        decltype(&gdb_scheduler_elapsed_ms) scheduler_elapsed_ms = nullptr;
    decltype(&gdb_scheduler_every) scheduler_every = nullptr;
    decltype(&gdb_scheduler_cancel) scheduler_cancel = nullptr;
    decltype(&gdb_scheduler_pause) scheduler_pause = nullptr;
    decltype(&gdb_scheduler_resume) scheduler_resume = nullptr;
    decltype(&gdb_scheduler_wait_idle) scheduler_wait_idle = nullptr;
    decltype(&gdb_scheduler_take_errors) scheduler_take_errors = nullptr;
    /* Attached surface: same header, separate symbols. */
    decltype(&gdb_attach) attach = nullptr;
    decltype(&gdb_remote_spawn) remote_spawn = nullptr;
    decltype(&gdb_remote_free) remote_free = nullptr;
    decltype(&gdb_remote_info) remote_info = nullptr;
    decltype(&gdb_remote_status) remote_status = nullptr;
    decltype(&gdb_remote_status_ext) remote_status_ext = nullptr;
    decltype(&gdb_remote_sessions) remote_sessions = nullptr;
    decltype(&gdb_remote_key) remote_key = nullptr;
    decltype(&gdb_remote_mouse_move) remote_mouse_move = nullptr;
    decltype(&gdb_remote_move_by) remote_move_by = nullptr;
    decltype(&gdb_remote_move_raw) remote_move_raw = nullptr;
    decltype(&gdb_remote_mouse_buttons) remote_mouse_buttons = nullptr;
    decltype(&gdb_remote_wheel) remote_wheel = nullptr;
    decltype(&gdb_remote_barrier) remote_barrier = nullptr;
    decltype(&gdb_remote_release_all) remote_release_all = nullptr;
    decltype(&gdb_remote_command) remote_command = nullptr;
    decltype(&gdb_remote_close) remote_close = nullptr;
    /* Presence surface: same header, separate symbols, loaded softly too. */
    decltype(&gdb_probe) probe = nullptr;
    decltype(&gdb_probe_all) probe_all = nullptr;
    decltype(&gdb_observer_start) observer_start = nullptr;
    decltype(&gdb_observer_stop) observer_stop = nullptr;
    decltype(&gdb_observer_free) observer_free = nullptr;
    decltype(&gdb_observer_info) observer_info = nullptr;
    decltype(&gdb_observer_state) observer_state = nullptr;
    decltype(&gdb_observer_status) observer_status = nullptr;
    decltype(&gdb_observer_barrier) observer_barrier = nullptr;
    decltype(&gdb_observer_release_all) observer_release_all = nullptr;
    decltype(&gdb_observer_command) observer_command = nullptr;
    /* Counters, screen targets and the arrival watch: loaded softly as well. */
    decltype(&gdb_metrics_get) metrics_get = nullptr;
    decltype(&gdb_metrics_reset) metrics_reset = nullptr;
    decltype(&gdb_metrics_clamps) metrics_clamps = nullptr;
    decltype(&gdb_monitors_json) monitors_json = nullptr;
    decltype(&gdb_window_metrics) window_metrics = nullptr;
    decltype(&gdb_platform_prepare) platform_prepare = nullptr;
    decltype(&gdb_mouse_move_to_on) mouse_move_to_on = nullptr;
    decltype(&gdb_mouse_move_path_on) mouse_move_path_on = nullptr;
    decltype(&gdb_mouse_start_move_to_on) mouse_start_move_to_on = nullptr;
    decltype(&gdb_watch_start) watch_start = nullptr;
    decltype(&gdb_watch_stop) watch_stop = nullptr;
    decltype(&gdb_watch_free) watch_free = nullptr;
    decltype(&gdb_watch_listing) watch_listing = nullptr;
};

inline Api& api();

inline std::string last_error_message() {
    char buffer[512] = {};
    api().last_error(buffer, sizeof(buffer));
    return std::string(buffer);
}

inline Api& api() {
    static Api api = [] {
        const char* path = std::getenv("GDBOX_LIBRARY");
        Api loaded;
        loaded.module = load_module(path != nullptr ? path : default_library());
        if (loaded.module == nullptr) {
            throw Error(ErrorCode::Io,
                        "cannot load the GDBOX library; set GDBOX_LIBRARY");
        }
#define GDB_SYMBOL(field, name)                                                  \
    loaded.field = reinterpret_cast<decltype(loaded.field)>(symbol(loaded.module, name)); \
    if (loaded.field == nullptr) {                                               \
        throw Error(ErrorCode::Io, "gdb: missing symbol " name);                 \
    }
        GDB_SYMBOL(abi_version, "gdb_abi_version")
        GDB_SYMBOL(version, "gdb_version")
        GDB_SYMBOL(last_error, "gdb_last_error")
        GDB_SYMBOL(string_free, "gdb_string_free")
        GDB_SYMBOL(set_log_level, "gdb_set_log_level")
        GDB_SYMBOL(set_log_sink, "gdb_set_log_sink")
        GDB_SYMBOL(device_list, "gdb_device_list")
        GDB_SYMBOL(open, "gdb_open")
        GDB_SYMBOL(open_mock, "gdb_open_mock")
        GDB_SYMBOL(open_shared, "gdb_open_shared")
        GDB_SYMBOL(open_mock_shared, "gdb_open_mock_shared")
        GDB_SYMBOL(stolen_frames, "gdb_stolen_frames")
        GDB_SYMBOL(device_free, "gdb_device_free")
        GDB_SYMBOL(close, "gdb_close")
        GDB_SYMBOL(reconnect, "gdb_reconnect")
        GDB_SYMBOL(set_auto_reconnect, "gdb_set_auto_reconnect")
        GDB_SYMBOL(device_state, "gdb_device_state")
        GDB_SYMBOL(device_info, "gdb_device_info")
        GDB_SYMBOL(status, "gdb_status")
        GDB_SYMBOL(status_ext, "gdb_status_ext")
        GDB_SYMBOL(sessions, "gdb_sessions")
        GDB_SYMBOL(release_all, "gdb_release_all")
        GDB_SYMBOL(barrier, "gdb_barrier")
        GDB_SYMBOL(bootloader, "gdb_bootloader")
        GDB_SYMBOL(set_event_handler, "gdb_set_event_handler")
        GDB_SYMBOL(profile_default, "gdb_profile_default")
        GDB_SYMBOL(command, "gdb_command")
        GDB_SYMBOL(packet_free, "gdb_packet_free")
        GDB_SYMBOL(move_raw, "gdb_move_raw")
        GDB_SYMBOL(mouse_move_by, "gdb_mouse_move_by")
        GDB_SYMBOL(mouse_move_to, "gdb_mouse_move_to")
        GDB_SYMBOL(mouse_move_path, "gdb_mouse_move_path")
        GDB_SYMBOL(mouse_button, "gdb_mouse_button")
        GDB_SYMBOL(mouse_click, "gdb_mouse_click")
        GDB_SYMBOL(mouse_double_click, "gdb_mouse_double_click")
        GDB_SYMBOL(mouse_wheel, "gdb_mouse_wheel")
        GDB_SYMBOL(mouse_drag, "gdb_mouse_drag")
        GDB_SYMBOL(mouse_start_move_to, "gdb_mouse_start_move_to")
        GDB_SYMBOL(mouse_start_move_by, "gdb_mouse_start_move_by")
        GDB_SYMBOL(mouse_start_move_path, "gdb_mouse_start_move_path")
        GDB_SYMBOL(movement_cancel, "gdb_movement_cancel")
        GDB_SYMBOL(movement_retarget, "gdb_movement_retarget")
        GDB_SYMBOL(movement_is_finished, "gdb_movement_is_finished")
        GDB_SYMBOL(movement_wait, "gdb_movement_wait")
        GDB_SYMBOL(movement_free, "gdb_movement_free")
        GDB_SYMBOL(key_down, "gdb_key_down")
        GDB_SYMBOL(key_up, "gdb_key_up")
        GDB_SYMBOL(key_press, "gdb_key_press")
        GDB_SYMBOL(key_long_press, "gdb_key_long_press")
        GDB_SYMBOL(keys_chord, "gdb_keys_chord")
        GDB_SYMBOL(keys_sequence, "gdb_keys_sequence")
        GDB_SYMBOL(type_text, "gdb_type_text")
        GDB_SYMBOL(scheduler_new, "gdb_scheduler_new")
        GDB_SYMBOL(scheduler_free, "gdb_scheduler_free")
        GDB_SYMBOL(scheduler_submit, "gdb_scheduler_submit")
        GDB_SYMBOL(scheduler_every, "gdb_scheduler_every")
        GDB_SYMBOL(scheduler_cancel, "gdb_scheduler_cancel")
        GDB_SYMBOL(scheduler_pause, "gdb_scheduler_pause")
        GDB_SYMBOL(scheduler_resume, "gdb_scheduler_resume")
        GDB_SYMBOL(scheduler_wait_idle, "gdb_scheduler_wait_idle")
        GDB_SYMBOL(scheduler_take_errors, "gdb_scheduler_take_errors")
#undef GDB_SYMBOL
        // The attached surface is loaded softly: a library from before it existed
        // still serves every direct call, and only Remote reports the gap.
#define GDB_OPTIONAL(field, name)                                                \
    loaded.field = reinterpret_cast<decltype(loaded.field)>(symbol(loaded.module, name));
        GDB_OPTIONAL(attach, "gdb_attach")
        GDB_OPTIONAL(remote_spawn, "gdb_remote_spawn")
        GDB_OPTIONAL(remote_free, "gdb_remote_free")
        GDB_OPTIONAL(remote_info, "gdb_remote_info")
        GDB_OPTIONAL(remote_status, "gdb_remote_status")
        GDB_OPTIONAL(remote_status_ext, "gdb_remote_status_ext")
        GDB_OPTIONAL(remote_sessions, "gdb_remote_sessions")
        GDB_OPTIONAL(remote_key, "gdb_remote_key")
        GDB_OPTIONAL(remote_mouse_move, "gdb_remote_mouse_move")
        GDB_OPTIONAL(remote_move_by, "gdb_remote_move_by")
        GDB_OPTIONAL(remote_move_raw, "gdb_remote_move_raw")
        GDB_OPTIONAL(remote_mouse_buttons, "gdb_remote_mouse_buttons")
        GDB_OPTIONAL(remote_wheel, "gdb_remote_wheel")
        GDB_OPTIONAL(remote_barrier, "gdb_remote_barrier")
        GDB_OPTIONAL(remote_release_all, "gdb_remote_release_all")
        GDB_OPTIONAL(remote_command, "gdb_remote_command")
        GDB_OPTIONAL(remote_close, "gdb_remote_close")
        GDB_OPTIONAL(probe, "gdb_probe")
        GDB_OPTIONAL(probe_all, "gdb_probe_all")
        GDB_OPTIONAL(observer_start, "gdb_observer_start")
        GDB_OPTIONAL(observer_stop, "gdb_observer_stop")
        GDB_OPTIONAL(observer_free, "gdb_observer_free")
        GDB_OPTIONAL(observer_info, "gdb_observer_info")
        GDB_OPTIONAL(observer_state, "gdb_observer_state")
        GDB_OPTIONAL(observer_status, "gdb_observer_status")
        GDB_OPTIONAL(observer_barrier, "gdb_observer_barrier")
        GDB_OPTIONAL(observer_release_all, "gdb_observer_release_all")
        GDB_OPTIONAL(observer_command, "gdb_observer_command")
        GDB_OPTIONAL(metrics_get, "gdb_metrics_get")
        GDB_OPTIONAL(metrics_reset, "gdb_metrics_reset")
        GDB_OPTIONAL(metrics_clamps, "gdb_metrics_clamps")
        GDB_OPTIONAL(monitors_json, "gdb_monitors_json")
        GDB_OPTIONAL(window_metrics, "gdb_window_metrics")
        GDB_OPTIONAL(platform_prepare, "gdb_platform_prepare")
        GDB_OPTIONAL(mouse_move_to_on, "gdb_mouse_move_to_on")
        GDB_OPTIONAL(mouse_move_path_on, "gdb_mouse_move_path_on")
        GDB_OPTIONAL(mouse_start_move_to_on, "gdb_mouse_start_move_to_on")
        GDB_OPTIONAL(mouse_click_timed, "gdb_mouse_click_timed")
        GDB_OPTIONAL(mouse_click_at_on, "gdb_mouse_click_at_on")
        GDB_OPTIONAL(watch_start, "gdb_watch_start")
        GDB_OPTIONAL(watch_stop, "gdb_watch_stop")
        GDB_OPTIONAL(watch_free, "gdb_watch_free")
        GDB_OPTIONAL(watch_listing, "gdb_watch_listing")
#undef GDB_OPTIONAL
        return loaded;
    }();
    return api;
}

inline void check(int32_t code) {
    if (code == GDB_OK) {
        return;
    }
    throw Error(static_cast<ErrorCode>(code), last_error_message());
}

inline std::string take(char* text) {
    if (text == nullptr) {
        char buffer[512] = {};
        const int32_t code = api().last_error(buffer, sizeof(buffer));
        throw Error(static_cast<ErrorCode>(code == 0 ? GDB_IO : code), buffer);
    }
    std::string value(text);
    api().string_free(text);
    return value;
}

// Throws with the code the library recorded, for a call that returns a handle and
// so has no int32 of its own to check.
[[noreturn]] inline void throw_last_error() {
    char buffer[512] = {};
    const int32_t code = api().last_error(buffer, sizeof(buffer));
    throw Error(static_cast<ErrorCode>(code == 0 ? GDB_IO : code), buffer);
}

using DeviceHandle = std::unique_ptr<gdb_device, decltype(&gdb_device_free)>;
using RemoteHandle = std::unique_ptr<gdb_remote, decltype(&gdb_remote_free)>;
using MovementHandle = std::unique_ptr<gdb_movement, decltype(&gdb_movement_free)>;
using SchedulerHandle = std::unique_ptr<gdb_scheduler, decltype(&gdb_scheduler_free)>;
using ObserverHandle = std::unique_ptr<gdb_observer, decltype(&gdb_observer_free)>;
using WatchHandle = std::unique_ptr<gdb_watch, decltype(&gdb_watch_free)>;

inline gdb_profile profile_or_default(const Profile* profile) {
    if (profile != nullptr) {
        return profile->raw();
    }
    return api().profile_default();
}

// Remote needs the whole attached set: it arrived with one library version, so
// checking one symbol's presence is enough to name the gap clearly.
inline void require_remote_surface() {
    const Api& loaded = api();
    if (loaded.attach != nullptr && loaded.remote_free != nullptr &&
        loaded.remote_info != nullptr && loaded.remote_status != nullptr &&
        loaded.remote_command != nullptr && loaded.remote_close != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates the attached surface; use a library that "
                "exports gdb_attach");
}

// The presence surface arrived after the attached one, so it is loaded softly as
// well: an older library still serves every direct call.
inline void require_presence_surface() {
    const Api& loaded = api();
    if (loaded.probe != nullptr && loaded.probe_all != nullptr &&
        loaded.observer_start != nullptr && loaded.observer_free != nullptr &&
        loaded.observer_info != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates the presence probe; use a library that "
                "exports gdb_probe");
}

// The counters arrived after the presence probe, and the screen targets after
// those: both are loaded softly, so only the calls that need them report the gap.
inline void require_metrics_surface() {
    if (api().metrics_get != nullptr && api().metrics_reset != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates the metrics snapshot; use a library that "
                "exports gdb_metrics_get");
}

// The clamp counters and the monitor listing arrived with the containing anchor,
// so they are loaded softly too: only the calls that read them report the gap.
inline void require_clamps_surface() {
    if (api().metrics_clamps != nullptr && api().monitors_json != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates the clamp counters; use a library that "
                "exports gdb_metrics_clamps");
}

inline void require_screen_surface() {
    if (api().mouse_move_to_on != nullptr && api().mouse_move_path_on != nullptr &&
        api().mouse_start_move_to_on != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates screen-targeted moves; use a library that "
                "exports gdb_mouse_move_to_on");
}

inline void require_watch_surface() {
    if (api().watch_start != nullptr && api().watch_stop != nullptr &&
        api().watch_free != nullptr && api().watch_listing != nullptr) {
        return;
    }
    throw Error(ErrorCode::Unsupported,
                "this gdb.dll predates the arrival watch; use a library that "
                "exports gdb_watch_start");
}

// Frees a packet's payload however the scope is left.
struct PacketGuard {
    gdb_packet packet{};
    PacketGuard() = default;
    PacketGuard(const PacketGuard&) = delete;
    PacketGuard& operator=(const PacketGuard&) = delete;
    ~PacketGuard() { api().packet_free(&packet); }
};

// A log sink is process-wide, so it outlives every device. The function object
// lives in this slot, and the ABI only ever receives its address.
using LogSink = std::function<void(int32_t, const std::string&)>;

inline std::shared_ptr<LogSink>& log_sink_slot() {
    static std::shared_ptr<LogSink> slot;
    return slot;
}

inline void log_trampoline(void* user, int32_t level, const char* message) {
    auto* sink = static_cast<LogSink*>(user);
    if (sink == nullptr || !*sink) {
        return;
    }
    try {
        (*sink)(level, message != nullptr ? std::string(message) : std::string());
    } catch (...) {
        // A throwing sink must not cross the ABI boundary.
    }
}

}  // namespace detail

using detail::api;
using detail::check;

// Presence: the board's identity from Feature 14 alone - no session, no packet,
// so it is safe while another program holds the session. The string is the JSON
// object info_json() returns; probe_all() is the array of them, "[]" on an empty
// bus. Both throw gdb::Error like every other call.
inline std::string probe(const std::string& selector = std::string()) {
    detail::require_presence_surface();
    return detail::take(api().probe(selector.empty() ? nullptr : selector.c_str()));
}

inline std::string probe_all() {
    detail::require_presence_surface();
    return detail::take(api().probe_all());
}

// Every monitor, as the JSON array the ABI sends: one object per display with
// left/top/right/bottom in virtual-desktop pixels, `primary` on the entry kind 0
// resolves to, and the effective `dpi` (96 = 100%, 0 when the system will not
// say). No handle is needed and no packet is sent, so it is safe beside a
// session. The order is the enumeration order kind 1 indexes.
inline std::string monitors_json() {
    detail::require_clamps_surface();
    return detail::take(api().monitors_json());
}

// One window as the system reports it: {"rect", "clientOrigin", "clientSize",
// "dpi", "visible", "minimized", "foreground"}. `rect` is the frame
// Screen::window clamps into; the client fields describe the area inside it,
// which is what Screen::window_client uses and what a capture shows. Throws
// Unsupported where the platform has no window APIs and StaleWindow for a handle
// that does not resolve. No device handle is needed.
inline std::string window_metrics_json(int64_t hwnd) {
    if (api().window_metrics == nullptr) {
        throw Error(ErrorCode::Unsupported,
                    "this gdb.dll predates the window metrics; use a library that "
                    "exports gdb_window_metrics");
    }
    return detail::take(api().window_metrics(hwnd));
}

// Makes the process per-monitor DPI aware where the system allows it and reports
// the mode: {"supported", "dpiAwareness", "value", "perMonitor"}. A host that
// converts a screenshot coordinate into a screen coordinate has to agree with the
// SDK about the scale factor, and this is what decides it. Safe beside a session;
// every open already calls it.
inline std::string prepare_platform() {
    if (api().platform_prepare == nullptr) {
        throw Error(ErrorCode::Unsupported,
                    "this gdb.dll predates the platform preparation; use a library "
                    "that exports gdb_platform_prepare");
    }
    return detail::take(api().platform_prepare());
}

class Movement {
public:
    Movement() = default;
    explicit Movement(gdb_movement* raw) : handle_(raw, api().movement_free) {}

    bool valid() const { return handle_ != nullptr; }
    bool finished() const { return api().movement_is_finished(handle_.get()); }
    void cancel() { detail::check(api().movement_cancel(handle_.get())); }
    void retarget(int32_t x, int32_t y) {
        detail::check(api().movement_retarget(handle_.get(), x, y));
    }
    void wait(uint32_t timeout_ms = 10000) {
        detail::check(api().movement_wait(handle_.get(), timeout_ms));
    }

private:
    detail::MovementHandle handle_{nullptr, api().movement_free};
};

class Mouse {
public:
    explicit Mouse(gdb_device* device) : device_(device) {}

    void move_by(int32_t dx, int32_t dy, uint32_t duration_ms = 0,
                 const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        detail::check(api().mouse_move_by(device_, dx, dy, duration_ms, &raw));
    }
    // One relative move of any size: split at the +/-1024 report limit and sent
    // with no barrier, so flush with Device::barrier() where it matters.
    void move_raw(int32_t dx, int32_t dy) {
        detail::check(api().move_raw(device_, dx, dy));
    }
    void move_to(int32_t x, int32_t y, uint32_t duration_ms = 0,
                 const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        detail::check(api().mouse_move_to(device_, x, y, duration_ms, &raw));
    }
    // The same move clamped into `screen` instead of the primary monitor: a game
    // on a second screen or inside a window is not reachable from the primary's
    // rectangle. A cursor on another monitor is not an error.
    void move_to(int32_t x, int32_t y, const Screen& screen, uint32_t duration_ms = 0,
                 const Profile* profile = nullptr) {
        detail::require_screen_surface();
        auto raw = detail::profile_or_default(profile);
        detail::check(api().mouse_move_to_on(device_, x, y, screen.raw(), duration_ms,
                                             &raw));
    }
    void move_path(const std::vector<Point>& points, uint32_t duration_ms = 0,
                   const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        auto data = to_points(points);
        detail::check(api().mouse_move_path(device_, data.data(), data.size(),
                                            duration_ms, &raw));
    }
    void move_path(const std::vector<Point>& points, const Screen& screen,
                   uint32_t duration_ms = 0, const Profile* profile = nullptr) {
        detail::require_screen_surface();
        auto raw = detail::profile_or_default(profile);
        auto data = to_points(points);
        detail::check(api().mouse_move_path_on(device_, data.data(), data.size(),
                                               screen.raw(), duration_ms, &raw));
    }
    Movement start_move_to(int32_t x, int32_t y, uint32_t duration_ms = 0,
                           const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        return Movement(api().mouse_start_move_to(device_, x, y, duration_ms, &raw));
    }
    Movement start_move_to(int32_t x, int32_t y, const Screen& screen,
                           uint32_t duration_ms = 0, const Profile* profile = nullptr) {
        detail::require_screen_surface();
        auto raw = detail::profile_or_default(profile);
        return Movement(
            api().mouse_start_move_to_on(device_, x, y, screen.raw(), duration_ms, &raw));
    }
    Movement start_move_by(int32_t dx, int32_t dy, uint32_t duration_ms = 0,
                           const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        return Movement(api().mouse_start_move_by(device_, dx, dy, duration_ms, &raw));
    }
    Movement start_move_path(const std::vector<Point>& points,
                             uint32_t duration_ms = 0,
                             const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        auto data = to_points(points);
        return Movement(api().mouse_start_move_path(device_, data.data(),
                                                    data.size(), duration_ms, &raw));
    }
    void button(Button button, bool down) {
        detail::check(api().mouse_button(device_, static_cast<uint8_t>(button), down));
    }
    void click(Button button = Button::Left, uint8_t count = 1) {
        detail::check(api().mouse_click(device_, static_cast<uint8_t>(button), count));
    }
    // A click with the press length the caller asked for: hold_ms down, gap_ms
    // after the release, jitter_ms of spread on each (0 = exact). A zero
    // hold_ms or gap_ms draws that half the way click() does.
    void click_timed(Button button = Button::Left, uint8_t count = 1,
                     uint32_t hold_ms = 0, uint32_t gap_ms = 0,
                     uint32_t jitter_ms = 0) {
        detail::check(api().mouse_click_timed(device_, static_cast<uint8_t>(button),
                                              count, hold_ms, gap_ms, jitter_ms));
    }
    // Aim and click as one call: the move clamped into `screen`, then the click.
    void click_at(int32_t x, int32_t y, const Screen& screen,
                  Button button = Button::Left, uint8_t count = 1,
                  uint32_t hold_ms = 0, uint32_t gap_ms = 0,
                  uint32_t jitter_ms = 0) {
        detail::check(api().mouse_click_at_on(device_, x, y, screen.raw(),
                                              static_cast<uint8_t>(button), count,
                                              hold_ms, gap_ms, jitter_ms));
    }
    void double_click(Button button = Button::Left) {
        detail::check(api().mouse_double_click(device_, static_cast<uint8_t>(button)));
    }
    void wheel(int32_t vertical, int32_t horizontal = 0) {
        detail::check(api().mouse_wheel(device_, vertical, horizontal));
    }
    void drag(Button button, int32_t x, int32_t y, uint32_t duration_ms = 0,
              const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        detail::check(api().mouse_drag(device_, static_cast<uint8_t>(button), x, y,
                                       duration_ms, &raw));
    }

private:
    static std::vector<gdb_point> to_points(const std::vector<Point>& points) {
        std::vector<gdb_point> data;
        data.reserve(points.size());
        for (const auto& point : points) {
            data.push_back(gdb_point{point.x, point.y});
        }
        return data;
    }

    gdb_device* device_;
};

class Keyboard {
public:
    explicit Keyboard(gdb_device* device) : device_(device) {}

    void key_down(uint16_t page, uint16_t usage) {
        detail::check(api().key_down(device_, page, usage));
    }
    void key_up(uint16_t page, uint16_t usage) {
        detail::check(api().key_up(device_, page, usage));
    }
    void press(uint16_t page, uint16_t usage) {
        detail::check(api().key_press(device_, page, usage));
    }
    void long_press(uint16_t page, uint16_t usage, uint32_t hold_ms) {
        detail::check(api().key_long_press(device_, page, usage, hold_ms));
    }
    void chord(const std::vector<uint16_t>& usages, uint32_t hold_ms = 60) {
        detail::check(api().keys_chord(device_, usages.data(), usages.size(), hold_ms));
    }
    void sequence(const std::vector<KeyStep>& steps) {
        std::vector<gdb_key_step> raw;
        raw.reserve(steps.size());
        for (const auto& step : steps) {
            raw.push_back(gdb_key_step{step.page, step.usage, step.hold_ms, step.gap_ms});
        }
        detail::check(api().keys_sequence(device_, raw.data(), raw.size()));
    }
    void type_text(const std::string& text, uint32_t interval_ms = 45,
                   uint32_t hold_ms = 18, uint32_t jitter_ms = 30) {
        detail::check(api().type_text(device_, text.c_str(), interval_ms, hold_ms,
                                      jitter_ms));
    }
    void release_all() { detail::check(api().release_all(device_)); }

private:
    gdb_device* device_;
};

class Scheduler;

// A client of a broker process: the device belongs to one program, and this is
// how every other program drives it over \\.\pipe\gdbox-<name>.
//
//     auto remote = gdb::Remote::attach();          // "default"
//     std::cout << remote.status_json();
//     remote.move_raw(400, -120);
//     remote.barrier();
//
// The JSON accessors return exactly what Device's return, so one parser serves
// both. What an attached handle cannot do is what needs the collection itself:
// absolute moves, drags, typing, chords, flashing and the bootloader.
class Remote {
public:
    // NULL and "" both mean the default pipe.
    static Remote attach(const std::string& name = std::string()) {
        detail::require_remote_surface();
        gdb_remote* raw = nullptr;
        detail::check(api().attach(name.empty() ? nullptr : name.c_str(), &raw));
        if (raw == nullptr) {
            throw Error(ErrorCode::Io, "attach returned GDB_OK without a handle");
        }
        return Remote(raw);
    }

    // Starts a broker when none is listening (GDBOX_BROKER_EXE names the
    // executable); attach afterwards.
    static void spawn(const std::string& name = std::string()) {
        detail::require_remote_surface();
        detail::check(api().remote_spawn(name.empty() ? nullptr : name.c_str()));
    }

    Remote(Remote&&) noexcept = default;
    Remote& operator=(Remote&&) noexcept = default;
    Remote(const Remote&) = delete;
    Remote& operator=(const Remote&) = delete;

    bool valid() const { return handle_ != nullptr; }
    gdb_remote* raw() const { return handle_.get(); }

    std::string info_json() const { return detail::take(api().remote_info(raw())); }
    std::string status_json() const { return detail::take(api().remote_status(raw())); }
    std::string status_ext_json() const { return detail::take(api().remote_status_ext(raw())); }
    std::string sessions_json() const { return detail::take(api().remote_sessions(raw())); }

    void key(uint16_t page, uint16_t usage, bool down) {
        detail::check(api().remote_key(raw(), page, usage, down));
    }
    // One MOUSE_MOVE report; the deltas must fit the +/-1024 of the protocol.
    void mouse_move(int16_t dx, int16_t dy) {
        detail::check(api().remote_mouse_move(raw(), dx, dy));
    }
    void move_by(int32_t dx, int32_t dy, uint32_t duration_ms = 0,
                 const Profile* profile = nullptr) {
        auto raw_profile = detail::profile_or_default(profile);
        detail::check(api().remote_move_by(raw(), dx, dy, duration_ms, &raw_profile));
    }
    void move_raw(int32_t dx, int32_t dy) {
        detail::check(api().remote_move_raw(raw(), dx, dy));
    }
    // Raw HID mask: left=1, right=2, middle=4, back=8, forward=16.
    void mouse_buttons(uint8_t bits) {
        detail::check(api().remote_mouse_buttons(raw(), bits));
    }
    void wheel(int32_t vertical, int32_t horizontal = 0) {
        detail::check(api().remote_wheel(raw(), vertical, horizontal));
    }
    void barrier() { detail::check(api().remote_barrier(raw())); }
    void release_all() { detail::check(api().remote_release_all(raw())); }

    // Device::command over the pipe: same opcode, same reply shape.
    std::optional<Packet> command(uint8_t opcode,
                                  const std::vector<uint8_t>& payload = {}) {
        detail::PacketGuard guard;
        detail::check(api().remote_command(raw(), opcode,
                                           payload.empty() ? nullptr : payload.data(),
                                           payload.size(), &guard.packet));
        if (guard.packet.opcode == 0) {
            return std::nullopt;
        }
        Packet packet;
        packet.opcode = guard.packet.opcode;
        packet.flags = guard.packet.flags;
        packet.session = guard.packet.session;
        packet.sequence = guard.packet.sequence;
        if (guard.packet.payload != nullptr && guard.packet.len > 0) {
            packet.payload.assign(guard.packet.payload,
                                  guard.packet.payload + guard.packet.len);
        }
        return packet;
    }

    // Hangs up; later calls throw ErrorCode::Disconnected.
    void close() {
        if (handle_) {
            detail::check(api().remote_close(raw()));
            handle_.reset();
        }
    }

private:
    explicit Remote(gdb_remote* raw) : handle_(raw, api().remote_free) {}

    detail::RemoteHandle handle_{nullptr, api().remote_free};
};

// Presence on a timer: the same Feature 14 read, and no way to command the
// board. Use this instead of Device when another program owns the session - a
// command would consume the IN queue its replies are read from.
class Observer {
public:
    Observer() = default;

    // Reads Feature 14 once, then polls it every interval_ms on its own thread.
    static Observer start(const std::string& selector = std::string(),
                          uint32_t interval_ms = 500) {
        detail::require_presence_surface();
        gdb_observer* raw = api().observer_start(
            selector.empty() ? nullptr : selector.c_str(), interval_ms);
        if (raw == nullptr) {
            detail::throw_last_error();
        }
        return Observer(raw);
    }

    Observer(Observer&&) noexcept = default;
    Observer& operator=(Observer&&) noexcept = default;
    Observer(const Observer&) = delete;
    Observer& operator=(const Observer&) = delete;

    bool valid() const { return handle_ != nullptr; }
    gdb_observer* raw() const { return handle_.get(); }

    // The last probe, shaped exactly like Device::info_json().
    std::string info_json() const { return detail::take(api().observer_info(raw())); }

    // Connected while the last poll answered, Disconnected while it failed.
    ConnectionState state() const {
        const int32_t value = api().observer_state(raw());
        if (value < 0) {
            throw Error(ErrorCode::Invalid, "observer handle is NULL");
        }
        return static_cast<ConnectionState>(value);
    }

    // Stops the polling thread; the handle stays valid.
    void stop() { api().observer_stop(raw()); }

    void close() {
        if (handle_) {
            api().observer_stop(raw());
            handle_.reset();
        }
    }

    // Every command throws gdb::Error with ErrorCode::Unsupported and the SDK's
    // own explanation: the handle has no session and sends no frame.
    void status() const { detail::check(api().observer_status(raw())); }
    void barrier() const { detail::check(api().observer_barrier(raw())); }
    void release_all() const { detail::check(api().observer_release_all(raw())); }
    void command(uint8_t opcode, const std::vector<uint8_t>& payload = {}) const {
        detail::PacketGuard guard;
        detail::check(api().observer_command(raw(), opcode,
                                             payload.empty() ? nullptr : payload.data(),
                                             payload.size(), &guard.packet));
    }

    ~Observer() = default;

private:
    explicit Observer(gdb_observer* raw) : handle_(raw, api().observer_free) {}

    detail::ObserverHandle handle_{nullptr, api().observer_free};
};

// Arrivals and removals without polling the bus yourself.
//
//     auto watched = gdb::Watch::start([](int32_t kind, const std::string& json) {
//         // kind 0 arrived, 1 removed (a device entry), 2 bootloader ({"path"})
//     }, 250);
//     std::cout << watched.listing_json();
//
// The callback runs on the watch's own thread: it must return quickly, and it
// must not stop or close this watch - the stop joins the thread it is running on.
// A throw is swallowed and the thread keeps running. A board already attached is
// reported as an arrival by the first poll.
class Watch {
public:
    using Callback = std::function<void(int32_t kind, const std::string& json)>;

    static Watch start(Callback callback, uint32_t interval_ms = 500) {
        detail::require_watch_surface();
        auto holder = std::make_unique<Callback>(std::move(callback));
        gdb_watch* raw = api().watch_start(interval_ms, &Watch::trampoline, holder.get());
        if (raw == nullptr) {
            detail::throw_last_error();
        }
        return Watch(raw, std::move(holder));
    }

    Watch(Watch&&) noexcept = default;
    Watch& operator=(Watch&&) noexcept = default;
    Watch(const Watch&) = delete;
    Watch& operator=(const Watch&) = delete;

    bool valid() const { return handle_ != nullptr; }

    // The bus right now: {"devices":[...],"volumes":["E:\\"]}.
    std::string listing_json() const {
        return detail::take(api().watch_listing(handle_.get()));
    }

    // Stops and joins the thread; the handle stays valid for the listing.
    void stop() {
        if (handle_) {
            api().watch_stop(handle_.get());
        }
    }

    void close() {
        if (handle_) {
            api().watch_stop(handle_.get());
            handle_.reset();
        }
        callback_.reset();
    }

    ~Watch() = default;

private:
    Watch(gdb_watch* raw, std::unique_ptr<Callback> callback)
        : callback_(std::move(callback)), handle_(raw, api().watch_free) {}

    static void trampoline(void* user, int32_t kind, const char* json) {
        auto* callback = static_cast<Callback*>(user);
        if (callback == nullptr || !*callback) {
            return;
        }
        try {
            (*callback)(kind, json != nullptr ? std::string(json) : std::string());
        } catch (...) {
            // A throwing callback must not cross the ABI boundary.
        }
    }

    // Declared before the handle, so it is destroyed after it: gdb_watch_free
    // joins the thread that calls the callback.
    std::unique_ptr<Callback> callback_;
    detail::WatchHandle handle_{nullptr, api().watch_free};
};

class Device {
public:
    Device() = default;

    static Device open(const std::string& selector = std::string(),
                       uint32_t timeout_ms = 1000, bool auto_reconnect = true,
                       bool heartbeat = true) {
        return Device(api().open(selector.empty() ? nullptr : selector.c_str(),
                                 timeout_ms, auto_reconnect, heartbeat));
    }
    static Device open_mock(uint32_t timeout_ms = 1000, bool auto_reconnect = true,
                            bool heartbeat = true) {
        return Device(api().open_mock(timeout_ms, auto_reconnect, heartbeat));
    }
    /// The same board in a non-exclusive mode: a reply another program took is
    /// one Busy naming the command, the session is never reopened behind the
    /// caller's back, and stolen_frames() counts what was not ours.
    static Device open_shared(const std::string& selector = std::string(),
                              uint32_t timeout_ms = 1000, bool auto_reconnect = true,
                              bool heartbeat = true) {
        return Device(api().open_shared(selector.empty() ? nullptr : selector.c_str(),
                                        timeout_ms, auto_reconnect, heartbeat));
    }
    static Device open_mock_shared(uint32_t timeout_ms = 1000,
                                   bool auto_reconnect = true, bool heartbeat = true) {
        return Device(api().open_mock_shared(timeout_ms, auto_reconnect, heartbeat));
    }
    static std::string list_json() { return detail::take(api().device_list()); }

    Device(Device&&) noexcept = default;
    Device& operator=(Device&&) noexcept = default;
    Device(const Device&) = delete;
    Device& operator=(const Device&) = delete;

    bool valid() const { return handle_ != nullptr; }
    gdb_device* raw() const { return handle_.get(); }

    std::string info_json() const { return detail::take(api().device_info(raw())); }
    std::string status_json() const { return detail::take(api().status(raw())); }
    std::string status_ext_json() const { return detail::take(api().status_ext(raw())); }
    std::string sessions_json() const { return detail::take(api().sessions(raw())); }

    /// Replies the reader looked past because they answered another session or
    /// sequence: the counter that tells a collision from a slow board.
    uint64_t stolen_frames() const { return api().stolen_frames(raw()); }

    /// The session's counters and latency percentiles. Every atomic is read
    /// once, so two fields may be one report apart.
    Metrics metrics() const {
        detail::require_metrics_surface();
        gdb_metrics counters{};
        detail::check(api().metrics_get(raw(), &counters));
        return Metrics::from_raw(counters);
    }
    /// Zeroes every counter and every latency bucket.
    void reset_metrics() {
        detail::require_metrics_surface();
        detail::check(api().metrics_reset(raw()));
    }
    /// The absolute moves the anchor rectangle pulled back, and the last one's
    /// coordinates: the numbers that say a requested pixel was never reached.
    /// Meaningful when clamped_total is not zero.
    Clamps clamps() const {
        detail::require_clamps_surface();
        gdb_clamps counters{};
        detail::check(api().metrics_clamps(raw(), &counters));
        return Clamps::from_raw(counters);
    }

    ConnectionState state() const {
        const int32_t value = api().device_state(raw());
        if (value < 0) {
            throw Error(ErrorCode::Invalid, "device handle is NULL");
        }
        return static_cast<ConnectionState>(value);
    }

    void close() {
        if (handle_) {
            detail::check(api().close(raw()));
            handle_.reset();
        }
    }
    void reconnect() { detail::check(api().reconnect(raw())); }
    void set_auto_reconnect(bool enabled) {
        detail::check(api().set_auto_reconnect(raw(), enabled));
    }
    void release_all() { detail::check(api().release_all(raw())); }
    void barrier() { detail::check(api().barrier(raw())); }
    void bootloader() { detail::check(api().bootloader(raw())); }

    // The raw escape hatch: one protocol command by opcode and payload. Same
    // session and error codes as the typed calls, but the SDK's bookkeeping
    // (held buttons, the mock cursor) is not updated. Input commands have no
    // reply, so the result is empty for them.
    std::optional<Packet> command(uint8_t opcode,
                                  const std::vector<uint8_t>& payload = {}) {
        detail::PacketGuard guard;
        detail::check(api().command(raw(), opcode,
                                    payload.empty() ? nullptr : payload.data(),
                                    payload.size(), &guard.packet));
        if (guard.packet.opcode == 0) {
            return std::nullopt;
        }
        Packet packet;
        packet.opcode = guard.packet.opcode;
        packet.flags = guard.packet.flags;
        packet.session = guard.packet.session;
        packet.sequence = guard.packet.sequence;
        if (guard.packet.payload != nullptr && guard.packet.len > 0) {
            packet.payload.assign(guard.packet.payload,
                                  guard.packet.payload + guard.packet.len);
        }
        return packet;
    }

    void set_event_handler(std::function<void(int32_t, int32_t, std::string)> handler);
    void clear_event_handler();

    Mouse mouse() const { return Mouse(raw()); }
    Keyboard keyboard() const { return Keyboard(raw()); }
    std::unique_ptr<Scheduler> scheduler() const;

    ~Device() = default;

private:
    explicit Device(gdb_device* raw) : handle_(raw, api().device_free) {
        if (raw == nullptr) {
            throw Error(ErrorCode::Io, detail::last_error_message());
        }
    }

    detail::DeviceHandle handle_{nullptr, api().device_free};
    std::shared_ptr<std::function<void(int32_t, int32_t, std::string)>> events_;
};

class Scheduler {
public:
    explicit Scheduler(gdb_device* device) : handle_(api().scheduler_new(device), api().scheduler_free) {
        if (!handle_) {
            throw Error(ErrorCode::Io, detail::last_error_message());
        }
    }

    uint64_t submit(uint32_t delay_ms, int32_t priority, std::function<int32_t()> callback) {
        auto stored = std::make_shared<std::function<int32_t()>>(std::move(callback));
        const uint64_t id = api().scheduler_submit(
            handle_.get(), delay_ms, priority, &Scheduler::trampoline,
            stored.get());
        if (id == 0) {
            throw Error(ErrorCode::Io, detail::last_error_message());
        }
        callbacks_[id] = stored;
        return id;
    }

    uint64_t every(uint32_t interval_ms, int32_t priority,
                   std::function<int32_t()> callback) {
        auto stored = std::make_shared<std::function<int32_t()>>(std::move(callback));
        const uint64_t id = api().scheduler_every(
            handle_.get(), interval_ms, priority, &Scheduler::trampoline,
            stored.get());
        if (id == 0) {
            throw Error(ErrorCode::Io, detail::last_error_message());
        }
        callbacks_[id] = stored;
        return id;
    }

    void cancel(uint64_t id) { detail::check(api().scheduler_cancel(handle_.get(), id)); }
    void pause() { detail::check(api().scheduler_pause(handle_.get())); }
    void resume() { detail::check(api().scheduler_resume(handle_.get())); }
    void wait_idle(uint32_t timeout_ms = 5000) {
        detail::check(api().scheduler_wait_idle(handle_.get(), timeout_ms));
    }
    std::string take_errors_json() {
        return detail::take(api().scheduler_take_errors(handle_.get()));
    }

    ~Scheduler() = default;

private:
    static int32_t trampoline(void* user) {
        auto* callback = static_cast<std::function<int32_t()>*>(user);
        try {
            return (*callback)();
        } catch (...) {
            return GDB_IO;
        }
    }

    detail::SchedulerHandle handle_{nullptr, api().scheduler_free};
    std::map<uint64_t, std::shared_ptr<std::function<int32_t()>>> callbacks_;
};

inline void Device::set_event_handler(
    std::function<void(int32_t, int32_t, std::string)> handler) {
    events_ = std::make_shared<std::function<void(int32_t, int32_t, std::string)>>(
        std::move(handler));
    detail::check(api().set_event_handler(
        raw(),
        [](void* user, int32_t kind, int32_t code, const char* message) {
            auto* callback =
                static_cast<std::function<void(int32_t, int32_t, std::string)>*>(user);
            try {
                (*callback)(kind, code, message != nullptr ? std::string(message)
                                                           : std::string());
            } catch (...) {
            }
        },
        events_.get()));
}

inline void Device::clear_event_handler() {
    detail::check(api().set_event_handler(raw(), nullptr, nullptr));
    events_.reset();
}

inline std::unique_ptr<Scheduler> Device::scheduler() const {
    return std::make_unique<Scheduler>(raw());
}

inline void set_log_level(int32_t level) {
    detail::check(api().set_log_level(static_cast<uint8_t>(level)));
}

// Routes SDK log lines to `sink`; a null sink turns it off again. The sink is
// process-wide and stays alive until it is replaced or cleared. Logging follows
// set_log_level(): nothing arrives while the level is off.
inline void set_log_sink(detail::LogSink sink) {
    if (!sink) {
        detail::check(api().set_log_sink(nullptr, nullptr));
        detail::log_sink_slot().reset();
        return;
    }
    auto holder = std::make_shared<detail::LogSink>(std::move(sink));
    // Stored before the ABI can call it, dropped only after it is unregistered.
    detail::log_sink_slot() = holder;
    detail::check(api().set_log_sink(&detail::log_trampoline, holder.get()));
}

inline std::string version() { return std::string(api().version()); }
inline uint32_t abi_version() { return api().abi_version(); }
// The header is the truth for the minor: gdb_attach arrived with 1.1.
inline uint32_t abi_minor_version() { return GDB_ABI_VERSION_MINOR; }

}  // namespace gdb
