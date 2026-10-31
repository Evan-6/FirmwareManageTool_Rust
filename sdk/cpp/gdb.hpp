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
    decltype(&gdb_mouse_move_by) mouse_move_by = nullptr;
    decltype(&gdb_mouse_move_to) mouse_move_to = nullptr;
    decltype(&gdb_mouse_move_path) mouse_move_path = nullptr;
    decltype(&gdb_mouse_button) mouse_button = nullptr;
    decltype(&gdb_mouse_click) mouse_click = nullptr;
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
    decltype(&gdb_scheduler_every) scheduler_every = nullptr;
    decltype(&gdb_scheduler_cancel) scheduler_cancel = nullptr;
    decltype(&gdb_scheduler_pause) scheduler_pause = nullptr;
    decltype(&gdb_scheduler_resume) scheduler_resume = nullptr;
    decltype(&gdb_scheduler_wait_idle) scheduler_wait_idle = nullptr;
    decltype(&gdb_scheduler_take_errors) scheduler_take_errors = nullptr;
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

using DeviceHandle = std::unique_ptr<gdb_device, decltype(&gdb_device_free)>;
using MovementHandle = std::unique_ptr<gdb_movement, decltype(&gdb_movement_free)>;
using SchedulerHandle = std::unique_ptr<gdb_scheduler, decltype(&gdb_scheduler_free)>;

inline gdb_profile profile_or_default(const Profile* profile) {
    if (profile != nullptr) {
        return profile->raw();
    }
    return api().profile_default();
}

}  // namespace detail

using detail::api;
using detail::check;

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
    void move_to(int32_t x, int32_t y, uint32_t duration_ms = 0,
                 const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        detail::check(api().mouse_move_to(device_, x, y, duration_ms, &raw));
    }
    void move_path(const std::vector<Point>& points, uint32_t duration_ms = 0,
                   const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        auto data = to_points(points);
        detail::check(api().mouse_move_path(device_, data.data(), data.size(),
                                            duration_ms, &raw));
    }
    Movement start_move_to(int32_t x, int32_t y, uint32_t duration_ms = 0,
                           const Profile* profile = nullptr) {
        auto raw = detail::profile_or_default(profile);
        return Movement(api().mouse_start_move_to(device_, x, y, duration_ms, &raw));
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

inline std::string version() { return std::string(api().version()); }
inline uint32_t abi_version() { return api().abi_version(); }

}  // namespace gdb
