package gdbnative

import (
	"encoding/binary"
	"sync"
	"time"
)

// MockState is the simulated device: what the SDK sent, what is held, who owns
// the collection and how the next write should go wrong.
type MockState struct {
	writes          []Packet
	held            InputState
	session         uint32
	sessions        []uint32
	closed          bool
	silent          bool
	disconnectAfter *int
	featureMissing  bool
	build           uint32
	uptimeMS        uint32
}

// MockTransport is an in-memory device that speaks the v3 wire protocol,
// including the reply timing, result bytes and input state of the firmware.
type MockTransport struct {
	mu      *sync.Mutex
	state   *MockState
	replies []Report
}

// MockHandle inspects and steers a MockTransport. Every clone shares the state
// of the transport the device is using.
type MockHandle struct {
	mu        *sync.Mutex
	state     *MockState
	transport *MockTransport
}

// NewMockTransport returns the transport and a handle to inspect it.
func NewMockTransport() (*MockTransport, *MockHandle) {
	mu := &sync.Mutex{}
	state := &MockState{
		build:    0x0DB00001,
		uptimeMS: 1000,
	}
	transport := &MockTransport{mu: mu, state: state}
	handle := &MockHandle{mu: mu, state: state, transport: transport}
	return transport, handle
}

// Writes is every packet the SDK sent, in order.
func (h *MockHandle) Writes() []Packet {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]Packet, len(h.state.writes))
	for index, packet := range h.state.writes {
		payload := make([]byte, len(packet.Payload))
		copy(payload, packet.Payload)
		packet.Payload = payload
		out[index] = packet
	}
	return out
}

// Count returns how many packets the SDK sent so far.
func (h *MockHandle) Count() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return len(h.state.writes)
}

// Sent returns the packets written at or after start.
func (h *MockHandle) Sent(start int) []Packet {
	h.mu.Lock()
	total := len(h.state.writes)
	h.mu.Unlock()
	if start >= total {
		return nil
	}
	return h.Writes()[start:]
}

// Held is the simulated input snapshot.
func (h *MockHandle) Held() InputState {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.state.held
}

// Session is the owner id currently accepted, 0 when nobody owns the collection.
func (h *MockHandle) Session() uint32 {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.state.session
}

// Sessions is every owner slot in use.
func (h *MockHandle) Sessions() []uint32 {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]uint32(nil), h.state.sessions...)
}

// Closed reports whether the owner sent CLOSE.
func (h *MockHandle) Closed() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.state.closed
}

// SetSilent makes the device stop answering control commands, which is how a
// host sees a dead link: reads time out instead of returning a result byte.
func (h *MockHandle) SetSilent(silent bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.state.silent = silent
}

// SetDisconnectAfter fails every write past the given count, like a USB reset.
func (h *MockHandle) SetDisconnectAfter(writes *int) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.state.disconnectAfter = writes
}

// SetFeatureMissing makes GET_FEATURE(14) return nothing.
func (h *MockHandle) SetFeatureMissing(missing bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.state.featureMissing = missing
}

// SetBuild changes the build id reported by Feature 14 and STATUS_EXT.
func (h *MockHandle) SetBuild(build uint32) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.state.build = build
}

// SetUptime changes the uptime reported by STATUS_EXT.
func (h *MockHandle) SetUptime(ms uint32) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.state.uptimeMS = ms
}

// Queue pushes an arbitrary inbound frame — a foreign session, a stale
// sequence, an EVENT, a malformed report. It is delivered before the replies
// the mock generates for the commands that follow.
func (h *MockHandle) Queue(report Report) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.transport.replies = append(h.transport.replies, report)
}

// NewFrame builds an inbound report for injection.
func NewFrame(opcode uint8, session, sequence uint32, payload []byte) (Report, error) {
	return Packet{Opcode: opcode, Session: session, Sequence: sequence, Payload: payload}.Encode(InReportID)
}

func (m *MockTransport) enqueue(report Report) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.replies = append(m.replies, report)
}

// Feature answers GET_FEATURE(14) with the same capability block as the firmware.
func (m *MockTransport) Feature() (Report, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var report Report
	if m.state.featureMissing {
		return report, errorf(Disconnected, "no Feature 14 reply")
	}
	report[0] = FeatureReportID
	copy(report[1:5], "FMT3")
	report[5] = ProtocolNumber
	report[6] = 2
	report[7] = 3
	report[8] = 2
	report[9] = 0
	binary.LittleEndian.PutUint16(report[10:12], 2000)
	report[14] = 1
	binary.LittleEndian.PutUint32(report[15:19], 15)
	for _, entry := range keyCatalog {
		page := uint16(entry >> 16)
		usage := uint16(entry)
		if page == 7 && usage < 224 {
			report[19+usage/8] |= uint8(1) << (usage % 8)
		}
	}
	report[47] = 0xFF
	report[48] = 0x7F
	copy(report[49:57], []byte{0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33})
	binary.LittleEndian.PutUint32(report[57:61], m.state.build)
	return report, nil
}

// Write decodes the outgoing packet, records it and produces the device's answer.
func (m *MockTransport) Write(report Report) error {
	packet, err := DecodePacket(report[:], OutReportID)
	if err != nil {
		return errorf(Framing, "%s", err)
	}
	m.mu.Lock()
	m.state.writes = append(m.state.writes, packet)
	if limit := m.state.disconnectAfter; limit != nil && len(m.state.writes) > *limit {
		m.mu.Unlock()
		return errorf(Disconnected, "mock transport disconnected")
	}
	silent := m.state.silent
	reply := m.answerLocked(packet)
	if silent {
		reply = nil
	}
	m.mu.Unlock()
	if reply != nil {
		m.enqueue(*reply)
	}
	return nil
}

// Read hands back the next queued frame, or ok=false when the queue is empty.
func (m *MockTransport) Read(timeout time.Duration) (Report, bool, error) {
	m.mu.Lock()
	if len(m.replies) > 0 {
		report := m.replies[0]
		m.replies = m.replies[1:]
		m.mu.Unlock()
		return report, true, nil
	}
	m.mu.Unlock()
	// Nothing arrived: behave like a read that waited and timed out, without
	// burning the CPU of the caller's reply loop.
	wait := timeout
	if wait > time.Millisecond {
		wait = time.Millisecond
	}
	if wait > 0 {
		time.Sleep(wait)
	}
	return Report{}, false, nil
}

// Close is a no-op; the mock owns no handle.
func (m *MockTransport) Close() error { return nil }

type mockReply struct {
	opcode   uint8
	session  uint32
	sequence uint32
	payload  []byte
}

func frameOf(reply mockReply) Report {
	// Every frame the device hands back is a control response: opcode | 0x80.
	report, err := Packet{
		Opcode:   reply.opcode | ReplyBit,
		Session:  reply.session,
		Sequence: reply.sequence,
		Payload:  reply.payload,
	}.Encode(InReportID)
	if err != nil {
		return Report{}
	}
	return report
}

// answerLocked implements the firmware side of one command. It returns the
// frame to hand back, or nil for commands the device does not acknowledge.
func (m *MockTransport) answerLocked(packet Packet) *Report {
	state := m.state
	session := packet.Session
	sequence := packet.Sequence
	payload := packet.Payload
	result := func(opcode uint8, body []byte) *Report {
		report := frameOf(mockReply{opcode: opcode, session: session, sequence: sequence, payload: body})
		return &report
	}
	switch packet.Opcode {
	case OpOpen:
		if session == 0 || sequence != 1 {
			return result(OpOpen, []byte{1})
		}
		if state.session == session || len(state.sessions) >= SessionSlots {
			return result(OpOpen, []byte{2})
		}
		state.session = session
		state.sessions = append(state.sessions, session)
		state.closed = false
		return result(OpOpen, []byte{0})
	case OpClose:
		remaining := state.sessions[:0]
		for _, id := range state.sessions {
			if id != session {
				remaining = append(remaining, id)
			}
		}
		state.sessions = remaining
		if state.session == session {
			state.session = 0
			state.closed = true
		}
		state.held = InputState{}
		return result(OpClose, []byte{0})
	case OpHeartbeat:
		return nil
	}
	if state.session != session {
		// Unknown session: the firmware drops input silently.
		return nil
	}
	switch packet.Opcode {
	case OpKey:
		if len(payload) < 5 {
			return result(OpEvent, []byte{3})
		}
		page := binary.LittleEndian.Uint16(payload[0:2])
		usage := binary.LittleEndian.Uint16(payload[2:4])
		if state.held.Set(page, usage, payload[4] != 0) != nil {
			return result(OpEvent, []byte{3})
		}
	case OpSnapshot:
		if next, err := DecodeInputState(payload); err == nil {
			state.held = next
		}
	case OpMouseMove, OpWheel:
	case OpMouseButton:
		if len(payload) > 0 {
			state.held.Buttons = payload[0]
		}
	case OpReleaseAll:
		state.held = InputState{}
		return result(OpReleaseAll, []byte{0})
	case OpBootloader:
		return result(OpBootloader, []byte{0})
	case OpBarrier:
		return result(OpBarrier, []byte{0})
	case OpStatus:
		body := make([]byte, StatusBytes)
		binary.LittleEndian.PutUint32(body[1:5], sequence)
		binary.LittleEndian.PutUint32(body[5:9], sequence)
		binary.LittleEndian.PutUint16(body[10:12], 2000)
		snapshot := state.held.Encode()
		copy(body[20:], snapshot[:])
		return result(OpStatus, body)
	case OpStatusExt:
		body := make([]byte, StatusExtBytes)
		binary.LittleEndian.PutUint32(body[1:5], state.uptimeMS)
		body[10] = uint8(len(state.sessions))
		binary.LittleEndian.PutUint32(body[12:16], ^uint32(0))
		body[16] = 3
		binary.LittleEndian.PutUint16(body[17:19], 1)
		binary.LittleEndian.PutUint32(body[19:23], state.build)
		return result(OpStatusExt, body)
	case OpSessionList:
		body := make([]byte, SessionListBytes)
		for index, id := range state.sessions {
			if index >= SessionSlots {
				break
			}
			base := index * SessionEntrySize
			binary.LittleEndian.PutUint16(body[base:base+2], uint16(id))
			binary.LittleEndian.PutUint16(body[base+2:base+4], 2000)
		}
		return result(OpSessionList, body)
	default:
		return result(OpEvent, []byte{3})
	}
	return nil
}
