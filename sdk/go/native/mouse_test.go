package gdbnative

import "testing"

func moves(packets []Packet) [][2]int32 {
	out := make([][2]int32, 0, len(packets))
	for _, packet := range packets {
		if packet.Opcode != OpMouseMove || len(packet.Payload) < 4 {
			continue
		}
		x := int32(int16(uint16(packet.Payload[0]) | uint16(packet.Payload[1])<<8))
		y := int32(int16(uint16(packet.Payload[2]) | uint16(packet.Payload[3])<<8))
		out = append(out, [2]int32{x, y})
	}
	return out
}

func countOpcode(packets []Packet, opcode uint8) int {
	total := 0
	for _, packet := range packets {
		if packet.Opcode == opcode {
			total++
		}
	}
	return total
}

func TestMoveBySplitsAndFlushesEveryChunk(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	start := handle.Count()
	linear := LinearProfile()
	if err := device.Mouse().MoveBy(1500, -1100, 40, &linear); err != nil {
		t.Fatalf("move by: %v", err)
	}
	sent := handle.Sent(start)
	got := moves(sent)
	want := [][2]int32{{1024, -1024}, {476, -76}}
	if len(got) != len(want) {
		t.Fatalf("moves = %v, want %v", got, want)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("moves = %v, want %v", got, want)
		}
	}
	if barriers := countOpcode(sent, OpBarrier); barriers != 2 {
		t.Fatalf("barriers = %d, want one per chunk", barriers)
	}
}

func TestMoveRawSendsWithoutABarrier(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	start := handle.Count()
	if err := device.Mouse().MoveRaw(1500, -1100); err != nil {
		t.Fatalf("move raw: %v", err)
	}
	sent := handle.Sent(start)
	got := moves(sent)
	want := [][2]int32{{1024, -1024}, {476, -76}}
	if len(got) != len(want) || got[0] != want[0] || got[1] != want[1] {
		t.Fatalf("moves = %v, want %v", got, want)
	}
	if barriers := countOpcode(sent, OpBarrier); barriers != 0 {
		t.Fatalf("move_raw sent %d barriers, want none", barriers)
	}
	if err := device.Mouse().MoveRaw(0, 0); err != nil {
		t.Fatalf("zero move: %v", err)
	}
}

func TestClicksSendTheButtonMask(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	start := handle.Count()
	if err := device.Mouse().Click(Left, 2); err != nil {
		t.Fatalf("click: %v", err)
	}
	var masks []byte
	for _, packet := range handle.Sent(start) {
		if packet.Opcode == OpMouseButton {
			masks = append(masks, packet.Payload[0])
		}
	}
	if len(masks) != 4 || masks[0] != 1 || masks[1] != 0 || masks[2] != 1 || masks[3] != 0 {
		t.Fatalf("masks = %v, want [1 0 1 0]", masks)
	}
	if handle.Held().Buttons != 0 {
		t.Fatalf("after two clicks buttons = %d", handle.Held().Buttons)
	}
	if code := CodeOf(device.Mouse().Click(Left, 0)); code != Invalid {
		t.Fatalf("zero clicks code = %v, want INVALID", code)
	}
	if code := CodeOf(device.Mouse().DoubleClick(Button(9))); code != Invalid {
		t.Fatalf("unknown button code = %v, want INVALID", code)
	}
}

func TestButtonHoldsAndReleaseAllClears(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	mouse := device.Mouse()
	if err := mouse.Button(Right, true); err != nil {
		t.Fatalf("button: %v", err)
	}
	if handle.Held().Buttons != 2 {
		t.Fatalf("held buttons = %d, want 2", handle.Held().Buttons)
	}
	if err := mouse.Button(Left, true); err != nil {
		t.Fatalf("button: %v", err)
	}
	if handle.Held().Buttons != 3 {
		t.Fatalf("held buttons = %d, want 3", handle.Held().Buttons)
	}
	if err := mouse.Button(Right, false); err != nil {
		t.Fatalf("button: %v", err)
	}
	if handle.Held().Buttons != 1 {
		t.Fatalf("held buttons = %d, want 1", handle.Held().Buttons)
	}
	if err := mouse.ReleaseAll(); err != nil {
		t.Fatalf("release all: %v", err)
	}
	if handle.Held().Buttons != 0 {
		t.Fatalf("held buttons = %d after release", handle.Held().Buttons)
	}
}

func TestWheelRangeAndPayload(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	if code := CodeOf(device.Mouse().Wheel(4000, 0)); code != Invalid {
		t.Fatalf("code = %v, want INVALID", code)
	}
	start := handle.Count()
	if err := device.Mouse().Wheel(120, -120); err != nil {
		t.Fatalf("wheel: %v", err)
	}
	sent := handle.Sent(start)
	if len(sent) != 1 || sent[0].Opcode != OpWheel {
		t.Fatalf("frames = %v", sent)
	}
	want := []byte{120, 0, 0x88, 0xFF}
	if string(sent[0].Payload) != string(want) {
		t.Fatalf("payload = %v, want %v", sent[0].Payload, want)
	}
}

func TestMoveToConvergesOnTheMockPointer(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	start := handle.Count()
	linear := LinearProfile()
	if err := device.Mouse().MoveTo(1100, 700, 120, &linear); err != nil {
		t.Fatalf("move to: %v", err)
	}
	cursor := device.cursorState()
	if absInt32(cursor[0]-1100) > 3 || absInt32(cursor[1]-700) > 3 {
		t.Fatalf("cursor = %v, want within 3 px of (1100, 700)", cursor)
	}
	sent := handle.Sent(start)
	if len(moves(sent)) == 0 {
		t.Fatal("no movement frames were sent")
	}
	for _, chunk := range moves(sent) {
		if absInt32(chunk[0]) > 512 || absInt32(chunk[1]) > 512 {
			t.Fatalf("chunk %v exceeds the tracker clamp", chunk)
		}
	}
	if countOpcode(sent, OpBarrier) == 0 {
		t.Fatal("move_to must wait for completion")
	}
}

func TestMoveToRefusesACursorOffThePrimaryMonitor(t *testing.T) {
	original := cursorProbe
	cursorProbe = func() (Screen, [2]int32, error) {
		return Screen{Width: 1920, Height: 1080}, [2]int32{-300, 700}, nil
	}
	defer func() { cursorProbe = original }()

	transport, _ := NewMockTransport()
	device, err := newDevice(transport, targetPath, "TEST", "test device", quietOptions())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	defer device.Close()

	failure := device.Mouse().MoveTo(100, 100, 100, nil)
	if CodeOf(failure) != Unsupported {
		t.Fatalf("code = %v, want UNSUPPORTED (err %v)", CodeOf(failure), failure)
	}
}

func TestMovePathVisitsEveryPoint(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	linear := LinearProfile()
	if err := device.Mouse().MovePath([][2]int32{{1000, 600}, {900, 500}}, 80, &linear); err != nil {
		t.Fatalf("move path: %v", err)
	}
	cursor := device.cursorState()
	if absInt32(cursor[0]-900) > 3 || absInt32(cursor[1]-500) > 3 {
		t.Fatalf("cursor = %v, want within 3 px of (900, 500)", cursor)
	}
	if code := CodeOf(device.Mouse().MovePath(nil, 10, nil)); code != Invalid {
		t.Fatalf("empty path code = %v, want INVALID", code)
	}
}

func TestMovementCanBeCancelled(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	movement := device.Mouse().StartMoveBy(4000, 4000, 400, nil)
	movement.Cancel()
	if code := CodeOf(movement.Wait(2000)); code != Cancelled {
		t.Fatalf("code = %v, want CANCELLED", code)
	}
	movement.Close()

	finished := device.Mouse().StartMoveBy(100, 0, 50, nil)
	if err := finished.Wait(5000); err != nil {
		t.Fatalf("short move: %v", err)
	}
	if !finished.Finished() {
		t.Fatal("movement reports not finished after Wait returned")
	}
	finished.Close()
}

func TestMovementRetargetKeepsTheWorkerAlive(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	movement := device.Mouse().StartMoveTo(1500, 900, 400, nil)
	movement.Retarget(700, 200)
	if err := movement.Wait(5000); err != nil {
		t.Fatalf("retargeted move: %v", err)
	}
	cursor := device.cursorState()
	if absInt32(cursor[0]-700) > 4 || absInt32(cursor[1]-200) > 4 {
		t.Fatalf("cursor = %v, want near the retargeted point (700, 200)", cursor)
	}
	movement.Close()
}

func TestDragReleasesTheButton(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	linear := LinearProfile()
	if err := device.Mouse().Drag(Left, 1000, 600, 60, &linear); err != nil {
		t.Fatalf("drag: %v", err)
	}
	if handle.Held().Buttons != 0 {
		t.Fatalf("drag left buttons = %d held", handle.Held().Buttons)
	}
	if device.buttonsMask() != 0 {
		t.Fatalf("SDK still believes %#x buttons are held", device.buttonsMask())
	}
}
