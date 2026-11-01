package gdbnative

import (
	"errors"
	"fmt"
)

// Code mirrors the C gdb_error enum (sdk/ffi/include/gdb.h). The numbers are
// part of the ABI of every binding, so they never get reordered.
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
	if int(c) >= 0 && int(c) < len(names) {
		return names[c]
	}
	return fmt.Sprintf("CODE(%d)", int32(c))
}

// message is the SDK-level wording for a code, matching sdk/rust/src/error.rs.
func (c Code) message() string {
	switch c {
	case OK:
		return "ok"
	case Invalid:
		return "invalid argument"
	case Timeout:
		return "device did not answer in time"
	case Busy:
		return "device busy: same session id or all eight owners taken"
	case Lease:
		return "session lease expired"
	case Framing:
		return "device rejected the frame"
	case Overflow:
		return "device queue overflow or stale output"
	case Unsupported:
		return "not supported on this platform or firmware"
	case Cancelled:
		return "cancelled"
	default:
		return "HID transfer failed"
	}
}

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

func errorf(code Code, format string, args ...any) *Error {
	return &Error{Code: code, Message: fmt.Sprintf(format, args...)}
}

func codeError(code Code) *Error { return &Error{Code: code, Message: code.message()} }

// CodeOf reports the SDK code of an error: OK for nil, IO for a foreign error.
func CodeOf(err error) Code {
	if err == nil {
		return OK
	}
	var failure *Error
	if errors.As(err, &failure) {
		return failure.Code
	}
	return IO
}

// messageOf is the SDK message of an error without the code prefix, which is
// what the log lines and the event callbacks report.
func messageOf(err error) string {
	if err == nil {
		return ""
	}
	var failure *Error
	if errors.As(err, &failure) {
		return failure.Message
	}
	return err.Error()
}

// FromResult maps a protocol result byte (firmware/protocol-v3.md, 命令) onto
// the SDK code space. Codes 6 and 7 are the two ways the device tells the host
// the USB link is gone, so both become Disconnected.
func FromResult(result uint8) *Error {
	var code Code
	var message string
	switch result {
	case 0:
		return nil
	case 1:
		code, message = Framing, "device reported a framing error"
	case 2:
		code, message = Busy, Busy.message()
	case 3:
		code, message = Invalid, "device rejected the input"
	case 5:
		code, message = Lease, Lease.message()
	case 6:
		code, message = Disconnected, "device failed to send the HID report"
	case 7:
		code, message = Disconnected, Disconnected.message()
	case 8:
		code, message = Overflow, Overflow.message()
	case 9:
		code, message = Framing, "duplicate sequence"
	case 10:
		code, message = Framing, "sequence gap"
	default:
		code, message = IO, fmt.Sprintf("device error code %d", result)
	}
	return &Error{Code: code, Message: message}
}

// recoverable reports whether reopening the transport can help.
func recoverable(code Code) bool {
	return code == Disconnected || code == IO || code == Lease
}
