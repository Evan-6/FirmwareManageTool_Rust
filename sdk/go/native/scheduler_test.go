package gdbnative

import (
	"encoding/json"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type recorder struct {
	mu    sync.Mutex
	names []string
}

func (r *recorder) add(name string) {
	r.mu.Lock()
	r.names = append(r.names, name)
	r.mu.Unlock()
}

func (r *recorder) list() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]string(nil), r.names...)
}

// alignToTick parks the scheduler clock at the start of a millisecond bucket.
// Zero-delay jobs are due "now", and one submitted a whole millisecond later is
// genuinely due later; steering clear of the boundary keeps the priority
// tie-break the only difference between the three jobs below.
func alignToTick(t *testing.T, scheduler *Scheduler) {
	t.Helper()
	deadline := time.Now().Add(50 * time.Millisecond)
	for time.Now().Before(deadline) {
		if time.Since(scheduler.started)%time.Millisecond < 150*time.Microsecond {
			return
		}
		time.Sleep(20 * time.Microsecond)
	}
	t.Fatal("could not align to a scheduler tick")
}

func TestSchedulerOrdersByDueThenPriorityThenSubmission(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	scheduler.Pause()
	alignToTick(t, scheduler)
	order := &recorder{}
	if _, err := scheduler.Submit(0, 0, func(*Device) error { order.add("low"); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if _, err := scheduler.Submit(0, 10, func(*Device) error { order.add("high"); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if _, err := scheduler.Submit(0, 0, func(*Device) error { order.add("second"); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	scheduler.Resume()
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	want := []string{"high", "low", "second"}
	got := order.list()
	if len(got) != len(want) {
		t.Fatalf("order = %v, want %v", got, want)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("order = %v, want %v", got, want)
		}
	}
}

func TestSchedulerRunsLaterJobsInDueOrder(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	order := &recorder{}
	if _, err := scheduler.Submit(60*time.Millisecond, 100, func(*Device) error { order.add("late"); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if _, err := scheduler.Submit(0, 0, func(*Device) error { order.add("early"); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	got := order.list()
	if len(got) != 2 || got[0] != "early" || got[1] != "late" {
		t.Fatalf("order = %v, want [early late]", got)
	}
}

func TestSchedulerPauseBlocksAndResumeContinues(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	var ran atomic.Bool
	scheduler.Pause()
	if _, err := scheduler.Submit(0, 0, func(*Device) error { ran.Store(true); return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	time.Sleep(80 * time.Millisecond)
	if ran.Load() {
		t.Fatal("a paused scheduler ran the job")
	}
	scheduler.Resume()
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	if !ran.Load() {
		t.Fatal("resume did not run the job")
	}
}

func TestSchedulerCancelWins(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	var ran atomic.Bool
	scheduler.Pause()
	id, err := scheduler.Submit(0, 0, func(*Device) error { ran.Store(true); return nil })
	if err != nil {
		t.Fatalf("submit: %v", err)
	}
	scheduler.Cancel(id)
	scheduler.Resume()
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	if ran.Load() {
		t.Fatal("a cancelled job ran")
	}
}

func TestSchedulerEveryRepeatsUntilCancelled(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	var count atomic.Int64
	id, err := scheduler.Every(10*time.Millisecond, 0, func(*Device) error {
		count.Add(1)
		return nil
	})
	if err != nil {
		t.Fatalf("every: %v", err)
	}
	time.Sleep(100 * time.Millisecond)
	scheduler.Cancel(id)
	if got := count.Load(); got < 2 {
		t.Fatalf("ran %d times, want at least 2", got)
	}
	stopped := count.Load()
	time.Sleep(60 * time.Millisecond)
	if count.Load() != stopped {
		t.Fatalf("repeating job kept running after cancel: %d -> %d", stopped, count.Load())
	}
	if _, err := scheduler.Every(0, 0, func(*Device) error { return nil }); CodeOf(err) != Invalid {
		t.Fatalf("zero interval code = %v, want INVALID", CodeOf(err))
	}
}

func TestSchedulerRecordsFailuresAsJson(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	if _, err := scheduler.Submit(0, 0, func(*Device) error { return codeError(Invalid) }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	failures := scheduler.TakeErrors()
	if len(failures) != 1 {
		t.Fatalf("errors = %+v, want 1", failures)
	}
	if failures[0].Code != Invalid {
		t.Errorf("code = %v, want INVALID", failures[0].Code)
	}
	if len(scheduler.TakeErrors()) != 0 {
		t.Error("take_errors did not clear the list")
	}

	if _, err := scheduler.Submit(0, 0, func(*Device) error { return codeError(Timeout) }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	raw, err := scheduler.TakeErrorsJSON()
	if err != nil {
		t.Fatalf("errors json: %v", err)
	}
	var decoded []map[string]any
	if err := json.Unmarshal([]byte(raw), &decoded); err != nil {
		t.Fatalf("errors json: %v", err)
	}
	if len(decoded) != 1 || decoded[0]["code"] != float64(Timeout) {
		t.Fatalf("errors json = %s", raw)
	}
	for _, key := range []string{"id", "code", "message"} {
		if _, ok := decoded[0][key]; !ok {
			t.Errorf("error JSON is missing %q: %s", key, raw)
		}
	}

	if _, err := scheduler.Submit(0, 0, nil); CodeOf(err) != Invalid {
		t.Fatalf("nil callback code = %v, want INVALID", CodeOf(err))
	}
}

func TestSchedulerRunsTasksAgainstTheDevice(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()

	var pressed atomic.Int32
	if _, err := scheduler.Submit(0, 5, func(inner *Device) error {
		pressed.Add(1)
		return inner.Keyboard().KeyDown(7, 0x04)
	}); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle: %v", err)
	}
	if pressed.Load() != 1 {
		t.Fatalf("task ran %d times", pressed.Load())
	}
	if handle.Held().Keys[0]&0x10 == 0 {
		t.Fatal("the scheduled task did not reach the device")
	}
	scheduler.Close()
	if _, err := scheduler.Submit(0, 0, func(*Device) error { return nil }); CodeOf(err) != IO {
		t.Fatalf("submit after close code = %v, want IO", CodeOf(err))
	}
}

func TestSchedulerWaitIdleTimesOut(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	scheduler := device.Scheduler()
	defer scheduler.Close()

	scheduler.Pause()
	if _, err := scheduler.Submit(0, 0, func(*Device) error { return nil }); err != nil {
		t.Fatalf("submit: %v", err)
	}
	if code := CodeOf(scheduler.WaitIdle(50 * time.Millisecond)); code != Timeout {
		t.Fatalf("code = %v, want TIMEOUT", code)
	}
	scheduler.Resume()
	if err := scheduler.WaitIdle(2 * time.Second); err != nil {
		t.Fatalf("wait idle after resume: %v", err)
	}
}
