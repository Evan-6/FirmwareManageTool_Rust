//go:build !windows

package gdbnative

// Outside Windows the SDK still compiles and the mock device still works; the
// vendor collection and the absolute-pointer maths do not exist here.

// List reports that enumeration needs the Windows HID APIs.
func List() ([]Entry, error) {
	return nil, errorf(Unsupported, "device enumeration needs Windows HID APIs")
}

// ListJSON reports that enumeration needs the Windows HID APIs.
func ListJSON() (string, error) {
	return "", errorf(Unsupported, "device enumeration needs Windows HID APIs")
}

// Open reports that hardware access needs the Windows HID APIs.
func Open(selector string, opts Options) (*Device, error) {
	return nil, errorf(Unsupported, "hardware access needs Windows HID APIs")
}

// screenTargetFor reports that absolute coordinates need Windows.
func screenTargetFor(screen ScreenTarget) (Target, error) {
	return Target{}, errorf(Unsupported, "absolute mouse coordinates need Windows cursor APIs")
}

// screenAndCursor reports that absolute coordinates need Windows.
func screenAndCursor() (Screen, [2]int32, error) {
	return Screen{}, [2]int32{}, errorf(Unsupported, "absolute mouse coordinates need Windows cursor APIs")
}

// enableDPIAwareness has nothing to do off Windows.
func enableDPIAwareness() {}
