package gdbnative

import "testing"

func TestStrokesOnAUSLayout(t *testing.T) {
	cases := []struct {
		text     string
		expected []Stroke
	}{
		{"a", []Stroke{{7, 0x04, false}}},
		{"A", []Stroke{{7, 0x04, true}}},
		{"!", []Stroke{{7, 0x1E, true}}},
		{"0", []Stroke{{7, 0x27, false}}},
		{"z", []Stroke{{7, 0x1D, false}}},
		{"\n", []Stroke{{7, 0x28, false}}},
		{" ", []Stroke{{7, 0x2C, false}}},
		{"\t", []Stroke{{7, 0x2B, false}}},
		{"?", []Stroke{{7, 0x38, true}}},
		// \r is dropped, so CRLF is one line break.
		{"a\r\nb", []Stroke{{7, 0x04, false}, {7, 0x28, false}, {7, 0x05, false}}},
		// Characters the firmware has no usage for are dropped.
		{"中", nil},
		{"", nil},
	}
	for _, testCase := range cases {
		got := Strokes(testCase.text)
		if len(got) != len(testCase.expected) {
			t.Fatalf("Strokes(%q) = %v, want %v", testCase.text, got, testCase.expected)
		}
		for index := range got {
			if got[index] != testCase.expected[index] {
				t.Fatalf("Strokes(%q)[%d] = %+v, want %+v", testCase.text, index, got[index], testCase.expected[index])
			}
		}
	}
}

func TestEveryStrokeUsesACatalogUsage(t *testing.T) {
	for _, character := range "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-=[]\\;'`,./!@#$%^&*()_+{}|:\"~<>? \n\t" {
		for _, stroke := range Strokes(string(character)) {
			if !ValidKey(stroke.Page, stroke.Usage) {
				t.Fatalf("%q maps to usage %#04x, which is not in the catalog", character, stroke.Usage)
			}
		}
	}
}

func TestKeyDownAndKeyUpReachTheDevice(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	keyboard := device.Keyboard()

	if err := keyboard.KeyDown(7, 0x04); err != nil {
		t.Fatalf("key down: %v", err)
	}
	if handle.Held().Keys[0]&0x10 == 0 {
		t.Fatal("the device did not see usage 4 go down")
	}
	if err := keyboard.KeyUp(7, 0x04); err != nil {
		t.Fatalf("key up: %v", err)
	}
	if handle.Held().Keys[0]&0x10 != 0 {
		t.Fatal("the device did not see usage 4 go up")
	}

	writes := handle.Writes()
	var keys []Packet
	for _, packet := range writes {
		if packet.Opcode == OpKey {
			keys = append(keys, packet)
		}
	}
	if len(keys) != 2 {
		t.Fatalf("key frames = %d, want 2", len(keys))
	}
	if want := []byte{7, 0, 0x04, 0, 1}; string(keys[0].Payload) != string(want) {
		t.Errorf("down payload = %v, want %v", keys[0].Payload, want)
	}
	if want := []byte{7, 0, 0x04, 0, 0}; string(keys[1].Payload) != string(want) {
		t.Errorf("up payload = %v, want %v", keys[1].Payload, want)
	}
}

func TestInvalidUsageIsATypedError(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	before := handle.Count()
	if code := CodeOf(device.Keyboard().KeyDown(7, 0x01)); code != Invalid {
		t.Fatalf("code = %v, want INVALID", code)
	}
	if handle.Count() != before {
		t.Fatal("an invalid usage still reached the device")
	}
}

func TestChordPressesAndReleasesInReverse(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if err := device.Keyboard().Chord([]uint16{0xE0, 0x06}, 5); err != nil {
		t.Fatalf("chord: %v", err)
	}
	var downs, ups []uint16
	for _, packet := range handle.Writes() {
		if packet.Opcode != OpKey || len(packet.Payload) < 5 {
			continue
		}
		usage := uint16(packet.Payload[2]) | uint16(packet.Payload[3])<<8
		if packet.Payload[4] == 1 {
			downs = append(downs, usage)
		} else {
			ups = append(ups, usage)
		}
	}
	if len(downs) != 2 || len(ups) != 2 {
		t.Fatalf("downs = %v, ups = %v", downs, ups)
	}
	if downs[0] != 0xE0 || downs[1] != 0x06 {
		t.Errorf("downs = %v, want [0xE0 0x06]", downs)
	}
	if ups[0] != 0x06 || ups[1] != 0xE0 {
		t.Errorf("ups = %v, want [0x06 0xE0]", ups)
	}
	if held := handle.Held(); held.Modifiers != 0 || held.Keys[0] != 0 {
		t.Fatalf("chord left something held: %+v", held)
	}
	if code := CodeOf(device.Keyboard().Chord(nil, 5)); code != Invalid {
		t.Fatalf("empty chord code = %v, want INVALID", code)
	}
}

func TestTypeTextLeavesNothingHeld(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if err := device.Keyboard().TypeText("hi", 1, 1, 0); err != nil {
		t.Fatalf("type text: %v", err)
	}
	if held := handle.Held(); held.Modifiers != 0 || held.Keys[0] != 0 || held.Consumer != 0 {
		t.Fatalf("text left something held: %+v", held)
	}
	writes := handle.Writes()
	var usages []uint16
	for _, packet := range writes {
		if packet.Opcode == OpKey && packet.Payload[4] == 1 {
			usages = append(usages, uint16(packet.Payload[2])|uint16(packet.Payload[3])<<8)
		}
	}
	if len(usages) != 2 || usages[0] != 0x0B || usages[1] != 0x0C {
		t.Fatalf("typed usages = %v, want h=0x0b and i=0x0c", usages)
	}
}

func TestShiftedTextHoldsLeftShift(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if err := device.Keyboard().TypeText("A", 1, 1, 0); err != nil {
		t.Fatalf("type text: %v", err)
	}
	var order []uint16
	for _, packet := range handle.Writes() {
		if packet.Opcode == OpKey {
			order = append(order, uint16(packet.Payload[2])|uint16(packet.Payload[3])<<8)
		}
	}
	want := []uint16{leftShiftUsage, 0x04, 0x04, leftShiftUsage}
	if len(order) != len(want) {
		t.Fatalf("frames = %v, want %v", order, want)
	}
	for index := range want {
		if order[index] != want[index] {
			t.Fatalf("frames = %v, want %v", order, want)
		}
	}
}

func TestSequenceRunsInOrder(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	steps := []KeyStep{
		{Page: 7, Usage: 0x04, HoldMs: 1, GapMs: 1},
		{Page: 7, Usage: 0x05, HoldMs: 1, GapMs: 0},
	}
	if err := device.Keyboard().Sequence(steps); err != nil {
		t.Fatalf("sequence: %v", err)
	}
	var downs []uint16
	for _, packet := range handle.Writes() {
		if packet.Opcode == OpKey && packet.Payload[4] == 1 {
			downs = append(downs, uint16(packet.Payload[2])|uint16(packet.Payload[3])<<8)
		}
	}
	if len(downs) != 2 || downs[0] != 0x04 || downs[1] != 0x05 {
		t.Fatalf("downs = %v, want [4 5]", downs)
	}
	if err := device.Keyboard().Sequence(nil); err != nil {
		t.Fatalf("empty sequence: %v", err)
	}
}

func TestSnapshotReplacesTheWholeState(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	state := InputState{}
	for _, usage := range []uint16{0x04, 0x05, 0xE1} {
		if err := state.Set(7, usage, true); err != nil {
			t.Fatalf("set: %v", err)
		}
	}
	if err := state.Set(12, 0xE9, true); err != nil {
		t.Fatalf("set: %v", err)
	}
	if err := device.Snapshot(state); err != nil {
		t.Fatalf("snapshot: %v", err)
	}
	if held := handle.Held(); held != state {
		t.Fatalf("held = %+v, want %+v", held, state)
	}
	empty := InputState{}
	if err := device.Snapshot(empty); err != nil {
		t.Fatalf("empty snapshot: %v", err)
	}
	if held := handle.Held(); held != empty {
		t.Fatalf("held = %+v after an empty snapshot", held)
	}
}

func TestConsumerKeyUsesTheConsumerPage(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if err := device.Keyboard().KeyDown(12, 0xE9); err != nil {
		t.Fatalf("consumer down: %v", err)
	}
	if handle.Held().Consumer&0x02 == 0 {
		t.Fatal("volume up did not reach the consumer byte")
	}
	if err := device.Keyboard().ReleaseAll(); err != nil {
		t.Fatalf("release all: %v", err)
	}
	if held := handle.Held(); held.Consumer != 0 {
		t.Fatalf("release all left %+v held", held)
	}
}
