package gdbnative

import "time"

// KeyStep is one entry in a key sequence.
type KeyStep struct {
	Page   uint16
	Usage  uint16
	HoldMs uint32
	GapMs  uint32
}

// Stroke is one character on a US layout: a usage, plus whether it needs shift.
type Stroke struct {
	Page  uint16
	Usage uint16
	Shift bool
}

// LeftShift is the modifier a shifted character holds.
const leftShiftUsage = 0xE1

// usLayout maps one character onto its keyboard usage, if the firmware knows it.
// Ported from sdk/rust/src/keyboard.rs.
func usLayout(character rune) (uint16, bool, bool) {
	switch {
	case character >= 'a' && character <= 'z':
		return 0x04 + uint16(character-'a'), false, true
	case character >= 'A' && character <= 'Z':
		return 0x04 + uint16(character-'A'), true, true
	case character >= '1' && character <= '9':
		return 0x1E + uint16(character-'1'), false, true
	}
	shifted := map[rune]uint16{
		'!': 0x1E, '@': 0x1F, '#': 0x20, '$': 0x21, '%': 0x22, '^': 0x23,
		'&': 0x24, '*': 0x25, '(': 0x26, ')': 0x27, '_': 0x2D, '+': 0x2E,
		'{': 0x2F, '}': 0x30, '|': 0x31, ':': 0x33, '"': 0x34, '~': 0x35,
		'<': 0x36, '>': 0x37, '?': 0x38,
	}
	plain := map[rune]uint16{
		'0': 0x27, '-': 0x2D, '=': 0x2E, '[': 0x2F, ']': 0x30, '\\': 0x31,
		';': 0x33, '\'': 0x34, '`': 0x35, ',': 0x36, '.': 0x37, '/': 0x38,
		'\n': 0x28, '\t': 0x2B, ' ': 0x2C,
	}
	if usage, ok := shifted[character]; ok {
		return usage, true, true
	}
	if usage, ok := plain[character]; ok {
		return usage, false, true
	}
	return 0, false, false
}

// Strokes converts text into keystrokes on a US layout. Characters the
// firmware has no usage for are dropped, so the stroke count is not the
// character count.
func Strokes(text string) []Stroke {
	out := make([]Stroke, 0, len(text))
	for _, character := range text {
		if character == '\r' {
			continue
		}
		usage, shift, ok := usLayout(character)
		if !ok {
			continue
		}
		out = append(out, Stroke{Page: 7, Usage: usage, Shift: shift})
	}
	return out
}

// Keyboard sends keys and text.
type Keyboard struct {
	device *Device
}

// KeyDown presses one usage.
func (k *Keyboard) KeyDown(page, usage uint16) error { return k.emit(page, usage, true) }

// KeyUp releases one usage.
func (k *Keyboard) KeyUp(page, usage uint16) error { return k.emit(page, usage, false) }

// Press taps a key: 18 ms down, then up.
func (k *Keyboard) Press(page, usage uint16) error { return k.LongPress(page, usage, 18) }

// LongPress holds a key for holdMs (minimum 1 ms).
func (k *Keyboard) LongPress(page, usage uint16, holdMs uint32) error {
	if err := k.KeyDown(page, usage); err != nil {
		return err
	}
	time.Sleep(time.Duration(maxUint32(holdMs, 1)) * time.Millisecond)
	return k.KeyUp(page, usage)
}

// Chord presses every usage, holds, then releases in reverse order.
func (k *Keyboard) Chord(usages []uint16, holdMs uint32) error {
	if len(usages) == 0 {
		return errorf(Invalid, "a chord needs at least one usage")
	}
	for _, usage := range usages {
		if err := k.KeyDown(7, usage); err != nil {
			return err
		}
	}
	time.Sleep(time.Duration(maxUint32(holdMs, 1)) * time.Millisecond)
	for index := len(usages) - 1; index >= 0; index-- {
		if err := k.KeyUp(7, usages[index]); err != nil {
			return err
		}
	}
	return nil
}

// Sequence runs typed keystrokes in order.
func (k *Keyboard) Sequence(steps []KeyStep) error {
	for _, step := range steps {
		if err := k.LongPress(step.Page, step.Usage, step.HoldMs); err != nil {
			return err
		}
		if step.GapMs > 0 {
			time.Sleep(time.Duration(step.GapMs) * time.Millisecond)
		}
	}
	return nil
}

// TypeText types text on a US layout. jitterMs adds a random extra delay to
// every interval; 0 keeps a metronome.
func (k *Keyboard) TypeText(text string, intervalMs, holdMs, jitterMs uint32) error {
	rng := NewEntropySplitMix64()
	for _, stroke := range Strokes(text) {
		if stroke.Shift {
			if err := k.KeyDown(7, leftShiftUsage); err != nil {
				return err
			}
		}
		if err := k.KeyDown(stroke.Page, stroke.Usage); err != nil {
			return err
		}
		time.Sleep(time.Duration(maxUint32(holdMs, 1)) * time.Millisecond)
		if err := k.KeyUp(stroke.Page, stroke.Usage); err != nil {
			return err
		}
		if stroke.Shift {
			if err := k.KeyUp(7, leftShiftUsage); err != nil {
				return err
			}
		}
		jitter := uint32(0)
		if jitterMs > 0 {
			jitter = uint32(rng.Unit() * float64(jitterMs))
		}
		time.Sleep(time.Duration(intervalMs+jitter) * time.Millisecond)
	}
	return nil
}

// ReleaseAll releases every key and button.
func (k *Keyboard) ReleaseAll() error { return k.device.ReleaseAll() }

func (k *Keyboard) emit(page, usage uint16, down bool) error {
	if !ValidKey(page, usage) {
		return errorf(Invalid, "usage %#06x:%#06x is not in the key catalog", page, usage)
	}
	payload := []byte{byte(page), byte(page >> 8), byte(usage), byte(usage >> 8), 0}
	if down {
		payload[4] = 1
	}
	return k.device.send(OpKey, payload)
}

func maxUint32(value, floor uint32) uint32 {
	if value < floor {
		return floor
	}
	return value
}
