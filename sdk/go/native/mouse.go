package gdbnative

import (
	"sync"
	"time"
)

// Button mirrors gdb_button; the values are HID button numbers, not bit masks.
type Button uint8

const (
	Left    Button = 1
	Right   Button = 2
	Middle  Button = 3
	Back    Button = 4
	Forward Button = 5
)

// bit is the snapshot mask bit for this button: left=bit0 … forward=bit4.
func (b Button) bit() uint8 { return uint8(1) << (uint8(b) - 1) }

// valid reports whether the value is one of the five HID buttons.
func (b Button) valid() bool { return b >= Left && b <= Forward }

// Mouse controls buttons, wheel and movement.
type Mouse struct {
	device *Device
}

// Button presses or releases one button and tells the device the new mask.
func (m *Mouse) Button(button Button, down bool) error {
	if !button.valid() {
		return errorf(Invalid, "button must be 1..=5")
	}
	// Each session owns its own mask; the firmware unions the owners.
	current := m.device.buttonsMask()
	next := current &^ button.bit()
	if down {
		next = current | button.bit()
	}
	if err := m.device.setButtonsMask(next); err != nil {
		return err
	}
	return m.device.send(OpMouseButton, []byte{next})
}

// Click clicks count times: down, 25 ms, up, 35 ms.
func (m *Mouse) Click(button Button, count uint8) error {
	if count == 0 {
		return errorf(Invalid, "click count must be at least 1")
	}
	for index := uint8(0); index < count; index++ {
		if err := m.Button(button, true); err != nil {
			return err
		}
		time.Sleep(25 * time.Millisecond)
		if err := m.Button(button, false); err != nil {
			return err
		}
		time.Sleep(35 * time.Millisecond)
	}
	return nil
}

// DoubleClick clicks twice.
func (m *Mouse) DoubleClick(button Button) error { return m.Click(button, 2) }

// Wheel scrolls; the range per axis is ±1024.
func (m *Mouse) Wheel(vertical, horizontal int32) error {
	if vertical < -1024 || vertical > 1024 || horizontal < -1024 || horizontal > 1024 {
		return errorf(Invalid, "wheel is limited to +/-1024 per axis")
	}
	return m.device.send(OpWheel, []byte{
		byte(vertical), byte(vertical >> 8),
		byte(horizontal), byte(horizontal >> 8),
	})
}

// MoveRaw sends the deltas as they are, split only at the ±1024 HID limit, with
// no interpolation and no barrier. The hot path for remote control: the caller
// flushes with Barrier only at the points that need a completion guarantee.
func (m *Mouse) MoveRaw(dx, dy int32) error {
	for _, chunk := range SplitDelta(dx, dy) {
		if err := m.device.sendMouseMove(int16(chunk[0]), int16(chunk[1])); err != nil {
			return err
		}
	}
	return nil
}

// MoveBy moves relatively, splitting the move and waiting for each chunk to
// reach USB. durationMs is the pause budget for the whole move.
func (m *Mouse) MoveBy(dx, dy int32, durationMs uint32, profile *Profile) error {
	_ = profile // the straight move has no curve to apply
	chunks := SplitDelta(dx, dy)
	if len(chunks) == 0 {
		return nil
	}
	pause := time.Duration(0)
	if durationMs > 0 {
		pause = time.Duration(durationMs/uint32(len(chunks))) * time.Millisecond
	}
	for index, chunk := range chunks {
		if err := m.device.sendMouseMove(int16(chunk[0]), int16(chunk[1])); err != nil {
			return err
		}
		if err := m.device.Barrier(); err != nil {
			return err
		}
		if index+1 != len(chunks) && pause > 0 {
			time.Sleep(pause)
		}
	}
	return nil
}

// MoveTo moves the real pointer to absolute primary-monitor coordinates.
func (m *Mouse) MoveTo(x, y int32, durationMs uint32, profile *Profile) error {
	shared := &movementShared{}
	return m.runMoveTo(x, y, durationMs, profileOr(profile), shared)
}

// MovePath visits each point in order; every segment uses the same profile.
func (m *Mouse) MovePath(points [][2]int32, durationMs uint32, profile *Profile) error {
	if len(points) == 0 {
		return errorf(Invalid, "a path needs at least one point")
	}
	chosen := profileOr(profile)
	shared := &movementShared{}
	for _, point := range points {
		if err := m.runMoveTo(point[0], point[1], durationMs, chosen, shared); err != nil {
			return err
		}
		if shared.cancelled() {
			return codeError(Cancelled)
		}
	}
	return nil
}

// Drag presses, moves and releases.
func (m *Mouse) Drag(button Button, x, y int32, durationMs uint32, profile *Profile) error {
	if err := m.Button(button, true); err != nil {
		return err
	}
	result := m.MoveTo(x, y, durationMs, profile)
	// Release even when the move failed, the way the C ABI's guard does.
	if err := m.ReleaseAll(); err != nil && result == nil {
		result = err
	}
	return result
}

// ReleaseAll releases every key and button.
func (m *Mouse) ReleaseAll() error { return m.device.ReleaseAll() }

// Movement is a move running on its own goroutine.
type Movement struct {
	shared *movementShared
	done   chan struct{}
	mu     sync.Mutex
	err    error
}

// movementShared is the cancel/retarget channel between caller and worker.
type movementShared struct {
	mu       sync.Mutex
	cancel   bool
	retarget *[2]int32
}

func (s *movementShared) stop() {
	s.mu.Lock()
	s.cancel = true
	s.mu.Unlock()
}

func (s *movementShared) cancelled() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cancel
}

func (s *movementShared) setTarget(x, y int32) {
	s.mu.Lock()
	s.retarget = &[2]int32{x, y}
	s.mu.Unlock()
}

func (s *movementShared) takeTarget() *[2]int32 {
	s.mu.Lock()
	defer s.mu.Unlock()
	target := s.retarget
	s.retarget = nil
	return target
}

// StartMoveTo starts an absolute move on a goroutine so the caller can retarget
// or cancel it while it runs.
func (m *Mouse) StartMoveTo(x, y int32, durationMs uint32, profile *Profile) *Movement {
	chosen := profileOr(profile)
	return m.spawn(func(shared *movementShared) error {
		return m.runMoveTo(x, y, durationMs, chosen, shared)
	})
}

// StartMoveBy starts a relative move on a goroutine.
func (m *Mouse) StartMoveBy(dx, dy int32, durationMs uint32, profile *Profile) *Movement {
	// The straight move has no curve to apply; the profile only exists so the
	// signature matches the absolute move.
	_ = profileOr(profile)
	return m.spawn(func(shared *movementShared) error {
		return m.moveByShared(dx, dy, durationMs, shared)
	})
}

// StartMovePath visits each point on a goroutine.
func (m *Mouse) StartMovePath(points [][2]int32, durationMs uint32, profile *Profile) *Movement {
	chosen := profileOr(profile)
	return m.spawn(func(shared *movementShared) error {
		for _, point := range points {
			if shared.cancelled() {
				return codeError(Cancelled)
			}
			if err := m.runMoveTo(point[0], point[1], durationMs, chosen, shared); err != nil {
				return err
			}
		}
		return nil
	})
}

func (m *Mouse) spawn(run func(*movementShared) error) *Movement {
	movement := &Movement{shared: &movementShared{}, done: make(chan struct{})}
	go func() {
		defer close(movement.done)
		err := run(movement.shared)
		movement.mu.Lock()
		movement.err = err
		movement.mu.Unlock()
	}()
	return movement
}

// Cancel stops the worker at its next step.
func (m *Movement) Cancel() { m.shared.stop() }

// Retarget changes the destination mid-flight.
func (m *Movement) Retarget(x, y int32) { m.shared.setTarget(x, y) }

// Finished reports whether the worker stopped, successfully or not.
func (m *Movement) Finished() bool {
	select {
	case <-m.done:
		return true
	default:
		return false
	}
}

// Wait blocks up to timeoutMs and returns the worker's error: Cancelled when it
// was cancelled, Timeout when it is still running.
func (m *Movement) Wait(timeoutMs uint32) error {
	timer := time.NewTimer(time.Duration(timeoutMs) * time.Millisecond)
	defer timer.Stop()
	select {
	case <-m.done:
		m.mu.Lock()
		defer m.mu.Unlock()
		return m.err
	case <-timer.C:
		return errorf(Timeout, "movement did not finish in time")
	}
}

// Close cancels a running move and waits for the worker to unwind.
func (m *Movement) Close() {
	m.shared.stop()
	<-m.done
}

// moveByShared is MoveBy with cancellation, used by StartMoveBy.
func (m *Mouse) moveByShared(dx, dy int32, durationMs uint32, shared *movementShared) error {
	chunks := SplitDelta(dx, dy)
	if len(chunks) == 0 {
		return nil
	}
	pause := time.Duration(0)
	if durationMs > 0 {
		pause = time.Duration(durationMs/uint32(len(chunks))) * time.Millisecond
	}
	for index, chunk := range chunks {
		if shared.cancelled() {
			return codeError(Cancelled)
		}
		if err := m.device.sendMouseMove(int16(chunk[0]), int16(chunk[1])); err != nil {
			return err
		}
		if err := m.device.Barrier(); err != nil {
			return err
		}
		if index+1 != len(chunks) && pause > 0 {
			time.Sleep(pause)
		}
	}
	return nil
}

// runMoveTo is the absolute move: read the pointer, plan a path, then close the
// loop with relative reports until the host pointer reaches the node.
func (m *Mouse) runMoveTo(x, y int32, durationMs uint32, profile Profile, shared *movementShared) error {
	screen, position, err := m.device.cursorPosition()
	if err != nil {
		return err
	}
	if position[0] < 0 || position[1] < 0 || position[0] >= screen.Width || position[1] >= screen.Height {
		return errorf(Unsupported,
			"absolute coordinates are relative to the primary monitor (%dx%d at 0,0) and the cursor is at (%d, %d), so the move would start off that screen",
			screen.Width, screen.Height, position[0], position[1])
	}
	target := clampTarget(x, y, screen)
	rng := NewEntropySplitMix64()
	nodes, err := TrajectoryWith(position, target, screen, durationMs, profile, rng)
	if err != nil {
		return err
	}
	started := time.Now()
	index := 0
	tracker := NewRelativeTracker()
	deadline := started.Add(nodeSpan(nodes) + 500*time.Millisecond)
	for {
		if shared.cancelled() {
			return codeError(Cancelled)
		}
		if next := shared.takeTarget(); next != nil {
			target = clampTarget(next[0], next[1], screen)
			currentScreen, current, err := m.device.cursorPosition()
			if err != nil {
				return err
			}
			if currentScreen.Width != screen.Width || currentScreen.Height != screen.Height {
				return errorf(Unsupported, "monitor layout changed during the movement (%dx%d -> %dx%d)",
					screen.Width, screen.Height, currentScreen.Width, currentScreen.Height)
			}
			nodes, err = TrajectoryWith(current, target, screen, durationMs, profile, rng)
			if err != nil {
				return err
			}
			started = time.Now()
			index = 0
			tracker = NewRelativeTracker()
			deadline = started.Add(nodeSpan(nodes) + 500*time.Millisecond)
		}
		if index >= len(nodes) {
			return nil
		}
		node := nodes[index]
		elapsed := time.Since(started)
		nodeAt := time.Duration(node.AtMs) * time.Millisecond
		if elapsed < nodeAt {
			wait := nodeAt - elapsed
			if wait > 2*time.Millisecond {
				wait = 2 * time.Millisecond
			}
			time.Sleep(wait)
			continue
		}
		_, current, err := m.device.cursorPosition()
		if err != nil {
			return err
		}
		switch step := tracker.Step(current, [2]int32{node.X, node.Y}, time.Now()); step.Kind {
		case StepWait:
			time.Sleep(time.Millisecond)
		case StepMove:
			if err := m.device.sendMouseMove(int16(step.DX), int16(step.DY)); err != nil {
				return err
			}
			if err := m.device.Barrier(); err != nil {
				return err
			}
		case StepArrived:
			if index+1 == len(nodes) {
				return m.device.Barrier()
			}
			index++
		}
		if time.Now().After(deadline) {
			return errorf(Timeout, "pointer did not reach the target")
		}
	}
}

func nodeSpan(nodes []Node) time.Duration {
	if len(nodes) == 0 {
		return 0
	}
	return time.Duration(nodes[len(nodes)-1].AtMs) * time.Millisecond
}

func clampTarget(x, y int32, screen Screen) [2]int32 {
	return [2]int32{
		clampInt32(x, 0, screen.Width-1),
		clampInt32(y, 0, screen.Height-1),
	}
}
