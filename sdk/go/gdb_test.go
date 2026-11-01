//go:build windows

package gdb

import (
	"encoding/json"
	"errors"
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
