package gdbnative

import (
	"encoding/json"
	"sync/atomic"
	"testing"
	"time"
)

// openTestDevice builds a device over a fresh mock and closes it afterwards.
func openTestDevice(t *testing.T, opts Options) (*Device, *MockHandle) {
	t.Helper()
	transport, handle := NewMockTransport()
	device, err := newDevice(transport, targetMock, "MOCK", "GDBOX mock", opts)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	device.reopen = func() (Transport, error) {
		next, _ := NewMockTransport()
		return next, nil
	}
	t.Cleanup(func() { _ = device.Close() })
	return device, handle
}

func quietOptions() Options {
	opts := DefaultOptions()
	opts.Heartbeat = false
	return opts
}

func TestOpenSendsOpenFirstAndReportsTheFirmwareContract(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	info := device.Info()
	if info.Board != 2 || info.Firmware != "3.2.0" || info.LeaseMS != 2000 || info.PollMS != 1 {
		t.Fatalf("info = %+v", info)
	}
	if info.Capabilities != 15 {
		t.Errorf("capabilities = %d, want 15", info.Capabilities)
	}
	if info.Serial != "MOCK" || info.Product != "GDBOX mock" {
		t.Errorf("identity = %q/%q", info.Serial, info.Product)
	}
	if device.State() != StateConnected {
		t.Errorf("state = %v, want connected", device.State())
	}
	if handle.Session() == 0 {
		t.Fatal("mock has no owner")
	}
	if !device.IsMock() {
		t.Error("device is not marked as a mock")
	}

	writes := handle.Writes()
	if len(writes) == 0 {
		t.Fatal("no frame was written")
	}
	first := writes[0]
	if first.Opcode != OpOpen || first.Sequence != 1 {
		t.Fatalf("first frame = opcode %d sequence %d, want OPEN/1", first.Opcode, first.Sequence)
	}
	if first.Session != handle.Session() {
		t.Errorf("OPEN session = %#x, owner = %#x", first.Session, handle.Session())
	}
	if len(first.Payload) != 0 {
		t.Errorf("OPEN payload = %v, want empty", first.Payload)
	}
}

func TestQueriesMatchTheJsonShapesOfTheDllBinding(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())

	status, err := device.Status()
	if err != nil {
		t.Fatalf("status: %v", err)
	}
	if status.LeaseMS != 2000 || status.Pending != 0 {
		t.Fatalf("status = %+v", status)
	}
	raw, err := device.StatusJSON()
	if err != nil {
		t.Fatalf("status json: %v", err)
	}
	var decoded map[string]any
	if err := json.Unmarshal([]byte(raw), &decoded); err != nil {
		t.Fatalf("status json is not JSON: %v", err)
	}
	for _, key := range []string{"received", "completed", "pending", "leaseMs", "rx", "tx", "hid", "failsafe", "state"} {
		if _, ok := decoded[key]; !ok {
			t.Errorf("status JSON is missing %q: %s", key, raw)
		}
	}
	state, ok := decoded["state"].(map[string]any)
	if !ok {
		t.Fatalf("status.state is not an object: %s", raw)
	}
	keys, ok := state["keys"].([]any)
	if !ok || len(keys) != 28 {
		t.Errorf("state.keys = %v, want 28 bytes", state["keys"])
	}

	ext, err := device.StatusExt()
	if err != nil {
		t.Fatalf("status ext: %v", err)
	}
	if ext.UptimeMS == 0 || ext.BootCount != 1 {
		t.Errorf("status ext = %+v", ext)
	}
	extJSON, err := device.StatusExtJSON()
	if err != nil {
		t.Fatalf("status ext json: %v", err)
	}
	var extMap map[string]any
	if err := json.Unmarshal([]byte(extJSON), &extMap); err != nil {
		t.Fatalf("status ext json: %v", err)
	}
	for _, key := range []string{"uptimeMs", "lateMaxUs", "outDepth", "rxDepth", "txDepth", "sessions", "lastFault", "lastFaultAgeMs", "resetReason", "bootCount", "build"} {
		if _, ok := extMap[key]; !ok {
			t.Errorf("status ext JSON is missing %q: %s", key, extJSON)
		}
	}

	sessions, err := device.Sessions()
	if err != nil {
		t.Fatalf("sessions: %v", err)
	}
	if len(sessions) != 1 {
		t.Fatalf("sessions = %+v, want the one owner", sessions)
	}
	if sessions[0].ID != uint16(handle.Session()) {
		t.Errorf("session id = %#x, want low 16 bits of %#x", sessions[0].ID, handle.Session())
	}
	sessionsJSON, err := device.SessionsJSON()
	if err != nil {
		t.Fatalf("sessions json: %v", err)
	}
	var owners []map[string]any
	if err := json.Unmarshal([]byte(sessionsJSON), &owners); err != nil {
		t.Fatalf("sessions json: %v", err)
	}
	if len(owners) != 1 {
		t.Fatalf("owners = %v", owners)
	}
	for _, key := range []string{"id", "leaseMs", "held", "flags"} {
		if _, ok := owners[0][key]; !ok {
			t.Errorf("session JSON is missing %q: %s", key, sessionsJSON)
		}
	}

	infoJSON, err := device.InfoJSON()
	if err != nil {
		t.Fatalf("info json: %v", err)
	}
	var info map[string]any
	if err := json.Unmarshal([]byte(infoJSON), &info); err != nil {
		t.Fatalf("info json: %v", err)
	}
	deviceID, ok := info["deviceId"].([]any)
	if !ok || len(deviceID) != 8 {
		t.Errorf("info.deviceId = %v, want 8 bytes", info["deviceId"])
	}
	if info["leaseMs"] != float64(2000) {
		t.Errorf("info.leaseMs = %v", info["leaseMs"])
	}
}

func TestForeignFramesAreIgnored(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	session := handle.Session()

	foreignStatus, err := NewFrame(OpStatus|ReplyBit, session+1, 1, make([]byte, StatusBytes))
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(foreignStatus)
	staleBarrier, err := NewFrame(OpBarrier|ReplyBit, session, 4096, []byte{0})
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(staleBarrier)
	wrongOpcode, err := NewFrame(OpClose|ReplyBit, session, 2, []byte{0})
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(wrongOpcode)

	status, err := device.Status()
	if err != nil {
		t.Fatalf("status after foreign frames: %v", err)
	}
	if status.LeaseMS != 2000 {
		t.Fatalf("status = %+v", status)
	}
	if err := device.Barrier(); err != nil {
		t.Fatalf("barrier after foreign frames: %v", err)
	}
}

func TestEventForThisSessionBecomesATypedError(t *testing.T) {
	// Recovery off: a LEASE failure is recoverable, so with auto reconnect on the
	// SDK would reopen the transport and retry, which is a different test.
	opts := quietOptions()
	opts.AutoReconnect = false
	device, handle := openTestDevice(t, opts)
	event, err := NewFrame(OpEvent, handle.Session(), 3, []byte{5})
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(event)
	if code := CodeOf(device.Barrier()); code != Lease {
		t.Fatalf("code = %v, want LEASE", code)
	}
}

func TestEventForAnotherSessionIsIgnored(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	event, err := NewFrame(OpEvent, handle.Session()+7, 3, []byte{5})
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(event)
	if err := device.Barrier(); err != nil {
		t.Fatalf("foreign EVENT leaked into this session: %v", err)
	}
}

func TestMalformedFrameIsAFramingFailure(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	handle.Queue(Report{})
	code := CodeOf(device.Barrier())
	if code != Framing && code != Invalid {
		t.Fatalf("code = %v, want a protocol error", code)
	}
}

func TestHeartbeatKeepsTheLeaseWhileIdle(t *testing.T) {
	device, handle := openTestDevice(t, DefaultOptions())
	start := handle.Count()
	time.Sleep(1200 * time.Millisecond)
	heartbeats := 0
	for _, packet := range handle.Sent(start) {
		if packet.Opcode == OpHeartbeat {
			heartbeats++
		}
	}
	if heartbeats < 1 {
		t.Fatalf("heartbeats = %d after 1.2 s, want at least 1", heartbeats)
	}
	if device.State() != StateConnected {
		t.Fatalf("state = %v", device.State())
	}
}

func TestStatusPollingDoesNotSuppressTheHeartbeat(t *testing.T) {
	// STATUS does not renew the lease (firmware/protocol-v3.md:120). A host that
	// polls STATUS in a loop must still see heartbeats, or the session dies
	// every lease period (sdk/rust/tests/sdk.rs).
	device, handle := openTestDevice(t, DefaultOptions())
	start := handle.Count()
	for poll := 0; poll < 12; poll++ {
		if _, err := device.Status(); err != nil {
			t.Fatalf("status poll %d: %v", poll, err)
		}
		time.Sleep(100 * time.Millisecond)
	}
	heartbeats := 0
	for _, packet := range handle.Sent(start) {
		if packet.Opcode == OpHeartbeat {
			heartbeats++
		}
	}
	if heartbeats < 1 {
		t.Fatalf("heartbeats = %d while polling STATUS, want at least 1", heartbeats)
	}
}

func TestStatusExtPollingDoesNotStarveTheHeartbeat(t *testing.T) {
	// STATUS_EXT does not renew the lease either (firmware/engine/src/engine.rs).
	// A host whose only traffic is a 100 ms STATUS_EXT poll must still heartbeat
	// inside the 2 s lease, or it loses the session to its own diagnostics.
	device, handle := openTestDevice(t, DefaultOptions())
	start := handle.Count()
	polls := 0
	deadline := time.Now().Add(1200 * time.Millisecond)
	for time.Now().Before(deadline) {
		if _, err := device.StatusExt(); err != nil {
			t.Fatalf("status_ext poll %d: %v", polls, err)
		}
		polls++
		time.Sleep(100 * time.Millisecond)
	}
	heartbeats := 0
	for _, packet := range handle.Sent(start) {
		if packet.Opcode == OpHeartbeat {
			heartbeats++
		}
	}
	if polls < 5 {
		t.Fatalf("polls = %d over 1.2 s, want at least 5", polls)
	}
	if heartbeats < 1 {
		t.Fatalf("heartbeats = %d while polling STATUS_EXT, want at least 1", heartbeats)
	}
}

func TestSilentDeviceTimesOut(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	handle.SetSilent(true)
	if _, err := device.Status(); CodeOf(err) != Timeout {
		t.Fatalf("code = %v, want TIMEOUT", CodeOf(err))
	}
}

func TestDisconnectedWriteReconnectsOnceAndKeepsWorking(t *testing.T) {
	device, handle := openTestDevice(t, DefaultOptions())
	limit := handle.Count()
	handle.SetDisconnectAfter(&limit)
	status, err := device.Status()
	if err != nil {
		t.Fatalf("status after reconnect: %v", err)
	}
	if status.LeaseMS != 2000 {
		t.Fatalf("status = %+v", status)
	}
	if device.State() != StateConnected {
		t.Fatalf("state = %v, want connected", device.State())
	}
}

func TestAutoReconnectOffSurfacesTheFailure(t *testing.T) {
	opts := quietOptions()
	opts.AutoReconnect = false
	device, handle := openTestDevice(t, opts)
	limit := handle.Count()
	handle.SetDisconnectAfter(&limit)
	if _, err := device.Status(); CodeOf(err) != Disconnected {
		t.Fatalf("code = %v, want DISCONNECTED", CodeOf(err))
	}
}

func TestMissingFeatureReportIsDisconnected(t *testing.T) {
	transport, handle := NewMockTransport()
	handle.SetFeatureMissing(true)
	if _, err := newDevice(transport, targetMock, "MOCK", "GDBOX mock", quietOptions()); CodeOf(err) != Disconnected {
		t.Fatalf("code = %v, want DISCONNECTED", CodeOf(err))
	}
}

func TestCloseSendsCloseAndRejectsFurtherCalls(t *testing.T) {
	device, handle := openTestDevice(t, DefaultOptions())
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if !handle.Closed() {
		t.Error("mock did not see CLOSE")
	}
	writes := handle.Writes()
	if last := writes[len(writes)-1]; last.Opcode != OpClose {
		t.Errorf("last frame = opcode %d, want CLOSE", last.Opcode)
	}
	if device.State() != StateDisconnected {
		t.Errorf("state = %v", device.State())
	}
	if _, err := device.Status(); CodeOf(err) != Disconnected {
		t.Errorf("status after close = %v, want DISCONNECTED", CodeOf(err))
	}
	if err := device.Close(); err != nil {
		t.Errorf("second close: %v", err)
	}
}

func TestEventCallbackSeesCloseAndErrors(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	var states atomic.Int32
	var kinds atomic.Int32
	device.SetEventHandler(func(event Event) {
		kinds.Add(1)
		if event.Kind == EventState {
			states.Store(event.Code)
		}
	})
	device.SetEventHandler(nil)
	device.SetEventHandler(func(event Event) {
		kinds.Add(1)
		if event.Kind == EventState {
			states.Store(event.Code)
		}
	})
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if states.Load() != int32(StateDisconnected) {
		t.Errorf("state event = %d, want DISCONNECTED", states.Load())
	}
	if kinds.Load() == 0 {
		t.Error("no callback fired")
	}

	other, otherHandle := openTestDevice(t, quietOptions())
	seen := atomic.Int32{}
	other.SetEventHandler(func(event Event) {
		if event.Kind == EventError {
			seen.Store(event.Code)
		}
	})
	otherHandle.SetSilent(true)
	_, _ = other.Status()
	if seen.Load() != int32(Timeout) {
		t.Errorf("error event code = %d, want TIMEOUT", seen.Load())
	}
}

func TestReconnectOpensAFreshSession(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	if err := device.Reconnect(); err != nil {
		t.Fatalf("reconnect: %v", err)
	}
	if device.State() != StateConnected {
		t.Fatalf("state = %v", device.State())
	}
	if _, err := device.Status(); err != nil {
		t.Fatalf("status after reconnect: %v", err)
	}
}

func TestSetAutoReconnectIsLive(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	device.SetAutoReconnect(true)
	if !device.autoReconnect.Load() {
		t.Fatal("auto reconnect flag was not set")
	}
	device.SetAutoReconnect(false)
	limit := handle.Count()
	handle.SetDisconnectAfter(&limit)
	if _, err := device.Status(); CodeOf(err) != Disconnected {
		t.Fatalf("code = %v, want DISCONNECTED with recovery off", CodeOf(err))
	}
}

func TestNewSessionIDIsNonZeroAndUnique(t *testing.T) {
	seen := map[uint32]bool{}
	for index := 0; index < 64; index++ {
		id := newSessionID()
		if id == 0 {
			t.Fatal("session id 0")
		}
		if seen[id] {
			t.Fatalf("duplicate session id %#x", id)
		}
		seen[id] = true
	}
}

func TestUnsupportedPlatformHooksAreTyped(t *testing.T) {
	// screenAndCursor and List report Unsupported when the platform cannot do
	// the job; the mock path must never reach them.
	device, _ := openTestDevice(t, quietOptions())
	screen, position, err := device.cursorPosition()
	if err != nil {
		t.Fatalf("mock cursor: %v", err)
	}
	if screen != DefaultScreen() {
		t.Errorf("mock screen = %+v", screen)
	}
	if position != [2]int32{960, 540} {
		t.Errorf("mock cursor = %v", position)
	}
}
