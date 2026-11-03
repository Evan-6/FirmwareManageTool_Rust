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
	ABIMinor   = 9
	VendorID   = 0x03F0
	ProductID  = 0x0024
	UsagePage  = 0xFF60
	Usage      = 0x61
)

// Code mirrors the C gdb_error enum.
type Code int32

const (
	OK            Code = 0
	Invalid       Code = 1
	Timeout       Code = 2
	Disconnected  Code = 3
	Busy          Code = 4
	Lease         Code = 5
	Framing       Code = 6
	Overflow      Code = 7
	Unsupported   Code = 8
	Cancelled     Code = 9
	IO            Code = 10
	InvalidScreen Code = 11 // the anchor names a monitor the system does not report
	StaleWindow   Code = 12 // the window handle is gone, minimised or on no monitor
)

func (c Code) String() string {
	names := []string{"OK", "INVALID", "TIMEOUT", "DISCONNECTED", "BUSY", "LEASE", "FRAMING", "OVERFLOW", "UNSUPPORTED", "CANCELLED", "IO", "INVALID_SCREEN", "STALE_WINDOW"}
	if int(c) >= 0 && int(c) < len(names) {
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

// Metrics is one snapshot of the session's counters, laid out field for field
// like gdb_metrics so the DLL can fill it in place.
//
// The latency figures are microseconds and come from the SDK's histogram: below
// 4 ms they are the exact samples, above it they are the edges of the bucket the
// sample fell in.
type Metrics struct {
	FramesSent     uint64 // reports written, handshake and CLOSE included
	FramesReceived uint64 // replies accepted for this session
	Retries        uint64 // reads that had to be looked past
	StolenFrames   uint64 // the part of Retries that answered someone else
	Reconnects     uint64
	LeaseFaults    uint64
	Timeouts       uint64
	LatencyCount   uint64
	LatencyMinUs   uint64
	LatencyP50Us   uint64
	LatencyP95Us   uint64
	LatencyMaxUs   uint64
}

// Clamps is the count of absolute moves the anchor rectangle had to pull back,
// with the last one's coordinates, laid out field for field like gdb_clamps.
//
// The four coordinates are meaningful when ClampedTotal is not zero, and they
// may be one move behind it: each field is read on its own.
type Clamps struct {
	ClampedTotal uint64
	RequestedX   int32
	RequestedY   int32
	AppliedX     int32
	AppliedY     int32
}

// Monitor is one display, as Monitors lists them: the order ScreenMonitor
// indexes them in, which is not the numbering Windows settings shows.
type Monitor struct {
	Left    int32  `json:"left"`
	Top     int32  `json:"top"`
	Right   int32  `json:"right"`
	Bottom  int32  `json:"bottom"`
	Primary bool   `json:"primary"`
	Dpi     uint32 `json:"dpi"`
}

// Screen is which rectangle an absolute move is clamped into. Coordinates stay
// virtual-desktop physical pixels: the target is not a new origin.
type Screen struct {
	Kind int32 // 0 primary, 1 monitor index, 2 window handle, 3 monitor holding a point
	Arg  int64
}

// ScreenPrimary is the primary monitor, the target a plain MoveTo uses.
func ScreenPrimary() Screen { return Screen{Kind: 0} }

// ScreenMonitor is the index-th monitor in enumeration order, which is not the
// display number Windows settings shows.
func ScreenMonitor(index int32) Screen { return Screen{Kind: 1, Arg: int64(index)} }

// ScreenWindow is a window's bounding rectangle; hwnd is the caller's pointer.
func ScreenWindow(hwnd int64) Screen { return Screen{Kind: 2, Arg: hwnd} }

// ScreenContaining anchors on whichever monitor holds the point, so a caller
// that starts a move from a coordinate does not have to know which display that
// is. The point is packed the way gdb_screen_containing packs it: x in the high
// 32 bits, y in the low 32, both as uint32. A point on no monitor resolves to the
// union of every screen, and the move then fails with Unsupported naming it.
func ScreenContaining(x, y int32) Screen {
	return Screen{Kind: 3, Arg: int64(uint64(uint32(x))<<32 | uint64(uint32(y)))}
}

// rawScreen mirrors gdb_screen, padding included.
type rawScreen struct {
	Kind int32
	Arg  int64
}

func (s Screen) raw() rawScreen { return rawScreen{Kind: s.Kind, Arg: s.Arg} }

var (
	dllOnce sync.Once
	dll     *syscall.LazyDLL

	procLastError      *syscall.LazyProc
	procVersion        *syscall.LazyProc
	procABIVersion     *syscall.LazyProc
	procStringFree     *syscall.LazyProc
	procSetLogLevel    *syscall.LazyProc
	procSetLogSink     *syscall.LazyProc
	procDeviceList     *syscall.LazyProc
	procOpen           *syscall.LazyProc
	procOpenMock       *syscall.LazyProc
	procOpenShared     *syscall.LazyProc
	procOpenMockShared *syscall.LazyProc
	procStolenFrames   *syscall.LazyProc
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
	procMoveToOn       *syscall.LazyProc
	procMovePathOn     *syscall.LazyProc
	procStartMoveToOn  *syscall.LazyProc
	procMetricsGet     *syscall.LazyProc
	procMetricsReset   *syscall.LazyProc
	procMetricsClamps  *syscall.LazyProc
	procMonitorsJSON   *syscall.LazyProc
	procButton         *syscall.LazyProc
	procClick          *syscall.LazyProc
	procClickTimed     *syscall.LazyProc
	procClickAtOn      *syscall.LazyProc
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
	procSchedSubmitAt  *syscall.LazyProc
	procSchedElapsed   *syscall.LazyProc
	procSchedEvery     *syscall.LazyProc
	procSchedCancel    *syscall.LazyProc
	procSchedPause     *syscall.LazyProc
	procSchedResume    *syscall.LazyProc
	procSchedWaitIdle  *syscall.LazyProc
	procSchedErrors    *syscall.LazyProc

	procAttach          *syscall.LazyProc
	procRemoteSpawn     *syscall.LazyProc
	procRemoteFree      *syscall.LazyProc
	procRemoteClose     *syscall.LazyProc
	procRemoteInfo      *syscall.LazyProc
	procRemoteStatus    *syscall.LazyProc
	procRemoteStatusExt *syscall.LazyProc
	procRemoteSessions  *syscall.LazyProc
	procRemoteKey       *syscall.LazyProc
	procRemoteMouseMove *syscall.LazyProc
	procRemoteMoveBy    *syscall.LazyProc
	procRemoteMoveRaw   *syscall.LazyProc
	procRemoteButtons   *syscall.LazyProc
	procRemoteWheel     *syscall.LazyProc
	procRemoteBarrier   *syscall.LazyProc
	procRemoteRelease   *syscall.LazyProc
	procRemoteCommand   *syscall.LazyProc

	procProbe           *syscall.LazyProc
	procProbeAll        *syscall.LazyProc
	procObserverStart   *syscall.LazyProc
	procObserverStop    *syscall.LazyProc
	procObserverFree    *syscall.LazyProc
	procObserverInfo    *syscall.LazyProc
	procObserverState   *syscall.LazyProc
	procObserverStatus  *syscall.LazyProc
	procObserverBarrier *syscall.LazyProc
	procObserverRelease *syscall.LazyProc
	procObserverCommand *syscall.LazyProc

	procWatchStart   *syscall.LazyProc
	procWatchStop    *syscall.LazyProc
	procWatchFree    *syscall.LazyProc
	procWatchListing *syscall.LazyProc
)

func library() *syscall.LazyDLL {
	dllOnce.Do(func() {
		path := os.Getenv("GDBOX_LIBRARY")
		if path == "" {
			path = "gdb.dll"
		}
		dll = syscall.NewLazyDLL(path)
		procLastError = dll.NewProc("gdb_last_error")
		procVersion = dll.NewProc("gdb_version")
		procABIVersion = dll.NewProc("gdb_abi_version")
		procStringFree = dll.NewProc("gdb_string_free")
		procSetLogLevel = dll.NewProc("gdb_set_log_level")
		procSetLogSink = dll.NewProc("gdb_set_log_sink")
		procDeviceList = dll.NewProc("gdb_device_list")
		procOpen = dll.NewProc("gdb_open")
		procOpenMock = dll.NewProc("gdb_open_mock")
		procOpenShared = dll.NewProc("gdb_open_shared")
		procOpenMockShared = dll.NewProc("gdb_open_mock_shared")
		procStolenFrames = dll.NewProc("gdb_stolen_frames")
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
		procMoveToOn = dll.NewProc("gdb_mouse_move_to_on")
		procMovePathOn = dll.NewProc("gdb_mouse_move_path_on")
		procStartMoveToOn = dll.NewProc("gdb_mouse_start_move_to_on")
		procMetricsGet = dll.NewProc("gdb_metrics_get")
		procMetricsReset = dll.NewProc("gdb_metrics_reset")
		procMetricsClamps = dll.NewProc("gdb_metrics_clamps")
		procMonitorsJSON = dll.NewProc("gdb_monitors_json")
		procButton = dll.NewProc("gdb_mouse_button")
		procClick = dll.NewProc("gdb_mouse_click")
		procClickTimed = dll.NewProc("gdb_mouse_click_timed")
		procClickAtOn = dll.NewProc("gdb_mouse_click_at_on")
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
		procSchedSubmitAt = dll.NewProc("gdb_scheduler_submit_at")
		procSchedElapsed = dll.NewProc("gdb_scheduler_elapsed_ms")
		procSchedEvery = dll.NewProc("gdb_scheduler_every")
		procSchedCancel = dll.NewProc("gdb_scheduler_cancel")
		procSchedPause = dll.NewProc("gdb_scheduler_pause")
		procSchedResume = dll.NewProc("gdb_scheduler_resume")
		procSchedWaitIdle = dll.NewProc("gdb_scheduler_wait_idle")
		procSchedErrors = dll.NewProc("gdb_scheduler_take_errors")
		procAttach = dll.NewProc("gdb_attach")
		procRemoteSpawn = dll.NewProc("gdb_remote_spawn")
		procRemoteFree = dll.NewProc("gdb_remote_free")
		procRemoteClose = dll.NewProc("gdb_remote_close")
		procRemoteInfo = dll.NewProc("gdb_remote_info")
		procRemoteStatus = dll.NewProc("gdb_remote_status")
		procRemoteStatusExt = dll.NewProc("gdb_remote_status_ext")
		procRemoteSessions = dll.NewProc("gdb_remote_sessions")
		procRemoteKey = dll.NewProc("gdb_remote_key")
		procRemoteMouseMove = dll.NewProc("gdb_remote_mouse_move")
		procRemoteMoveBy = dll.NewProc("gdb_remote_move_by")
		procRemoteMoveRaw = dll.NewProc("gdb_remote_move_raw")
		procRemoteButtons = dll.NewProc("gdb_remote_mouse_buttons")
		procRemoteWheel = dll.NewProc("gdb_remote_wheel")
		procRemoteBarrier = dll.NewProc("gdb_remote_barrier")
		procRemoteRelease = dll.NewProc("gdb_remote_release_all")
		procRemoteCommand = dll.NewProc("gdb_remote_command")
		procProbe = dll.NewProc("gdb_probe")
		procProbeAll = dll.NewProc("gdb_probe_all")
		procObserverStart = dll.NewProc("gdb_observer_start")
		procObserverStop = dll.NewProc("gdb_observer_stop")
		procObserverFree = dll.NewProc("gdb_observer_free")
		procObserverInfo = dll.NewProc("gdb_observer_info")
		procObserverState = dll.NewProc("gdb_observer_state")
		procObserverStatus = dll.NewProc("gdb_observer_status")
		procObserverBarrier = dll.NewProc("gdb_observer_barrier")
		procObserverRelease = dll.NewProc("gdb_observer_release_all")
		procObserverCommand = dll.NewProc("gdb_observer_command")
		procWatchStart = dll.NewProc("gdb_watch_start")
		procWatchStop = dll.NewProc("gdb_watch_stop")
		procWatchFree = dll.NewProc("gdb_watch_free")
		procWatchListing = dll.NewProc("gdb_watch_listing")
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

// Version is the SDK version string the library was built from, e.g. "0.1.0".
func Version() string {
	library()
	pointer, _, _ := procVersion.Call()
	return cString(pointer)
}

// LibraryABIVersion is the major version of the C ABI the loaded library
// exports; one that disagrees with ABIVersion is a library this binding cannot
// drive. It is a function rather than a constant because only the library
// knows, and the constant is what the binding was written against.
func LibraryABIVersion() uint32 {
	library()
	value, _, _ := procABIVersion.Call()
	return uint32(value)
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

// OpenShared is Open for a board other programs may be driving too: a reply
// that never arrives is Code Busy naming the command instead of Code Timeout,
// the session is never reopened behind the caller's back, and StolenFrames
// counts the replies that were not ours. Which program gets a given reply is
// still the callers' discipline.
func OpenShared(selector string, timeoutMs uint32, autoReconnect, heartbeat bool) (*Device, error) {
	library()
	var pointer uintptr
	if selector == "" {
		pointer, _, _ = procOpenShared.Call(0, uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
	} else {
		name, cerr := syscall.BytePtrFromString(selector)
		if cerr != nil {
			return nil, &Error{Code: Invalid, Message: cerr.Error()}
		}
		pointer, _, _ = procOpenShared.Call(uintptr(unsafe.Pointer(name)), uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
	}
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

// OpenMockShared opens the in-memory device in shared mode, for a self-test
// that wants the stolen-reply path without hardware.
func OpenMockShared(timeoutMs uint32, autoReconnect, heartbeat bool) (*Device, error) {
	library()
	pointer, _, _ := procOpenMockShared.Call(uintptr(timeoutMs), boolArg(autoReconnect), boolArg(heartbeat))
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

// StolenFrames is the number of replies the reader looked past because they
// answered another session or another sequence.
func (d *Device) StolenFrames() uint64 {
	library()
	count, _, _ := procStolenFrames.Call(d.handle)
	return uint64(count)
}

// Metrics reads the session's counters once: frames, retries, lease faults and
// the latency percentiles, without a copy of the SDK.
func (d *Device) Metrics() (Metrics, error) {
	library()
	var value Metrics
	code, _, _ := procMetricsGet.Call(d.handle, uintptr(unsafe.Pointer(&value)))
	if code != 0 {
		return Metrics{}, check(code)
	}
	return value, nil
}

// ResetMetrics zeroes every counter and every latency bucket.
func (d *Device) ResetMetrics() error {
	library()
	code, _, _ := procMetricsReset.Call(d.handle)
	return check(code)
}

// Clamps reports the absolute moves whose destination was outside the anchor
// rectangle and were pulled back into it, plus the last one's coordinates.
//
// The move still converges and the click still lands inside the window: only
// these numbers say the requested pixel was never reached, which is the shape
// of a multi-monitor mistake. The coordinates are meaningful when ClampedTotal
// is not zero.
func (d *Device) Clamps() (Clamps, error) {
	library()
	var value Clamps
	code, _, _ := procMetricsClamps.Call(d.handle, uintptr(unsafe.Pointer(&value)))
	if code != 0 {
		return Clamps{}, check(code)
	}
	return value, nil
}

// MonitorsJSON lists every monitor as the ABI's JSON array, in exactly the order
// ScreenMonitor indexes them. No handle is needed and no packet is sent.
func MonitorsJSON() (string, error) {
	library()
	pointer, _, _ := procMonitorsJSON.Call()
	return take(pointer)
}

// Monitors is MonitorsJSON decoded: one entry per display, so a caller reads the
// rectangles instead of guessing an index from the display number Windows
// settings shows.
func Monitors() ([]Monitor, error) {
	text, err := MonitorsJSON()
	if err != nil {
		return nil, err
	}
	var monitors []Monitor
	if err := json.Unmarshal([]byte(text), &monitors); err != nil {
		return nil, &Error{Code: Framing, Message: err.Error()}
	}
	return monitors, nil
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

// Remote is a client of a broker process: one program owns the device and this
// is how every other program drives it, over \\.\pipe\gdbox-<name>.
//
//	remote, err := gdb.Attach("")          // the "default" pipe
//	if err != nil { log.Fatal(err) }
//	defer remote.Close()
//	fmt.Println(remote.StatusJSON())
//	remote.MoveRaw(400, -120)
//	remote.Barrier()
//
// The JSON accessors return exactly what Device's return, so one parser serves
// both. What an attached handle cannot do is what needs the collection itself:
// absolute moves, drags, typing, chords, flashing and the bootloader.
type Remote struct {
	handle uintptr
	name   string
}

// Attach connects to the broker on \\.\pipe\gdbox-<name>; "" means "default".
// Nothing listening is Disconnected; a name with a path separator is Invalid.
func Attach(name string) (*Remote, error) {
	library()
	var pointer uintptr
	var code uintptr
	if name == "" {
		code, _, _ = procAttach.Call(0, uintptr(unsafe.Pointer(&pointer)))
	} else {
		value, err := syscall.BytePtrFromString(name)
		if err != nil {
			return nil, &Error{Code: Invalid, Message: err.Error()}
		}
		code, _, _ = procAttach.Call(uintptr(unsafe.Pointer(value)), uintptr(unsafe.Pointer(&pointer)))
	}
	if code != 0 {
		return nil, check(code)
	}
	if pointer == 0 {
		return nil, &Error{Code: IO, Message: "attach returned OK without a handle"}
	}
	if name == "" {
		name = "default"
	}
	return &Remote{handle: pointer, name: name}, nil
}

// Spawn starts a broker when none is listening on name, then returns; attach
// afterwards. The executable comes from GDBOX_BROKER_EXE, a host that serves
// --gdbox-broker <name>; without it there is nothing to start and this is
// Unsupported. A broker that is already listening is a no-op.
func Spawn(name string) error {
	library()
	var code uintptr
	if name == "" {
		code, _, _ = procRemoteSpawn.Call(0)
	} else {
		value, err := syscall.BytePtrFromString(name)
		if err != nil {
			return &Error{Code: Invalid, Message: err.Error()}
		}
		code, _, _ = procRemoteSpawn.Call(uintptr(unsafe.Pointer(value)))
	}
	return check(code)
}

// Handle exposes the raw pointer (for debugging only).
func (r *Remote) Handle() uintptr { return r.handle }

// Name is the broker this handle is attached to ("default" when unnamed).
func (r *Remote) Name() string { return r.name }

// InfoJSON returns the device information as JSON, like Device.InfoJSON.
func (r *Remote) InfoJSON() (string, error) {
	pointer, _, _ := procRemoteInfo.Call(r.handle)
	return take(pointer)
}

// StatusJSON returns STATUS as JSON, like Device.StatusJSON.
func (r *Remote) StatusJSON() (string, error) {
	pointer, _, _ := procRemoteStatus.Call(r.handle)
	return take(pointer)
}

// StatusExtJSON returns STATUS_EXT as JSON, like Device.StatusExtJSON.
func (r *Remote) StatusExtJSON() (string, error) {
	pointer, _, _ := procRemoteStatusExt.Call(r.handle)
	return take(pointer)
}

// SessionsJSON returns SESSION_LIST as JSON, like Device.SessionsJSON.
func (r *Remote) SessionsJSON() (string, error) {
	pointer, _, _ := procRemoteSessions.Call(r.handle)
	return take(pointer)
}

// Key presses or releases one usage through the broker.
func (r *Remote) Key(page, usage uint16, down bool) error {
	code, _, _ := procRemoteKey.Call(r.handle, uintptr(page), uintptr(usage), boolArg(down))
	return check(code)
}

// MouseMove sends one MOUSE_MOVE report; the deltas must fit ±1024 of the wire.
func (r *Remote) MouseMove(dx, dy int16) error {
	code, _, _ := procRemoteMouseMove.Call(r.handle, uintptr(intArg(int32(dx))), uintptr(intArg(int32(dy))))
	return check(code)
}

// MoveBy moves relatively, paced inside the broker; durationMs 0 picks a
// natural speed from the distance.
func (r *Remote) MoveBy(dx, dy int32, durationMs uint32, profile *Profile) error {
	var raw *rawProfile
	if profile != nil {
		value := profile.raw()
		raw = &value
	}
	code, _, _ := procRemoteMoveBy.Call(r.handle, uintptr(intArg(dx)), uintptr(intArg(dy)), uintptr(durationMs), uintptr(unsafe.Pointer(raw)))
	return check(code)
}

// MoveRaw moves relatively by any amount, split at the ±1024 report limit and
// sent with no barrier; call Barrier where a completion guarantee matters.
func (r *Remote) MoveRaw(dx, dy int32) error {
	code, _, _ := procRemoteMoveRaw.Call(r.handle, uintptr(intArg(dx)), uintptr(intArg(dy)))
	return check(code)
}

// MouseButtons sets the raw HID button mask: left=1, right=2, middle=4,
// back=8, forward=16.
func (r *Remote) MouseButtons(bits uint8) error {
	code, _, _ := procRemoteButtons.Call(r.handle, uintptr(bits))
	return check(code)
}

// Wheel scrolls; the range per axis is ±1024.
func (r *Remote) Wheel(vertical, horizontal int32) error {
	code, _, _ := procRemoteWheel.Call(r.handle, uintptr(intArg(vertical)), uintptr(intArg(horizontal)))
	return check(code)
}

// Barrier waits for the device to finish the queued input.
func (r *Remote) Barrier() error {
	code, _, _ := procRemoteBarrier.Call(r.handle)
	return check(code)
}

// ReleaseAll releases every key and button.
func (r *Remote) ReleaseAll() error {
	code, _, _ := procRemoteRelease.Call(r.handle)
	return check(code)
}

// Command sends one protocol command by opcode and payload through the broker
// and returns its reply, or nil for the input commands that have none.
func (r *Remote) Command(opcode uint8, payload []byte) (*Packet, error) {
	var data *uint8
	if len(payload) > 0 {
		data = &payload[0]
	}
	var raw rawPacket
	code, _, _ := procRemoteCommand.Call(
		r.handle,
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

// Close hangs up on the broker and releases the handle.
func (r *Remote) Close() error {
	if r.handle == 0 {
		return nil
	}
	code, _, _ := procRemoteClose.Call(r.handle)
	procRemoteFree.Call(r.handle)
	r.handle = 0
	return check(code)
}

// Presence: Feature 14 only, no session, no packet.
//
// The identity arrives in GET_FEATURE(14), a control transfer that never touches
// the interrupt IN queue, so this is safe while another program holds the
// session. A command is not: it consumes the shared queue, and a second reader
// that drains a reply the owner was waiting for kills that owner's session. A
// program that must not disturb another one probes or observes, and does not
// open a Device at all.

// Probe returns the identity as the JSON object InfoJSON returns. selector is a
// serial, a HID path, a unique fragment of either, or "" for the first board.
func Probe(selector string) (string, error) {
	library()
	var pointer uintptr
	if selector == "" {
		pointer, _, _ = procProbe.Call(0)
	} else {
		name, err := syscall.BytePtrFromString(selector)
		if err != nil {
			return "", &Error{Code: Invalid, Message: err.Error()}
		}
		pointer, _, _ = procProbe.Call(uintptr(unsafe.Pointer(name)))
	}
	return take(pointer)
}

// ProbeAll returns a JSON array with one object per attached vendor collection.
// An empty bus is "[]", not an error.
func ProbeAll() (string, error) {
	library()
	pointer, _, _ := procProbeAll.Call()
	return take(pointer)
}

// Observer is presence on a timer: Feature 14 every interval, and no way to
// command the board.
type Observer struct {
	handle uintptr
}

// Observe starts polling the board selector names ("" for the first one). It
// fails when the board is not there, so a handle always has an identity.
func Observe(selector string, intervalMs uint32) (*Observer, error) {
	library()
	var pointer uintptr
	if selector == "" {
		pointer, _, _ = procObserverStart.Call(0, uintptr(intervalMs))
	} else {
		name, err := syscall.BytePtrFromString(selector)
		if err != nil {
			return nil, &Error{Code: Invalid, Message: err.Error()}
		}
		pointer, _, _ = procObserverStart.Call(uintptr(unsafe.Pointer(name)), uintptr(intervalMs))
	}
	if pointer == 0 {
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "no board to observe"}
	}
	return &Observer{handle: pointer}, nil
}

// InfoJSON returns the last probe as JSON, the object InfoJSON returns.
func (o *Observer) InfoJSON() (string, error) {
	pointer, _, _ := procObserverInfo.Call(o.handle)
	return take(pointer)
}

// State is 2 while the last poll answered and 0 while it failed.
func (o *Observer) State() (ConnectionState, error) {
	value, _, _ := procObserverState.Call(o.handle)
	state := int32(value)
	if state < 0 {
		return StateDisconnected, &Error{Code: Invalid, Message: "observer handle is NULL"}
	}
	return ConnectionState(state), nil
}

// Stop stops the polling thread; the handle stays valid.
func (o *Observer) Stop() {
	if o.handle == 0 {
		return
	}
	procObserverStop.Call(o.handle)
}

// Close stops the thread and releases the handle.
func (o *Observer) Close() {
	if o.handle == 0 {
		return
	}
	procObserverStop.Call(o.handle)
	procObserverFree.Call(o.handle)
	o.handle = 0
}

// Status, Barrier, ReleaseAll and Command are refused with Unsupported: an
// observer has no session and must not put a frame on the shared IN queue.
func (o *Observer) Status() error {
	code, _, _ := procObserverStatus.Call(o.handle)
	return check(code)
}

// Barrier is always Unsupported.
func (o *Observer) Barrier() error {
	code, _, _ := procObserverBarrier.Call(o.handle)
	return check(code)
}

// ReleaseAll is always Unsupported.
func (o *Observer) ReleaseAll() error {
	code, _, _ := procObserverRelease.Call(o.handle)
	return check(code)
}

// Command is always Unsupported, whatever the opcode.
func (o *Observer) Command(opcode uint8, payload []byte) error {
	var data *uint8
	if len(payload) > 0 {
		data = &payload[0]
	}
	code, _, _ := procObserverCommand.Call(o.handle, uintptr(opcode), uintptr(unsafe.Pointer(data)), uintptr(len(payload)), 0)
	return check(code)
}

var (
	callbackMu      sync.Mutex
	nextCallbackID  uintptr = 1
	eventCallbacks          = map[uintptr]func(int32, int32, string){}
	taskCallbacks           = map[uintptr]func() int32{}
	watchCallbacks          = map[uintptr]func(int32, map[string]any){}
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
	// The watch thread calls this with one JSON object per event; the decode
	// happens here so a caller never sees the DLL's buffer, which is only valid
	// for the duration of the call.
	watchTrampoline = syscall.NewCallback(func(user, kind, body uintptr) uintptr {
		callbackMu.Lock()
		callback := watchCallbacks[user]
		callbackMu.Unlock()
		if callback == nil {
			return 0
		}
		text := "{}"
		if body != 0 {
			text = cString(body)
		}
		event := map[string]any{}
		if err := json.Unmarshal([]byte(text), &event); err != nil {
			event = map[string]any{"json": text}
		}
		callback(int32(kind), event)
		return 0
	})
)

// Watch kinds, as the C ABI reports them.
const (
	WatchArrived    int32 = 0
	WatchRemoved    int32 = 1
	WatchBootloader int32 = 2
)

// Watch reports device arrivals and removals without polling the bus.
//
//	watched, err := gdb.NewWatch(250, func(kind int32, event map[string]any) {
//		fmt.Println(kind, event["serial"])
//	})
//	if err != nil { log.Fatal(err) }
//	defer watched.Close()
//
// The callback runs on the watch's own thread: it must return quickly, must not
// call Stop or Close (the stop joins that very thread), and must copy anything it
// keeps from `event`, which belongs to the call. An event is a device entry
// (path, serial, product, manufacturer, vid, pid) or {"path": "E:\\"} for an
// RP2040 bootloader drive.
type Watch struct {
	handle uintptr
	id     uintptr
}

// NewWatch starts the watch thread; intervalMs under 10 ms is rounded up. The
// first poll reports what is already attached as arrivals.
func NewWatch(intervalMs uint32, callback func(kind int32, event map[string]any)) (*Watch, error) {
	library()
	var id, trampoline uintptr
	if callback != nil {
		id, trampoline = registerWatch(callback), watchTrampoline
	}
	pointer, _, _ := procWatchStart.Call(uintptr(intervalMs), trampoline, id)
	if pointer == 0 {
		unregisterWatch(id)
		if failure := lastError(); failure != nil {
			return nil, failure
		}
		return nil, &Error{Code: IO, Message: "watch did not start"}
	}
	return &Watch{handle: pointer, id: id}, nil
}

// ListingJSON returns the bus right now, as
// {"devices":[{...}], "volumes":["E:\\"]}.
func (w *Watch) ListingJSON() (string, error) {
	library()
	pointer, _, _ := procWatchListing.Call(w.handle)
	return take(pointer)
}

// Stop stops and joins the thread; the handle stays valid for the listing.
func (w *Watch) Stop() {
	if w.handle != 0 {
		procWatchStop.Call(w.handle)
	}
}

// Close stops the thread and releases the handle.
func (w *Watch) Close() {
	if w.handle == 0 {
		return
	}
	procWatchStop.Call(w.handle)
	procWatchFree.Call(w.handle)
	w.handle = 0
	unregisterWatch(w.id)
}

// registerWatch reserves the id the C side hands back as user data and keeps the
// function under it; the entry must outlive the watch thread.
func registerWatch(callback func(int32, map[string]any)) uintptr {
	callbackMu.Lock()
	defer callbackMu.Unlock()
	id := nextCallbackID
	nextCallbackID++
	watchCallbacks[id] = callback
	return id
}

func unregisterWatch(id uintptr) {
	if id == 0 {
		return
	}
	callbackMu.Lock()
	delete(watchCallbacks, id)
	callbackMu.Unlock()
}

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

// MoveToOn is MoveTo clamped into `screen` instead of the primary monitor: a
// game on a second screen or in a window is not reachable from the primary's
// rectangle. A destination that is on no screen at all fails with Unsupported.
//
// A 16-byte struct is passed by reference on the amd64 calling convention, which
// is what gdb_screen compiles to; Go's syscall carries uintptr arguments only, so
// this hands the DLL the address of a copy.
func (m *Mouse) MoveToOn(x, y int32, screen Screen, durationMs uint32, profile *Profile) error {
	raw, prof := screen.raw(), profileArg(profile)
	code, _, _ := procMoveToOn.Call(
		m.device.handle,
		uintptr(intArg(x)),
		uintptr(intArg(y)),
		uintptr(unsafe.Pointer(&raw)),
		uintptr(durationMs),
		uintptr(unsafe.Pointer(prof)),
	)
	return check(code)
}

// MovePathOn is MovePath against an explicit screen target.
func (m *Mouse) MovePathOn(points [][2]int32, screen Screen, durationMs uint32, profile *Profile) error {
	array, err := pathPoints(points)
	if err != nil {
		return err
	}
	raw, prof := screen.raw(), profileArg(profile)
	code, _, _ := procMovePathOn.Call(
		m.device.handle,
		uintptr(unsafe.Pointer(&array[0])),
		uintptr(len(array)),
		uintptr(unsafe.Pointer(&raw)),
		uintptr(durationMs),
		uintptr(unsafe.Pointer(prof)),
	)
	return check(code)
}

// StartMoveToOn starts a screen-targeted absolute move on a worker thread.
func (m *Mouse) StartMoveToOn(x, y int32, screen Screen, durationMs uint32, profile *Profile) (*Movement, error) {
	raw, prof := screen.raw(), profileArg(profile)
	pointer, _, _ := procStartMoveToOn.Call(
		m.device.handle,
		uintptr(intArg(x)),
		uintptr(intArg(y)),
		uintptr(unsafe.Pointer(&raw)),
		uintptr(durationMs),
		uintptr(unsafe.Pointer(prof)),
	)
	return newMovement(pointer)
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

// ClickTimed clicks count times with the press length the caller asked for.
//
// holdMs is how long the button stays down and gapMs how long to wait after the
// release; 0 for either draws that half the way Click does. jitterMs spreads
// each half by up to that much in either direction, so 0 is an exact press.
func (m *Mouse) ClickTimed(button Button, count uint8, holdMs, gapMs, jitterMs uint32) error {
	code, _, _ := procClickTimed.Call(
		m.device.handle,
		uintptr(button),
		uintptr(count),
		uintptr(holdMs),
		uintptr(gapMs),
		uintptr(jitterMs),
	)
	return check(code)
}

// ClickAtOn moves to x, y on `screen` and clicks there, in one call.
func (m *Mouse) ClickAtOn(x, y int32, screen Screen, button Button, count uint8, holdMs, gapMs, jitterMs uint32) error {
	raw := screen.raw()
	code, _, _ := procClickAtOn.Call(
		m.device.handle,
		uintptr(intArg(x)),
		uintptr(intArg(y)),
		uintptr(unsafe.Pointer(&raw)),
		uintptr(button),
		uintptr(count),
		uintptr(holdMs),
		uintptr(gapMs),
		uintptr(jitterMs),
	)
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

// profileArg is the pointer the ABI reads; nil means "the SDK defaults".
func profileArg(profile *Profile) *rawProfile {
	if profile == nil {
		return nil
	}
	value := profile.raw()
	return &value
}

// pathPoints copies a caller's path into the C layout, refusing an empty one.
func pathPoints(points [][2]int32) ([]point, error) {
	if len(points) == 0 {
		return nil, &Error{Code: Invalid, Message: "a path needs at least one point"}
	}
	array := make([]point, len(points))
	for index, value := range points {
		array[index] = point{value[0], value[1]}
	}
	return array, nil
}

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
// ElapsedMs is milliseconds since the scheduler started; the unit SubmitAt takes.
func (s *Scheduler) ElapsedMs() uint64 {
	value, _, _ := procSchedElapsed.Call(s.handle)
	return uint64(value)
}

// SubmitAt queues at an absolute due. Jobs sharing a due are ordered by
// priority; Submit stamps its due from the clock, so two calls a millisecond
// apart never tie.
func (s *Scheduler) SubmitAt(dueMs uint64, priority int32, callback func() int32) (uint64, error) {
	id := s.register(callback)
	value, _, _ := procSchedSubmitAt.Call(s.handle, uintptr(dueMs), uintptr(intArg(priority)), taskTrampoline, id)
	if value == 0 {
		if failure := lastError(); failure != nil {
			return 0, failure
		}
		return 0, &Error{Code: IO, Message: "submit failed"}
	}
	return uint64(value), nil
}

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
