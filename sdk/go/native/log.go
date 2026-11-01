package gdbnative

import (
	"fmt"
	"strconv"
	"strings"
	"sync"
)

// Level is one SDK log level, ordered from silent to noisiest.
type Level uint8

// Log levels, matching gdb_log_level in sdk/ffi/include/gdb.h (which is a copy
// of sdk/rust/src/log.rs). Off is the default: nothing is formatted until a
// level is raised, and a line is only handed to the sink when its level is at
// or below the configured one.
const (
	LogOff   = 0
	LogError = 1
	LogWarn  = 2
	LogInfo  = 3
	LogDebug = 4
	LogTrace = 5
)

// String is the level name, for log lines and test failures.
func (l Level) String() string {
	switch l {
	case LogOff:
		return "off"
	case LogError:
		return "error"
	case LogWarn:
		return "warn"
	case LogInfo:
		return "info"
	case LogDebug:
		return "debug"
	case LogTrace:
		return "trace"
	default:
		return "level(" + strconv.Itoa(int(l)) + ")"
	}
}

// The logger is process-wide, like sdk/rust/src/log.rs: the level and the sink
// belong to the process, not to a device, so a caller can turn tracing on
// before it opens anything.
var processLogger struct {
	mu    sync.RWMutex
	level Level
	sink  func(level int32, message string)
}

// SetLogLevel sets the process-wide level (LogOff … LogTrace). A value above
// LogTrace is clamped rather than rejected: a caller asking for "everything"
// must not fail on an off-by-one.
func SetLogLevel(level uint8) {
	if level > LogTrace {
		level = LogTrace
	}
	processLogger.mu.Lock()
	processLogger.level = Level(level)
	processLogger.mu.Unlock()
}

// CurrentLogLevel is the level SetLogLevel installed.
func CurrentLogLevel() Level {
	processLogger.mu.RLock()
	defer processLogger.mu.RUnlock()
	return processLogger.level
}

// SetLogSink installs the callback that receives formatted lines; nil turns
// logging off again without touching the level. The callback runs on whichever
// goroutine produced the line — including the heartbeat goroutine — so it must
// not block for long.
func SetLogSink(sink func(level int32, message string)) {
	processLogger.mu.Lock()
	processLogger.sink = sink
	processLogger.mu.Unlock()
}

// logEnabled reports whether a line at this level would reach the sink.
func logEnabled(level Level) bool {
	processLogger.mu.RLock()
	defer processLogger.mu.RUnlock()
	return level <= processLogger.level && processLogger.sink != nil
}

// logf formats one line and hands it to the sink. The message is only built
// when the level is enabled, which is what keeps tracing cheap in a hot loop.
func logf(level Level, format string, args ...any) {
	processLogger.mu.RLock()
	enabled := level <= processLogger.level
	sink := processLogger.sink
	processLogger.mu.RUnlock()
	if !enabled || sink == nil {
		return
	}
	sink(int32(level), fmt.Sprintf(format, args...))
}

// formatPayload renders a payload the way sdk/rust/src/session.rs does with
// `{:?}`: "[1, 2, 3]", and "[]" for an empty payload. The trace lines are
// compared against the Rust SDK's, so the shape has to match.
func formatPayload(payload []byte) string {
	var out strings.Builder
	out.Grow(3 * len(payload))
	out.WriteByte('[')
	for index, value := range payload {
		if index != 0 {
			out.WriteString(", ")
		}
		out.WriteString(strconv.Itoa(int(value)))
	}
	out.WriteByte(']')
	return out.String()
}
