# GDBOX API reference

`include/gdb.h` is the contract; every other layer names the same operations
differently and none of them adds behaviour. Rust is `gdb`, C++ is `gdb::`,
Python is module `gdb`, Go is package `gdb`. Start with [README.md](README.md)
for the handle you want and a working call.

## Symbol map

| Concept | Rust | C++ | Python | Go |
| --- | --- | --- | --- | --- |
| open by selector | `Manager::open(sel, opts)` | `Device::open(sel)` | `Device.open(sel)` | `Open(sel, …)` |
| open the mock | `Manager::open_mock` | `Device::open_mock` | `Device.open_mock` | `OpenMock` |
| open for a board others drive too | `Device::open_shared(sel, opts)` | `Device::open_shared(sel)` | `Device.open(sel, shared=True)` | `OpenShared(sel, …)` |
| open the shared mock | `Device::open_mock_shared(opts)` | `Device::open_mock_shared()` | `Device.open_mock(shared=True)` | `OpenMockShared(…)` |
| device list | `Manager::list` | `list_json()` | `list_devices()` | `ListJSON()` |
| replies that belonged to somebody else | `device.stolen_frames()` | `stolen_frames()` | `.stolen_frames` | `StolenFrames()` |
| the device's latest failure | `device.last_error()` | `last_error()` | `.last_error()` | `LastError()` |
| info | `device.info()` | `info_json()` | `.info` | `InfoJSON()` |
| status | `device.status()` | `status_json()` | `.status` | `StatusJSON()` |
| status ext | `device.status_ext()` | `status_ext_json()` | `.status_ext` | `StatusExtJSON()` |
| sessions | `device.sessions()` | `sessions_json()` | `.sessions` | `SessionsJSON()` |
| state | `device.state()` | `state()` | `.state` | `State()` |
| reconnect | `device.reconnect()` | `reconnect()` | `.reconnect()` | `Reconnect()` |
| release all | `device.release_all()` | `release_all()` | `.release_all()` | `ReleaseAll()` |
| barrier | `device.barrier()` | `barrier()` | `.barrier()` | `Barrier()` |
| bootloader | `device.bootloader()` | `bootloader()` | `.bootloader()` | `Bootloader()` |
| events | `set_event_handler(Arc<dyn EventHandler>)` | `set_event_handler(fn)` | `.set_event_handler(fn)` | `SetEventHandler(fn)` |
| log level | `log::set_level(Level)` | `set_log_level(int)` | `set_log_level(int)` | `SetLogLevel(uint8)` |
| log sink | `log::set_sink(fn)` | `set_log_sink(fn)` | `set_log_sink(fn)` | `SetLogSink(fn)` |
| relative move | `move_by(dx, dy, opts)` | `move_by(dx, dy, ms)` | `.move_by(dx, dy, ms)` | `MoveBy(dx, dy, ms, p)` |
| raw relative move (no barrier) | `move_raw(dx, dy)` | `move_raw(dx, dy)` | `.move_raw(dx, dy)` | `MoveRaw(dx, dy)` |
| absolute move | `move_to(x, y, opts)` | `move_to(x, y, ms)` | `.move_to(x, y, ms)` | `MoveTo(x, y, ms, p)` |
| absolute move on a screen | `MotionOptions::with_screen` | `move_to(x, y, Screen::monitor(i))` | `.move_to(…, screen=Screen.monitor(1))` | `MoveToOn(x, y, ScreenMonitor(1), …)` |
| absolute move in pursuit mode | `MotionOptions::pursuit()` | `move_to_on_pursuit(x, y, screen)` | `.move_to_on_pursuit(x, y, screen)` | `MoveToOnPursuit(x, y, screen)` |
| absolute move from the monitor holding a point | `Screen::Containing(x, y)` | `Screen::containing(x, y)` | `Screen.containing(x, y)` | `ScreenContaining(x, y)` |
| monitor layout | `platform::monitors()` | `monitors_json()` | `monitors()` | `Monitors()` |
| path | `move_path(&[(x, y)], opts)` | `move_path({{…}}, ms)` | `.move_path([(x, y)], ms)` | `MovePath([][2]int32, ms, p)` |
| async move | `start_move_to/by/path` | `start_move_to/by/path` | `.start_move_to/by/path` | `StartMoveTo/By` |
| button | `button(Button, down)` | `button(Button, down)` | `.button(btn, down)` | `Button(btn, down)` |
| click | `click(Button, count)` | `click(Button, n)` | `.click(btn, n)` | `Click(btn, n)` |
| double click | `double_click(Button)` | `double_click(Button)` | `.double_click(btn)` | `DoubleClick(btn)` |
| wheel | `wheel(v, h)` | `wheel(v, h)` | `.wheel(v, h)` | `Wheel(v, h)` |
| drag | `drag(Button, x, y, opts)` | `drag(Button, x, y, ms)` | `.drag(btn, x, y, ms)` | `Drag(btn, x, y, ms, p)` |
| key down/up | `key_down/up(page, usage)` | same | same | `KeyDown/KeyUp` |
| press / long press | `press`, `long_press(page, usage, ms)` | same | same | `Press`, `LongPress` |
| chord | `chord(&[usage], hold_ms)` | `chord({…}, hold_ms)` | `.chord([usage], hold_ms)` | `Chord([]uint16, ms)` |
| sequence | `sequence(&[KeyStep])` | `sequence({…})` | `.sequence([KeyStep])` | `Sequence([]KeyStep)` |
| text | `type_text(&TypeTextConfig)` | `type_text(s, interval, hold, jitter)` | `.type_text(s, interval, hold, jitter)` | `TypeText(s, interval, hold, jitter)` |
| raw command passthrough | `command(&Command)` | `command(opcode, payload)` | `.command(opcode, payload)` | `Command(opcode, payload)` |
| scheduler | `device.scheduler()` | `device.scheduler()` | `.scheduler()` | `Scheduler()` |
| submit / every | `submit(delay, Priority, f)` | `submit(ms, prio, f)` | `.submit(ms, prio, f)` | `Submit(ms, prio, f)` |
| absolute due / elapsed | `submit_at(due)`, `elapsed_ms()` | — | `.submit_at(ms, prio, f)`, `.elapsed_ms()` | `SubmitAt(ms, prio, f)`, `ElapsedMs()` |
| cancel / pause / resume | `cancel(id)`, `pause()`, `resume()` | same | same | same |
| wait idle | `wait_idle(dur)` | `wait_idle(ms)` | `.wait_idle(ms)` | `WaitIdle(ms)` |
| take errors | `take_errors()` | `take_errors_json()` | `.take_errors()` | `TakeErrors()` |
| counters | `device.metrics()` | `metrics()` | `.metrics()` | `Metrics()` |
| clamped moves | `MetricsSnapshot::last_clamp` | `clamps()` | `.clamps()` | `Clamps()` |
| counter reset | `reset_metrics()` | `reset_metrics()` | `.reset_metrics()` | `ResetMetrics()` |
| presence, no session | `probe::read_selector(sel)` | `probe(sel)` | `probe(sel)` | `Probe(sel)` |
| presence for the whole bus | `probe::all()` | `probe_all()` | `probe_all()` | `ProbeAll()` |
| presence on a timer | `Observer::start` | `Observer::start` | `Observer(...)` | `Observe(sel, ms)` |
| watch the bus | `Watch::start` | `Watch::start` | `watch(fn)` | `NewWatch(ms, fn)` |
| attach to a broker | `broker::Client::connect(name)` | `Remote::attach(name)` | `Remote.attach(name)` | `Attach(name)` |
| start a broker if none is listening | `Client::connect_or_spawn(name, exe)` | `Remote::spawn(name)` | `Remote.spawn(name)` | `Spawn(name)` |
| record / replay | `replay::Recorder`, `Recording` | — | — | — |

## Device

```c
typedef struct gdb_device gdb_device;

gdb_device *gdb_open(const char *selector, uint32_t timeout_ms,
                     bool auto_reconnect, bool heartbeat);
gdb_device *gdb_open_mock(uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);
gdb_device *gdb_open_shared(const char *selector, uint32_t timeout_ms,
                            bool auto_reconnect, bool heartbeat);
gdb_device *gdb_open_mock_shared(uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);

void    gdb_device_free(gdb_device *device);
int32_t gdb_close(gdb_device *device);
int32_t gdb_reconnect(gdb_device *device);
int32_t gdb_set_auto_reconnect(gdb_device *device, bool enabled);
int32_t gdb_device_state(const gdb_device *device);      /* -1 on a NULL handle */
char   *gdb_device_list(void);                            /* JSON array */
```

`selector` is a USB serial or a HID path — exact first, then a substring of the
serial; `NULL` takes the first attached board. `timeout_ms` is the per-command
reply deadline (default 1000). An open returns `NULL` on failure with
`gdb_last_error` set.

`gdb_close` releases the session, stops the heartbeat and clears held input; the
handle is then unusable. `gdb_device_free` releases the handle and closes it if
that has not happened.

Rust options, which the four booleans/arguments spell out:

```rust
pub struct DeviceOptions { pub auto_reconnect: bool, pub timeout_ms: u32, pub heartbeat: bool }
// default: true, 1000, true

DeviceInfo { serial, product, board, firmware, lease_ms, poll_ms,
             capabilities, device_id: [u8; 8], build: u32 }
DeviceEntry { path, serial, product, manufacturer, vid, pid }
```

## Sharing a board

Ownership is fixed when the device is opened, because it decides whether the SDK
may reopen the session at all; it is not a `DeviceOptions` field.

| | `Exclusive` | `Shared` |
| --- | --- | --- |
| a reply that never arrives | `Timeout` | `Busy`, message names the command: `STATUS got no reply; another process may have taken it (no reply to opcode 6 within 120 ms)` |
| the SDK reopens the session on its own | yes, for a fault it believes it can fix | never — `reconnect()` is the host's call |
| heartbeat whose reply was taken | reconnects, as above | retried on the next tick, no CLOSE |
| a frame for another session | dropped | counted in `stolen_frames`, offered to the stray hook |
| sequence numbers, lease, counters | unchanged | unchanged |

```rust
pub enum Ownership { Exclusive, Shared }   // default: Exclusive

impl Device {
    pub fn open_shared(selector: Option<&str>, options: DeviceOptions) -> Result<Device>;
    pub fn open_mock_shared(options: DeviceOptions) -> Result<(Device, MockHandle)>;
    pub fn over_transport(transport: Box<dyn Transport>, target: Target,
                          options: DeviceOptions, ownership: Ownership) -> Result<Device>;
    pub fn ownership(&self) -> Ownership;
    pub fn set_ownership(&self, ownership: Ownership);          // for commands not yet sent
    pub fn stolen_frames(&self) -> u64;
    pub fn set_stray_hook(&self, hook: Arc<dyn Fn(&[u8]) + Send + Sync>);
    pub fn clear_stray_hook(&self);
}

uint64_t gdb_stolen_frames(const gdb_device *device);   // C ABI, 0 on a NULL handle
```

`stolen_frames` counts frames the reply loop looked past because they answered
another session or sequence — a subset of `retries`, which also counts reads that
returned nothing. `over_transport` and the stray hook are Rust-only. The hook runs
on the thread reading the queue with that device's session busy, so it must not
call back into the same `Device`; only a foreign session reaches it.

## Queries

```c
char *gdb_device_info(const gdb_device *device);
char *gdb_status(const gdb_device *device);
char *gdb_status_ext(const gdb_device *device);
char *gdb_sessions(const gdb_device *device);            /* JSON array */
int32_t gdb_release_all(gdb_device *device);
int32_t gdb_barrier(gdb_device *device);
int32_t gdb_bootloader(gdb_device *device);
```

Every returned string is UTF-8 JSON owned by the caller: `gdb_string_free`.

```json
{"serial":"MOCK","product":"GDBOX mock","board":2,"firmware":"3.2.0",
 "leaseMs":2000,"pollMs":1,"capabilities":15,"deviceId":[66,80,51,48,52,66,56,51],"build":229638145}
```

```json
{"received":143,"completed":143,"pending":0,"leaseMs":2000,"rx":0,"tx":0,
 "hid":0,"failsafe":0,
 "state":{"modifiers":0,"keys":[…28 bytes…],"consumer":0,"buttons":0}}
```

```json
{"uptimeMs":1000,"lateMaxUs":0,"outDepth":0,"rxDepth":0,"txDepth":0,"sessions":1,
 "lastFault":0,"lastFaultAgeMs":null,"resetReason":3,"bootCount":1,"build":229638145}
```

`SESSIONS` → array of `{ "id": 1234, "leaseMs": 2000, "held": 0, "flags": 0 }`.

## Mouse

```c
typedef struct gdb_point { int32_t x; int32_t y; } gdb_point;

typedef struct gdb_profile {
    int32_t ease;        /* gdb_ease: 0 jerk, 1 in-out, 2 linear, 3 in, 4 out, 5 bezier */
    double  bezier[4];   /* x1, y1, x2, y2 — used when ease == 5 */
    double  curve;       /* 0 straight, 1 bow, 2 exaggerated arc */
    double  jitter_px;
    double  overshoot_pct;
    uint32_t settle_ms;
    int32_t min_step_px;
} gdb_profile;

gdb_profile gdb_profile_default(void);   /* the SDK default; a NULL profile means this */

int32_t gdb_mouse_move_by(gdb_device *device, int32_t dx, int32_t dy,
                          uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_move_raw(gdb_device *device, int32_t dx, int32_t dy);
int32_t gdb_mouse_move_to(gdb_device *device, int32_t x, int32_t y,
                          uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_mouse_move_path(gdb_device *device, const gdb_point *points, size_t count,
                            uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_mouse_button(gdb_device *device, uint8_t button, bool down);
int32_t gdb_mouse_click(gdb_device *device, uint8_t button, uint8_t count);
int32_t gdb_mouse_click_timed(gdb_device *device, uint8_t button, uint8_t count,
                              uint32_t hold_ms, uint32_t gap_ms, uint32_t jitter_ms);
int32_t gdb_mouse_double_click(gdb_device *device, uint8_t button);
int32_t gdb_mouse_wheel(gdb_device *device, int32_t vertical, int32_t horizontal);
int32_t gdb_mouse_drag(gdb_device *device, uint8_t button, int32_t x, int32_t y,
                       uint32_t duration_ms, const gdb_profile *profile);
```

`button` is `gdb_button`: left 1, right 2, middle 3, back 4, forward 5.
`duration_ms == 0` picks a natural duration from the distance. `move_by` splits
at ±1024 per axis and ends with a barrier; `move_raw` splits the same way with no
barrier, so flush with `gdb_barrier` where a completion guarantee matters.

`MotionOptions::pursuit()` drives an absolute move as a direct chase: the pointer
is read, whatever is left is sent, and no barrier waits for each step, so a
pointer that reports quickly is followed at the device's rate. `duration_ms` and
the human profile are ignored. A movement that has finished cannot be retargeted,
so a caller following a stream of targets starts the next movement when
`Movement::is_finished()` turns true. Over the C ABI it is
`gdb_mouse_move_to_on_pursuit(device, x, y, screen)` (`mouse.move_to_on_pursuit`
in C++ and Python, `Mouse.MoveToOnPursuit` in Go): it takes the anchor and nothing
else, because the mode ignores both the duration and the profile.

Rust shapes:

```rust
pub struct MotionOptions { pub duration_ms: u32, pub profile: HumanProfile,
                           pub screen: platform::Screen, pub pursuit: bool }
impl MotionOptions {
    pub fn linear(duration_ms: u32) -> Self;
    pub fn pursuit() -> Self;
    pub fn with_screen(self, screen: platform::Screen) -> Self;
}
pub struct HumanProfile { curve, jitter_px, overshoot_pct, settle_ms, ease, min_step_px,
                          organic }
pub enum Ease { Jerk, EaseInOut, Linear, EaseIn, EaseOut, CubicBezier { x1, y1, x2, y2 } }
```

`organic` (0..=1) decides how much of the profile is re-sampled per move. At 0
the fields are used literally and two identical moves produce the same path; the
default 0.6 varies the curvature, adds the occasional overshoot on moves longer
than 350 px and jitters the settle time. `click()` and `type_text()` draw their
holds and gaps from log-normal distributions for the same reason, and
`gdb::timing::sleep` waits on a high-resolution timer so a 25 ms hold lasts
25 ms instead of rounding up to the 15.6 ms system tick.

### Screen targets

```c
typedef struct gdb_screen {
    int32_t kind;   /* 0 primary, 1 monitor (arg = index), 2 window (arg = HWND),
                       3 monitor containing a point (arg = packed x,y),
                       4 window client area (arg = HWND) */
    int64_t arg;    /* HWND is pointer-sized, so this is 64-bit even in a 32-bit host */
} gdb_screen;

/* kind 3: the high 32 bits of arg are x and the low 32 bits are y. */
static inline gdb_screen gdb_screen_containing(int32_t x, int32_t y);
/* kind 4: the area inside the frame, where a capture's coordinates start. */
static inline gdb_screen gdb_screen_window_client(int64_t hwnd);

int32_t gdb_mouse_move_to_on(gdb_device *, int32_t x, int32_t y, gdb_screen screen,
                             uint32_t duration_ms, const gdb_profile *);
int32_t gdb_mouse_move_path_on(gdb_device *, const gdb_point *, size_t count,
                               gdb_screen screen, uint32_t duration_ms, const gdb_profile *);
gdb_movement *gdb_mouse_start_move_to_on(gdb_device *, int32_t x, int32_t y,
                                         gdb_screen screen, uint32_t duration_ms,
                                         const gdb_profile *);

/* Every monitor in the order kind 1 indexes them, no handle needed:
   [{"left":0,"top":0,"right":1920,"bottom":1080,"primary":true,"dpi":96}, …] */
char *gdb_monitors_json(void);
/* One window: the frame kind 2 uses, the client area kind 4 uses, and the flags:
   {"rect":{"left","top","right","bottom"},"clientOrigin":{"x","y"},
    "clientSize":{"width","height"},"dpi":96,"visible":true,"minimized":false,
    "foreground":false} */
char *gdb_window_metrics(int64_t hwnd);
/* Per-monitor DPI awareness, raised if it can be:
   {"supported":true,"dpiAwareness":"perMonitorAware","value":2,"perMonitor":true} */
char *gdb_platform_prepare(void);
```

`gdb_mouse_move_to` and `gdb_mouse_drag` clamp to the primary monitor; the `_on`
calls take the rectangle explicitly. Coordinates stay virtual-desktop physical
pixels — the target says which rectangle the point is clamped into, not a new
origin.

| Target | Rust | C++ | Python | Go |
| --- | --- | --- | --- | --- |
| primary | `Screen::Primary` | `Screen::primary()` | `Screen.primary` | `ScreenPrimary()` |
| monitor *i* | `Screen::Monitor(i)` | `Screen::monitor(i)` | `Screen.monitor(i)` | `ScreenMonitor(i)` |
| window | `Screen::Window(hwnd)` | `Screen::window(hwnd)` | `Screen.window(hwnd)` | `ScreenWindow(hwnd)` |
| window client area | `Screen::WindowClient(hwnd)` | `Screen::window_client(hwnd)` | `Screen.window_client(hwnd)` | `ScreenWindowClient(hwnd)` |
| monitor holding a point | `Screen::Containing(x, y)` | `Screen::containing(x, y)` | `Screen.containing(x, y)` | `ScreenContaining(x, y)` |
| the layout | `platform::monitors()` | `monitors_json()` | `monitors()` | `Monitors()` / `MonitorsJSON()` |
| one window | `platform::window_metrics(hwnd)` | `window_metrics_json(hwnd)` | `window_metrics(hwnd)` | `WindowMetrics(hwnd)` |
| the DPI mode | `platform::prepare()` | `prepare_platform()` | `prepare_platform()` | `PreparePlatform()` |

`Window` is `GetWindowRect`, so its rectangle includes the title bar and the
borders. `WindowClient` is `GetClientRect` plus `ClientToScreen`: the area inside
the frame, which is where a game draws and what a capture shows, so a coordinate
measured on a screenshot starts at its top-left corner. Both are clamped the same
way and report the same two anchor errors; a client area with no size or on no
monitor is `StaleWindow` with a message naming the client area.

`window_metrics` is the read a host makes before converting a capture coordinate:
the frame, `clientOrigin` and `clientSize`, the effective `dpi` of the window's
display, and `visible`/`minimized`/`foreground`. It needs no handle and sends no
packet. `prepare` raises the process to per-monitor DPI awareness (v2 where the
system allows it) and reports the mode it ended up in: `per_monitor` is what says
the coordinates are physical pixels and not scaled, which is also the mode every
`gdb_open*` puts the process in before it returns.

`Primary` follows the primary-monitor flag, which need not be index 0.
`Monitor(i)` is the `i`-th rectangle in `EnumDisplayMonitors` order — not the
display number Windows settings shows — so a caller that picks an index reads
`monitors()` first instead of guessing. `Containing(x, y)` is the anchor for a
destination the caller only knows as a coordinate: it resolves to whichever
monitor holds that point, so a move aimed at a second screen no longer lands on
the primary monitor's edge.

Three failures are told apart by code, not by message: `Unsupported` when the
platform has no display API, or the destination is on no monitor at all (the
message names the target and the union of the screens); `InvalidScreen` when a
monitor index is past the end or the layout changes under a running move;
`StaleWindow` when the window handle no longer resolves — gone, minimised, empty
or on no monitor — so the caller knows to re-read the handle or fall back to a
monitor instead of retrying a call this machine will never support. The mock
reports one 1920×1080 screen, where `Primary`, `Monitor(0)` and `Containing`
work. Relative moves ignore the target.

The trajectory duration is a floor, not a budget: the deadline moves forward every
time the pointer closes the gap, and a move whose reports stop making progress
fails after 400 ms with `Timeout` and a message carrying the remaining distance and
the time spent. A long jump paired with a short duration therefore finishes; a
stuck pointer reports where it stopped. A destination outside the target
rectangle but on a screen is still clamped into it, and that is counted — see
[Clamped moves](#clamped-moves) — because a converging move with the wrong anchor
otherwise looks like success.

### Async moves

```c
gdb_movement *gdb_mouse_start_move_to(gdb_device *, int32_t x, int32_t y,
                                      uint32_t duration_ms, const gdb_profile *);
gdb_movement *gdb_mouse_start_move_by(gdb_device *, int32_t dx, int32_t dy,
                                      uint32_t duration_ms, const gdb_profile *);
gdb_movement *gdb_mouse_start_move_path(gdb_device *, const gdb_point *, size_t count,
                                        uint32_t duration_ms, const gdb_profile *);
int32_t gdb_movement_cancel(gdb_movement *movement);
int32_t gdb_movement_retarget(gdb_movement *movement, int32_t x, int32_t y);
bool    gdb_movement_is_finished(const gdb_movement *movement);
int32_t gdb_movement_wait(gdb_movement *movement, uint32_t timeout_ms);
void    gdb_movement_free(gdb_movement *movement);       /* cancels */
```

`Movement` in Rust and the bindings: `cancel()`, `retarget(x, y)`, `finished()` /
`is_finished()`, `wait(timeout)` (`Timeout` leaves the movement running).
`gdb_movement_wait` consumes nothing; dropping the handle cancels the movement.

## Keyboard

```c
int32_t gdb_key_down(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_up(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_press(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_long_press(gdb_device *device, uint16_t page, uint16_t usage, uint32_t hold_ms);
int32_t gdb_keys_chord(gdb_device *device, const uint16_t *usages, size_t count, uint32_t hold_ms);
int32_t gdb_keys_sequence(gdb_device *device, const gdb_key_step *steps, size_t count);
int32_t gdb_type_text(gdb_device *device, const char *utf8, uint32_t interval_ms,
                      uint32_t hold_ms, uint32_t jitter_ms);

typedef struct gdb_key_step { uint16_t page; uint16_t usage;
                              uint32_t hold_ms; uint32_t gap_ms; } gdb_key_step;
```

`page`/`usage` are HID usage ids: page 7 for the keyboard, page 12 for consumer
keys. A usage outside the firmware catalog is `Invalid` before anything is sent.

`TypeTextConfig { text, interval_ms, hold_ms, jitter_ms, typo_rate }` (defaults
45/18/30/0.02). `interval_ms` and `hold_ms` are **medians**: each gap and hold is
drawn log-normally around them, `jitter_ms` sets the spread as a fraction of the
interval (0 = metronome), and `typo_rate` is the chance a key lands on its
neighbour and is corrected with a backspace. `\n` and `\t` map to Enter and Tab;
unsupported characters are dropped, so the stroke count is not the character
count.

### Who owns the clock

Every call has a version that does not sleep, so the caller decides the timing:

| you want | use |
|---|---|
| full manual control | `key_down` / `key_up`, `button(button, down)` — they send and return |
| a hold of your own length | `long_press(…, hold_ms)`, `chord(…, hold_ms)`, `sequence(&[KeyStep { hold_ms, gap_ms }])` |
| nothing human at all | `HumanProfile { organic: 0, .. }` / `HumanProfile::linear()`, `TypeTextConfig { jitter_ms: 0, typo_rate: 0, .. }` |
| sub-millisecond waits | `gdb::timing::sleep` / `sleep_ms` (Rust; C callers use their own sleep) |

`click()` draws its hold and gap from log-normal distributions and takes no
parameters: a constant press length is the easiest thing to detect. When an exact
value matters, `click_timed(button, count, hold_ms, gap_ms, jitter_ms)` is the
same call with the numbers spelled out: `hold_ms` is how long the button stays
down, `gap_ms` how long to wait after the release, `jitter_ms` spreads each by up
to that much and `0` is exact. A zero `hold_ms` or `gap_ms` means "draw that half
the way `click()` does", so one parameter can be fixed while the other stays
human. A press is never left down: if the call fails while the button is down,
the release still goes out.

| | Call |
| --- | --- |
| Rust | `mouse.click_timed(Button, count, hold_ms, gap_ms, jitter_ms)` |
| C ABI | `gdb_mouse_click_timed(device, button, count, hold_ms, gap_ms, jitter_ms)` |
| C++ | `mouse.click_timed(button, count, hold_ms, gap_ms, jitter_ms)` |
| Python | `mouse.click_timed(button, count, hold_ms=…, gap_ms=…, jitter_ms=…)` |
| Go | `mouse.ClickTimed(button, count, holdMs, gapMs, jitterMs)` |

`click_at` is the pair a task repeats — aim, then press and release — as one
call, so a caller crosses the language boundary once: it moves to `(x, y)`
clamped into the screen target and then clicks there. Rust
`mouse.click_at(x, y, Screen, Button, count, …)`, C
`gdb_mouse_click_at_on(device, x, y, screen, button, count, hold_ms, gap_ms,
jitter_ms)`, C++ `mouse.click_at`, Python `mouse.click_at`, Go
`mouse.ClickAtOn`. A click count of zero is `Invalid` in every layer.

`MotionOptions.duration_ms` is the caller's number whenever it is non-zero; only
`0` asks the SDK to pick a natural (Fitts-shaped) duration. `gdb_profile` has no
`organic` field: `jitterPx == 0` maps to `organic == 0`, which keeps a path
repeatable.

The C surface has no `typo_rate` parameter either: `gdb_type_text` maps
`jitter_ms == 0` to no slips (a caller that asked for a metronome) and any other
value to the default 2%, so every binding agrees with `TypeTextConfig::paced`.

## Scheduler

```c
typedef int32_t (*gdb_task_fn)(void *user);   /* 0 = ok, otherwise a gdb_error */

gdb_scheduler *gdb_scheduler_new(gdb_device *device);
void     gdb_scheduler_free(gdb_scheduler *scheduler);
uint64_t gdb_scheduler_submit(gdb_scheduler *, uint32_t delay_ms, int32_t priority,
                              gdb_task_fn func, void *user);
uint64_t gdb_scheduler_submit_at(gdb_scheduler *, uint64_t due_ms, int32_t priority,
                                 gdb_task_fn func, void *user);
uint64_t gdb_scheduler_elapsed_ms(gdb_scheduler *);
uint64_t gdb_scheduler_every(gdb_scheduler *, uint32_t interval_ms, int32_t priority,
                             gdb_task_fn func, void *user);
int32_t  gdb_scheduler_cancel(gdb_scheduler *, uint64_t task_id);
int32_t  gdb_scheduler_pause(gdb_scheduler *);
int32_t  gdb_scheduler_resume(gdb_scheduler *);
int32_t  gdb_scheduler_wait_idle(gdb_scheduler *, uint32_t timeout_ms);
char    *gdb_scheduler_take_errors(gdb_scheduler *);   /* JSON array of {taskId, code, message} */
```

Rust callbacks receive the device and return `Result<()>`:

```rust
let scheduler = device.scheduler();
let id = scheduler.submit(Duration::from_millis(250), Priority(5), |device| {
    device.mouse().click(Button::Left, 1)
})?;
scheduler.pause();
scheduler.resume();
scheduler.cancel(id);
scheduler.wait_idle(Duration::from_secs(5))?;
let errors = scheduler.take_errors();
```

`submit_at` takes an absolute due in milliseconds since the scheduler started,
which is what `elapsed_ms` / `elapsed()` reports; jobs sharing a due are ordered by
priority. Failures are collected (32 max) instead of killing the worker. Repeating
tasks are stopped with `cancel`; `wait_idle` is `Timeout` while one keeps the queue
busy. Order is due, then higher priority, then submission; a cancel wins only if
the callback has not started.

## Metrics

```c
typedef struct gdb_metrics {
    uint64_t frames_sent;      /* reports written, handshake and CLOSE included */
    uint64_t frames_received;  /* replies accepted for this session */
    uint64_t retries;          /* reads that had to be looked past */
    uint64_t stolen_frames;    /* the part of retries that answered someone else */
    uint64_t reconnects;
    uint64_t lease_faults;
    uint64_t timeouts;
    uint64_t latency_count;
    uint64_t latency_min_us;
    uint64_t latency_p50_us;
    uint64_t latency_p95_us;
    uint64_t latency_max_us;
} gdb_metrics;

int32_t gdb_metrics_get(const gdb_device *device, gdb_metrics *out);
int32_t gdb_metrics_reset(gdb_device *device);

typedef struct gdb_clamps {
    uint64_t clamped_total;
    int32_t requested_x;
    int32_t requested_y;
    int32_t applied_x;
    int32_t applied_y;
} gdb_clamps;

int32_t gdb_metrics_clamps(const gdb_device *device, gdb_clamps *out);
```

`gdb_metrics_get` is `Invalid` on a NULL handle or NULL `out`, and leaves `out`
untouched. Rust: `MetricsSnapshot`, the same fields plus `latency: LatencyStats`,
read by `device.metrics()`; `reset_metrics()` zeroes everything.

- `retries` counts reads the reply loop looked past. The SDK never re-sends a
  command, so it is not a retransmission count.
- Counters are per device, not per session: `reconnects` and `lease_faults` keep
  adding up across a reconnect.
- A latency sample exists only for a command that waited; an input opcode is a
  frame without one, and a timeout is counted, never timed.
- Percentiles come from a 4176-bucket histogram — 1 µs buckets below 4096 µs, then
  eight per doubling up to 4,194,304 µs: exact below 4 ms, within 6% above.
- Reading takes no session lock, two fields may be one report apart, and a report
  racing `reset` may be lost. Nothing ever blocks the sender.

### Clamped moves

An absolute move whose destination lies outside its anchor rectangle is clamped
into it and still converges, which is what a wrong or missing anchor looks like:
the mouse ends up on the primary monitor's edge and nothing reports an error.
`gdb_metrics_clamps` is the readout that tells that apart from a move that never
arrived — `timeouts` covers the second case.

| Field | Meaning |
| --- | --- |
| `clamped_total` | absolute moves that had to be clamped since the last reset |
| `requested_x`, `requested_y` | the point the caller asked for, in the most recent one |
| `applied_x`, `applied_y` | where the move actually aimed inside the anchor rectangle |

The coordinates are meaningful while `clamped_total` is not zero; `reset_metrics`
zeroes them with everything else. Rust reads the same numbers through
`MetricsSnapshot::clamped_total` and `MetricsSnapshot::last_clamp`; C++ is
`device.clamps()`, Python `device.clamps()`, Go `device.Clamps()`.

## Presence

```rust
pub struct Probe {                    // Serialize: camelCase, DeviceInfo's keys
    pub serial: String, pub product: String, pub board: u8, pub firmware: String,
    pub lease_ms: u16, pub poll_ms: u8, pub capabilities: u32,
    pub device_id: [u8; 8], pub build: u32,
}
pub fn probe::read(path: &str) -> Result<Probe>;
pub fn probe::read_selector(selector: Option<&str>) -> Result<Probe>;
pub fn probe::read_from(transport: Box<dyn Transport>, serial: &str, product: &str) -> Result<Probe>;
pub fn probe::all() -> Result<Vec<Probe>>;
pub enum ProbeEvent { Present(Probe), Gone(Error) }
```

```c
char *gdb_probe(const char *selector);        /* JSON: the gdb_device_info object, NULL when nothing matches */
char *gdb_probe_all(void);                    /* JSON array; an empty bus is "[]" */

typedef struct gdb_observer gdb_observer;
gdb_observer *gdb_observer_start(const char *selector, uint32_t interval_ms);
void    gdb_observer_stop(gdb_observer *observer);    /* the handle still answers info and state */
void    gdb_observer_free(gdb_observer *observer);
char   *gdb_observer_info(const gdb_observer *observer);
int32_t gdb_observer_state(const gdb_observer *observer);   /* 2 connected, 0 disconnected, -1 NULL */
int32_t gdb_observer_status(const gdb_observer *observer);        /* always GDB_UNSUPPORTED */
int32_t gdb_observer_barrier(const gdb_observer *observer);       /* always GDB_UNSUPPORTED */
int32_t gdb_observer_release_all(const gdb_observer *observer);   /* always GDB_UNSUPPORTED */
int32_t gdb_observer_command(const gdb_observer *, uint8_t opcode, const uint8_t *payload,
                             size_t len, gdb_packet *out);        /* always GDB_UNSUPPORTED, out zeroed */
```

`GET_FEATURE(14)` carries the identity — board id, firmware, lease, poll interval,
capability bits, device id, build — as a control transfer, so this never touches
the interrupt IN queue, opens no session and writes no packet. It takes no owner
slot, and an observer (an interval under 50 ms is rounded up) resolves its selector
on every poll, so a renumbered HID path survives a replug. `gdb_observer_state` is
2 while the last poll answered.

Every command on an observer is a refusal, because a frame would land on the queue
the owner reads — a typed refusal rather than a write it cannot take back.
`probe::all` skips a board that vanishes mid-listing, answers `Ok(vec![])` on an
empty bus and is `Err` when the bus cannot be enumerated; a selector that matches
nothing is `Disconnected`. `start_with` is the seam for a host that reads Feature 14
through its own HID backend.

## Watch

```rust
pub const DEFAULT_INTERVAL: Duration;            // 500 ms
pub enum WatchEvent { Arrived(DeviceEntry), Removed(DeviceEntry), BootloaderArrived(PathBuf) }
pub struct Listing { pub devices: Vec<DeviceEntry>, pub volumes: Vec<PathBuf> }
pub trait Inventory: Send + 'static { fn listing(&self) -> Result<Listing>; }
pub struct SystemInventory;                      // Manager::list + flash::drives

impl Watch {
    pub fn start(interval: Duration,
                 callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static) -> Result<Self>;
    pub fn start_with(inventory: Box<dyn Inventory>, interval: Duration,
                      callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static) -> Result<Self>;
    pub fn stop(&mut self);
    pub fn is_running(&self) -> bool;             // and Drop stops
}
```

```c
typedef void (*gdb_watch_fn)(void *user, int32_t kind, const char *json);

gdb_watch *gdb_watch_start(uint32_t interval_ms, gdb_watch_fn func, void *user);
void       gdb_watch_stop(gdb_watch *watch);      /* handle stays valid, safe twice */
void       gdb_watch_free(gdb_watch *watch);
char      *gdb_watch_listing(const gdb_watch *watch);   /* {"devices":[…],"volumes":["E:\\"]} */
```

`kind` is 0 arrived, 1 removed (`{path, serial, product, manufacturer, vid, pid}`)
or 2 bootloader (`{"path":"E:\\"}`). The watch enumerates the HID and drive tables
and never reads or writes a report on the vendor collection, so it cannot steal a
reply. The callback runs on the polling thread: it must not block and must not stop
or free its own handle; the JSON pointer is valid only for the call.

The first poll reports everything already present as an arrival; an unchanged
listing emits nothing, and a bootloader volume that stays mounted is reported once
with no removal event. Devices are keyed by serial, or by HID path when the serial
is empty. An interval below 10 ms is rounded up; a failed enumeration keeps the
previous listing and logs once, so a bus that cannot be read never invents
removals. `stop` wakes the sleep, joins, and is safe twice.

## Broker

One process owns the device; the others drive it over `\\.\pipe\gdbox-<name>`
(`default` when unnamed). The wire is one JSON object per line, UTF-8,
`\n`-terminated; a request carries an `id` the reply echoes, events carry none, and
`code` is the numeric `ErrorCode`, so every language maps a failure the same way.

```rust
pub const DEFAULT_NAME: &str = "default";
pub fn pipe_path(name: &str) -> String;
pub fn run_if_requested() -> bool;                      // main() hook for --gdbox-broker

pub enum Source { Auto, Selector(String), Device(Device) }
pub struct Server;                                      // Server::new(name, source)
    // name(), state(), clients(), wait(), shutdown()  — and Drop shuts down

pub struct Client;                                      // Client::connect(name)
    // connect_timeout(name, wait), available(name), connect_or_spawn(name, exe),
    // set_timeout(Duration), events(), is_closed(), close()  — and Drop closes

pub enum Event { State(ConnectionState), Error(Error), Closed }
```

```c
int32_t gdb_attach(const char *name, gdb_remote **out);  /* NULL/"" = "default" */
int32_t gdb_remote_spawn(const char *name);              /* GDBOX_BROKER_EXE, else Unsupported */
void    gdb_remote_free(gdb_remote *remote);

char *gdb_remote_info(const gdb_remote *remote);
char *gdb_remote_status(const gdb_remote *remote);
char *gdb_remote_status_ext(const gdb_remote *remote);
char *gdb_remote_sessions(const gdb_remote *remote);

int32_t gdb_remote_key(gdb_remote *, uint16_t page, uint16_t usage, bool down);
int32_t gdb_remote_mouse_move(gdb_remote *, int16_t dx, int16_t dy);    /* must fit ±1024 */
int32_t gdb_remote_move_by(gdb_remote *, int32_t dx, int32_t dy, uint32_t duration_ms,
                           const gdb_profile *profile);
int32_t gdb_remote_move_raw(gdb_remote *, int32_t dx, int32_t dy);
int32_t gdb_remote_mouse_buttons(gdb_remote *, uint8_t bits);  /* 1/2/4/8/16 */
int32_t gdb_remote_wheel(gdb_remote *, int32_t vertical, int32_t horizontal);
int32_t gdb_remote_barrier(gdb_remote *);
int32_t gdb_remote_release_all(gdb_remote *);
int32_t gdb_remote_command(gdb_remote *, uint8_t opcode, const uint8_t *payload,
                           size_t len, gdb_packet *out);
int32_t gdb_remote_close(gdb_remote *);
```

An attached handle has no absolute move, `start_move_*`, drag, typing, chord,
scheduler, events or metrics: each needs the collection or the process that owns
it. Its JSON is the direct handle's JSON, so one parser serves both.

`Server::new` creates the pipe before it returns, so an immediate `connect`
succeeds. `Source::Auto` and `Source::Selector` retry every 500 ms until a device
appears. `run_if_requested` reads `--gdbox-broker [name]` /
`--gdbox-broker=<name>`, serves that name with `Source::Auto` and exits, or returns
`false` and touches nothing. `connect_or_spawn` starts `exe --gdbox-broker <name>`
detached and retries the pipe for three seconds. A call waits 30 s for its reply
unless `set_timeout` says otherwise; `connect` waits 250 ms for a busy pipe.

| op | args | result |
| --- | --- | --- |
| `info`, `status`, `status_ext`, `sessions` | `{}` | device info, STATUS, STATUS_EXT, `[{id, leaseMs, held, flags}]` |
| `key` | `{page?, usage, down?}` | `null` |
| `mouse_move` | `{dx, dy}` (i16) | `null` |
| `move_by` | `{dx, dy, durationMs?, profile?}` | `null` |
| `move_raw` | `{dx, dy}` (i32) | `null` |
| `mouse_buttons` | `{buttons}` | `null` |
| `wheel` | `{vertical, horizontal}` | `null` |
| `barrier`, `release_all` | `{}` | `null` |
| `command` | `{command: {kind, …}}` | `{packet: {opcode, flags, session, sequence, payload} \| null}` |
| `close` | `{}` | `null`, then the server hangs up |

`key` defaults `page` to 7 and `down` to true, so `{"usage":4}` is a tap.
`command.kind` is one of `open`, `close`, `heartbeat`, `status`, `barrier`,
`release_all`, `status_ext`, `session_list`, `bootloader`, `key {page, usage,
down}`, `snapshot {state}`, `mouse_move {dx, dy}`, `mouse_buttons {buttons}`,
`wheel {vertical, horizontal}`.

```
request  {"id":1,"op":"status","args":{}}
reply    {"id":1,"ok":true,"result":{…}}
reply    {"id":1,"ok":false,"error":{"code":7,"message":"device queue overflow"}}
event    {"event":"state","state":"connected"}
event    {"event":"error","code":3,"message":"device disconnected"}
```

Every op runs to completion behind the device lock: one command is atomic, a
sequence of them is not, so `barrier` is where a client draws the line. A line the
server cannot parse ends that client's connection and nothing else; an unknown op
is answered with `code: 8` and the connection stays up; each new connection is
greeted with the current state event first. Flashing is out of scope, but
`BOOTLOADER` is a `command`. Named pipes are Windows-only; elsewhere `Server::new`,
`Client::connect` and `gdb_attach` are `Unsupported`.

## Raw command

`Device::command(&Command) -> Result<Option<Packet>>` sends one protocol command
with the same session, sequence counter, timeout and reconnect path as any typed
call, but none of the SDK bookkeeping: held buttons are not recorded and a mock
device's cursor does not move. Control opcodes (OPEN, CLOSE, STATUS, BARRIER,
RELEASE_ALL, STATUS_EXT, SESSION_LIST, BOOTLOADER) return `Some(Packet)`; every
input opcode is fire and forget. `Command` and `Packet` are `input_protocol::v3`
types, so a Rust caller needs `input-protocol` as a dependency.

| | Signature | No reply |
| --- | --- | --- |
| Rust | `Device::command(&Command) -> Result<Option<Packet>>` | `None` |
| C ABI | `gdb_command(device, opcode, payload, len, gdb_packet *out)` | `*out` zeroed, `opcode == 0` |
| C++ | `Device::command(opcode, payload)` → `std::optional<Packet>` | `std::nullopt` |
| Python | `Device.command(opcode, payload=b"")` → `Packet \| None` | `None` |
| Go | `Device.Command(opcode, payload) (*Packet, error)` | `nil, nil` |

```c
typedef struct gdb_packet {
    uint8_t opcode;
    uint8_t flags;       /* always 0 on the v3 wire */
    uint32_t session;
    uint32_t sequence;
    uint8_t *payload;    /* NULL when len == 0 */
    size_t len;
} gdb_packet;

void gdb_packet_free(gdb_packet *packet);   /* safe on a zeroed one */
```

An opcode the protocol does not define is `Invalid` before anything is sent.
Prefer the typed calls: they keep the held state and the mock honest.

## Errors

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | Ok | — |
| 1 | Invalid | bad argument, unknown usage, wrong session id |
| 2 | Timeout | no reply / the movement stopped closing the gap |
| 3 | Disconnected | device gone, no match for the selector |
| 4 | Busy | eight owners, the session id already open, or — on a shared device — a reply another process took |
| 5 | Lease | the 2 s lease expired |
| 6 | Framing | malformed frame or response |
| 7 | Overflow | device queue overflow / stale output |
| 8 | Unsupported | platform or firmware lacks the feature |
| 9 | Cancelled | the operation was cancelled |
| 10 | Io | HID read/write failed, or a panic at the ABI boundary |
| 11 | InvalidScreen | the anchor names a monitor the system does not report, or the monitor layout changed mid-move |
| 12 | StaleWindow | the window handle no longer resolves: gone, minimised, or on no monitor |

The value a failing call returns is the code, and `gdb_last_error(buffer,
capacity)` yields the same code plus a message. Bindings convert it to
`gdb::Error` (carrying `code()`), `gdb.GdbError` and `*gdb.Error`.

### Whose error is it

There are two records, and they answer different questions:

| | `gdb_last_error` | `gdb_device_last_error` |
| --- | --- | --- |
| scope | the calling thread | the device |
| cleared by a success | yes | never |
| survives the thread that failed | no | yes |
| carries the failing opcode | no | yes, when the failure came from a command |
| carries the last STATUS | no | yes, when one has been read |

`gdb_last_error` is the one a single-threaded host reads right after a failure,
and it behaves as it always has: a failing call writes the code and the message,
a succeeding one clears them. It is per thread now, so two threads that fail
different calls each read their own instead of whichever wrote last.

`gdb_device_last_error(device, buffer, capacity)` returns the device's own
record and fills `buffer` with the message `gdb_last_error` would have carried on
the failing thread, with the structure appended:

```
no reply to opcode 6 (STATUS) within 120 ms (session 1234) (opcode=STATUS session=1234 status=received:143,completed:142,pending:1)
absolute move to (5000, 5000) targets monitor 1, but that point is on no screen: … (session=1234)
```

The prefix is the message the call returned; the parenthesised tail names the
failing opcode, the session, and the last STATUS the device answered. A negative
case is left alone on purpose: the SDK never invents a STATUS it has not read.
`0` means the device has not failed yet; a NULL handle is `Invalid`.

Rust reads the same record through `device.last_error() -> Option<DeviceError>`,
whose fields are `code`, `message`, `opcode`, `session`, `status` and `ageMs`, and
whose `describe()` is the string above. C++ is `device.last_error()` returning
`std::optional<Failure>`, Python `device.last_error()` returning
`(code, message)`, Go `device.LastError()` returning `(Code, string)`.

A host whose commands run on the heartbeat, a scheduler worker or a movement
thread should read the device record: the thread-local slot belongs to whichever
OS thread the failing call happened to run on, and a binding may run the failing
call and the read on two different ones.

`Disconnected`, `Io` and `Lease` failures trigger one reconnect and one retry when
auto-reconnect is on; `Timeout` never retries, because retrying a half-delivered
input command is worse than reporting it.

## Logging

Logging is process-wide and silent until both a level and a sink are set.

| | Level | Sink |
| --- | --- | --- |
| Rust | `log::set_level(Level)` | `log::set_sink(Some(Box::new(f)))` |
| C ABI | `gdb_set_log_level(uint8_t)` | `gdb_set_log_sink(gdb_log_fn, void *)` |
| C++ | `set_log_level(int32_t)` | `set_log_sink(fn)` |
| Python | `set_log_level(int)` | `set_log_sink(fn)` |
| Go | `SetLogLevel(uint8_t)` | `SetLogSink(fn)` |

Levels are `0 Off, 1 Error, 2 Warn, 3 Info, 4 Debug, 5 Trace`. The SDK writes at
`Info` (connect, close, reconnect), `Warn` (a failed command, with its code) and
`Trace`; nothing emits `Error` or `Debug`.

`Trace` is the packet log. Frames are logged in `Session`, underneath the C ABI, so
every binding prints identical lines:

```
out opcode=6 session=1234 sequence=2 payload=[…]
in opcode=134 session=1234 sequence=2 payload=[…]
skip opcode=134 session=4321 sequence=7 (not ours)
```

`skip` is an IN frame that belonged to another session and was dropped while
waiting for a reply. A packet window or log pane subscribes to the sink; a second
reader on the IN endpoint would break the sequence numbers of both sessions.

## C ABI

```c
uint32_t    gdb_abi_version(void);
int32_t     gdb_abi_version_full(uint32_t *major, uint32_t *minor);
char       *gdb_capabilities(void);
const char *gdb_version(void);
int32_t     gdb_last_error(char *buffer, size_t capacity);
int32_t     gdb_device_last_error(const gdb_device *device, char *buffer, size_t capacity);
void        gdb_string_free(char *text);
```

The header is the authority on the minor revision, `GDB_ABI_VERSION_MINOR`;
`gdb::abi_minor_version()` and `gdb.ABIMinor` read the same number, and the
library answers the pair it implements through `gdb_abi_version_full`. Every
revision is additive — no existing symbol changes shape.

| | Version | Capabilities |
| --- | --- | --- |
| Rust | `gdb::abi_minor_version()` (compile time) | — |
| C ABI | `gdb_abi_version_full(&major, &minor)` | `gdb_capabilities()` |
| C++ | `abi_versions()` | `capabilities()` |
| Python | `abi_versions()` | `capabilities()` |
| Go | `LibraryABIVersions()` | `Capabilities()` |

`gdb_abi_version_full` returns `Invalid` and writes neither slot when one of the
out pointers is NULL. `gdb_abi_version()` keeps answering the major alone, so
the call that existed before still means what it meant.

### Capabilities

`gdb_capabilities()` is one JSON object of booleans, read once after the library
loads:

```json
{"click_timed":true,"click_at_on":true,"window_metrics":true,
 "window_client_screen":true,"pursuit":true,"move_report":true,
 "event_json":true,"scheduler":true,"broker":true,"replay":false,"flash":false}
```

A revision number says the symbols may be there; this says whether the ability
behind them is. `window_metrics` and `window_client_screen` are false off
Windows, `broker` is false off Windows too (named pipes), and `replay` and
`flash` are false everywhere: they are Rust-only and a binding over `gdb.dll`
cannot reach them. A host that finds a `false` it needs links the Rust crate
instead of waiting for a symbol.

| Handle | Opened by | Carries |
| --- | --- | --- |
| `gdb_device` | `gdb_open`, `gdb_open_mock`, `gdb_open_shared`, `gdb_open_mock_shared` | session, typed input, queries, errors, logging, `gdb_command`, `gdb_move_raw`, scheduler, metrics |
| `gdb_remote` | `gdb_attach`, `gdb_remote_spawn` | the broker op set; JSON identical to the direct handle's |
| `gdb_observer` | `gdb_observer_start` | Feature 14 on a timer, no session; commands are refusals |
| `gdb_watch` | `gdb_watch_start` | bus enumeration and change callbacks |
| `gdb_movement` | `gdb_mouse_start_move_*` | one running move |
| `gdb_scheduler` | `gdb_scheduler_new` | one worker and its task queue |

```c
typedef void (*gdb_event_fn)(void *user, int32_t kind, int32_t code, const char *message);
typedef int32_t (*gdb_task_fn)(void *user);
typedef void (*gdb_log_fn)(void *user, int32_t level, const char *message);
typedef void (*gdb_watch_fn)(void *user, int32_t kind, const char *json);
```

Event kinds: `0` state (`code` = `gdb_connection_state`: 0 disconnected, 1
connecting, 2 connected, 3 error), `2` error (`code` = `gdb_error`, `message` set).
`message` is NULL otherwise. Enums in the header: `gdb_error`, `gdb_button`,
`gdb_ease`, `gdb_event_kind`, `gdb_connection_state`. Strings the library returns
are freed with `gdb_string_free`, packets with `gdb_packet_free`.

## Replay

Rust only, `gdb::replay`. A recording stores intent — "key 7:0x04 down", "move by
40,-12", "one barrier" — not bytes, so a firmware change that keeps the opcodes
cannot make it stale; it cannot say what the device did (use `Level::Trace` for
that).

```rust
pub const VERSION: u32 = 1;

pub enum Op {
    Key { page: u16, usage: u16, down: bool },
    Buttons { bits: u8 },              // the whole mask, not a delta
    Wheel { dx: i16, dy: i16 },        // dx horizontal, dy vertical
    MoveBy { dx: i32, dy: i32 },       // interpolated, barrier at the end
    MoveRaw { dx: i32, dy: i32 },      // one report, no barrier
    Barrier, ReleaseAll, Wait,
}

pub struct Entry { pub at_ms: u64, pub op: Op }
pub struct Recording;   // new(seed, (w, h)), push, save, load, play, len, entries()
pub struct Recorder;    // new(seed, (w, h)), record, record_at, save, load, play, clear
pub struct Outcome { pub played: usize, pub skipped: usize, pub elapsed_ms: u64 }
pub struct NoCancel;    // a Cancel that never fires
```

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | number | format version; `1` is the only one `load` accepts, anything else (or a missing field) is `Invalid` |
| `seed` | number | the u64 PRNG seed the gesture was captured with |
| `screen` | `[width, height]` | the screen the session was recorded on, pixels |
| `started_at_ms` | number | Unix milliseconds when recording started |
| `entries` | array | capture order, one object per call |
| `entries[].at_ms` | number | milliseconds since `started_at_ms` |
| `entries[].op` | string | `key`, `buttons`, `wheel`, `move_by`, `move_raw`, `barrier`, `release_all`, `wait` |
| `key` fields | `page`, `usage`, `down` | HID page/usage and direction |
| `buttons` fields | `bits` | whole button mask, not a delta |
| `wheel` fields | `dx`, `dy` | horizontal, vertical |
| `move_by` / `move_raw` fields | `dx`, `dy` | relative pixels |

Keys are snake_case and appear in that order. `save` writes pretty JSON with a
trailing newline, and two saves of one recording are byte for byte identical —
no maps, no timestamp added at save time.

`play(&mut Device, speed: f64, cancel: &dyn Cancel) -> Result<Outcome>`: `speed` is
`0.001..=1000` and divides every `at_ms` gap. Playback is deterministic — entries
apply in order from a fixed timeline with no random source of its own — and
`Outcome::is_complete` is `skipped == 0`. Cancellation is checked between entries
and inside each wait; a cancelled playback sends RELEASE_ALL when at least one
entry played, then returns `Ok` with the rest `skipped`, where a device failure is
an `Err`.

## Flashing

Rust only, `gdb::flash`; `src/uf2.rs` re-exports its packer.

```rust
gdb::flash::Config              // cargo, firmware dir, auto_bootloader, boot_drive
gdb::flash::build(&config, &runner, &cancel, &mut report) -> Result<Image>
gdb::flash::push(&config, Some(&device), &image, &cancel, &mut report) -> Result<Handshake>
gdb::flash::Host                // volumes() + devices() + verify(): what push reads
gdb::flash::SystemHost          // the real one: drives(), Manager::list(), open and check
gdb::flash::Waits { drive, handshake }   // 15 s each; push runs on the defaults
gdb::flash::uf2::pack(&elf) -> Result<Vec<u8>>          // the packer on its own
gdb::flash::write_image(path, bytes, &cancel) -> Result<()>
gdb::flash::drives() -> Result<Vec<PathBuf>>            // mounted RPI-RP2 volumes
```

`Image { bytes, blocks }`. `build` adds the `thumbv6m-none-eabi` target
(skippable), runs cargo with the `hardware` feature against
`firmware/app/Cargo.toml`, and packs the ELF: flash segments
(`0x1000_0000..0x1100_0000`) flattened in address order into one 256-byte aligned
image, written as 512-byte UF2 blocks with family id `0xE48BFF56`. An ELF with no
flash segment is rejected.

`push` sends BOOTLOADER (opcode 9), closes the runtime session — the board refuses
BOOTLOADER while a host keeps polling it — waits for the `RPI-RP2` volume that was
not mounted before the call (15 s), copies `Firmware.uf2` in 16 KiB chunks, then
waits for the runtime HID to re-enumerate and reads Feature 14 on it (15 s).

`Handshake` is `Verified(Verified { board, firmware, lease_ms })` or
`Unverified(Error)`: `Err` from `push` means the transfer failed, `Unverified` means
the bytes are on the flash but the board never answered. `Host`, `Waits`, `Runner`,
`Reporter` and `Cancel` are the seams that let the whole sequence run headless.

## Contract

- **Lease** — the device renews only on OPEN, HEARTBEAT, RELEASE_ALL, BOOTLOADER
  and valid input; STATUS, BARRIER, STATUS_EXT and SESSION_LIST do not renew, so
  polling any of them is not a heartbeat. The SDK tracks the last renewing command
  and sends HEARTBEAT once 600–900 ms have passed since that renewal, against the
  2000 ms lease. The delay is drawn per beat: a beat that lands on one exact
  interval is visible to anything watching the bus.
- **Threads** — one heartbeat thread that wakes every 100 ms and beats only when
  the renewal age passes the sampled delay (it skips a tick rather than block on
  the session lock); one thread per async movement, where `retarget` is
  thread-safe; one worker per scheduler, whose callbacks never overlap. Event
  callbacks may run on the heartbeat, worker or caller thread: they must return
  quickly and must not call back into the same device synchronously.
- **Absolute moves** — the SDK reads the physical cursor and feeds the delta back
  through `RelativeTracker`, so the target rectangle only decides where the point
  is clamped.
- **Broker** — one device per broker, one session behind it, one reader thread per
  client; a client's `EventReceiver` is `Clone` and readable from any thread.
- **Close** — releases the session, stops the heartbeat, clears held input; the
  handle is unusable afterwards.

## Platform

| | Physical HID | Mock |
| --- | --- | --- |
| Windows | yes (hidapi) | yes |
| Linux / macOS | compiles, returns `Unsupported` | yes |

The mock sits above the transport, so everything except the Go binding runs on any
platform without hardware. `gdb::broker` needs Windows named pipes; `sdk/go/gdb.go`
needs Windows and a built `gdb.dll`.
