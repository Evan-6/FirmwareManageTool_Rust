package gdbnative

import (
	"fmt"
	"math"
	"time"
)

// Screen is the host display the movement is planned on: the primary monitor,
// in physical pixels, anchored at 0,0 like GetMonitorInfoW reports it.
type Screen struct {
	Width  int32 `json:"width"`
	Height int32 `json:"height"`
}

// DefaultScreen is the layout the mock device pretends to have.
func DefaultScreen() Screen { return Screen{Width: 1920, Height: 1080} }

// Rect is a physical-pixel rectangle in virtual-desktop coordinates. Monitors
// are half-open on the right and bottom edges: the pixel at Right belongs to
// whatever sits next to it.
type Rect struct {
	Left   int32
	Top    int32
	Right  int32
	Bottom int32
}

// Width is the rectangle width in pixels.
func (r Rect) Width() int32 { return r.Right - r.Left }

// Height is the rectangle height in pixels.
func (r Rect) Height() int32 { return r.Bottom - r.Top }

// Origin is the top-left corner.
func (r Rect) Origin() [2]int32 { return [2]int32{r.Left, r.Top} }

// Contains reports whether the point is inside the rectangle.
func (r Rect) Contains(point [2]int32) bool {
	return point[0] >= r.Left && point[0] < r.Right && point[1] >= r.Top && point[1] < r.Bottom
}

// Union is the smallest rectangle covering both.
func (r Rect) Union(other Rect) Rect {
	return Rect{
		Left:   minInt32(r.Left, other.Left),
		Top:    minInt32(r.Top, other.Top),
		Right:  maxInt32(r.Right, other.Right),
		Bottom: maxInt32(r.Bottom, other.Bottom),
	}
}

// UnionPoint is the smallest rectangle covering this one and one more point.
func (r Rect) UnionPoint(point [2]int32) Rect {
	return Rect{
		Left:   minInt32(r.Left, point[0]),
		Top:    minInt32(r.Top, point[1]),
		Right:  maxInt32(r.Right, point[0]+1),
		Bottom: maxInt32(r.Bottom, point[1]+1),
	}
}

// Clamp is the nearest point inside the rectangle.
func (r Rect) Clamp(point [2]int32) [2]int32 {
	return [2]int32{
		clampInt32(point[0], r.Left, maxInt32(r.Right-1, r.Left)),
		clampInt32(point[1], r.Top, maxInt32(r.Bottom-1, r.Top)),
	}
}

// ScreenKind names the three shapes a ScreenTarget can take.
type ScreenKind int32

const (
	// ScreenPrimary is the primary monitor: the zero value and the default.
	ScreenPrimary ScreenKind = iota
	// ScreenMonitor is the index-th display in EnumDisplayMonitors order.
	ScreenMonitor
	// ScreenWindow is a window's bounding rectangle.
	ScreenWindow
)

// ScreenTarget is which rectangle an absolute move is anchored to.
//
// The zero value is the primary monitor, so a caller that does not ask for
// another screen keeps the coordinates it always had.
type ScreenTarget struct {
	Kind  ScreenKind
	Value int
}

// PrimaryScreen is the primary monitor.
func PrimaryScreen() ScreenTarget { return ScreenTarget{} }

// MonitorScreen is the index-th display in EnumDisplayMonitors order. That order
// is the order the system enumerates the displays, so it is not the display
// number Windows settings shows and index 0 need not be the primary monitor.
func MonitorScreen(index int) ScreenTarget {
	return ScreenTarget{Kind: ScreenMonitor, Value: index}
}

// WindowScreen is a window's bounding rectangle, hwnd being the caller's handle.
func WindowScreen(hwnd int) ScreenTarget {
	return ScreenTarget{Kind: ScreenWindow, Value: hwnd}
}

// Describe names the target the way error messages do.
func (s ScreenTarget) Describe() string {
	switch s.Kind {
	case ScreenMonitor:
		return fmt.Sprintf("monitor %d", s.Value)
	case ScreenWindow:
		return fmt.Sprintf("window 0x%x", s.Value)
	default:
		return "the primary monitor"
	}
}

// String is Describe.
func (s ScreenTarget) String() string { return s.Describe() }

// Target is the pointer, the anchored rectangle and every monitor, all in
// virtual-desktop physical pixels.
type Target struct {
	// Rect is the requested rectangle: the move is clamped into it.
	Rect Rect
	// Cursor is where the pointer really is, which may be on another monitor.
	Cursor [2]int32
	// Screens is every monitor, so "is this point on a screen at all" has an
	// answer.
	Screens []Rect
}

// Bounds is the union of every monitor: the desktop as one rectangle.
func (t Target) Bounds() Rect {
	if len(t.Screens) == 0 {
		return t.Rect
	}
	bounds := t.Screens[0]
	for _, rect := range t.Screens[1:] {
		bounds = bounds.Union(rect)
	}
	return bounds
}

// Reaches reports whether some monitor covers the point.
func (t Target) Reaches(point [2]int32) bool {
	for _, rect := range t.Screens {
		if rect.Contains(point) {
			return true
		}
	}
	return false
}

// Desktop is the platform queries target resolution needs. Tests implement it
// instead of moving a real pointer; the Windows one is systemDesktop.
type Desktop interface {
	// Monitors is every monitor rectangle, in enumeration order.
	Monitors() ([]Rect, error)
	// Primary is the index into Monitors of the primary monitor.
	Primary() (int, error)
	// Cursor is the pointer, in virtual-desktop pixels.
	Cursor() ([2]int32, error)
	// Window is a window's bounding rectangle, in virtual-desktop pixels.
	Window(hwnd int) (Rect, error)
}

// Resolve resolves screen against a desktop without touching the pointer.
func Resolve(desktop Desktop, screen ScreenTarget) (Target, error) {
	screens, err := desktop.Monitors()
	if err != nil {
		return Target{}, err
	}
	if len(screens) == 0 {
		return Target{}, errorf(Unsupported, "the system reports no monitors")
	}
	var rect Rect
	switch screen.Kind {
	case ScreenMonitor:
		if screen.Value < 0 || screen.Value >= len(screens) {
			return Target{}, errorf(Unsupported,
				"monitor index %d is out of range: the system reports %d monitor(s)",
				screen.Value, len(screens))
		}
		rect = screens[screen.Value]
	case ScreenWindow:
		rect, err = desktop.Window(screen.Value)
		if err != nil {
			return Target{}, err
		}
		if rect.Width() <= 0 || rect.Height() <= 0 {
			return Target{}, errorf(Unsupported,
				"window 0x%x has an empty rectangle (%dx%d)",
				screen.Value, rect.Width(), rect.Height())
		}
	default:
		index, err := desktop.Primary()
		if err != nil {
			return Target{}, err
		}
		if index < 0 || index >= len(screens) {
			return Target{}, errorf(Unsupported,
				"the primary monitor is index %d, but the system reports %d monitor(s)",
				index, len(screens))
		}
		rect = screens[index]
	}
	cursor, err := desktop.Cursor()
	if err != nil {
		return Target{}, err
	}
	return Target{Rect: rect, Cursor: cursor, Screens: screens}, nil
}

// OutsideMessage is what an absolute move reports when its point is on no
// monitor at all: which target was asked for and the desktop the host could see.
func OutsideMessage(screen ScreenTarget, point [2]int32, bounds Rect) string {
	return fmt.Sprintf(
		"absolute move to (%d, %d) targets %s, but that point is on no screen: "+
			"the monitors cover (%d, %d)-(%d, %d)",
		point[0], point[1], screen.Describe(), bounds.Left, bounds.Top, bounds.Right, bounds.Bottom)
}

// MockTarget is the single 1920x1080 desktop a mock device pretends to have.
func MockTarget(screen ScreenTarget, cursor [2]int32) (Target, error) {
	switch screen.Kind {
	case ScreenMonitor:
		if screen.Value != 0 {
			return Target{}, errorf(Unsupported,
				"monitor index %d is out of range: a mock device has one 1920x1080 screen",
				screen.Value)
		}
	case ScreenWindow:
		return Target{}, errorf(Unsupported,
			"window 0x%x has no rectangle: a mock device has no windows", screen.Value)
	}
	rect := Rect{Right: 1920, Bottom: 1080}
	return Target{Rect: rect, Cursor: cursor, Screens: []Rect{rect}}, nil
}

// Node is one host-side waypoint. AtMs is relative to the movement start; only
// relative deltas ever reach the device.
type Node struct {
	AtMs uint64
	X    int32
	Y    int32
}

// EaseKind names the progress curves; the numbers match gdb_ease so a profile
// can cross the C ABI unchanged.
type EaseKind int32

const (
	EaseJerk        EaseKind = 0
	EaseInOut       EaseKind = 1
	EaseLinear      EaseKind = 2
	EaseIn          EaseKind = 3
	EaseOut         EaseKind = 4
	EaseCubicBezier EaseKind = 5
)

// Ease is a progress curve. Bezier holds x1, y1, x2, y2 and is only read when
// Kind is EaseCubicBezier.
type Ease struct {
	Kind   EaseKind
	Bezier [4]float64
}

// CubicEase builds a cubic bezier easing curve.
func CubicEase(x1, y1, x2, y2 float64) Ease {
	return Ease{Kind: EaseCubicBezier, Bezier: [4]float64{x1, y1, x2, y2}}
}

// Apply maps progress 0..1 onto 0..1.
func (e Ease) Apply(t float64) float64 {
	t = clampFloat(t, 0, 1)
	switch e.Kind {
	case EaseLinear:
		return t
	case EaseJerk:
		// Smooth at both ends, no overshoot: the original GUI curve.
		return t * t * t * (10 + t*(-15+6*t))
	case EaseIn:
		return t * t * t
	case EaseOut:
		u := 1 - t
		return 1 - u*u*u
	case EaseInOut:
		return t * t * (3 - 2*t)
	case EaseCubicBezier:
		return cubicBezierEase(t, e.Bezier[0], e.Bezier[1], e.Bezier[2], e.Bezier[3])
	default:
		return t
	}
}

// cubicBezierEase undoes the x axis with a fixed Newton iteration count, so the
// result never depends on floating-point convergence luck.
func cubicBezierEase(t, x1, y1, x2, y2 float64) float64 {
	x1 = clampFloat(x1, 0, 1)
	x2 = clampFloat(x2, 0, 1)
	curveX := func(u float64) float64 { return bezier(0, x1, x2, 1, u) }
	curveY := func(u float64) float64 { return bezier(0, y1, y2, 1, u) }
	u := t
	for step := 0; step < 8; step++ {
		x := curveX(u) - t
		if math.Abs(x) < 1e-9 {
			break
		}
		slope := (curveX(math.Min(u+1e-6, 1)) - curveX(math.Max(u-1e-6, 0))) / 2e-6
		if math.Abs(slope) < 1e-9 {
			break
		}
		u = clampFloat(u-x/slope, 0, 1)
	}
	return clampFloat(curveY(u), 0, 1)
}

func bezier(a, b, c, d, t float64) float64 {
	u := 1 - t
	return u*u*u*a + 3*u*u*t*b + 3*u*t*t*c + t*t*t*d
}

// Profile is the movement profile. Field names and defaults are the SDK API.
type Profile struct {
	// Curve: 0 straight, 1 the original bow, 2 an exaggerated arc.
	Curve float64
	// JitterPx is the micro-jitter amplitude in pixels.
	JitterPx float64
	// OvershootPct is how far past the target the pointer goes, in percent.
	OvershootPct float64
	// SettleMs is the pause at the end, before a click would land.
	SettleMs uint32
	// Ease maps progress onto time.
	Ease Ease
	// MinStepPx drops steps smaller than this, like a coarser sensor.
	MinStepPx int32
}

// DefaultProfile is the SDK default: a slight bow, 1 px of jitter, jerk easing.
func DefaultProfile() Profile {
	return Profile{
		Curve:     1,
		JitterPx:  1,
		Ease:      Ease{Kind: EaseJerk},
		MinStepPx: 1,
	}
}

// LinearProfile is a straight, constant-speed path with no jitter or overshoot.
func LinearProfile() Profile {
	return Profile{
		Curve:     0,
		JitterPx:  0,
		Ease:      Ease{Kind: EaseLinear},
		MinStepPx: 1,
	}
}

// profileOr reports the profile a nil pointer stands for: the same default
// gdb_profile_default hands the C ABI.
func profileOr(profile *Profile) Profile {
	if profile == nil {
		return DefaultProfile()
	}
	return *profile
}

// SplitMix64 is the seedable, platform-independent RNG the motion maths uses.
type SplitMix64 struct {
	state uint64
}

// NewSplitMix64 seeds the generator.
func NewSplitMix64(seed uint64) *SplitMix64 { return &SplitMix64{state: seed} }

// NewEntropySplitMix64 seeds from the clock, the process id and the golden
// ratio constant, the same mixture as the Rust SDK.
func NewEntropySplitMix64() *SplitMix64 {
	now := uint64(time.Now().UnixNano())
	pid := uint64(processID())
	return &SplitMix64{state: now ^ (pid << 32) ^ 0x9E3779B97F4A7C15}
}

func (r *SplitMix64) next() uint64 {
	r.state += 0x9E3779B97F4A7C15
	z := r.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// Unit returns a value in [0, 1) with 53 significant bits, the same value every
// language can reproduce.
func (r *SplitMix64) Unit() float64 {
	return float64(r.next()>>11) / float64(uint64(1)<<53)
}

// SplitDelta splits one relative move into HID-sized chunks, ±1024 per axis.
func SplitDelta(dx, dy int32) [][2]int32 {
	parts := make([][2]int32, 0, 4)
	for dx != 0 || dy != 0 {
		stepX := clampInt32(dx, -1024, 1024)
		stepY := clampInt32(dy, -1024, 1024)
		parts = append(parts, [2]int32{stepX, stepY})
		dx -= stepX
		dy -= stepY
	}
	return parts
}

// NaturalDuration is the distance-adaptive duration used when the caller passes
// durationMs = 0.
func NaturalDuration(distance float64, rng *SplitMix64) uint32 {
	value := 150 + distance*0.42 + rng.Unit()*70 - 25
	return uint32(clampFloat(value, 170, 900))
}

// TrajectoryWith samples a human-ish path from start to target on screen.
// durationMs = 0 picks a natural duration. The nodes are host coordinates.
func TrajectoryWith(start, target [2]int32, screen Screen, durationMS uint32, profile Profile, rng *SplitMix64) ([]Node, error) {
	if screen.Width <= 1 || screen.Height <= 1 {
		return nil, errorf(Invalid, "screen size is invalid")
	}
	target = [2]int32{
		clampInt32(target[0], 0, screen.Width-1),
		clampInt32(target[1], 0, screen.Height-1),
	}
	dx := float64(target[0]) - float64(start[0])
	dy := float64(target[1]) - float64(start[1])
	distance := math.Hypot(dx, dy)
	duration := durationMS
	if duration == 0 {
		duration = NaturalDuration(distance, rng)
	} else {
		duration = clampUint32(duration, 80, 2000)
	}
	if distance < 1 {
		return []Node{{AtMs: 0, X: target[0], Y: target[1]}}, nil
	}
	nx := -dy / distance
	ny := dx / distance
	over := clampFloat(distance*profile.OvershootPct/100, 0, 160)
	aimX, aimY := float64(target[0]), float64(target[1])
	if over > 0.5 {
		aimX = clampFloat(float64(target[0])+dx/distance*over, 0, float64(screen.Width-1))
		aimY = clampFloat(float64(target[1])+dy/distance*over, 0, float64(screen.Height-1))
	}
	limit := clampFloat(distance*0.13, 4, 110) * clampFloat(profile.Curve, 0, 2)
	bend := (rng.Unit()*2 - 1) * limit
	a1 := 0.20 + rng.Unit()*0.18
	a2 := 0.62 + rng.Unit()*0.22
	bend2 := bend*0.55 + (rng.Unit()*2-1)*limit*0.20
	c1x := float64(start[0]) + dx*a1 + nx*bend
	c1y := float64(start[1]) + dy*a1 + ny*bend
	c2x := float64(start[0]) + dx*a2 + nx*bend2
	c2y := float64(start[1]) + dy*a2 + ny*bend2
	phase := rng.Unit() * 2 * math.Pi
	frequency := 1.1 + rng.Unit()*1.2
	amplitude := clampFloat(profile.JitterPx, 0, 24)
	steps := clampUint32(duration/8, 10, 180)
	minStep := profile.MinStepPx
	if minStep < 1 {
		minStep = 1
	}
	nodes := make([]Node, 0, steps)
	last := start
	for step := uint32(1); step <= steps; step++ {
		p := float64(step) / float64(steps)
		t := profile.Ease.Apply(p)
		jitter := amplitude * math.Sin(math.Pi*p) * math.Sin(phase+p*2*math.Pi*frequency)
		var point [2]int32
		if step == steps {
			point = [2]int32{int32(math.Round(aimX)), int32(math.Round(aimY))}
		} else {
			point = [2]int32{
				int32(math.Round(bezier(float64(start[0]), c1x, c2x, aimX, t) + nx*jitter)),
				int32(math.Round(bezier(float64(start[1]), c1y, c2y, aimY, t) + ny*jitter)),
			}
		}
		point = [2]int32{
			clampInt32(point[0], 0, screen.Width-1),
			clampInt32(point[1], 0, screen.Height-1),
		}
		gap := absInt32(point[0]-last[0]) + absInt32(point[1]-last[1])
		if gap >= minStep || step == steps {
			nodes = append(nodes, Node{AtMs: uint64(duration) * uint64(step) / uint64(steps), X: point[0], Y: point[1]})
			last = point
		}
	}
	lastAt := uint64(0)
	if len(nodes) > 0 {
		lastAt = nodes[len(nodes)-1].AtMs
	}
	if over > 0.5 {
		lastAt += 90
		nodes = append(nodes, Node{AtMs: lastAt, X: target[0], Y: target[1]})
	}
	if profile.SettleMs > 0 {
		lastAt += uint64(profile.SettleMs)
		nodes = append(nodes, Node{AtMs: lastAt, X: target[0], Y: target[1]})
	}
	return nodes, nil
}

// CurrentNode picks the node that should be active at elapsedMs, never
// moving backwards.
func CurrentNode(nodes []Node, next int, elapsedMS uint64) int {
	if len(nodes) == 0 {
		return 0
	}
	active := 0
	for active < len(nodes) && nodes[active].AtMs <= elapsedMS {
		active++
	}
	active--
	if active < next {
		active = next
	}
	if active > len(nodes)-1 {
		active = len(nodes) - 1
	}
	if active < 0 {
		active = 0
	}
	return active
}

// TrackerTick is the minimum age of a sent delta before the next correction.
const TrackerTick = 8 * time.Millisecond

// StepKind is what the tracker wants done with the pointer.
type StepKind int

const (
	StepArrived StepKind = iota
	StepWait
	StepMove
)

// Step is one tracker decision: a relative delta, a wait, or arrival.
type Step struct {
	Kind StepKind
	DX   int32
	DY   int32
}

type pendingMotion struct {
	sentAt   time.Time
	previous [2]int32
	delta    [2]int32
}

// RelativeTracker drives host feedback for absolute moves: the SDK only ever
// sends relative reports, so it measures what the host did with each one.
type RelativeTracker struct {
	gain    [2]float64
	pending *pendingMotion
}

// NewRelativeTracker returns a tracker with unit gain on both axes.
func NewRelativeTracker() *RelativeTracker {
	return &RelativeTracker{gain: [2]float64{1, 1}}
}

// Step decides the next relative delta from the current pointer position.
func (r *RelativeTracker) Step(position, target [2]int32, now time.Time) Step {
	if r.gain[0] == 0 {
		r.gain = [2]float64{1, 1}
	}
	if pending := r.pending; pending != nil {
		// Do not integrate the same stale feedback on every tick: give USB
		// delivery time to become visible before the next correction.
		age := now.Sub(pending.sentAt)
		if age < TrackerTick {
			return Step{Kind: StepWait}
		}
		if position == pending.previous && age < 32*time.Millisecond {
			return Step{Kind: StepWait}
		}
		for axis := 0; axis < 2; axis++ {
			moved := position[axis] - pending.previous[axis]
			sent := pending.delta[axis]
			if sent != 0 && moved != 0 && signInt32(moved) == signInt32(sent) {
				// Track effective host sensitivity, including acceleration.
				measured := clampFloat(float64(moved)/float64(sent), 0.25, 32)
				r.gain[axis] = math.Max(measured, r.gain[axis]*0.75)
			}
		}
		r.pending = nil
	}
	errorX := target[0] - position[0]
	errorY := target[1] - position[1]
	if absInt32(errorX) <= 1 && absInt32(errorY) <= 1 {
		return Step{Kind: StepArrived}
	}
	var delta [2]int32
	for axis, difference := range [2]int32{errorX, errorY} {
		if absInt32(difference) <= 1 {
			continue
		}
		damping := 0.75
		if absInt32(difference) <= 8 {
			damping = 1
		}
		step := int32(math.Round(float64(difference) * damping / r.gain[axis]))
		if step == 0 {
			delta[axis] = signInt32(difference)
		} else {
			delta[axis] = clampInt32(step, -512, 512)
		}
	}
	r.pending = &pendingMotion{sentAt: now, previous: position, delta: delta}
	return Step{Kind: StepMove, DX: delta[0], DY: delta[1]}
}

func signInt32(value int32) int32 {
	switch {
	case value > 0:
		return 1
	case value < 0:
		return -1
	default:
		return 0
	}
}

func absInt32(value int32) int32 {
	if value < 0 {
		return -value
	}
	return value
}

func minInt32(a, b int32) int32 {
	if a < b {
		return a
	}
	return b
}

func maxInt32(a, b int32) int32 {
	if a > b {
		return a
	}
	return b
}

func clampFloat(value, low, high float64) float64 {
	if value < low {
		return low
	}
	if value > high {
		return high
	}
	return value
}

func clampUint32(value, low, high uint32) uint32 {
	if value < low {
		return low
	}
	if value > high {
		return high
	}
	return value
}
