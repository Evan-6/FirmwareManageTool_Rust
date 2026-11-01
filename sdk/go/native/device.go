package gdbnative

import (
	"encoding/json"
	"os"
	"sync"
	"sync/atomic"
	"time"
)

// HeartbeatInterval is how long the host may stay quiet before it renews the
// lease. The lease itself is 2000 ms (protocol-v3.md, 完成與租約), so 500 ms
// leaves three retries before the device drops the session.
const HeartbeatInterval = 500 * time.Millisecond

// DefaultLease is the firmware lease used when the capability block does not
// say otherwise.
const DefaultLease = 2000 * time.Millisecond

// ConnectionState mirrors gdb_connection_state.
type ConnectionState int32

const (
	StateDisconnected ConnectionState = 0
	StateConnecting   ConnectionState = 1
	StateConnected    ConnectionState = 2
	StateError        ConnectionState = 3
)

// EventKind distinguishes the callback shapes. It matches the `kind` argument
// of gdb_set_event_handler, and the three cases of sdk/rust/src/event.rs.
type EventKind int32

const (
	EventState  EventKind = 0
	EventError  EventKind = 1
	EventStatus EventKind = 2
)

// Event is one callback invocation. Code and Message are the state or error
// payload; Status carries the block of an EventStatus report.
type Event struct {
	Kind    EventKind
	Code    int32
	Message string
	Status  Status
}

// Handler is the full callback set of sdk/rust/src/event.rs: a connection state
// change, a STATUS observation and a typed error. Embed HandlerFuncs (or
// implement only the three methods you care about) and install it with
// SetHandler; the older single-function SetEventHandler keeps working next to it.
type Handler interface {
	State(state ConnectionState)
	Status(status Status)
	Error(err *Error)
}

// HandlerFuncs adapts three optional closures to Handler; a nil field is not
// called, which is how a caller subscribes to one kind of event.
type HandlerFuncs struct {
	OnState  func(ConnectionState)
	OnStatus func(Status)
	OnError  func(*Error)
}

// State forwards to OnState.
func (h HandlerFuncs) State(state ConnectionState) {
	if h.OnState != nil {
		h.OnState(state)
	}
}

// Status forwards to OnStatus.
func (h HandlerFuncs) Status(status Status) {
	if h.OnStatus != nil {
		h.OnStatus(status)
	}
}

// Error forwards to OnError.
func (h HandlerFuncs) Error(err *Error) {
	if h.OnError != nil {
		h.OnError(err)
	}
}

// Options are the connection settings the C ABI exposes.
type Options struct {
	// TimeoutMS bounds one command reply, minimum 100 ms.
	TimeoutMS uint32
	// AutoReconnect reopens the transport once after a recoverable failure.
	AutoReconnect bool
	// Heartbeat keeps the lease alive from a background goroutine.
	Heartbeat bool
}

// DefaultOptions matches gdb_open's defaults: 1 s replies, auto reconnect on,
// heartbeat on.
func DefaultOptions() Options {
	return Options{TimeoutMS: 1000, AutoReconnect: true, Heartbeat: true}
}

// Info is the device identity, shaped like gdb_device_info's JSON.
type Info struct {
	Serial       string  `json:"serial"`
	Product      string  `json:"product"`
	Board        uint8   `json:"board"`
	Firmware     string  `json:"firmware"`
	LeaseMS      uint16  `json:"leaseMs"`
	PollMS       uint8   `json:"pollMs"`
	Capabilities uint32  `json:"capabilities"`
	DeviceID     [8]byte `json:"deviceId"`
	Build        uint32  `json:"build"`
}

// Entry is one device in the enumeration, shaped like gdb_device_list's JSON.
type Entry struct {
	Path         string `json:"path"`
	Serial       string `json:"serial"`
	Product      string `json:"product"`
	Manufacturer string `json:"manufacturer"`
	VID          uint16 `json:"vid"`
	PID          uint16 `json:"pid"`
}

// session is one vendor-HID owner: Feature 14 handshake, OPEN, sequence
// numbers, reply matching and the lease clock. Nothing above it touches a
// sequence number.
type session struct {
	transport    Transport
	id           uint32
	sequence     uint32
	capabilities Capabilities
	timeout      time.Duration
	// lastRenew is when this session last sent a lease-renewing command.
	lastRenew time.Time
}

func openSession(transport Transport, timeoutMS uint32, id uint32) (*session, error) {
	report, err := transport.Feature()
	if err != nil {
		return nil, err
	}
	capabilities, err := ParseCapabilities(report[:])
	if err != nil {
		return nil, err
	}
	if timeoutMS < 100 {
		timeoutMS = 100
	}
	if id == 0 {
		id = 1
	}
	opened := &session{
		transport:    transport,
		id:           id,
		capabilities: capabilities,
		timeout:      time.Duration(timeoutMS) * time.Millisecond,
		lastRenew:    time.Now(),
	}
	if _, err := opened.command(OpOpen, nil); err != nil {
		return nil, err
	}
	return opened, nil
}

// renewAge is how long ago the device last saw something lease-renewing.
func (s *session) renewAge() time.Duration { return time.Since(s.lastRenew) }

// command sends one packet and, for control commands, waits for its own reply.
// Foreign frames — another session's traffic, a stale sequence, an EVENT for
// somebody else — are skipped, never mistaken for this command's answer.
func (s *session) command(opcode uint8, payload []byte) (Packet, error) {
	if len(payload) > PayloadSize {
		return Packet{}, errorf(Invalid, "payload is %d bytes, limit %d", len(payload), PayloadSize)
	}
	next := s.sequence + 1
	if next == 0 {
		return Packet{}, errorf(Invalid, "sequence exhausted; reopen the device")
	}
	s.sequence = next
	packet := Packet{Opcode: opcode, Session: s.id, Sequence: next, Payload: payload}
	report, err := packet.Encode(OutReportID)
	if err != nil {
		return Packet{}, err
	}
	if RenewsLease(opcode) {
		s.lastRenew = time.Now()
	}
	logf(LogTrace, "out opcode=%d session=%d sequence=%d payload=%s", opcode, s.id, next, formatPayload(payload))
	if err := s.transport.Write(report); err != nil {
		return Packet{}, err
	}
	if !ExpectsReply(opcode) {
		return Packet{}, nil
	}
	deadline := time.Now().Add(s.timeout)
	for {
		remaining := time.Until(deadline)
		if remaining <= 0 {
			return Packet{}, errorf(Timeout, "no reply to opcode %d within %d ms", opcode, s.timeout.Milliseconds())
		}
		reply, ok, err := s.transport.Read(remaining)
		if err != nil {
			return Packet{}, err
		}
		if !ok {
			continue
		}
		answer, err := DecodePacket(reply[:], InReportID)
		if err != nil {
			return Packet{}, errorf(Invalid, "%s", err)
		}
		if answer.Session != s.id {
			logf(LogTrace, "skip opcode=%d session=%d sequence=%d (not ours)", answer.Opcode, answer.Session, answer.Sequence)
			continue
		}
		logf(LogTrace, "in opcode=%d session=%d sequence=%d payload=%s",
			answer.Opcode, answer.Session, answer.Sequence, formatPayload(answer.Payload))
		if answer.Opcode == OpEvent {
			code := uint8(0)
			if len(answer.Payload) > 0 {
				code = answer.Payload[0]
			}
			if failure := FromResult(code); failure != nil {
				return Packet{}, failure
			}
			return Packet{}, errorf(IO, "device sent EVENT with result 0")
		}
		if answer.Opcode != opcode|ReplyBit || answer.Sequence != next {
			continue
		}
		if err := CheckResponse(answer, s.id, next, opcode); err != nil {
			return Packet{}, err
		}
		return answer, nil
	}
}

func (s *session) close() error {
	_, err := s.command(OpClose, nil)
	return err
}

// Device is one controller session. All command traffic is serialised, so the
// heartbeat goroutine and the caller never interleave sequence numbers.
type Device struct {
	mu     sync.Mutex
	sess   *session
	opts   Options
	info   Info
	mock   bool
	reopen func() (Transport, error)

	closed        atomic.Bool
	autoReconnect atomic.Bool
	buttons       atomic.Uint32

	cursorMu sync.Mutex
	cursor   [2]int32

	eventMu  sync.RWMutex
	handler  func(Event)
	handlers Handler

	stopHeartbeat chan struct{}
	heartbeatDone chan struct{}
}

// OpenMock opens the in-memory device with the default options.
func OpenMock() (*Device, error) { return OpenMockOptions(DefaultOptions()) }

// OpenMockOptions is OpenMock with explicit options.
func OpenMockOptions(opts Options) (*Device, error) {
	transport, _ := NewMockTransport()
	device, err := newDevice(transport, targetMock, "MOCK", "GDBOX mock", opts)
	if err != nil {
		return nil, err
	}
	device.reopen = func() (Transport, error) {
		next, _ := NewMockTransport()
		return next, nil
	}
	return device, nil
}

// OpenWithTransport builds a device on top of an existing transport: the entry
// point for tests and for callers that bring their own HID layer. reopen is
// called by the auto-reconnect path; pass nil to make recovery impossible.
func OpenWithTransport(transport Transport, serial, product string, opts Options, reopen func() (Transport, error)) (*Device, error) {
	device, err := newDevice(transport, targetPath, serial, product, opts)
	if err != nil {
		return nil, err
	}
	device.reopen = reopen
	return device, nil
}

// The two kinds of device the SDK can drive.
const (
	targetPath = "path"
	targetMock = "mock"
)

func newDevice(transport Transport, target, serial, product string, opts Options) (*Device, error) {
	if opts.TimeoutMS == 0 {
		opts.TimeoutMS = DefaultOptions().TimeoutMS
	}
	opened, err := openSession(transport, opts.TimeoutMS, newSessionID())
	if err != nil {
		_ = transport.Close()
		return nil, err
	}
	capabilities := opened.capabilities
	device := &Device{
		sess: opened,
		opts: opts,
		mock: target == targetMock,
		info: Info{
			Serial:       serial,
			Product:      product,
			Board:        capabilities.Board,
			Firmware:     capabilities.Firmware,
			LeaseMS:      capabilities.LeaseMS,
			PollMS:       capabilities.PollMS,
			Capabilities: capabilities.Flags,
			DeviceID:     capabilities.DeviceID,
			Build:        capabilities.Build,
		},
		cursor: [2]int32{960, 540},
	}
	device.autoReconnect.Store(opts.AutoReconnect)
	if opts.Heartbeat {
		device.startHeartbeat()
	}
	logf(LogInfo, "connected serial=%s firmware=%s board=%d",
		device.info.Serial, device.info.Firmware, device.info.Board)
	return device, nil
}

// Info is the identity read during the Feature 14 handshake.
func (d *Device) Info() Info { return d.info }

// InfoJSON returns the identity as JSON, the shape gdb_device_info produces.
func (d *Device) InfoJSON() (string, error) {
	return jsonString(d.info)
}

// Serial is the USB serial number the device was opened with.
func (d *Device) Serial() string { return d.info.Serial }

// IsMock reports whether the device talks to MockTransport.
func (d *Device) IsMock() bool { return d.mock }

// State is the connection state.
func (d *Device) State() ConnectionState {
	if d.closed.Load() {
		return StateDisconnected
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.sess != nil {
		return StateConnected
	}
	return StateDisconnected
}

// SetEventHandler installs the callback for state changes, errors and STATUS
// reports; nil removes it. Handlers run on whichever goroutine observed the
// change.
func (d *Device) SetEventHandler(handler func(Event)) {
	d.eventMu.Lock()
	d.handler = handler
	d.eventMu.Unlock()
}

// SetHandler installs the three-method callback set sdk/rust/src/event.rs
// exposes; nil removes it. Both kinds of handler can be installed at once and
// both see every event.
func (d *Device) SetHandler(handler Handler) {
	d.eventMu.Lock()
	d.handlers = handler
	d.eventMu.Unlock()
}

// SetAutoReconnect toggles recovery after a recoverable failure.
func (d *Device) SetAutoReconnect(enabled bool) { d.autoReconnect.Store(enabled) }

// Status asks for the 51-byte STATUS block. The parsed block is also reported
// to the handler as an EventStatus, which is this SDK's form of the status
// callback in sdk/rust/src/event.rs.
func (d *Device) Status() (Status, error) {
	var status Status
	err := d.withSession(func(s *session) error {
		reply, err := s.command(OpStatus, nil)
		if err != nil {
			return err
		}
		parsed, err := ParseStatus(reply.Payload)
		if err != nil {
			return err
		}
		status = parsed
		return nil
	})
	if err != nil {
		return status, err
	}
	d.notify(Event{Kind: EventStatus, Status: status})
	return status, nil
}

// StatusJSON returns STATUS as JSON, the shape gdb_status produces.
func (d *Device) StatusJSON() (string, error) {
	status, err := d.Status()
	if err != nil {
		return "", err
	}
	return jsonString(status)
}

// StatusExt asks for the 23-byte diagnostics block.
func (d *Device) StatusExt() (StatusExt, error) {
	var ext StatusExt
	err := d.withSession(func(s *session) error {
		reply, err := s.command(OpStatusExt, nil)
		if err != nil {
			return err
		}
		parsed, err := ParseStatusExt(reply.Payload)
		if err != nil {
			return err
		}
		ext = parsed
		return nil
	})
	return ext, err
}

// StatusExtJSON returns STATUS_EXT as JSON.
func (d *Device) StatusExtJSON() (string, error) {
	ext, err := d.StatusExt()
	if err != nil {
		return "", err
	}
	return jsonString(ext)
}

// Sessions asks who owns the collection.
func (d *Device) Sessions() ([]SessionInfo, error) {
	var sessions []SessionInfo
	err := d.withSession(func(s *session) error {
		reply, err := s.command(OpSessionList, nil)
		if err != nil {
			return err
		}
		sessions = ParseSessionList(reply.Payload)
		return nil
	})
	return sessions, err
}

// SessionsJSON returns SESSION_LIST as JSON, the shape gdb_sessions produces.
func (d *Device) SessionsJSON() (string, error) {
	sessions, err := d.Sessions()
	if err != nil {
		return "", err
	}
	return jsonString(sessions)
}

// Barrier waits until every earlier output has left the device's USB queue.
// It does not renew the lease.
func (d *Device) Barrier() error {
	return d.withSession(func(s *session) error {
		_, err := s.command(OpBarrier, nil)
		return err
	})
}

// Command sends one opcode and payload exactly as given and hands back the raw
// reply — the escape hatch sdk/rust/src/device.rs calls `command`.
//
// It goes through the same session, sequence numbering and reconnect path as
// the typed methods, but nothing is bookkept: held buttons and the mock cursor
// are left as they were, so a caller that sends MOUSE_MOVE, MOUSE_BUTTON or
// SNAPSHOT through here keeps its own state in step. For an opcode that expects
// no reply (ExpectsReply is false) the returned Packet is zero.
func (d *Device) Command(opcode uint8, payload []byte) (Packet, error) {
	var reply Packet
	err := d.withSession(func(s *session) error {
		answer, err := s.command(opcode, payload)
		if err != nil {
			return err
		}
		reply = answer
		return nil
	})
	return reply, err
}

// ReleaseAll drops this session's held keys and buttons.
func (d *Device) ReleaseAll() error {
	d.buttons.Store(0)
	return d.withSession(func(s *session) error {
		_, err := s.command(OpReleaseAll, nil)
		return err
	})
}

// Snapshot replaces this session's whole input state in one report. Other
// owners keep their own state; the firmware unions the keyboard and buttons.
func (d *Device) Snapshot(state InputState) error {
	raw := state.Encode()
	if _, err := DecodeInputState(raw[:]); err != nil {
		return err
	}
	if state.Buttons&^uint8(buttonMask) != 0 {
		return errorf(Invalid, "buttons are bits 0-4")
	}
	d.buttons.Store(uint32(state.Buttons))
	return d.send(OpSnapshot, raw[:])
}

// Bootloader reboots the board into BOOTSEL.
func (d *Device) Bootloader() error {
	return d.withSession(func(s *session) error {
		_, err := s.command(OpBootloader, nil)
		return err
	})
}

// Reconnect reopens the transport and opens a fresh session; held keys are not
// restored.
func (d *Device) Reconnect() error {
	if d.closed.Load() {
		return errorf(Disconnected, "device is closed")
	}
	var events []Event
	d.mu.Lock()
	err := d.reopenLocked(&events)
	d.mu.Unlock()
	for _, event := range events {
		d.notify(event)
	}
	return err
}

// Close sends CLOSE, releases the transport and stops the heartbeat.
func (d *Device) Close() error {
	if d.closed.Swap(true) {
		return nil
	}
	if d.stopHeartbeat != nil {
		close(d.stopHeartbeat)
		<-d.heartbeatDone
		d.stopHeartbeat = nil
	}
	d.buttons.Store(0)
	logf(LogInfo, "closed serial=%s", d.info.Serial)
	d.mu.Lock()
	current := d.sess
	d.sess = nil
	var err error
	if current != nil {
		err = current.close()
		_ = current.transport.Close()
	}
	d.mu.Unlock()
	d.notify(Event{Kind: EventState, Code: int32(StateDisconnected)})
	return err
}

// Mouse returns the mouse handle.
func (d *Device) Mouse() *Mouse { return &Mouse{device: d} }

// Keyboard returns the keyboard handle.
func (d *Device) Keyboard() *Keyboard { return &Keyboard{device: d} }

// withSession runs one operation against the live session, reopening the
// transport once when the failure is recoverable and auto reconnect is on.
func (d *Device) withSession(operation func(*session) error) error {
	if d.closed.Load() {
		return errorf(Disconnected, "device is closed")
	}
	var events []Event
	d.mu.Lock()
	err := d.runLocked(operation, &events)
	d.mu.Unlock()
	for _, event := range events {
		d.notify(event)
	}
	return err
}

func (d *Device) runLocked(operation func(*session) error, events *[]Event) error {
	if d.sess != nil {
		err := operation(d.sess)
		if err == nil {
			return nil
		}
		logf(LogWarn, "command failed (%d): %s", int32(CodeOf(err)), messageOf(err))
		if !recoverable(CodeOf(err)) || !d.autoReconnect.Load() {
			*events = append(*events, errorEvent(err))
			return err
		}
		d.sess = nil
	}
	if err := d.reopenLocked(events); err != nil {
		d.closed.Store(true)
		*events = append(*events, errorEvent(err))
		return err
	}
	err := operation(d.sess)
	if err != nil {
		*events = append(*events, errorEvent(err))
	}
	return err
}

func (d *Device) reopenLocked(events *[]Event) error {
	if d.reopen == nil {
		return errorf(Disconnected, "no transport to reopen")
	}
	transport, err := d.reopen()
	if err != nil {
		return err
	}
	opened, err := openSession(transport, d.opts.TimeoutMS, newSessionID())
	if err != nil {
		_ = transport.Close()
		return err
	}
	d.sess = opened
	logf(LogInfo, "reconnected serial=%s", d.info.Serial)
	if events != nil {
		*events = append(*events, Event{Kind: EventState, Code: int32(StateConnected)})
	}
	return nil
}

// send emits one input command on the session.
func (d *Device) send(opcode uint8, payload []byte) error {
	return d.withSession(func(s *session) error {
		_, err := s.command(opcode, payload)
		return err
	})
}

func (d *Device) sendMouseMove(dx, dy int16) error {
	if err := d.send(OpMouseMove, []byte{byte(dx), byte(dx >> 8), byte(dy), byte(dy >> 8)}); err != nil {
		return err
	}
	if d.mock {
		d.cursorMu.Lock()
		d.cursor[0] = clampInt32(d.cursor[0]+int32(dx), 0, 1919)
		d.cursor[1] = clampInt32(d.cursor[1]+int32(dy), 0, 1079)
		d.cursorMu.Unlock()
	}
	return nil
}

// cursorProbe is the platform probe for the pointer and the primary monitor.
// Tests replace it to put the cursor on a second monitor without moving the
// real one.
var cursorProbe = screenAndCursor

// cursorPosition is the mock's simulated pointer or the real one, plus the
// screen absolute coordinates are expressed on.
func (d *Device) cursorPosition() (Screen, [2]int32, error) {
	if d.mock {
		return DefaultScreen(), d.cursorState(), nil
	}
	return cursorProbe()
}

func processID() int { return os.Getpid() }

func (d *Device) cursorState() [2]int32 {
	d.cursorMu.Lock()
	defer d.cursorMu.Unlock()
	return d.cursor
}

func (d *Device) buttonsMask() uint8 { return uint8(d.buttons.Load()) }

func (d *Device) setButtonsMask(mask uint8) error {
	if mask&^uint8(buttonMask) != 0 {
		return errorf(Invalid, "buttons are bits 0-4")
	}
	d.buttons.Store(uint32(mask))
	return nil
}

func (d *Device) startHeartbeat() {
	stop := make(chan struct{})
	d.stopHeartbeat = stop
	d.heartbeatDone = make(chan struct{})
	done := d.heartbeatDone
	go func() {
		defer close(done)
		interval := HeartbeatInterval / 5
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			select {
			case <-stop:
				return
			case <-ticker.C:
			}
			if d.closed.Load() {
				return
			}
			// TryLock, not Lock: a STATUS poll in flight must not stall the
			// heartbeat, and the next tick is 100 ms away anyway.
			if !d.mu.TryLock() {
				continue
			}
			due := d.sess != nil && d.sess.renewAge() >= HeartbeatInterval
			var failure error
			if due {
				_, failure = d.sess.command(OpHeartbeat, nil)
			}
			d.mu.Unlock()
			if failure == nil {
				continue
			}
			d.notify(errorEvent(failure))
			if d.autoReconnect.Load() && recoverable(CodeOf(failure)) {
				_ = d.Reconnect()
			}
		}
	}()
}

func (d *Device) notify(event Event) {
	d.eventMu.RLock()
	handler := d.handler
	handlers := d.handlers
	d.eventMu.RUnlock()
	if handler != nil {
		handler(event)
	}
	if handlers == nil {
		return
	}
	switch event.Kind {
	case EventState:
		handlers.State(ConnectionState(event.Code))
	case EventStatus:
		handlers.Status(event.Status)
	case EventError:
		handlers.Error(&Error{Code: Code(event.Code), Message: event.Message})
	}
}

func errorEvent(err error) Event {
	code := CodeOf(err)
	message := ""
	if failure, ok := err.(*Error); ok {
		message = failure.Message
	}
	return Event{Kind: EventError, Code: int32(code), Message: message}
}

func jsonString(value any) (string, error) {
	encoded, err := json.Marshal(value)
	if err != nil {
		return "", errorf(IO, "%s", err)
	}
	return string(encoded), nil
}

var (
	sessionCounter atomic.Uint32
)

// newSessionID returns a non-zero, per-process-unique session id.
func newSessionID() uint32 {
	now := time.Now().UnixNano()
	pid := uint32(os.Getpid())
	next := sessionCounter.Add(0x9E3779B9)
	id := uint32(now) ^ uint32(now>>32) ^ pid ^ next
	if id == 0 {
		id = 1
	}
	return id
}

func clampInt32(value, low, high int32) int32 {
	if value < low {
		return low
	}
	if value > high {
		return high
	}
	return value
}
