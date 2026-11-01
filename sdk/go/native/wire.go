package gdbnative

import (
	"encoding/binary"
	"fmt"
)

// Report identifiers of the vendor collection.
const (
	OutReportID     = 12
	InReportID      = 13
	FeatureReportID = 14
	// ReportSize is the full report length, report id included. Feature 14 must
	// answer 64 bytes too: a 63-byte payload with no id reads as a short report.
	ReportSize     = 64
	PayloadSize    = 51
	ProtocolNumber = 3
)

// Opcodes (shared/input-protocol/src/v3.rs).
const (
	OpOpen        = 0x01
	OpClose       = 0x02
	OpHeartbeat   = 0x03
	OpBarrier     = 0x04
	OpReleaseAll  = 0x05
	OpStatus      = 0x06
	OpStatusExt   = 0x07
	OpSessionList = 0x08
	OpBootloader  = 0x09
	OpKey         = 0x10
	OpSnapshot    = 0x11
	OpMouseMove   = 0x12
	OpMouseButton = 0x13
	OpWheel       = 0x14
	OpEvent       = 0x7F
	// ReplyBit marks a control response: response opcode == command | 0x80.
	ReplyBit = 0x80
)

// Payload sizes fixed by the firmware.
const (
	SnapshotBytes    = 31
	StatusBytes      = 51
	StatusExtBytes   = 23
	SessionEntrySize = 6
	SessionSlots     = 8
	SessionListBytes = SessionEntrySize * SessionSlots
)

// Mouse button bits, in HID order.
const (
	buttonMask = 0x1F
)

// Packet is one 63-byte v3 payload plus its header fields.
type Packet struct {
	Opcode   uint8
	Flags    uint8
	Session  uint32
	Sequence uint32
	Payload  []byte
}

// Encode lays the packet out exactly as shared/input-protocol/src/v3.rs does:
// id, version, opcode, length, flags, session, sequence, payload, zero padding.
func (p Packet) Encode(id uint8) ([ReportSize]byte, error) {
	var report [ReportSize]byte
	if id != OutReportID && id != InReportID {
		return report, errorf(Invalid, "v3 report id %d is not %d or %d", id, OutReportID, InReportID)
	}
	if len(p.Payload) > PayloadSize {
		return report, errorf(Invalid, "v3 payload is %d bytes, limit %d", len(p.Payload), PayloadSize)
	}
	if p.Flags != 0 {
		return report, errorf(Invalid, "v3 flags %d are not supported", p.Flags)
	}
	report[0] = id
	report[1] = ProtocolNumber
	report[2] = p.Opcode
	report[3] = uint8(len(p.Payload))
	report[4] = p.Flags
	binary.LittleEndian.PutUint32(report[5:9], p.Session)
	binary.LittleEndian.PutUint32(report[9:13], p.Sequence)
	copy(report[13:], p.Payload)
	return report, nil
}

// DecodePacket parses one inbound report, rejecting a wrong report id, version,
// flags, an over-long length or non-zero padding.
func DecodePacket(report []byte, id uint8) (Packet, error) {
	if len(report) != ReportSize || report[0] != id || report[1] != ProtocolNumber || report[4] != 0 {
		return Packet{}, errorf(Framing, "invalid v3 report id %d, version %d, flags %d, length %d",
			firstOrZero(report, 0), firstOrZero(report, 1), firstOrZero(report, 4), len(report))
	}
	length := int(report[3])
	if length > PayloadSize {
		return Packet{}, errorf(Framing, "v3 payload length %d exceeds %d", length, PayloadSize)
	}
	for _, value := range report[13+length:] {
		if value != 0 {
			return Packet{}, errorf(Framing, "v3 padding is not zero")
		}
	}
	payload := make([]byte, length)
	copy(payload, report[13:13+length])
	return Packet{
		Opcode:   report[2],
		Session:  binary.LittleEndian.Uint32(report[5:9]),
		Sequence: binary.LittleEndian.Uint32(report[9:13]),
		Payload:  payload,
	}, nil
}

func firstOrZero(buffer []byte, index int) uint8 {
	if index < len(buffer) {
		return buffer[index]
	}
	return 0
}

// ExpectsReply reports whether the device answers a command with a control
// response. Input commands are fire-and-forget; completion is BARRIER's job.
func ExpectsReply(opcode uint8) bool {
	switch opcode {
	case OpOpen, OpClose, OpStatus, OpBarrier, OpReleaseAll, OpStatusExt, OpSessionList, OpBootloader:
		return true
	default:
		return false
	}
}

// RenewsLease reports whether the device counts the command as a lease renewal.
// It writes lease_at for OPEN, HEARTBEAT, RELEASE_ALL, BOOTLOADER and valid input
// only (firmware/engine/src/engine.rs). STATUS, BARRIER, STATUS_EXT and
// SESSION_LIST do not renew, so a host polling any of them still has to
// heartbeat.
func RenewsLease(opcode uint8) bool {
	switch opcode {
	case OpOpen, OpHeartbeat, OpReleaseAll, OpBootloader,
		OpKey, OpSnapshot, OpMouseMove, OpMouseButton, OpWheel:
		return true
	default:
		return false
	}
}

// InputState is the 31-byte snapshot: modifiers, 28 key bytes, consumer, buttons.
type InputState struct {
	Modifiers uint8    `json:"modifiers"`
	Keys      [28]byte `json:"keys"`
	Consumer  uint8    `json:"consumer"`
	Buttons   uint8    `json:"buttons"`
}

// Set adds or removes one usage from the state.
func (s *InputState) Set(page, usage uint16, down bool) error {
	if !ValidKey(page, usage) {
		return errorf(Invalid, "usage %#06x:%#06x is not in the key catalog", page, usage)
	}
	mask := uint8(1) << (usage % 8)
	switch {
	case page == 7 && usage >= 0xE0 && usage <= 0xE7:
		if down {
			s.Modifiers |= uint8(1) << (usage - 0xE0)
		} else {
			s.Modifiers &^= uint8(1) << (usage - 0xE0)
		}
	case page == 7 && usage < 224:
		if down {
			s.Keys[usage/8] |= mask
		} else {
			s.Keys[usage/8] &^= mask
		}
	default:
		index, ok := consumerIndex(usage)
		if !ok {
			return errorf(Invalid, "usage %#06x:%#06x is not a consumer usage", page, usage)
		}
		if down {
			s.Consumer |= uint8(1) << index
		} else {
			s.Consumer &^= uint8(1) << index
		}
	}
	return nil
}

// Contains reports whether one usage is held.
func (s InputState) Contains(page, usage uint16) bool {
	if !ValidKey(page, usage) {
		return false
	}
	switch {
	case page == 7 && usage >= 0xE0 && usage <= 0xE7:
		return s.Modifiers&(uint8(1)<<(usage-0xE0)) != 0
	case page == 7 && usage < 224:
		return s.Keys[usage/8]&(uint8(1)<<(usage%8)) != 0
	default:
		index, ok := consumerIndex(usage)
		return ok && s.Consumer&(uint8(1)<<index) != 0
	}
}

// Encode returns the wire representation of the snapshot.
func (s InputState) Encode() [SnapshotBytes]byte {
	var out [SnapshotBytes]byte
	out[0] = s.Modifiers
	copy(out[1:29], s.Keys[:])
	out[29] = s.Consumer
	out[30] = s.Buttons
	return out
}

// DecodeInputState validates and parses a snapshot, rejecting reserved usages.
func DecodeInputState(raw []byte) (InputState, error) {
	var state InputState
	if len(raw) != SnapshotBytes || raw[29]&0x80 != 0 || raw[30]&^uint8(buttonMask) != 0 {
		return state, errorf(Framing, "invalid input snapshot")
	}
	state.Modifiers = raw[0]
	copy(state.Keys[:], raw[1:29])
	state.Consumer = raw[29]
	state.Buttons = raw[30]
	for usage := 0; usage < 224; usage++ {
		if state.Keys[usage/8]&(uint8(1)<<(usage%8)) != 0 && !ValidKey(7, uint16(usage)) {
			return state, errorf(Framing, "snapshot holds reserved usage %d", usage)
		}
	}
	return state, nil
}

// Capabilities is the Feature 14 answer. Byte 0 is the report id.
type Capabilities struct {
	Board     uint8
	Firmware  string
	LeaseMS   uint16
	PollMS    uint8
	Flags     uint32
	Keyboard  [28]byte
	Modifiers uint8
	Consumer  uint8
	DeviceID  [8]byte
	Build     uint32
}

// ParseCapabilities reads the 64-byte Feature 14 report.
func ParseCapabilities(report []byte) (Capabilities, error) {
	var caps Capabilities
	if len(report) != ReportSize || report[0] != FeatureReportID || string(report[1:5]) != "FMT3" || report[5] != ProtocolNumber {
		return caps, errorf(Framing, "invalid capability reply")
	}
	for _, value := range report[61:] {
		if value != 0 {
			return caps, errorf(Framing, "capability reply has a reserved byte set")
		}
	}
	caps.Board = report[6]
	caps.Firmware = fmt.Sprintf("%d.%d.%d", report[7], report[8], report[9])
	caps.LeaseMS = binary.LittleEndian.Uint16(report[10:12])
	caps.PollMS = report[14]
	caps.Flags = binary.LittleEndian.Uint32(report[15:19])
	copy(caps.Keyboard[:], report[19:47])
	caps.Modifiers = report[47]
	caps.Consumer = report[48]
	copy(caps.DeviceID[:], report[49:57])
	caps.Build = binary.LittleEndian.Uint32(report[57:61])
	if caps.LeaseMS < 1000 || caps.PollMS == 0 {
		return caps, errorf(Framing, "invalid lease %d ms / poll %d ms", caps.LeaseMS, caps.PollMS)
	}
	return caps, nil
}

// Supports reports whether the firmware advertises one usage.
func (c Capabilities) Supports(page, usage uint16) bool {
	if !ValidKey(page, usage) {
		return false
	}
	switch {
	case page == 7 && usage >= 0xE0:
		return c.Modifiers&(uint8(1)<<(usage-0xE0)) != 0
	case page == 7:
		return c.Keyboard[usage/8]&(uint8(1)<<(usage%8)) != 0
	default:
		index, ok := consumerIndex(usage)
		return ok && c.Consumer&(uint8(1)<<index) != 0
	}
}

// Status is the 51-byte STATUS reply.
type Status struct {
	Received  uint32     `json:"received"`
	Completed uint32     `json:"completed"`
	Pending   uint8      `json:"pending"`
	LeaseMS   uint16     `json:"leaseMs"`
	RX        uint16     `json:"rx"`
	TX        uint16     `json:"tx"`
	HID       uint16     `json:"hid"`
	Failsafe  uint16     `json:"failsafe"`
	State     InputState `json:"state"`
}

// ParseStatus reads the STATUS payload, which carries its own result byte.
func ParseStatus(payload []byte) (Status, error) {
	var status Status
	if len(payload) != StatusBytes || payload[0] != 0 {
		return status, errorf(Framing, "invalid v3 status")
	}
	status.Received = binary.LittleEndian.Uint32(payload[1:5])
	status.Completed = binary.LittleEndian.Uint32(payload[5:9])
	status.Pending = payload[9]
	status.LeaseMS = binary.LittleEndian.Uint16(payload[10:12])
	status.RX = binary.LittleEndian.Uint16(payload[12:14])
	status.TX = binary.LittleEndian.Uint16(payload[14:16])
	status.HID = binary.LittleEndian.Uint16(payload[16:18])
	status.Failsafe = binary.LittleEndian.Uint16(payload[18:20])
	state, err := DecodeInputState(payload[20:])
	if err != nil {
		return status, err
	}
	status.State = state
	return status, nil
}

// StatusExt is the 23-byte STATUS_EXT diagnostics block.
type StatusExt struct {
	UptimeMS       uint32  `json:"uptimeMs"`
	LateMaxUS      uint16  `json:"lateMaxUs"`
	OutDepth       uint8   `json:"outDepth"`
	RXDepth        uint8   `json:"rxDepth"`
	TXDepth        uint8   `json:"txDepth"`
	Sessions       uint8   `json:"sessions"`
	LastFault      uint8   `json:"lastFault"`
	LastFaultAgeMS *uint32 `json:"lastFaultAgeMs"`
	ResetReason    uint8   `json:"resetReason"`
	BootCount      uint16  `json:"bootCount"`
	Build          uint32  `json:"build"`
}

// ParseStatusExt reads the STATUS_EXT payload.
func ParseStatusExt(payload []byte) (StatusExt, error) {
	var ext StatusExt
	if len(payload) != StatusExtBytes || payload[0] != 0 {
		return ext, errorf(Framing, "invalid STATUS_EXT")
	}
	ext.UptimeMS = binary.LittleEndian.Uint32(payload[1:5])
	ext.LateMaxUS = binary.LittleEndian.Uint16(payload[5:7])
	ext.OutDepth = payload[7]
	ext.RXDepth = payload[8]
	ext.TXDepth = payload[9]
	ext.Sessions = payload[10]
	ext.LastFault = payload[11]
	if age := binary.LittleEndian.Uint32(payload[12:16]); age != ^uint32(0) {
		value := age
		ext.LastFaultAgeMS = &value
	}
	ext.ResetReason = payload[16]
	ext.BootCount = binary.LittleEndian.Uint16(payload[17:19])
	ext.Build = binary.LittleEndian.Uint32(payload[19:23])
	return ext, nil
}

// SessionInfo is one owner as reported by SESSION_LIST.
type SessionInfo struct {
	// ID is the low 16 bits of the session id, enough to match a host log line.
	ID      uint16 `json:"id"`
	LeaseMS uint16 `json:"leaseMs"`
	Held    uint8  `json:"held"`
	// Flags: 1 pending command, 2 released, 4 bootloader requested.
	Flags uint8 `json:"flags"`
}

// ParseSessionList walks the eight fixed slots; the payload has no result byte,
// so the first id sits in byte 0.
func ParseSessionList(payload []byte) []SessionInfo {
	sessions := make([]SessionInfo, 0, SessionSlots)
	for index := 0; index < SessionSlots; index++ {
		base := index * SessionEntrySize
		if base+SessionEntrySize > len(payload) {
			break
		}
		entry := payload[base : base+SessionEntrySize]
		id := binary.LittleEndian.Uint16(entry[0:2])
		if id == 0 {
			continue
		}
		sessions = append(sessions, SessionInfo{
			ID:      id,
			LeaseMS: binary.LittleEndian.Uint16(entry[2:4]),
			Held:    entry[4],
			Flags:   entry[5],
		})
	}
	return sessions
}

// CheckResponse validates one control reply against what was asked for,
// including the fixed payload sizes and the result byte.
func CheckResponse(packet Packet, session, sequence uint32, opcode uint8) error {
	if packet.Session != session || packet.Sequence != sequence || packet.Opcode != opcode|ReplyBit {
		return errorf(Framing, "reply session/sequence/opcode does not match")
	}
	var expected int
	switch opcode {
	case OpStatus:
		expected = StatusBytes
	case OpStatusExt:
		expected = StatusExtBytes
	case OpSessionList:
		expected = SessionListBytes
	default:
		expected = 1
	}
	if len(packet.Payload) != expected {
		return errorf(Framing, "control reply is %d bytes, expected %d", len(packet.Payload), expected)
	}
	if opcode == OpSessionList {
		return nil
	}
	if len(packet.Payload) == 0 {
		return errorf(Framing, "empty control reply")
	}
	if code := packet.Payload[0]; code != 0 {
		return FromResult(code)
	}
	return nil
}
