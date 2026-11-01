// Package gdb is the Go binding for the GDBOX SDK.
//
// It loads gdb.dll at runtime with the Windows loader, so there is no cgo and no
// import library to link. Set GDBOX_LIBRARY to the full path of the DLL, or put
// gdb.dll next to the executable.
//
//	device, err := gdb.OpenMock()
//	if err != nil { log.Fatal(err) }
//	defer device.Close()
//	fmt.Println(device.InfoJSON())
//	device.Keyboard().TypeText("hello", 45, 18, 30)
//
// Behaviour matches the Rust, C++ and Python SDKs: all calls return gdb.Error
// with a Code when they fail.
//
//go:build windows

package gdb

import (
	"encoding/json"
	"fmt"
	"os"
	"sync"
	"syscall"
	"unsafe"
)

// ABI versions must match gdb.h.
const (
	ABIVersion = 1
	VendorID   = 0x03F0
	ProductID  = 0x0024
	UsagePage  = 0xFF60
	Usage      = 0x61
)

// Code mirrors the C gdb_error enum.
type Code int32

const (
	OK           Code = 0
	Invalid      Code = 1
	Timeout      Code = 2
	Disconnected Code = 3
	Busy         Code = 4
	Lease        Code = 5
	Framing      Code = 6
	Overflow     Code = 7
	Unsupported  Code = 8
	Cancelled    Code = 9
	IO           Code = 10
)

func (c Code) String() string {
	names := []string{"OK", "INVALID", "TIMEOUT", "DISCONNECTED", "BUSY", "LEASE", "FRAMING", "OVERFLOW", "UNSUPPORTED", "CANCELLED", "IO"}
	if int(c) < len(names) {
		return names[c]
	}
	return fmt.Sprintf("CODE(%d)", int32(c))
}

// ConnectionState mirrors gdb_connection_state.
type ConnectionState int32

const (
	StateDisconnected ConnectionState = 0
	StateConnecting   ConnectionState = 1
	StateConnected    ConnectionState = 2
	StateError        ConnectionState = 3
)

// Button mirrors gdb_button.
type Button uint8

const (
	Left    Button = 1
	Right   Button = 2
	Middle  Button = 3
	Back    Button = 4
	Forward Button = 5
)

// Ease mirrors gdb_ease.
type Ease int32

const (
	EaseJerk        Ease = 0
	EaseInOut       Ease = 1
	EaseLinear      Ease = 2
	EaseIn          Ease = 3
	EaseOut         Ease = 4
	EaseCubicBezier Ease = 5
)

// Error is returned by every failing call.
type Error struct {
	Code    Code
	Message string
}

func (e *Error) Error() string {
	if e.Message == "" {
		return e.Code.String()
	}
	return e.Code.String() + ": " + e.Message
}

type point struct{ X, Y int32 }

// rawPacket mirrors gdb_packet; Payload is the DLL's allocation, released with
// gdb_packet_free.
type rawPacket struct {
	Opcode   uint8
	Flags    uint8
	Session  uint32
	Sequence uint32
	Payload  *uint8
	Len      uintptr
}

// Packet is one reply from Device.Command; every input command has none.
type Packet struct {
	Opcode   uint8
	Flags    uint8
	Session  uint32
	Sequence uint32
	Payload  []byte
}

type keyStep struct {
	Page   uint16
	Usage  uint16
	HoldMs uint32
	GapMs  uint32
}

// Profile is the motion profile; DefaultProfile mirrors the SDK defaults.
type Profile struct {
	Ease         Ease
	Bezier       [4]float64
	Curve        float64
	JitterPx     float64
	OvershootPct float64
	SettleMs     uint32
	MinStepPx    int32
}

// LinearProfile is a straight, constant-speed path with no jitter.
func LinearProfile() Profile {
	return Profile{Ease: EaseLinear, Bezier: [4]float64{0.42, 0, 0.58, 1}, MinStepPx: 1}
}

type rawProfile struct {
	ease         int32
	bezier       [4]float64
	curve        float64
	jitterPx     float64
	overshootPct float64
	settleMs     uint32
	minStepPx    int32
}

func (p Profile) raw() rawProfile {
	return rawProfile{p.ease(), p.Bezier, p.Curve, p.JitterPx, p.OvershootPct, p.SettleMs, p.MinStepPx}
}

func (p Profile) ease() int32 { return int32(p.Ease) }

var (
	dllOnce sync.Once
	dll     *syscall.LazyDLL

	procLastError      *syscall.LazyProc
	procStringFree     *syscall.LazyProc
	procSetLogLevel    *syscall.LazyProc
	procSetLogSink     *syscall.LazyProc
	procDeviceList     *syscall.LazyProc
	procOpen           *syscall.LazyProc
	procOpenMock       *syscall.LazyProc
	procDeviceFree     *syscall.LazyProc
	procClose          *syscall.LazyProc
	procReconnect      *syscall.LazyProc
	procSetReconnect   *syscall.LazyProc
	procState          *syscall.LazyProc
	procInfo           *syscall.LazyProc
	procStatus         *syscall.LazyProc
	procStatusExt      *syscall.LazyProc
	procSessions       *syscall.LazyProc
	procReleaseAll     *syscall.LazyProc
	procBarrier        *syscall.LazyProc
	procBootloader     *syscall.LazyProc
	procSetEvents      *syscall.LazyProc
	procProfileDefault *syscall.LazyProc
	procCommand        *syscall.LazyProc
	procPacketFree     *syscall.LazyProc
	procMoveRaw        *syscall.LazyProc
	procMoveBy         *syscall.LazyProc
	procMoveTo         *syscall.LazyProc
	procMovePath       *syscall.LazyProc
	procButton         *syscall.LazyProc
	procClick          *syscall.LazyProc
	procDoubleClick    *syscall.LazyProc
	procWheel          *syscall.LazyProc
	procDrag           *syscall.LazyProc
	procStartMoveTo    *syscall.LazyProc
	procStartMoveBy    *syscall.LazyProc
	procStartMovePath  *syscall.LazyProc
	procMoveCancel     *syscall.LazyProc
	procMoveRetarget   *syscall.LazyProc
	procMoveFinished   *syscall.LazyProc
	procMoveWait       *syscall.LazyProc
	procMoveFree       *syscall.LazyProc
	procKeyDown        *syscall.LazyProc
	procKeyUp          *syscall.LazyProc
	procKeyPress       *syscall.LazyProc
	procKeyLongPress   *syscall.LazyProc
	procChord          *syscall.LazyProc
	procSequence       *syscall.LazyProc
	procTypeText       *syscall.LazyProc
	procSchedNew       *syscall.LazyProc
	procSchedFree      *syscall.LazyProc
	procSchedSubmit    *syscall.LazyProc
	procSchedEvery     *syscall.LazyProc
	procSchedCancel    *syscall.LazyProc
	procSchedPause     *syscall.LazyProc
	procSchedResume    *syscall.LazyProc
	procSchedWaitIdle  *syscall.LazyProc
	procSchedErrors    *syscall.LazyProc
)

func library() *syscall.LazyDLL {
	dllOnce.Do(func() {
		path := os.Getenv("GDBOX_LIBRARY")
		if path == "" {
			path = "gdb.dll"
		}
		dll = syscall.NewLazyDLL(path)
		procLastError = dll.NewProc("gdb_last_error")
		procStringFree = dll.NewProc("gdb_string_free")
		procSetLogLevel = dll.NewProc("gdb_set_log_level")
		procSetLogSink = dll.NewProc("gdb_set_log_sink")
		procDeviceList = dll.NewProc("gdb_device_list")
		procOpen = dll.NewProc("gdb_open")
		procOpenMock = dll.NewProc("gdb_open_mock")
		procDeviceFree = dll.NewProc("gdb_device_free")
		procClose = dll.NewProc("gdb_close")
		procReconnect = dll.NewProc("gdb_reconnect")
		procSetReconnect = dll.NewProc("gdb_set_auto_reconnect")
		procState = dll.NewProc("gdb_device_state")
		procInfo = dll.NewProc("gdb_device_info")
		procStatus = dll.NewProc("gdb_status")
		procStatusExt = dll.NewProc("gdb_status_ext")
		procSessions = dll.NewProc("gdb_sessions")
		procReleaseAll = dll.NewProc("gdb_release_all")
		procBarrier = dll.NewProc("gdb_barrier")
		procBootloader = dll.NewProc("gdb_bootloader")
		procSetEvents = dll.NewProc("gdb_set_event_handler")
		procProfileDefault = dll.NewProc("gdb_profile_default")
		procCommand = dll.NewProc("gdb_command")
		procPacketFree = dll.NewProc("gdb_packet_free")
		procMoveRaw = dll.NewProc("gdb_move_raw")
		procMoveBy = dll.NewProc("gdb_mouse_move_by")
		procMoveTo = dll.NewProc("gdb_mouse_move_to")
		procMovePath = dll.NewProc("gdb_mouse_move_path")
		procButton = dll.NewProc("gdb_mouse_button")
		procClick = dll.NewProc("gdb_mouse_click")
		procDoubleClick = dll.NewProc("gdb_mouse_double_click")
		procWheel = dll.NewProc("gdb_mouse_wheel")
		procDrag = dll.NewProc("gdb_mouse_drag")
		procStartMoveTo = dll.NewProc("gdb_mouse_start_move_to")
		procStartMoveBy = dll.NewProc("gdb_mouse_start_move_by")
		procStartMovePath = dll.NewProc("gdb_mouse_start_move_path")
		procMoveCancel = dll.NewProc("gdb_movement_cancel")
		procMoveRetarget = dll.NewProc("gdb_movement_retarget")
		procMoveFinished = dll.NewProc("gdb_movement_is_finished")
		procMoveWait = dll.NewProc("gdb_movement_wait")
		procMoveFree = dll.NewProc("gdb_movement_free")
		procKeyDown = dll.NewProc("gdb_key_down")
		procKeyUp = dll.NewProc("gdb_key_up")
		procKeyPress = dll.NewProc("gdb_key_press")
		procKeyLongPress = dll.NewProc("gdb_key_long_press")
		procChord = dll.NewProc("gdb_keys_chord")
		procSequence = dll.NewProc("gdb_keys_sequence")
		procTypeText = dll.NewProc("gdb_type_text")
		procSchedNew = dll.NewProc("gdb_scheduler_new")
		procSchedFree = dll.NewProc("gdb_scheduler_free")
		procSchedSubmit = dll.NewProc("gdb_scheduler_submit")
		procSchedEvery = dll.NewProc("gdb_scheduler_every")
		procSchedCancel = dll.NewProc("gdb_scheduler_cancel")
		procSchedPause = dll.NewProc("gdb_scheduler_pause")
		procSchedResume = dll.NewProc("gdb_scheduler_resume")
		procSchedWaitIdle = dll.NewProc("gdb_scheduler_wait_idle")
		procSchedErrors = dll.NewProc("gdb_scheduler_take_errors")
	})
	return dll
}

func lastError() *Error {
	library()
	buffer := make([]byte, 512)
	code, _, _ := procLastError.Call(uintptr(unsafe.Pointer(&buffer[0])), uintptr(len(buffer)))
	message := ""
	for index, value := range buffer {
		if value == 0 {
			message = string(buffer[:index])
			break
		}
	}
	if code == 0 {
		return nil
	}
	return &Error{Code: Code(code), Message: message}
}

func check(code uintptr) error {
	if code == 0 {
		return nil
	}
	if failure := lastError(); failure != nil {
		return failure
	}
	return &Error{Code: Code(code)}
}

func take(pointer uintptr) (string, error) {
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return "", failure
		}
		return "", &Error{Code: IO, Message: "no reply"}
	}
	defer procStringFree.Call(pointer)
	return cString(pointer), nil
}

// Log levels accepted by SetLogLevel; they match the C ABI.
const (
	LogOff   = 0
	LogError = 1
	LogWarn  = 2
	LogInfo  = 3
	LogDebug = 4
	LogTrace = 5
)

var (
	logMu         sync.Mutex
	logSink       func(level int32, message string)
	logTrampoline = syscall.NewCallback(func(_, level, message uintptr) uintptr {
		logMu.Lock()
		sink := logSink
		logMu.Unlock()
		if sink != nil {
			sink(int32(level), cString(message))
		}
		return 0
	})
)

// SetLogLevel sets the process-wide SDK log level (LogOff … LogTrace).
func SetLogLevel(level uint8) error {
	library()
	code, _, _ := procSetLogLevel.Call(uintptr(level))
	return check(code)
}

// SetLogSink installs a callback for SDK log lines; nil turns it off again.
// Logging stays silent until SetLogLevel raises the level.
func SetLogSink(sink func(level int32, message string)) error {
	library()
	logMu.Lock()
	logSink = sink
	logMu.Unlock()
	var code uintptr
	if sink == nil {
		code, _, _ = procSetLogSink.Call(0, 0)
	} else {
		code, _, _ = procSetLogSink.Call(logTrampoline, 0)
	}
	return check(code)
}

// Device is one controller session.
type Device struct {
	handle   uintptr
	closed   bool
	mouse    Mouse
	keyboard Keyboard
	events   map[uintptr]func(int32, int32, string)
}

// ListJSON returns the device list as JSON.
func ListJSON() (string, error) {
	library()
	pointer, _, _ := procDeviceList.Call()
	return take(pointer)
}

// Open connects to the first device matching selector (serial or HID path).
func Open(selector string, timeoutMs uint32, autoReconnect, heartbeat bool) (*Device, error) {
	library()
	var pointer uintptr
	var err error
	if selector == "" {
		pointer, _, err = procOpen.Call(0, uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
	} else {
		name, cerr := syscall.BytePtrFromString(selector)
		if cerr != nil {
			return nil, &Error{Code: Invalid, Message: cerr.Error()}
		}
		pointer, _, err = procOpen.Call(uintptr(unsafe.Pointer(name)), uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
	}
	_ = err
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "open failed"}
	}
	device := &Device{handle: pointer, events: map[uintptr]func(int32, int32, string){}}
	device.mouse = Mouse{device: device}
	device.keyboard = Keyboard{device: device}
	return device, nil
}

// OpenMock opens the in-memory device; no hardware needed.
func OpenMock() (*Device, error) {
	return OpenMockOptions(1000, true, true)
}

// OpenMockOptions is OpenMock with explicit options.
func OpenMockOptions(timeoutMs uint32, autoReconnect, heartbeat bool) (*Device, error) {
	library()
	pointer, _, _ := procOpenMock.Call(uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "mock open failed"}
	}
	device := &Device{handle: pointer, events: map[uintptr]func(int32, int32, string){}}
	device.mouse = Mouse{device: device}
	device.keyboard = Keyboard{device: device}
	return device, nil
}

func boolArg(value bool) uintptr {
	if value {
		return 1
	}
	return 0
}

// Handle exposes the raw pointer (for debugging only).
func (d *Device) Handle() uintptr { return d.handle }

// Mouse returns the mouse handle.
func (d *Device) Mouse() *Mouse { return &d.mouse }

// Keyboard returns the keyboard handle.
func (d *Device) Keyboard() *Keyboard { return &d.keyboard }

// InfoJSON returns the device information as JSON.
func (d *Device) InfoJSON() (string, error) {
	pointer, _, _ := procInfo.Call(d.handle)
	return take(pointer)
}

// StatusJSON returns STATUS as JSON.
func (d *Device) StatusJSON() (string, error) {
	pointer, _, _ := procStatus.Call(d.handle)
	return take(pointer)
}

// StatusExtJSON returns STATUS_EXT as JSON.
func (d *Device) StatusExtJSON() (string, error) {
	pointer, _, _ := procStatusExt.Call(d.handle)
	return take(pointer)
}

// SessionsJSON returns SESSION_LIST as JSON.
func (d *Device) SessionsJSON() (string, error) {
	pointer, _, _ := procSessions.Call(d.handle)
	return take(pointer)
}

// State returns the connection state.
func (d *Device) State() (ConnectionState, error) {
	value, _, _ := procState.Call(d.handle)
	state := int32(value)
	if state < 0 {
		return StateDisconnected, &Error{Code: Invalid, Message: "device handle is NULL"}
	}
	return ConnectionState(state), nil
}

// Close ends the session and releases the handle.
func (d *Device) Close() error {
	if d.handle == 0 {
		return nil
	}
	code, _, _ := procClose.Call(d.handle)
	procDeviceFree.Call(d.handle)
	d.handle = 0
	d.closed = true
	return check(code)
}

// Reconnect re-opens the transport and a fresh session.
func (d *Device) Reconnect() error {
	code, _, _ := procReconnect.Call(d.handle)
	return check(code)
}

// SetAutoReconnect toggles automatic recovery.
func (d *Device) SetAutoReconnect(enabled bool) error {
	code, _, _ := procSetReconnect.Call(d.handle, boolArg(enabled))
	return check(code)
}

// ReleaseAll releases every key and button.
func (d *Device) ReleaseAll() error {
	code, _, _ := procReleaseAll.Call(d.handle)
	return check(code)
}

// Barrier waits for the device to finish the queued input.
func (d *Device) Barrier() error {
	code, _, _ := procBarrier.Call(d.handle)
	return check(code)
}

// Bootloader reboots the board into BOOTSEL.
func (d *Device) Bootloader() error {
	code, _, _ := procBootloader.Call(d.handle)
	return check(code)
}

// Command sends one protocol command by opcode and payload and returns its
// reply, or nil for the input commands that have none.
//
// This is the raw escape hatch: the same session, sequence numbers and error
// codes as the typed calls, but the SDK's bookkeeping (held buttons, the mock
// cursor) is not updated. The opcode and payload must name a command the
// protocol defines; anything else fails with Invalid.
func (d *Device) Command(opcode uint8, payload []byte) (*Packet, error) {
	var data *uint8
	if len(payload) > 0 {
		data = &payload[0]
	}
	var raw rawPacket
	code, _, _ := procCommand.Call(
		d.handle,
		uintptr(opcode),
		uintptr(unsafe.Pointer(data)),
		uintptr(len(payload)),
		uintptr(unsafe.Pointer(&raw)),
	)
	if code != 0 {
		return nil, check(code)
	}
	defer procPacketFree.Call(uintptr(unsafe.Pointer(&raw)))
	if raw.Opcode == 0 {
		return nil, nil
	}
	packet := &Packet{
		Opcode:   raw.Opcode,
		Flags:    raw.Flags,
		Session:  raw.Session,
		Sequence: raw.Sequence,
	}
	if raw.Payload != nil && raw.Len > 0 {
		packet.Payload = make([]byte, raw.Len)
		copy(packet.Payload, unsafe.Slice(raw.Payload, raw.Len))
	}
	return packet, nil
}

var (
	callbackMu      sync.Mutex
	nextCallbackID  uintptr = 1
	eventCallbacks          = map[uintptr]func(int32, int32, string){}
	taskCallbacks           = map[uintptr]func() int32{}
	eventTrampoline         = syscall.NewCallback(func(user, kind, code, message uintptr) uintptr {
		callbackMu.Lock()
		callback := eventCallbacks[user]
		callbackMu.Unlock()
		text := ""
		if message != 0 {
			text = cString(message)
		}
		if callback != nil {
			callback(int32(kind), int32(code), text)
		}
		return 0
	})
	taskTrampoline = syscall.NewCallback(func(user uintptr) uintptr {
		callbackMu.Lock()
		callback := taskCallbacks[user]
		callbackMu.Unlock()
		if callback == nil {
			return uintptr(IO)
		}
		return uintptr(callback())
	})
)

// SetEventHandler installs a callback for connection state and errors.
func (d *Device) SetEventHandler(callback func(kind, code int32, message string)) error {
	callbackMu.Lock()
	id := nextCallbackID
	nextCallbackID++
	if callback == nil {
		delete(eventCallbacks, id)
	} else {
		eventCallbacks[id] = callback
	}
	callbackMu.Unlock()
	var code uintptr
	if callback == nil {
		code, _, _ = procSetEvents.Call(d.handle, 0, 0)
	} else {
		d.events[id] = callback
		code, _, _ = procSetEvents.Call(d.handle, eventTrampoline, id)
	}
	return check(code)
}

// Mouse controls buttons, wheel and movement.
type Mouse struct {
	device *Device
}

// MoveBy moves relatively.
func (m *Mouse) MoveBy(dx, dy int32, durationMs uint32, profile *Profile) error {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	code, _, _ := procMoveBy.Call(m.device.handle, uintptr(intArg(dx)), uintptr(intArg(dy)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return check(code)
}

// MoveRaw moves relatively by any amount, split at the ±1024 report limit and
// sent with no barrier; call Barrier where a completion guarantee matters.
func (m *Mouse) MoveRaw(dx, dy int32) error {
	code, _, _ := procMoveRaw.Call(m.device.handle, uintptr(intArg(dx)), uintptr(intArg(dy)))
	return check(code)
}

// MoveTo moves the real pointer to absolute screen coordinates.
func (m *Mouse) MoveTo(x, y int32, durationMs uint32, profile *Profile) error {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	code, _, _ := procMoveTo.Call(m.device.handle, uintptr(intArg(x)), uintptr(intArg(y)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return check(code)
}

// MovePath visits each point in order.
func (m *Mouse) MovePath(points [][2]int32, durationMs uint32, profile *Profile) error {
	if len(points) == 0 {
		return &Error{Code: Invalid, Message: "a path needs at least one point"}
	}
	array := make([]point, len(points))
	for index, value := range points {
		array[index] = point{value[0], value[1]}
	}
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	code, _, _ := procMovePath.Call(m.device.handle, uintptr(unsafe.Pointer(&array[0])), uintptr(len(array)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return check(code)
}

// StartMoveTo starts an absolute move on a worker thread.
func (m *Mouse) StartMoveTo(x, y int32, durationMs uint32, profile *Profile) (*Movement, error) {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	pointer, _, _ := procStartMoveTo.Call(m.device.handle, uintptr(intArg(x)), uintptr(intArg(y)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return newMovement(pointer)
}

// StartMoveBy starts a relative move on a worker thread.
func (m *Mouse) StartMoveBy(dx, dy int32, durationMs uint32, profile *Profile) (*Movement, error) {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	pointer, _, _ := procStartMoveBy.Call(m.device.handle, uintptr(intArg(dx)), uintptr(intArg(dy)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return newMovement(pointer)
}

// Button presses or releases a button.
func (m *Mouse) Button(button Button, down bool) error {
	code, _, _ := procButton.Call(m.device.handle, uintptr(button), boolArg(down))
	return check(code)
}

// Click clicks count times.
func (m *Mouse) Click(button Button, count uint8) error {
	code, _, _ := procClick.Call(m.device.handle, uintptr(button), uintptr(count))
	return check(code)
}

// DoubleClick clicks twice.
func (m *Mouse) DoubleClick(button Button) error {
	code, _, _ := procDoubleClick.Call(m.device.handle, uintptr(button))
	return check(code)
}

// Wheel scrolls; the range per axis is +/-1024.
func (m *Mouse) Wheel(vertical, horizontal int32) error {
	code, _, _ := procWheel.Call(m.device.handle, uintptr(intArg(vertical)), uintptr(intArg(horizontal)))
	return check(code)
}

// Drag presses, moves and releases.
func (m *Mouse) Drag(button Button, x, y int32, durationMs uint32, profile *Profile) error {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	code, _, _ := procDrag.Call(m.device.handle, uintptr(button), uintptr(intArg(x)), uintptr(intArg(y)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return check(code)
}

func intArg(value int32) uintptr { return uintptr(int64(value)) }

// Movement is a running move.
type Movement struct {
	handle uintptr
}

func newMovement(pointer uintptr) (*Movement, error) {
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "movement did not start"}
	}
	return &Movement{handle: pointer}, nil
}

// Cancel stops the movement.
func (m *Movement) Cancel() error {
	code, _, _ := procMoveCancel.Call(m.handle)
	return check(code)
}

// Retarget changes the destination mid-flight.
func (m *Movement) Retarget(x, y int32) error {
	code, _, _ := procMoveRetarget.Call(m.handle, uintptr(intArg(x)), uintptr(intArg(y)))
	return check(code)
}

// Finished reports whether the worker stopped.
func (m *Movement) Finished() bool {
	value, _, _ := procMoveFinished.Call(m.handle)
	return value != 0
}

// Wait blocks up to timeoutMs.
func (m *Movement) Wait(timeoutMs uint32) error {
	code, _, _ := procMoveWait.Call(m.handle, uintptr(timeoutMs))
	return check(code)
}

// Close frees the handle (and cancels a running move).
func (m *Movement) Close() {
	if m.handle != 0 {
		procMoveFree.Call(m.handle)
		m.handle = 0
	}
}

// Keyboard sends keys and text.
type Keyboard struct {
	device *Device
}

// KeyDown presses a usage.
func (k *Keyboard) KeyDown(page, usage uint16) error {
	code, _, _ := procKeyDown.Call(k.device.handle, uintptr(page), uintptr(usage))
	return check(code)
}

// KeyUp releases a usage.
func (k *Keyboard) KeyUp(page, usage uint16) error {
	code, _, _ := procKeyUp.Call(k.device.handle, uintptr(page), uintptr(usage))
	return check(code)
}

// Press taps a key.
func (k *Keyboard) Press(page, usage uint16) error {
	code, _, _ := procKeyPress.Call(k.device.handle, uintptr(page), uintptr(usage))
	return check(code)
}

// LongPress holds a key for holdMs.
func (k *Keyboard) LongPress(page, usage uint16, holdMs uint32) error {
	code, _, _ := procKeyLongPress.Call(k.device.handle, uintptr(page), uintptr(usage), uintptr(holdMs))
	return check(code)
}

// Chord presses all usages, waits, then releases in reverse order.
func (k *Keyboard) Chord(usages []uint16, holdMs uint32) error {
	if len(usages) == 0 {
		return &Error{Code: Invalid, Message: "a chord needs at least one usage"}
	}
	code, _, _ := procChord.Call(k.device.handle, uintptr(unsafe.Pointer(&usages[0])), uintptr(len(usages)), uintptr(holdMs))
	return check(code)
}

// KeyStep is one entry in a key sequence.
type KeyStep struct {
	Page   uint16
	Usage  uint16
	HoldMs uint32
	GapMs  uint32
}

// Sequence runs typed keystrokes in order.
func (k *Keyboard) Sequence(steps []KeyStep) error {
	if len(steps) == 0 {
		return &Error{Code: Invalid, Message: "a sequence needs at least one step"}
	}
	array := make([]keyStep, len(steps))
	for index, step := range steps {
		array[index] = keyStep{step.Page, step.Usage, step.HoldMs, step.GapMs}
	}
	code, _, _ := procSequence.Call(k.device.handle, uintptr(unsafe.Pointer(&array[0])), uintptr(len(array)))
	return check(code)
}

// TypeText types UTF-8 text on a US layout.
func (k *Keyboard) TypeText(text string, intervalMs, holdMs, jitterMs uint32) error {
	value, err := syscall.BytePtrFromString(text)
	if err != nil {
		return &Error{Code: Invalid, Message: err.Error()}
	}
	code, _, _ := procTypeText.Call(k.device.handle, uintptr(unsafe.Pointer(value)), uintptr(intervalMs), uintptr(holdMs), uintptr(jitterMs))
	return check(code)
}

// ReleaseAll releases every key and button.
func (k *Keyboard) ReleaseAll() error { return k.device.ReleaseAll() }

// Scheduler runs tasks with delays, priorities, repeat, pause and cancel.
type Scheduler struct {
	handle    uintptr
	callbacks map[uintptr]func() int32
}

// Scheduler creates a scheduler bound to this device.
func (d *Device) Scheduler() (*Scheduler, error) {
	pointer, _, _ := procSchedNew.Call(d.handle)
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "scheduler did not start"}
	}
	return &Scheduler{handle: pointer, callbacks: map[uintptr]func() int32{}}, nil
}

func (s *Scheduler) register(callback func() int32) uintptr {
	callbackMu.Lock()
	id := nextCallbackID
	nextCallbackID++
	taskCallbacks[id] = callback
	callbackMu.Unlock()
	s.callbacks[id] = callback
	return id
}

// Submit runs a callback once after delayMs.
func (s *Scheduler) Submit(delayMs uint32, priority int32, callback func() int32) (uint64, error) {
	id := s.register(callback)
	value, _, _ := procSchedSubmit.Call(s.handle, uintptr(delayMs), uintptr(intArg(priority)), taskTrampoline, id)
	if value == 0 {
		if failure := lastError(); failure != nil {
			return 0, failure
		}
		return 0, &Error{Code: IO, Message: "submit failed"}
	}
	return uint64(value), nil
}

// Every runs a callback repeatedly until cancelled.
func (s *Scheduler) Every(intervalMs uint32, priority int32, callback func() int32) (uint64, error) {
	id := s.register(callback)
	value, _, _ := procSchedEvery.Call(s.handle, uintptr(intervalMs), uintptr(intArg(priority)), taskTrampoline, id)
	if value == 0 {
		if failure := lastError(); failure != nil {
			return 0, failure
		}
		return 0, &Error{Code: IO, Message: "schedule failed"}
	}
	return uint64(value), nil
}

// Cancel removes a queued or repeating task.
func (s *Scheduler) Cancel(id uint64) error {
	code, _, _ := procSchedCancel.Call(s.handle, uintptr(id))
	return check(code)
}

// Pause stops executing queued tasks.
func (s *Scheduler) Pause() error {
	code, _, _ := procSchedPause.Call(s.handle)
	return check(code)
}

// Resume continues executing tasks.
func (s *Scheduler) Resume() error {
	code, _, _ := procSchedResume.Call(s.handle)
	return check(code)
}

// WaitIdle blocks until the queue is empty.
func (s *Scheduler) WaitIdle(timeoutMs uint32) error {
	code, _, _ := procSchedWaitIdle.Call(s.handle, uintptr(timeoutMs))
	return check(code)
}

// TakeErrors returns failed tasks as decoded JSON objects.
func (s *Scheduler) TakeErrors() ([]map[string]any, error) {
	pointer, _, _ := procSchedErrors.Call(s.handle)
	text, err := take(pointer)
	if err != nil {
		return nil, err
	}
	var errors []map[string]any
	if err := json.Unmarshal([]byte(text), &errors); err != nil {
		return nil, &Error{Code: Framing, Message: err.Error()}
	}
	return errors, nil
}

// Close shuts the scheduler down.
func (s *Scheduler) Close() {
	if s.handle != 0 {
		procSchedFree.Call(s.handle)
		s.handle = 0
	}
	callbackMu.Lock()
	for id := range s.callbacks {
		delete(taskCallbacks, id)
	}
	callbackMu.Unlock()
	s.callbacks = nil
}
