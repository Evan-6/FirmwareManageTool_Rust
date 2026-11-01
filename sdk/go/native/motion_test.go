package gdbnative

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"
)

type goldenProfile struct {
	Curve        float64 `json:"curve"`
	JitterPx     float64 `json:"jitterPx"`
	OvershootPct float64 `json:"overshootPct"`
	SettleMS     uint32  `json:"settleMs"`
	Ease         struct {
		Kind   int32      `json:"kind"`
		Bezier [4]float64 `json:"bezier"`
	} `json:"ease"`
	MinStepPx int32 `json:"minStepPx"`
}

type goldenCase struct {
	Name       string        `json:"name"`
	Start      [2]int32      `json:"start"`
	Target     [2]int32      `json:"target"`
	Screen     [2]int32      `json:"screen"`
	DurationMS uint32        `json:"durationMs"`
	Seed       uint64        `json:"seed"`
	Profile    goldenProfile `json:"profile"`
	Nodes      [][3]int64    `json:"nodes"`
}

type goldenDocument struct {
	Source string       `json:"_source"`
	Cases  []goldenCase `json:"cases"`
	Splits []struct {
		Delta [2]int32   `json:"delta"`
		Parts [][2]int32 `json:"parts"`
	} `json:"splits"`
}

func loadGolden(t *testing.T) goldenDocument {
	t.Helper()
	path := filepath.Join("testdata", "motion.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	var document goldenDocument
	if err := json.Unmarshal(raw, &document); err != nil {
		t.Fatalf("decode %s: %v", path, err)
	}
	if len(document.Cases) == 0 {
		t.Fatal("no golden cases")
	}
	return document
}

// TestTrajectoryMatchesTheRustGoldenVectors replays the node lists gdb-motion
// produces for the same seed and profile.
func TestTrajectoryMatchesTheRustGoldenVectors(t *testing.T) {
	for _, testCase := range loadGolden(t).Cases {
		t.Run(testCase.Name, func(t *testing.T) {
			profile := Profile{
				Curve:        testCase.Profile.Curve,
				JitterPx:     testCase.Profile.JitterPx,
				OvershootPct: testCase.Profile.OvershootPct,
				SettleMs:     testCase.Profile.SettleMS,
				Ease:         Ease{Kind: EaseKind(testCase.Profile.Ease.Kind), Bezier: testCase.Profile.Ease.Bezier},
				MinStepPx:    testCase.Profile.MinStepPx,
			}
			screen := Screen{Width: testCase.Screen[0], Height: testCase.Screen[1]}
			nodes, err := TrajectoryWith(testCase.Start, testCase.Target, screen, testCase.DurationMS, profile, NewSplitMix64(testCase.Seed))
			if err != nil {
				t.Fatalf("trajectory: %v", err)
			}
			if len(nodes) != len(testCase.Nodes) {
				t.Fatalf("nodes = %d, want %d\n got %v", len(nodes), len(testCase.Nodes), nodes)
			}
			for index, want := range testCase.Nodes {
				got := nodes[index]
				if int64(got.AtMs) != want[0] || int64(got.X) != want[1] || int64(got.Y) != want[2] {
					t.Fatalf("node %d = (%d,%d @%d ms), want (%d,%d @%d ms)",
						index, got.X, got.Y, got.AtMs, want[1], want[2], want[0])
				}
			}
		})
	}
}

func TestSplitDeltaMatchesTheRustGoldenVectors(t *testing.T) {
	for _, entry := range loadGolden(t).Splits {
		got := SplitDelta(entry.Delta[0], entry.Delta[1])
		if len(got) != len(entry.Parts) {
			t.Fatalf("%v split into %v, want %v", entry.Delta, got, entry.Parts)
		}
		for index := range got {
			if got[index] != entry.Parts[index] {
				t.Fatalf("%v split into %v, want %v", entry.Delta, got, entry.Parts)
			}
		}
	}
}

func TestSplitDeltaEdges(t *testing.T) {
	if parts := SplitDelta(0, 0); len(parts) != 0 {
		t.Fatalf("zero move produced %v", parts)
	}
	parts := SplitDelta(1025, -1)
	if len(parts) != 2 || parts[0] != [2]int32{1024, -1} || parts[1] != [2]int32{1, 0} {
		t.Fatalf("parts = %v", parts)
	}
	for _, chunk := range SplitDelta(-3000, 2500) {
		if chunk[0] < -1024 || chunk[0] > 1024 || chunk[1] < -1024 || chunk[1] > 1024 {
			t.Fatalf("chunk out of HID range: %v", chunk)
		}
	}
}

func TestTrajectoryRejectsAnInvalidScreen(t *testing.T) {
	_, err := TrajectoryWith([2]int32{0, 0}, [2]int32{10, 10}, Screen{Width: 0, Height: 0}, 100, DefaultProfile(), NewSplitMix64(1))
	if CodeOf(err) != Invalid {
		t.Fatalf("code = %v, want INVALID", CodeOf(err))
	}
}

func TestEasingIsPinnedAndMonotonic(t *testing.T) {
	curves := []Ease{
		{Kind: EaseLinear},
		{Kind: EaseJerk},
		{Kind: EaseIn},
		{Kind: EaseOut},
		{Kind: EaseInOut},
		CubicEase(0.25, 0.1, 0.25, 1.0),
		CubicEase(0.42, 0, 0.58, 1),
	}
	for _, curve := range curves {
		if math.Abs(curve.Apply(0)) > 1e-6 {
			t.Errorf("%+v at 0 = %v", curve, curve.Apply(0))
		}
		if math.Abs(curve.Apply(1)-1) > 1e-6 {
			t.Errorf("%+v at 1 = %v", curve, curve.Apply(1))
		}
		previous := 0.0
		for step := 0; step <= 100; step++ {
			value := curve.Apply(float64(step) / 100)
			if value+1e-6 < previous {
				t.Fatalf("%+v is not monotonic at %d: %v < %v", curve, step, value, previous)
			}
			previous = value
		}
	}
}

func TestNaturalDurationStaysInRange(t *testing.T) {
	for seed := uint64(0); seed < 32; seed++ {
		duration := NaturalDuration(float64(seed)*50, NewSplitMix64(seed))
		if duration < 170 || duration > 900 {
			t.Fatalf("duration = %d for seed %d", duration, seed)
		}
	}
}

func TestTrackerConvergesAtEveryHostSensitivity(t *testing.T) {
	for _, gain := range []float64{0.5, 1, 2, 4} {
		tracker := NewRelativeTracker()
		position := [2]int32{-300, 700} // may start on another monitor
		target := [2]int32{1600, 200}
		start := time.Now()
		arrived := false
		for step := 0; step < 38; step++ {
			switch result := tracker.Step(position, target, start.Add(time.Duration(step)*TrackerTick)); result.Kind {
			case StepMove:
				if absInt32(result.DX) > 512 || absInt32(result.DY) > 512 {
					t.Fatalf("gain %v: delta %v exceeds the 512 clamp", gain, result)
				}
				position[0] += int32(math.Round(float64(result.DX) * gain))
				position[1] += int32(math.Round(float64(result.DY) * gain))
			case StepArrived:
				arrived = true
			case StepWait:
			}
			if arrived {
				break
			}
		}
		if !arrived {
			t.Fatalf("gain %v did not converge, pointer at %v", gain, position)
		}
	}
}

func TestTrackerWaitsForUsbFeedback(t *testing.T) {
	tracker := NewRelativeTracker()
	now := time.Now()
	if result := tracker.Step([2]int32{0, 0}, [2]int32{100, 0}, now); result.Kind != StepMove || result.DX <= 0 {
		t.Fatalf("first step = %+v", result)
	}
	for step := 1; step < 4; step++ {
		if result := tracker.Step([2]int32{0, 0}, [2]int32{100, 0}, now.Add(time.Duration(step)*TrackerTick)); result.Kind != StepWait {
			t.Fatalf("step %d = %+v, want WAIT", step, result)
		}
	}
	if result := tracker.Step([2]int32{100, 0}, [2]int32{100, 0}, now.Add(4*TrackerTick)); result.Kind != StepArrived {
		t.Fatalf("arrival = %+v, want ARRIVED", result)
	}
}

func TestTrackerCorrectsATwoPixelOffset(t *testing.T) {
	tracker := NewRelativeTracker()
	now := time.Now()
	if result := tracker.Step([2]int32{100, 100}, [2]int32{102, 98}, now); result.Kind != StepMove || result.DX != 2 || result.DY != -2 {
		t.Fatalf("correction = %+v", result)
	}
	if result := tracker.Step([2]int32{102, 98}, [2]int32{102, 98}, now.Add(TrackerTick)); result.Kind != StepArrived {
		t.Fatalf("arrival = %+v", result)
	}
}

func TestCurrentNodeNeverMovesBackwards(t *testing.T) {
	nodes := []Node{{AtMs: 0, X: 0}, {AtMs: 10, X: 1}, {AtMs: 20, X: 2}, {AtMs: 30, X: 3}}
	if got := CurrentNode(nodes, 0, 0); got != 0 {
		t.Errorf("at 0 ms = %d, want 0", got)
	}
	if got := CurrentNode(nodes, 0, 25); got != 2 {
		t.Errorf("at 25 ms = %d, want 2", got)
	}
	if got := CurrentNode(nodes, 3, 0); got != 3 {
		t.Errorf("with next=3 at 0 ms = %d, want 3", got)
	}
	if got := CurrentNode(nodes, 0, 10_000); got != len(nodes)-1 {
		t.Errorf("past the end = %d, want %d", got, len(nodes)-1)
	}
	if got := CurrentNode(nil, 0, 10); got != 0 {
		t.Errorf("empty list = %d, want 0", got)
	}
}
