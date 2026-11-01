package gdbnative

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// vectorFile is the canonical frame list; it is the spec, not a copy of it.
func vectorFile(t *testing.T) string {
	t.Helper()
	path := filepath.Join("..", "..", "..", "shared", "input-protocol", "vectors.json")
	if _, err := os.Stat(path); err != nil {
		t.Skipf("golden vectors not found: %s", path)
	}
	return path
}

type wireVector struct {
	Name     string `json:"name"`
	Opcode   uint8  `json:"opcode"`
	Sequence uint32 `json:"sequence"`
	Bytes    []byte `json:"bytes"`
}

func loadVectors(t *testing.T) []wireVector {
	t.Helper()
	raw, err := os.ReadFile(vectorFile(t))
	if err != nil {
		t.Fatalf("read vectors: %v", err)
	}
	var vectors []wireVector
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatalf("decode vectors: %v", err)
	}
	if len(vectors) == 0 {
		t.Fatal("no golden vectors")
	}
	return vectors
}

// TestGoldenFramesReencodeExactly is the byte-for-byte contract: decode a
// vector, re-encode it, get the same 64 bytes back.
func TestGoldenFramesReencodeExactly(t *testing.T) {
	const session = 0x12345678
	for _, vector := range loadVectors(t) {
		t.Run(vector.Name, func(t *testing.T) {
			decoded, err := DecodePacket(vector.Bytes, OutReportID)
			if err != nil {
				t.Fatalf("decode: %v", err)
			}
			if decoded.Opcode != vector.Opcode {
				t.Errorf("opcode = %d, want %d", decoded.Opcode, vector.Opcode)
			}
			if decoded.Session != session {
				t.Errorf("session = %#x, want %#x", decoded.Session, session)
			}
			if decoded.Sequence != vector.Sequence {
				t.Errorf("sequence = %d, want %d", decoded.Sequence, vector.Sequence)
			}
			encoded, err := decoded.Encode(OutReportID)
			if err != nil {
				t.Fatalf("encode: %v", err)
			}
			if string(encoded[:]) != string(vector.Bytes) {
				t.Fatalf("frame differs:\n got %v\nwant %v", encoded[:], vector.Bytes)
			}
		})
	}
}

// TestGoldenFramesBuildFromCommandPayloads rebuilds the same frames from the
// payloads the typed commands produce.
func TestGoldenFramesBuildFromCommandPayloads(t *testing.T) {
	const session = 0x12345678
	built := map[string]Packet{
		"open":         {Opcode: OpOpen, Session: session, Sequence: 1},
		"digit1_down":  {Opcode: OpKey, Session: session, Sequence: 2, Payload: []byte{7, 0, 30, 0, 1}},
		"keypad1_down": {Opcode: OpKey, Session: session, Sequence: 3, Payload: []byte{7, 0, 89, 0, 1}},
		"volume_up":    {Opcode: OpKey, Session: session, Sequence: 4, Payload: []byte{12, 0, 0xE9, 0, 1}},
		"barrier":      {Opcode: OpBarrier, Session: session, Sequence: 5},
	}
	for _, vector := range loadVectors(t) {
		packet, ok := built[vector.Name]
		if !ok {
			t.Fatalf("vector %q has no payload recipe", vector.Name)
		}
		encoded, err := packet.Encode(OutReportID)
		if err != nil {
			t.Fatalf("%s: encode: %v", vector.Name, err)
		}
		if string(encoded[:]) != string(vector.Bytes) {
			t.Errorf("%s: frame differs:\n got %v\nwant %v", vector.Name, encoded[:], vector.Bytes)
		}
	}
}

func TestEncodeRejectsMalformedPackets(t *testing.T) {
	cases := []struct {
		name   string
		id     uint8
		packet Packet
	}{
		{"wrong report id", 9, Packet{Opcode: OpOpen}},
		{"flags set", OutReportID, Packet{Opcode: OpOpen, Flags: 1}},
		{"payload too long", OutReportID, Packet{Opcode: OpKey, Payload: make([]byte, PayloadSize+1)}},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			if _, err := testCase.packet.Encode(testCase.id); CodeOf(err) != Invalid {
				t.Fatalf("code = %v, want INVALID (err %v)", CodeOf(err), err)
			}
		})
	}
}

func TestDecodeRejectsMalformedFrames(t *testing.T) {
	good, err := Packet{Opcode: OpKey, Session: 1, Sequence: 2, Payload: []byte{7, 0, 4, 0, 1}}.Encode(OutReportID)
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	cases := []struct {
		name   string
		mutate func([]byte) []byte
	}{
		{"short report", func(frame []byte) []byte { return frame[:63] }},
		{"wrong version", func(frame []byte) []byte { frame[1] = 2; return frame }},
		{"flags set", func(frame []byte) []byte { frame[4] = 1; return frame }},
		{"payload too long", func(frame []byte) []byte { frame[3] = 52; return frame }},
		{"padding set", func(frame []byte) []byte { frame[63] = 1; return frame }},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			frame := testCase.mutate(append([]byte(nil), good[:]...))
			if _, err := DecodePacket(frame, OutReportID); CodeOf(err) != Framing {
				t.Fatalf("code = %v, want FRAMING (err %v)", CodeOf(err), err)
			}
		})
	}
}

func TestCapabilitiesFromTheMockFeatureReport(t *testing.T) {
	transport, _ := NewMockTransport()
	report, err := transport.Feature()
	if err != nil {
		t.Fatalf("feature: %v", err)
	}
	caps, err := ParseCapabilities(report[:])
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if caps.Board != 2 {
		t.Errorf("board = %d, want 2", caps.Board)
	}
	if caps.Firmware != "3.2.0" {
		t.Errorf("firmware = %q, want 3.2.0", caps.Firmware)
	}
	if caps.LeaseMS != 2000 {
		t.Errorf("lease = %d, want 2000", caps.LeaseMS)
	}
	if caps.PollMS != 1 {
		t.Errorf("poll = %d, want 1", caps.PollMS)
	}
	if caps.Flags != 15 {
		t.Errorf("capabilities = %d, want 15", caps.Flags)
	}
	if caps.Modifiers != 0xFF || caps.Consumer != 0x7F {
		t.Errorf("masks = %#x/%#x, want 0xff/0x7f", caps.Modifiers, caps.Consumer)
	}
	if !caps.Supports(7, 0x04) || caps.Supports(7, 0x01) {
		t.Error("keyboard bitmap does not match the catalog")
	}
	if !caps.Supports(12, 0xE9) {
		t.Error("consumer bit for volume up is missing")
	}
}

func TestCapabilitiesRejectAReservedByte(t *testing.T) {
	transport, _ := NewMockTransport()
	report, _ := transport.Feature()
	report[62] = 1
	if _, err := ParseCapabilities(report[:]); CodeOf(err) != Framing {
		t.Fatalf("code = %v, want FRAMING", CodeOf(err))
	}
}

// payloadOf decodes an injected frame back into its payload.
func payloadOf(t *testing.T, report *Report) []byte {
	t.Helper()
	if report == nil {
		t.Fatal("the mock produced no frame")
	}
	packet, err := DecodePacket(report[:], InReportID)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	return packet.Payload
}

func TestParseStatusAndStatusExt(t *testing.T) {
	transport, _ := NewMockTransport()
	state := InputState{}
	if err := state.Set(7, 0x04, true); err != nil {
		t.Fatalf("set: %v", err)
	}
	transport.state.held = state
	// The mock only answers the session that owns the collection.
	transport.state.session = 7

	reply := transport.answerLocked(Packet{Opcode: OpStatus, Session: 7, Sequence: 9})
	statusPayload := payloadOf(t, reply)
	status, err := ParseStatus(statusPayload)
	if err != nil {
		t.Fatalf("status: %v", err)
	}
	if status.Received != 9 || status.Completed != 9 {
		t.Errorf("counters = %d/%d, want 9/9", status.Received, status.Completed)
	}
	if status.LeaseMS != 2000 {
		t.Errorf("lease = %d, want 2000", status.LeaseMS)
	}
	if !status.State.Contains(7, 0x04) {
		t.Error("snapshot lost the held key")
	}

	ext, err := ParseStatusExt(payloadOf(t, transport.answerLocked(Packet{Opcode: OpStatusExt, Session: 7, Sequence: 10})))
	if err != nil {
		t.Fatalf("status ext: %v", err)
	}
	if ext.UptimeMS == 0 || ext.BootCount != 1 {
		t.Errorf("uptime/bootCount = %d/%d", ext.UptimeMS, ext.BootCount)
	}
	if ext.LastFaultAgeMS != nil {
		t.Errorf("lastFaultAgeMs = %v, want nil for 0xFFFFFFFF", *ext.LastFaultAgeMS)
	}
	if ext.Build != 0x0DB00001 {
		t.Errorf("build = %#x", ext.Build)
	}
}

func TestParseStatusRejectsAFailedResultByte(t *testing.T) {
	payload := make([]byte, StatusBytes)
	payload[0] = 3
	if _, err := ParseStatus(payload); CodeOf(err) != Framing {
		t.Fatalf("code = %v, want FRAMING", CodeOf(err))
	}
}

func TestParseSessionListSkipsEmptySlots(t *testing.T) {
	payload := make([]byte, SessionListBytes)
	payload[0] = 0x34
	payload[1] = 0x12
	payload[2] = 0xD0
	payload[3] = 0x07
	payload[4] = 2
	payload[5] = 4
	// Slot 1 empty; slot 2 holds a second owner.
	payload[12] = 0x78
	payload[13] = 0x56
	payload[14] = 0x10
	payload[15] = 0x01

	sessions := ParseSessionList(payload)
	if len(sessions) != 2 {
		t.Fatalf("sessions = %d, want 2", len(sessions))
	}
	want := SessionInfo{ID: 0x1234, LeaseMS: 2000, Held: 2, Flags: 4}
	if sessions[0] != want {
		t.Errorf("first = %+v, want %+v", sessions[0], want)
	}
	if sessions[1].ID != 0x5678 || sessions[1].LeaseMS != 0x0110 {
		t.Errorf("second = %+v", sessions[1])
	}
}

func TestCheckResponseMatching(t *testing.T) {
	cases := []struct {
		name    string
		packet  Packet
		session uint32
		seq     uint32
		opcode  uint8
		code    Code
	}{
		{"ok", Packet{Opcode: OpBarrier | ReplyBit, Session: 1, Sequence: 4, Payload: []byte{0}}, 1, 4, OpBarrier, OK},
		{"foreign session", Packet{Opcode: OpBarrier | ReplyBit, Session: 2, Sequence: 4, Payload: []byte{0}}, 1, 4, OpBarrier, Framing},
		{"stale sequence", Packet{Opcode: OpBarrier | ReplyBit, Session: 1, Sequence: 5, Payload: []byte{0}}, 1, 4, OpBarrier, Framing},
		{"wrong opcode", Packet{Opcode: OpClose | ReplyBit, Session: 1, Sequence: 4, Payload: []byte{0}}, 1, 4, OpBarrier, Framing},
		{"short reply", Packet{Opcode: OpBarrier | ReplyBit, Session: 1, Sequence: 4, Payload: nil}, 1, 4, OpBarrier, Framing},
		{"busy result", Packet{Opcode: OpOpen | ReplyBit, Session: 1, Sequence: 1, Payload: []byte{2}}, 1, 1, OpOpen, Busy},
		{"lease result", Packet{Opcode: OpBarrier | ReplyBit, Session: 1, Sequence: 4, Payload: []byte{5}}, 1, 4, OpBarrier, Lease},
		{"session list has no result byte", Packet{Opcode: OpSessionList | ReplyBit, Session: 1, Sequence: 4, Payload: make([]byte, SessionListBytes)}, 1, 4, OpSessionList, OK},
		{"status needs 51 bytes", Packet{Opcode: OpStatus | ReplyBit, Session: 1, Sequence: 4, Payload: make([]byte, 23)}, 1, 4, OpStatus, Framing},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			err := CheckResponse(testCase.packet, testCase.session, testCase.seq, testCase.opcode)
			if CodeOf(err) != testCase.code {
				t.Fatalf("code = %v, want %v (err %v)", CodeOf(err), testCase.code, err)
			}
		})
	}
}

func TestErrorResultMapping(t *testing.T) {
	cases := []struct {
		result uint8
		code   Code
	}{
		{0, OK}, {1, Framing}, {2, Busy}, {3, Invalid}, {5, Lease},
		{6, Disconnected}, {7, Disconnected}, {8, Overflow},
		{9, Framing}, {10, Framing}, {200, IO},
	}
	for _, testCase := range cases {
		failure := FromResult(testCase.result)
		if testCase.code == OK {
			if failure != nil {
				t.Fatalf("result 0 mapped to %v", failure)
			}
			continue
		}
		if failure == nil || failure.Code != testCase.code {
			t.Errorf("result %d mapped to %v, want %v", testCase.result, failure, testCase.code)
		}
	}
}

func TestCodeStrings(t *testing.T) {
	cases := map[Code]string{
		OK: "OK", Invalid: "INVALID", Timeout: "TIMEOUT", Disconnected: "DISCONNECTED",
		Busy: "BUSY", Lease: "LEASE", Framing: "FRAMING", Overflow: "OVERFLOW",
		Unsupported: "UNSUPPORTED", Cancelled: "CANCELLED", IO: "IO", Code(42): "CODE(42)",
	}
	for code, want := range cases {
		if got := code.String(); got != want {
			t.Errorf("%d = %q, want %q", int32(code), got, want)
		}
	}
	if CodeOf(nil) != OK {
		t.Error("nil error must be OK")
	}
}

func TestInputStateRejectsReservedUsages(t *testing.T) {
	state := InputState{}
	if err := state.Set(7, 0x01, true); CodeOf(err) != Invalid {
		t.Fatalf("reserved usage accepted: %v", err)
	}
	if err := state.Set(7, 0x04, true); err != nil {
		t.Fatalf("letter rejected: %v", err)
	}
	if !state.Contains(7, 0x04) {
		t.Fatal("held key not reported")
	}
	if err := state.Set(7, 0xE1, true); err != nil {
		t.Fatalf("modifier rejected: %v", err)
	}
	if state.Modifiers&0x02 == 0 {
		t.Fatal("left shift did not land in the modifier byte")
	}
	if err := state.Set(12, 0xE9, true); err != nil {
		t.Fatalf("consumer rejected: %v", err)
	}
	if state.Consumer&0x02 == 0 {
		t.Fatal("volume up did not land in consumer bit 1")
	}
	encoded := state.Encode()
	decoded, err := DecodeInputState(encoded[:])
	if err != nil {
		t.Fatalf("roundtrip: %v", err)
	}
	if decoded != state {
		t.Fatalf("roundtrip = %+v, want %+v", decoded, state)
	}
	reserved := encoded
	reserved[1] = 1
	if _, err := DecodeInputState(reserved[:]); CodeOf(err) != Framing {
		t.Fatalf("reserved usage in snapshot accepted: %v", err)
	}
}

func TestExpectedReplySet(t *testing.T) {
	for opcode, want := range map[uint8]bool{
		OpOpen: true, OpClose: true, OpStatus: true, OpBarrier: true,
		OpReleaseAll: true, OpStatusExt: true, OpSessionList: true, OpBootloader: true,
		OpHeartbeat: false, OpKey: false, OpSnapshot: false, OpMouseMove: false,
		OpMouseButton: false, OpWheel: false, OpEvent: false,
	} {
		if got := ExpectsReply(opcode); got != want {
			t.Errorf("opcode %d expects reply = %v, want %v", opcode, got, want)
		}
	}
}

func TestLeaseRenewalSet(t *testing.T) {
	// Mirrors firmware/engine/src/engine.rs: lease_at is written for OPEN,
	// HEARTBEAT, RELEASE_ALL, BOOTLOADER and valid input only. STATUS, BARRIER,
	// STATUS_EXT and SESSION_LIST must not renew, or a host whose only traffic
	// is a query poll would starve its own heartbeat.
	for opcode, want := range map[uint8]bool{
		OpOpen:        true,
		OpHeartbeat:   true,
		OpReleaseAll:  true,
		OpBootloader:  true,
		OpKey:         true,
		OpSnapshot:    true,
		OpMouseMove:   true,
		OpMouseButton: true,
		OpWheel:       true,
		OpClose:       false,
		OpStatus:      false,
		OpBarrier:     false,
		OpStatusExt:   false,
		OpSessionList: false,
		OpEvent:       false,
	} {
		if got := RenewsLease(opcode); got != want {
			t.Errorf("opcode %d renews the lease = %v, want %v", opcode, got, want)
		}
	}
}
