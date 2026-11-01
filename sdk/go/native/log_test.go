package gdbnative

import (
	"bytes"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// captureLog turns tracing on and collects every line until the test ends.
func captureLog(t *testing.T, level uint8) func() []string {
	t.Helper()
	var mu sync.Mutex
	var lines []string
	SetLogLevel(level)
	SetLogSink(func(_ int32, message string) {
		mu.Lock()
		lines = append(lines, message)
		mu.Unlock()
	})
	t.Cleanup(func() {
		SetLogLevel(LogOff)
		SetLogSink(nil)
	})
	return func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), lines...)
	}
}

func findLine(lines []string, prefix string) (string, bool) {
	for _, line := range lines {
		if strings.HasPrefix(line, prefix) {
			return line, true
		}
	}
	return "", false
}

func TestTraceLinesDescribeTheFrames(t *testing.T) {
	lines := captureLog(t, LogTrace)
	device, handle := openTestDevice(t, quietOptions())
	session := handle.Session()

	// A frame from another session has to be visible as skipped, not silently
	// dropped: that line is what a bug report is read from.
	foreign, err := NewFrame(OpStatus|ReplyBit, session+1, 1, make([]byte, StatusBytes))
	if err != nil {
		t.Fatalf("frame: %v", err)
	}
	handle.Queue(foreign)
	if _, err := device.Status(); err != nil {
		t.Fatalf("status: %v", err)
	}

	all := lines()
	outgoing, ok := findLine(all, "out opcode=6 ")
	if !ok {
		t.Fatalf("no outgoing STATUS line in %v", all)
	}
	want := "out opcode=6 session=" + strconv.Itoa(int(session)) + " sequence=2 payload=[]"
	if outgoing != want {
		t.Errorf("outgoing line = %q, want %q", outgoing, want)
	}
	incoming, ok := findLine(all, "in opcode=134 ")
	if !ok {
		t.Fatalf("no incoming STATUS line in %v", all)
	}
	if !strings.Contains(incoming, "session="+strconv.Itoa(int(session))) || !strings.Contains(incoming, "payload=[0, ") {
		t.Errorf("incoming line = %q", incoming)
	}
	skipped, ok := findLine(all, "skip opcode=134 ")
	if !ok {
		t.Fatalf("the foreign frame was not logged as skipped: %v", all)
	}
	if !strings.HasSuffix(skipped, "(not ours)") {
		t.Errorf("skip line = %q", skipped)
	}
}

func TestLogLevelGatesWhatReachesTheSink(t *testing.T) {
	lines := captureLog(t, LogInfo)
	device, _ := openTestDevice(t, quietOptions())
	if _, ok := findLine(lines(), "connected serial=MOCK firmware=3.2.0 board=2"); !ok {
		t.Fatalf("no connect line in %v", lines())
	}
	if _, err := device.Status(); err != nil {
		t.Fatalf("status: %v", err)
	}
	for _, line := range lines() {
		if strings.HasPrefix(line, "out opcode=") || strings.HasPrefix(line, "in opcode=") {
			t.Errorf("info level produced a trace line: %q", line)
		}
	}
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if _, ok := findLine(lines(), "closed serial=MOCK"); !ok {
		t.Errorf("no close line in %v", lines())
	}

	SetLogLevel(LogOff)
	before := len(lines())
	if _, err := device.Status(); err != nil && CodeOf(err) != Disconnected {
		t.Fatalf("status after close: %v", err)
	}
	if len(lines()) != before {
		t.Errorf("level off still produced %d lines", len(lines())-before)
	}
}

func TestFormatPayloadFollowsTheRustDebugShape(t *testing.T) {
	cases := map[string][]byte{
		"[]":          nil,
		"[0]":         {0},
		"[1, 2, 255]": {1, 2, 255},
	}
	for want, payload := range cases {
		if got := formatPayload(payload); got != want {
			t.Errorf("formatPayload(%v) = %q, want %q", payload, got, want)
		}
	}
}

func TestCommandWritesTheExactBytesAndSkipsBookkeeping(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if !device.IsMock() {
		t.Skip("the cursor bookkeeping only exists on the mock")
	}
	payload := []byte{0xF6, 0xFF, 0x0A, 0x00}
	before := handle.Count()
	reply, err := device.Command(OpMouseMove, payload)
	if err != nil {
		t.Fatalf("command: %v", err)
	}
	// MOUSE_MOVE expects no reply, so the packet is zero.
	if reply.Opcode != 0 || reply.Payload != nil {
		t.Errorf("reply = %+v, want the zero packet", reply)
	}
	sent := handle.Sent(before)
	if len(sent) != 1 {
		t.Fatalf("sent %d frames, want 1", len(sent))
	}
	if sent[0].Opcode != OpMouseMove {
		t.Errorf("opcode = %d, want MOUSE_MOVE", sent[0].Opcode)
	}
	if !bytes.Equal(sent[0].Payload, payload) {
		t.Errorf("payload = %v, want %v", sent[0].Payload, payload)
	}
	if sent[0].Sequence != 2 {
		t.Errorf("sequence = %d, want 2", sent[0].Sequence)
	}
	// The escape hatch leaves the mock cursor alone; the typed path moves it.
	if cursor := device.cursorState(); cursor != [2]int32{960, 540} {
		t.Errorf("cursor = %v after a raw move, want it untouched", cursor)
	}
	if err := device.Mouse().MoveRaw(10, 10); err != nil {
		t.Fatalf("move raw: %v", err)
	}
	if cursor := device.cursorState(); cursor != [2]int32{970, 550} {
		t.Errorf("cursor = %v after the typed move, want 970,550", cursor)
	}
}

func TestCommandReturnsTheControlReply(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	reply, err := device.Command(OpStatus, nil)
	if err != nil {
		t.Fatalf("command: %v", err)
	}
	if reply.Opcode != OpStatus|ReplyBit {
		t.Errorf("reply opcode = %d, want STATUS|0x80", reply.Opcode)
	}
	if len(reply.Payload) != StatusBytes {
		t.Fatalf("reply payload = %d bytes, want %d", len(reply.Payload), StatusBytes)
	}
	status, err := ParseStatus(reply.Payload)
	if err != nil || status.LeaseMS != 2000 {
		t.Errorf("status = %+v, %v", status, err)
	}
}

func TestCommandRejectsAnOverlongPayloadAndSurfacesErrors(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	if _, err := device.Command(OpSnapshot, make([]byte, PayloadSize+1)); CodeOf(err) != Invalid {
		t.Fatalf("err = %v, want INVALID", CodeOf(err))
	}
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if _, err := device.Command(OpStatus, nil); CodeOf(err) != Disconnected {
		t.Fatalf("err = %v, want DISCONNECTED", CodeOf(err))
	}
}

func TestHandlerReceivesTheThreeRustEventKinds(t *testing.T) {
	opts := quietOptions()
	opts.AutoReconnect = false
	device, handle := openTestDevice(t, opts)

	var states, statuses, errors int
	var lease uint16
	device.SetHandler(HandlerFuncs{
		OnState:  func(state ConnectionState) { states++ },
		OnStatus: func(status Status) { statuses++; lease = status.LeaseMS },
		OnError:  func(err *Error) { errors++ },
	})
	// The single-function handler keeps working next to the richer one.
	var kinds []EventKind
	device.SetEventHandler(func(event Event) { kinds = append(kinds, event.Kind) })

	if _, err := device.Status(); err != nil {
		t.Fatalf("status: %v", err)
	}
	if statuses != 1 || lease != 2000 {
		t.Fatalf("status callbacks = %d (lease %d), want 1 / 2000", statuses, lease)
	}
	if len(kinds) == 0 || kinds[len(kinds)-1] != EventStatus {
		t.Fatalf("the func handler saw %v, want a trailing STATUS kind", kinds)
	}

	handle.SetSilent(true)
	if _, err := device.Status(); CodeOf(err) != Timeout {
		t.Fatalf("err = %v, want TIMEOUT", CodeOf(err))
	}
	if errors != 1 {
		t.Errorf("error callbacks = %d, want 1", errors)
	}

	handle.SetSilent(false)
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if states == 0 {
		t.Error("no state callback fired")
	}
}
