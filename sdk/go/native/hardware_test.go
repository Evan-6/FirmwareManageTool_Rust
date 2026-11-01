//go:build windows

package gdbnative

import (
	"os"
	"testing"
	"time"
)

// TestLiveDevice talks to a real board. It is opt-in, because opening the
// vendor collection takes the owner slot away from anything else that reads it
// (only one reader per collection, firmware/protocol-v3.md).
//
//	GDBOX_NATIVE_HARDWARE=1 go test ./native -run TestLiveDevice -v
func TestLiveDevice(t *testing.T) {
	if os.Getenv("GDBOX_NATIVE_HARDWARE") != "1" {
		t.Skip("set GDBOX_NATIVE_HARDWARE=1 to talk to the board")
	}
	entries, err := List()
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(entries) == 0 {
		t.Skip("no vendor collection attached")
	}
	for _, entry := range entries {
		t.Logf("device path=%s serial=%q product=%q manufacturer=%q", entry.Path, entry.Serial, entry.Product, entry.Manufacturer)
	}
	opts := DefaultOptions()
	opts.Heartbeat = false
	device, err := Open("", opts)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	defer device.Close()
	t.Logf("info: %+v", device.Info())
	if device.State() != StateConnected {
		t.Fatalf("state = %v", device.State())
	}
	status, err := device.Status()
	if err != nil {
		t.Fatalf("status: %v", err)
	}
	t.Logf("status: lease=%d ms pending=%d completed=%d", status.LeaseMS, status.Pending, status.Completed)
	ext, err := device.StatusExt()
	if err != nil {
		t.Fatalf("status ext: %v", err)
	}
	t.Logf("status ext: uptime=%d ms sessions=%d build=%#x", ext.UptimeMS, ext.Sessions, ext.Build)
	sessions, err := device.Sessions()
	if err != nil {
		t.Fatalf("sessions: %v", err)
	}
	t.Logf("owners: %+v", sessions)
	if err := device.Barrier(); err != nil {
		t.Fatalf("barrier: %v", err)
	}
	time.Sleep(20 * time.Millisecond)
	if err := device.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
}

// TestLiveHeartbeatKeepsTheLease idles past the 2000 ms lease with only the
// heartbeat goroutine keeping the session alive.
func TestLiveHeartbeatKeepsTheLease(t *testing.T) {
	if os.Getenv("GDBOX_NATIVE_HARDWARE") != "1" {
		t.Skip("set GDBOX_NATIVE_HARDWARE=1 to talk to the board")
	}
	opts := DefaultOptions()
	opts.TimeoutMS = 2000
	device, err := Open("", opts)
	if err != nil {
		t.Skipf("no device: %v", err)
	}
	defer device.Close()
	if device.Info().LeaseMS == 0 {
		t.Fatal("no lease reported")
	}
	time.Sleep(3 * time.Second)
	status, err := device.Status()
	if err != nil {
		t.Fatalf("status after idling past the lease: %v", err)
	}
	if status.LeaseMS < 500 {
		t.Fatalf("lease left = %d ms; the heartbeat did not renew it", status.LeaseMS)
	}
	if device.State() != StateConnected {
		t.Fatalf("state = %v", device.State())
	}
}
