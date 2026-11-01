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
void gdb_device_free(gdb_device *device);
int32_t gdb_close(gdb_device *device);
int32_t gdb_reconnect(gdb_device *device);
int32_t gdb_set_auto_reconnect(gdb_device *device, bool enabled);
int32_t gdb_device_state(const gdb_device *device); /* -1 on a NULL handle */

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

#ifdef __cplusplus
}
#endif

#endif /* GDBOX_GDB_H */
