//go:build windows

package gdb

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func openMock(t *testing.T) *Device {
	t.Helper()
	device, err := OpenMock()
	if err != nil {
		t.Fatalf("OpenMock: %v", err)
	}
	t.Cleanup(func() { _ = device.Close() })
	return device
}

func TestMockReportsTheFirmwareContract(t *testing.T) {
	device := openMock(t)

	text, err := device.InfoJSON()
	if err != nil {
		t.Fatal(err)
	}
	var info map[string]any
	if err := json.Unmarshal([]byte(text), &info); err != nil {
		t.Fatal(err)
	}
	if info["board"].(float64) != 2 || info["firmware"] != "3.2.0" {
		t.Fatalf("unexpected info: %s", text)
	}
	if info["capabilities"].(float64) != 15 || info["leaseMs"].(float64) != 2000 {
		t.Fatalf("unexpected capabilities: %s", text)
	}

	state, err := device.State()
	if err != nil || state != StateConnected {
		t.Fatalf("state = %v, %v", state, err)
	}

	statusText, err := device.StatusJSON()
	if err != nil {
		t.Fatal(err)
	}
	var status map[string]any
	if err := json.Unmarshal([]byte(statusText), &status); err != nil {
		t.Fatal(err)
	}
	if status["leaseMs"].(float64) != 2000 {
		t.Fatalf("unexpected status: %s", statusText)
	}

	sessionsText, err := device.SessionsJSON()
	if err != nil {
		t.Fatal(err)
	}
	var sessions []map[string]any
	if err := json.Unmarshal([]byte(sessionsText), &sessions); err != nil {
		t.Fatal(err)
	}
	if len(sessions) != 1 {
		t.Fatalf("expected one owner, got %s", sessionsText)
	}
}

func TestSharedOpenReportsTheStolenFrames(t *testing.T) {
	device, err := OpenMockShared(1000, false, false)
	if err != nil {
		t.Fatalf("OpenMockShared: %v", err)
	}
	defer func() { _ = device.Close() }()

	if device.StolenFrames() != 0 {
		t.Fatalf("a quiet board reported %d stolen frames", device.StolenFrames())
	}
	if _, err := device.StatusJSON(); err != nil {
		t.Fatalf("a shared handle still answers STATUS: %v", err)
	}
	if device.StolenFrames() != 0 {
		t.Fatal("a reply of our own must not count as stolen")
	}
}

func TestKeyboardRejectsUnknownUsages(t *testing.T) {
	device := openMock(t)
	if err := device.Keyboard().TypeText("hello", 1, 1, 0); err != nil {
		t.Fatal(err)
	}
	if err := device.Keyboard().Chord([]uint16{0xE0, 0x06}, 5); err != nil {
		t.Fatal(err)
	}
	err := device.Keyboard().KeyDown(7, 0x01)
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Invalid {
		t.Fatalf("expected Invalid, got %v", err)
	}
}

func TestMouseAndMovement(t *testing.T) {
	device := openMock(t)
	profile := LinearProfile()
	mouse := device.Mouse()
	if err := mouse.MoveBy(120, -40, 20, &profile); err != nil {
		t.Fatal(err)
	}
	if err := mouse.MoveTo(400, 300, 50, nil); err != nil {
		t.Fatal(err)
	}
	if err := mouse.Click(Left, 2); err != nil {
		t.Fatal(err)
	}
	if err := mouse.Wheel(120, 0); err != nil {
		t.Fatal(err)
	}
	if err := mouse.Wheel(4000, 0); err == nil {
		t.Fatal("expected the wheel range to be enforced")
	}

	movement, err := mouse.StartMoveBy(400, 400, 400, &profile)
	if err != nil {
		t.Fatal(err)
	}
	if err := movement.Cancel(); err != nil {
		t.Fatal(err)
	}
	err = movement.Wait(2000)
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Cancelled {
		t.Fatalf("expected Cancelled, got %v", err)
	}
	movement.Close()
}

func TestLoggingReachesTheSink(t *testing.T) {
	var mu sync.Mutex
	var lines []string
	if err := SetLogSink(func(level int32, message string) {
		mu.Lock()
		lines = append(lines, message)
		mu.Unlock()
	}); err != nil {
		t.Fatal(err)
	}
	defer func() {
		_ = SetLogSink(nil)
		_ = SetLogLevel(LogOff)
	}()
	if err := SetLogLevel(LogInfo); err != nil {
		t.Fatal(err)
	}

	device, err := OpenMock()
	if err != nil {
		t.Fatal(err)
	}
	if err := device.Close(); err != nil {
		t.Fatal(err)
	}

	mu.Lock()
	defer mu.Unlock()
	if len(lines) == 0 {
		t.Fatal("expected the close to reach the log sink")
	}
}

func TestCommandRoundTripsAStatusReply(t *testing.T) {
	device := openMock(t)

	reply, err := device.Command(0x06, nil)
	if err != nil {
		t.Fatalf("STATUS: %v", err)
	}
	if reply == nil {
		t.Fatal("STATUS must answer")
	}
	if reply.Opcode != 0x86 || len(reply.Payload) != 51 || reply.Payload[0] != 0 {
		t.Fatalf("unexpected STATUS reply: %+v", reply)
	}

	// MOUSE_MOVE has no reply: the call succeeds and returns nil.
	reply, err = device.Command(0x12, []byte{0x2c, 0x01, 0xd8, 0xff})
	if err != nil {
		t.Fatalf("MOUSE_MOVE: %v", err)
	}
	if reply != nil {
		t.Fatalf("an input command must not report a reply: %+v", reply)
	}

	var failure *Error
	if _, err := device.Command(0x7f, nil); !errors.As(err, &failure) || failure.Code != Invalid {
		t.Fatalf("expected Invalid for an unknown opcode, got %v", err)
	}
}

func TestMoveRawSplitsLargeDeltas(t *testing.T) {
	device := openMock(t)
	if err := device.Mouse().MoveRaw(2400, -1200); err != nil {
		t.Fatalf("MoveRaw: %v", err)
	}
	if err := device.Mouse().MoveRaw(-30000, 30000); err != nil {
		t.Fatalf("MoveRaw with a large delta: %v", err)
	}
	// No barrier was sent by MoveRaw itself; the session still flushes cleanly.
	if err := device.Barrier(); err != nil {
		t.Fatalf("Barrier: %v", err)
	}
	if state, err := device.State(); err != nil || state != StateConnected {
		t.Fatalf("state after raw moves = %v, %v", state, err)
	}
}

func TestSchedulerOrderAndErrors(t *testing.T) {
	device := openMock(t)
	scheduler, err := device.Scheduler()
	if err != nil {
		t.Fatal(err)
	}
	defer scheduler.Close()

	if err := scheduler.Pause(); err != nil {
		t.Fatal(err)
	}
	var order int32
	if _, err := scheduler.Submit(0, 0, func() int32 { atomic.AddInt32(&order, 1); return 0 }); err != nil {
		t.Fatal(err)
	}
	if _, err := scheduler.Submit(0, 10, func() int32 { atomic.AddInt32(&order, 10); return 0 }); err != nil {
		t.Fatal(err)
	}
	if err := scheduler.Resume(); err != nil {
		t.Fatal(err)
	}
	if err := scheduler.WaitIdle(2000); err != nil {
		t.Fatal(err)
	}
	if got := atomic.LoadInt32(&order); got != 11 {
		t.Fatalf("priority order broken: %d", got)
	}

	if _, err := scheduler.Submit(0, 0, func() int32 { return int32(Invalid) }); err != nil {
		t.Fatal(err)
	}
	if err := scheduler.WaitIdle(2000); err != nil {
		t.Fatal(err)
	}
	errorsSeen, err := scheduler.TakeErrors()
	if err != nil {
		t.Fatal(err)
	}
	if len(errorsSeen) != 1 || errorsSeen[0]["code"].(float64) != float64(Invalid) {
		t.Fatalf("unexpected errors: %v", errorsSeen)
	}

	var ticks int32
	id, err := scheduler.Every(10, 0, func() int32 { atomic.AddInt32(&ticks, 1); return 0 })
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(60 * time.Millisecond)
	if err := scheduler.Cancel(id); err != nil {
		t.Fatal(err)
	}
	if atomic.LoadInt32(&ticks) < 2 {
		t.Fatalf("periodic task did not run: %d", ticks)
	}
}

// The attached surface. No broker runs here, so these cover what a caller can
// rely on without one: the symbols, the parity with Device, and the typed errors.
func TestRemoteAndDeviceShareTheSameNames(t *testing.T) {
	// Method expressions: this stops compiling the day the two drift apart.
	var _ func(*Device) (string, error) = (*Device).InfoJSON
	var _ func(*Remote) (string, error) = (*Remote).InfoJSON
	var _ func(*Device) (string, error) = (*Device).StatusJSON
	var _ func(*Remote) (string, error) = (*Remote).StatusJSON
	var _ func(*Device) (string, error) = (*Device).StatusExtJSON
	var _ func(*Remote) (string, error) = (*Remote).StatusExtJSON
	var _ func(*Device) (string, error) = (*Device).SessionsJSON
	var _ func(*Remote) (string, error) = (*Remote).SessionsJSON
	var _ func(*Device) error = (*Device).Barrier
	var _ func(*Remote) error = (*Remote).Barrier
	var _ func(*Device) error = (*Device).ReleaseAll
	var _ func(*Remote) error = (*Remote).ReleaseAll
	var _ func(*Device, uint8, []byte) (*Packet, error) = (*Device).Command
	var _ func(*Remote, uint8, []byte) (*Packet, error) = (*Remote).Command
	var _ func(*Device) error = (*Device).Close
	var _ func(*Remote) error = (*Remote).Close
}

func TestRemoteAttachWithoutABrokerIsDisconnected(t *testing.T) {
	remote, err := Attach(fmt.Sprintf("go-test-no-broker-%d", os.Getpid()))
	if err == nil {
		_ = remote.Close()
		t.Fatal("attach succeeded with no broker listening")
	}
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Disconnected {
		t.Fatalf("err = %v, want a Disconnected *gdb.Error", err)
	}
	if failure.Message == "" {
		t.Fatal("a failure needs a message")
	}
}

func TestRemoteAttachRejectsABadName(t *testing.T) {
	_, err := Attach(`nested/name`)
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Invalid {
		t.Fatalf("err = %v, want an Invalid *gdb.Error", err)
	}
}

func TestRemoteSpawnWithoutAnExecutableIsUnsupported(t *testing.T) {
	t.Setenv("GDBOX_BROKER_EXE", "")
	err := Spawn(fmt.Sprintf("go-test-no-broker-%d", os.Getpid()))
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Unsupported {
		t.Fatalf("err = %v, want an Unsupported *gdb.Error", err)
	}
}

// Presence: Feature 14 only, so a second program can see the board without
// disturbing whoever owns the session. These need no board.
func TestProbeWithoutABoardIsTyped(t *testing.T) {
	_, err := Probe("go-test-no-such-board")
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Disconnected {
		t.Fatalf("err = %v, want a Disconnected *gdb.Error", err)
	}
	if failure.Message == "" {
		t.Fatal("a failure needs a message")
	}
}

func TestProbeAllIsAJSONArray(t *testing.T) {
	text, err := ProbeAll()
	if err != nil {
		t.Fatal(err)
	}
	var entries []map[string]any
	if err := json.Unmarshal([]byte(text), &entries); err != nil {
		t.Fatalf("%v: %s", err, text)
	}
	for _, entry := range entries {
		if entry["board"].(float64) != 2 || entry["firmware"] == "" {
			t.Fatalf("unexpected probe: %v", entry)
		}
	}
}

func TestObserveWithoutABoardIsTyped(t *testing.T) {
	observer, err := Observe("go-test-no-such-board", 100)
	if err == nil {
		observer.Close()
		t.Fatal("observe succeeded with no board")
	}
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Disconnected {
		t.Fatalf("err = %v, want a Disconnected *gdb.Error", err)
	}
}

// The presence names, pinned: a rename has to change this line.
func TestPresenceSurfaceIsNamed(t *testing.T) {
	var _ func(string) (string, error) = Probe
	var _ func() (string, error) = ProbeAll
	var _ func(string, uint32) (*Observer, error) = Observe
	var _ func(*Observer) (string, error) = (*Observer).InfoJSON
	var _ func(*Observer) (ConnectionState, error) = (*Observer).State
	var _ func(*Observer) error = (*Observer).Status
	var _ func(*Observer) error = (*Observer).Barrier
	var _ func(*Observer) error = (*Observer).ReleaseAll
	var _ func(*Observer, uint8, []byte) error = (*Observer).Command
}

func TestObserverRefusesEveryCommand(t *testing.T) {
	text, err := ProbeAll()
	if err != nil {
		t.Fatal(err)
	}
	var entries []map[string]any
	if err := json.Unmarshal([]byte(text), &entries); err != nil {
		t.Fatal(err)
	}
	if len(entries) == 0 {
		t.Skip("no board attached, so there is nothing to observe")
	}

	observer, err := Observe("", 100)
	if err != nil {
		t.Fatal(err)
	}
	defer observer.Close()

	if state, err := observer.State(); err != nil || state != StateConnected {
		t.Fatalf("state = %v, %v", state, err)
	}
	info, err := observer.InfoJSON()
	if err != nil {
		t.Fatal(err)
	}
	var parsed map[string]any
	if err := json.Unmarshal([]byte(info), &parsed); err != nil {
		t.Fatal(err)
	}
	if parsed["board"].(float64) != 2 {
		t.Fatalf("unexpected info: %s", info)
	}

	refusals := map[string]error{
		"status":      observer.Status(),
		"barrier":     observer.Barrier(),
		"release_all": observer.ReleaseAll(),
		"command":     observer.Command(0x06, nil),
	}
	for name, err := range refusals {
		var failure *Error
		if !errors.As(err, &failure) || failure.Code != Unsupported {
			t.Fatalf("%s = %v, want Unsupported", name, err)
		}
		if !strings.Contains(failure.Message, "Feature 14") {
			t.Fatalf("%s message %q must name Feature 14", name, failure.Message)
		}
	}
}

// Metrics and screen targets: counters and a rectangle, no hardware.
func TestMetricsCountsASentFrameAndResetZeroesIt(t *testing.T) {
	// No heartbeat and no auto-reconnect, so nothing moves between the calls
	// below and the counters mean exactly what this test did.
	device, err := OpenMockOptions(1000, false, false)
	if err != nil {
		t.Fatalf("OpenMockOptions: %v", err)
	}
	defer func() { _ = device.Close() }()

	if err := device.ResetMetrics(); err != nil {
		t.Fatalf("ResetMetrics: %v", err)
	}
	if _, err := device.StatusJSON(); err != nil {
		t.Fatalf("STATUS: %v", err)
	}
	counted, err := device.Metrics()
	if err != nil {
		t.Fatalf("Metrics: %v", err)
	}
	if counted.FramesSent == 0 {
		t.Fatal("a STATUS frame left the session but FramesSent is 0")
	}
	if counted.FramesReceived == 0 {
		t.Fatal("STATUS was answered but FramesReceived is 0")
	}
	if counted.LatencyCount == 0 {
		t.Fatal("a command that waited for a reply must be timed")
	}

	if err := device.ResetMetrics(); err != nil {
		t.Fatalf("ResetMetrics: %v", err)
	}
	empty, err := device.Metrics()
	if err != nil {
		t.Fatalf("Metrics after reset: %v", err)
	}
	if empty != (Metrics{}) {
		t.Fatalf("reset left counters behind: %+v", empty)
	}
}

func TestMoveToOnTargetsAScreenAndRejectsAnUnknownKind(t *testing.T) {
	device := openMock(t)
	mouse := device.Mouse()

	if err := mouse.MoveToOn(200, 200, ScreenMonitor(0), 20, nil); err != nil {
		t.Fatalf("MoveToOn(monitor 0): %v", err)
	}
	if err := mouse.MovePathOn([][2]int32{{10, 10}, {20, 20}}, ScreenMonitor(0), 10, nil); err != nil {
		t.Fatalf("MovePathOn: %v", err)
	}
	movement, err := mouse.StartMoveToOn(100, 100, ScreenMonitor(0), 20, nil)
	if err != nil {
		t.Fatalf("StartMoveToOn: %v", err)
	}
	if err := movement.Wait(5000); err != nil {
		t.Fatalf("waiting for a screen-targeted move: %v", err)
	}
	movement.Close()

	var failure *Error
	if err := mouse.MoveToOn(10, 10, Screen{Kind: 3}, 0, nil); !errors.As(err, &failure) || failure.Code != Invalid {
		t.Fatalf("kind 3 = %v, want Invalid", err)
	}
	// A mock has one 1920x1080 screen, so index 1 names nothing.
	if err := mouse.MoveToOn(10, 10, ScreenMonitor(1), 0, nil); !errors.As(err, &failure) || failure.Code != Unsupported {
		t.Fatalf("monitor 1 = %v, want Unsupported", err)
	}
}

// The watch: the bus as it changes, and silence once it is stopped. A board may
// or may not be attached, so the only event assertion is conditional.
func TestWatchReportsTheBusAndStops(t *testing.T) {
	var mu sync.Mutex
	var kinds []int32
	var first map[string]any
	watched, err := NewWatch(100, func(kind int32, event map[string]any) {
		mu.Lock()
		defer mu.Unlock()
		kinds = append(kinds, kind)
		if first == nil {
			first = event
		}
	})
	if err != nil {
		t.Fatalf("NewWatch: %v", err)
	}
	defer watched.Close()

	listing, err := watched.ListingJSON()
	if err != nil {
		t.Fatalf("ListingJSON: %v", err)
	}
	var parsed map[string]any
	if err := json.Unmarshal([]byte(listing), &parsed); err != nil {
		t.Fatalf("the listing is not JSON: %v: %s", err, listing)
	}
	if _, ok := parsed["devices"]; !ok {
		t.Fatalf("the listing must name the devices: %s", listing)
	}
	if _, ok := parsed["volumes"]; !ok {
		t.Fatalf("the listing must name the volumes: %s", listing)
	}

	// Never a fixed sleep for the first event: the first poll runs on its own
	// thread and a freshly spawned one can be late.
	deadline := time.Now().Add(5 * time.Second)
	for {
		mu.Lock()
		count := len(kinds)
		mu.Unlock()
		if count > 0 || time.Now().After(deadline) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}

	mu.Lock()
	seen, kind, event := len(kinds), int32(-1), first
	if seen > 0 {
		kind = kinds[0]
	}
	mu.Unlock()
	if seen > 0 {
		if kind != WatchArrived {
			t.Fatalf("the first event kind = %d, want %d", kind, WatchArrived)
		}
		if _, ok := event["serial"]; !ok {
			t.Fatalf("an arrival must name the serial: %v", event)
		}
	}

	watched.Stop()
	time.Sleep(300 * time.Millisecond)
	mu.Lock()
	after := len(kinds)
	mu.Unlock()
	if after != seen {
		t.Fatalf("a stopped watch called back %d more time(s)", after-seen)
	}
}

func TestWatchWithoutACallbackIsInvalid(t *testing.T) {
	watched, err := NewWatch(100, nil)
	if err == nil {
		watched.Close()
		t.Fatal("a watch without a callback must be refused")
	}
	var failure *Error
	if !errors.As(err, &failure) || failure.Code != Invalid {
		t.Fatalf("err = %v, want an Invalid *gdb.Error", err)
	}
}

// The names, pinned: a rename has to change one of these lines.
func TestMetricsScreenAndWatchSurfaceIsNamed(t *testing.T) {
	var _ func(*Device) (Metrics, error) = (*Device).Metrics
	var _ func(*Device) error = (*Device).ResetMetrics
	var _ func(*Mouse, int32, int32, Screen, uint32, *Profile) error = (*Mouse).MoveToOn
	var _ func(*Mouse, [][2]int32, Screen, uint32, *Profile) error = (*Mouse).MovePathOn
	var _ func(*Mouse, int32, int32, Screen, uint32, *Profile) (*Movement, error) = (*Mouse).StartMoveToOn
	var _ func() Screen = ScreenPrimary
	var _ func(int32) Screen = ScreenMonitor
	var _ func(int64) Screen = ScreenWindow
	var _ func(uint32, func(int32, map[string]any)) (*Watch, error) = NewWatch
	var _ func(*Watch) (string, error) = (*Watch).ListingJSON
	var _ func(*Watch) = (*Watch).Stop
	var _ func(*Watch) = (*Watch).Close
}
