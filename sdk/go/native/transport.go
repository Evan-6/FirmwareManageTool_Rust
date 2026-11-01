package gdbnative

import "time"

// Report is one full 64-byte HID report, report id in byte 0.
type Report = [ReportSize]byte

// Transport is the whole platform surface the SDK needs: the Feature 14 read,
// one output report, one input report. Everything else — framing, sequencing,
// reply matching — lives above it, so a mock is a first-class transport.
type Transport interface {
	// Feature performs GET_FEATURE(14) and returns the 64-byte answer.
	Feature() (Report, error)
	// Write sends one output report (id 12 in byte 0). A short write is an error.
	Write(report Report) error
	// Read returns one input report, or ok=false when the timeout expired.
	Read(timeout time.Duration) (report Report, ok bool, err error)
	// Close releases the underlying handle. It is called once by Device.Close.
	Close() error
}

// Vendor HID identity: the firmware answers only this pair.
const (
	VendorID  = 0x03F0
	ProductID = 0x0024
	UsagePage = 0xFF60
	Usage     = 0x61
)
