/*
 * gdb.h - C ABI for the GDBOX host SDK.
 *
 * The same header is consumed by the C++, Python and Go bindings. All strings
 * returned by the library are UTF-8 and must be released with gdb_string_free.
 * All int32-returning calls return a gdb_error; on failure gdb_last_error
 * yields the same code plus a message.
 */
#ifndef GDBOX_GDB_H
#define GDBOX_GDB_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GDB_ABI_VERSION 1u

typedef enum gdb_error {
    GDB_OK = 0,
    GDB_INVALID = 1,
    GDB_TIMEOUT = 2,
    GDB_DISCONNECTED = 3,
    GDB_BUSY = 4,
    GDB_LEASE = 5,
    GDB_FRAMING = 6,
    GDB_OVERFLOW = 7,
    GDB_UNSUPPORTED = 8,
    GDB_CANCELLED = 9,
    GDB_IO = 10
} gdb_error;

typedef enum gdb_connection_state {
    GDB_STATE_DISCONNECTED = 0,
    GDB_STATE_CONNECTING = 1,
    GDB_STATE_CONNECTED = 2,
    GDB_STATE_ERROR = 3
} gdb_connection_state;

typedef enum gdb_button {
    GDB_BUTTON_LEFT = 1,
    GDB_BUTTON_RIGHT = 2,
    GDB_BUTTON_MIDDLE = 3,
    GDB_BUTTON_BACK = 4,
    GDB_BUTTON_FORWARD = 5
} gdb_button;

typedef enum gdb_ease {
    GDB_EASE_JERK = 0,
    GDB_EASE_IN_OUT = 1,
    GDB_EASE_LINEAR = 2,
    GDB_EASE_IN = 3,
    GDB_EASE_OUT = 4,
    GDB_EASE_CUBIC_BEZIER = 5
} gdb_ease;

typedef enum gdb_event_kind {
    GDB_EVENT_STATE = 0,
    GDB_EVENT_ERROR = 2
} gdb_event_kind;

typedef struct gdb_device gdb_device;
typedef struct gdb_movement gdb_movement;
typedef struct gdb_scheduler gdb_scheduler;

typedef struct gdb_point {
    int32_t x;
    int32_t y;
} gdb_point;

typedef struct gdb_key_step {
    uint16_t page;
    uint16_t usage;
    uint32_t hold_ms;
    uint32_t gap_ms;
} gdb_key_step;

typedef struct gdb_profile {
    int32_t ease;        /* gdb_ease */
    double bezier[4];    /* x1, y1, x2, y2 - used when ease == 5 */
    double curve;        /* 0 straight, 1 bow, 2 exaggerated arc */
    double jitter_px;    /* micro jitter in pixels */
    double overshoot_pct;
    uint32_t settle_ms;
    int32_t min_step_px;
} gdb_profile;

/* One reply packet. The payload is owned by the caller until gdb_packet_free.
   opcode == 0 means the command had no reply (every input opcode). */
typedef struct gdb_packet {
    uint8_t opcode;
    uint8_t flags;       /* always 0 on the v3 wire */
    uint32_t session;
    uint32_t sequence;
    uint8_t *payload;    /* NULL when len == 0 */
    size_t len;
} gdb_packet;

/* user callback for connection events; message is NULL unless kind == ERROR */
typedef void (*gdb_event_fn)(void *user, int32_t kind, int32_t code, const char *message);
/* task callback: return 0 to report success, or a gdb_error value */
typedef int32_t (*gdb_task_fn)(void *user);
typedef void (*gdb_log_fn)(void *user, int32_t level, const char *message);

uint32_t gdb_abi_version(void);
const char *gdb_version(void);

/* Fills buffer with the last error message and returns its code (0 = none). */
int32_t gdb_last_error(char *buffer, size_t capacity);
void gdb_string_free(char *text);

/* Logging: 0 off, 1 error, 2 warn, 3 info, 4 debug, 5 trace. */
int32_t gdb_set_log_level(uint8_t level);
int32_t gdb_set_log_sink(gdb_log_fn func, void *user);

/* JSON array of {path, serial, product, manufacturer, vid, pid}. */
char *gdb_device_list(void);

gdb_device *gdb_open(const char *selector, uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);
gdb_device *gdb_open_mock(uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);

/* The same board in a non-exclusive mode: several programs may drive it, and the
   SDK stops trying to recover from a reply another program took. One difference
   from gdb_open, and only one:
     - a reply that never arrives is GDB_BUSY naming the command ("STATUS got no
       reply; another process may have taken it (...)") instead of GDB_TIMEOUT;
     - the session is never reopened on the SDK's own initiative, because CLOSE
       plus OPEN on a board it does not own is exactly the traffic a collision
       must not cost. gdb_reconnect() is still there for a caller that knows the
       board is its own again.
   Everything else - sequence numbers, the lease, the heartbeat, the counters -
   is unchanged, and a lost heartbeat is simply retried on the next tick.
   What it cannot promise: which program gets a given reply. Avoiding the
   collision is the callers' discipline, not the library's. */
gdb_device *gdb_open_shared(const char *selector, uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);
gdb_device *gdb_open_mock_shared(uint32_t timeout_ms, bool auto_reconnect, bool heartbeat);

void gdb_device_free(gdb_device *device);
int32_t gdb_close(gdb_device *device);
int32_t gdb_reconnect(gdb_device *device);
int32_t gdb_set_auto_reconnect(gdb_device *device, bool enabled);
int32_t gdb_device_state(const gdb_device *device); /* -1 on a NULL handle */

/* Frames the reply loop looked past because they answered another session or
   another sequence: the counter that tells a collision from a slow board. Reads
   one atomic; 0 on a NULL handle, with gdb_last_error set. */
uint64_t gdb_stolen_frames(const gdb_device *device);

/* JSON objects/arrays for the device info, STATUS, STATUS_EXT and SESSION_LIST. */
char *gdb_device_info(const gdb_device *device);
char *gdb_status(const gdb_device *device);
char *gdb_status_ext(const gdb_device *device);
char *gdb_sessions(const gdb_device *device);

int32_t gdb_release_all(gdb_device *device);
int32_t gdb_barrier(gdb_device *device);
int32_t gdb_bootloader(gdb_device *device);
int32_t gdb_set_event_handler(gdb_device *device, gdb_event_fn func, void *user);

gdb_profile gdb_profile_default(void);

/* Mouse. duration_ms == 0 picks a natural duration. profile may be NULL. */
int32_t gdb_mouse_move_by(gdb_device *device, int32_t dx, int32_t dy, uint32_t duration_ms, const gdb_profile *profile);
/* One relative move of any size: split at the +/-1024 report limit, no barrier.
   The caller flushes with gdb_barrier() where a completion guarantee matters. */
int32_t gdb_move_raw(gdb_device *device, int32_t dx, int32_t dy);
int32_t gdb_mouse_move_to(gdb_device *device, int32_t x, int32_t y, uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_mouse_move_path(gdb_device *device, const gdb_point *points, size_t count, uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_mouse_button(gdb_device *device, uint8_t button, bool down);
int32_t gdb_mouse_click(gdb_device *device, uint8_t button, uint8_t count);
int32_t gdb_mouse_double_click(gdb_device *device, uint8_t button);
int32_t gdb_mouse_wheel(gdb_device *device, int32_t vertical, int32_t horizontal);
int32_t gdb_mouse_drag(gdb_device *device, uint8_t button, int32_t x, int32_t y, uint32_t duration_ms, const gdb_profile *profile);

/* Async moves: NULL on failure. */
gdb_movement *gdb_mouse_start_move_to(gdb_device *device, int32_t x, int32_t y, uint32_t duration_ms, const gdb_profile *profile);
gdb_movement *gdb_mouse_start_move_by(gdb_device *device, int32_t dx, int32_t dy, uint32_t duration_ms, const gdb_profile *profile);
gdb_movement *gdb_mouse_start_move_path(gdb_device *device, const gdb_point *points, size_t count, uint32_t duration_ms, const gdb_profile *profile);
int32_t gdb_movement_cancel(gdb_movement *movement);
int32_t gdb_movement_retarget(gdb_movement *movement, int32_t x, int32_t y);
bool gdb_movement_is_finished(const gdb_movement *movement);
int32_t gdb_movement_wait(gdb_movement *movement, uint32_t timeout_ms);
void gdb_movement_free(gdb_movement *movement);

/* Keyboard. */
int32_t gdb_key_down(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_up(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_press(gdb_device *device, uint16_t page, uint16_t usage);
int32_t gdb_key_long_press(gdb_device *device, uint16_t page, uint16_t usage, uint32_t hold_ms);
int32_t gdb_keys_chord(gdb_device *device, const uint16_t *usages, size_t count, uint32_t hold_ms);
int32_t gdb_keys_sequence(gdb_device *device, const gdb_key_step *steps, size_t count);
int32_t gdb_type_text(gdb_device *device, const char *utf8, uint32_t interval_ms, uint32_t hold_ms, uint32_t jitter_ms);

/* Raw command: the escape hatch, for callers that already speak the protocol.
   It rides the same session, sequence numbers and reconnect path as every other
   call, but the SDK's bookkeeping is skipped - held buttons and the mock cursor
   are not updated, so prefer the typed calls. opcode and payload must name a
   command the protocol defines; anything else fails with GDB_INVALID.
   A control command fills *out; an input command has no reply and leaves it
   zeroed. out may be NULL when the reply is not wanted. */
int32_t gdb_command(gdb_device *device, uint8_t opcode, const uint8_t *payload, size_t len, gdb_packet *out);
/* Releases the payload inside a packet from gdb_command; safe on a zeroed one. */
void gdb_packet_free(gdb_packet *packet);

/* Automation: one worker thread per scheduler, jobs run in (due, priority, seq) order. */
gdb_scheduler *gdb_scheduler_new(gdb_device *device);
void gdb_scheduler_free(gdb_scheduler *scheduler);
uint64_t gdb_scheduler_submit(gdb_scheduler *scheduler, uint32_t delay_ms, int32_t priority, gdb_task_fn func, void *user);
uint64_t gdb_scheduler_every(gdb_scheduler *scheduler, uint32_t interval_ms, int32_t priority, gdb_task_fn func, void *user);
int32_t gdb_scheduler_cancel(gdb_scheduler *scheduler, uint64_t task_id);
int32_t gdb_scheduler_pause(gdb_scheduler *scheduler);
int32_t gdb_scheduler_resume(gdb_scheduler *scheduler);
int32_t gdb_scheduler_wait_idle(gdb_scheduler *scheduler, uint32_t timeout_ms);
/* JSON array of {taskId, code, message} for jobs that failed. */
char *gdb_scheduler_take_errors(gdb_scheduler *scheduler);

/* ------------------------------------------------------------------- presence
 *
 * What a program may read while another program owns the board. The identity -
 * board id, firmware version, lease length, poll interval, capability flags,
 * device id, build - arrives in GET_FEATURE(14), a control transfer that never
 * touches the interrupt IN queue, so this needs no session and sends no packet.
 * No OPEN either, so no owner slot is taken and the program that holds one keeps
 * it.
 *
 * That is the whole difference from gdb_open. A direct handle may also send
 * commands, and a command consumes the shared IN queue: a second reader that
 * drains a reply the owner was waiting for costs that owner one error for that
 * command - GDB_BUSY if it opened with gdb_open_shared, GDB_TIMEOUT if it did
 * not - and the session stays. So a program that only wants the numbers probes
 * or observes; one that also sends opens shared.
 */

/* JSON: the same object gdb_device_info returns. selector is a USB serial, a HID
   path, a unique fragment of either, or NULL for the first attached board.
   NULL when nothing matches, with gdb_last_error set (GDB_DISCONNECTED, or
   GDB_UNSUPPORTED where there is no HID backend). */
char *gdb_probe(const char *selector);
/* JSON array of those objects, one per attached vendor collection. An empty bus
   is "[]", not NULL. */
char *gdb_probe_all(void);
/* Both are freed with gdb_string_free; there is no separate free call. */

typedef struct gdb_observer gdb_observer;

/* Reads Feature 14 once, then again every interval_ms on its own thread, and
   keeps the last answer. selector may be NULL. NULL when the board is not there
   or does not answer, with gdb_last_error set. An interval under 50 ms is
   rounded up. */
gdb_observer *gdb_observer_start(const char *selector, uint32_t interval_ms);
/* Stops the thread. The handle stays valid for info and state. */
void gdb_observer_stop(gdb_observer *observer);
/* Stops the thread and releases the handle. */
void gdb_observer_free(gdb_observer *observer);

/* The last probe, as the object gdb_device_info returns. */
char *gdb_observer_info(const gdb_observer *observer);
/* 2 connected while the last poll answered, 0 disconnected while it failed.
   -1 on a NULL handle. */
int32_t gdb_observer_state(const gdb_observer *observer);

/* Every command on an observer is GDB_UNSUPPORTED, with a message naming the
   reason: the handle has no session and must not put a frame on the shared IN
   queue. They exist so a program can swap a device handle for an observer and
   get a typed refusal instead of a write it cannot take back. */
int32_t gdb_observer_status(const gdb_observer *observer);
int32_t gdb_observer_barrier(const gdb_observer *observer);
int32_t gdb_observer_release_all(const gdb_observer *observer);
/* Always GDB_UNSUPPORTED: the opcode is not parsed and out is zeroed. */
int32_t gdb_observer_command(const gdb_observer *observer, uint8_t opcode, const uint8_t *payload, size_t len, gdb_packet *out);

/* ------------------------------------------------------------------ attached
 *
 * A second, separate surface for the case the direct one cannot serve: only one
 * process may read the vendor collection, so a broker process owns the device
 * and everyone else attaches to it. The pipe is \\.\pipe\gdbox-<name> and the
 * wire is one JSON object per line, which is gdb::broker on the Rust side.
 *
 * Nothing above changes: gdb_remote is its own handle type with its own symbols,
 * so a program may link both and use a direct handle in the process that owns
 * the device while every other process uses an attached one.
 *
 * Results reuse the direct shapes, so parsing does not change either:
 *   gdb_remote_info      -> the object gdb_device_info returns
 *   gdb_remote_status    -> the object gdb_status returns
 *   gdb_remote_status_ext-> the object gdb_status_ext returns
 *   gdb_remote_sessions  -> the array gdb_sessions returns
 * Errors map to the same gdb_error codes, and a broker that is gone is
 * GDB_DISCONNECTED.
 *
 * What an attached handle does not offer: raw HID reports, the capability scan,
 * host-side motion workers (gdb_mouse_move_to, drags, typing, chords), flashing
 * and the bootloader. Those need the collection itself, so they stay on the
 * broker's side of the pipe.
 */

/* Minor additions, each one additive: 1 added the attached surface, 2 the
   presence probe and the observer handle, 3 the shared-ownership opens and
   gdb_stolen_frames. Major stays 1: no existing symbol changed shape. */
#define GDB_ABI_VERSION_MINOR 3u

typedef struct gdb_remote gdb_remote;

/* Attaches to the broker on \\.\pipe\gdbox-<name>; name may be NULL or "" for
   "default". *out is zeroed first and receives the handle on success. Nothing
   listening is GDB_DISCONNECTED; a name containing a path separator, or a NULL
   out, is GDB_INVALID. The caller frees the handle with gdb_remote_free. */
int32_t gdb_attach(const char* name, gdb_remote** out);

/* Starts a broker when none is listening on <name>, then returns; attach
   afterwards. The executable is taken from the GDBOX_BROKER_EXE environment
   variable - a host built on this SDK, which serves `--gdbox-broker <name>` and
   otherwise ignores the switch. The child is detached and outlives the caller.
   With GDBOX_BROKER_EXE unset there is nothing to start and this returns
   GDB_UNSUPPORTED (the ABI cannot guess the caller's own binary); when a broker
   is already listening it returns GDB_OK and starts nothing. */
int32_t gdb_remote_spawn(const char* name);

/* Hangs up and releases the handle. */
void gdb_remote_free(gdb_remote* remote);

char* gdb_remote_info(const gdb_remote* remote);
char* gdb_remote_status(const gdb_remote* remote);
char* gdb_remote_status_ext(const gdb_remote* remote);
char* gdb_remote_sessions(const gdb_remote* remote);

int32_t gdb_remote_key(gdb_remote* remote, uint16_t page, uint16_t usage, bool down);
/* One MOUSE_MOVE report; the deltas must fit the protocol's +/-1024. */
int32_t gdb_remote_mouse_move(gdb_remote* remote, int16_t dx, int16_t dy);
/* Relative move paced inside the broker; duration_ms == 0 picks a natural
   speed and profile may be NULL. */
int32_t gdb_remote_move_by(gdb_remote* remote, int32_t dx, int32_t dy, uint32_t duration_ms, const gdb_profile* profile);
/* One relative move of any size: split at the +/-1024 report limit, no barrier. */
int32_t gdb_remote_move_raw(gdb_remote* remote, int32_t dx, int32_t dy);
/* Raw HID button mask: left=1, right=2, middle=4, back=8, forward=16. */
int32_t gdb_remote_mouse_buttons(gdb_remote* remote, uint8_t bits);
int32_t gdb_remote_wheel(gdb_remote* remote, int32_t vertical, int32_t horizontal);
int32_t gdb_remote_barrier(gdb_remote* remote);
int32_t gdb_remote_release_all(gdb_remote* remote);

/* gdb_command over the pipe: the same frame leaves the broker's session and the
   device's reply comes back in *out, filled exactly as gdb_command fills it. */
int32_t gdb_remote_command(gdb_remote* remote, uint8_t opcode, const uint8_t* payload, size_t len, gdb_packet* out);

/* Ends this client's connection. The handle stays valid and later calls report
   GDB_DISCONNECTED until gdb_remote_free. */
int32_t gdb_remote_close(gdb_remote* remote);

#ifdef __cplusplus
}
#endif

#endif /* GDBOX_GDB_H */
