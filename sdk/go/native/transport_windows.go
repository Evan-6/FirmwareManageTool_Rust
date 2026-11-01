//go:build windows

package gdbnative

import (
	"strings"
	"sync"
	"syscall"
	"time"
	"unsafe"
)

// Windows entry points. Everything that needs a 64-bit report goes through
// syscall, which takes slices; the DLL calls below only ever pass pointers to
// byte arrays and structs, never a stored uintptr.
var (
	hidDLL      = syscall.NewLazyDLL("hid.dll")
	kernel32DLL = syscall.NewLazyDLL("kernel32.dll")
	cfgmgr32DLL = syscall.NewLazyDLL("cfgmgr32.dll")
	user32DLL   = syscall.NewLazyDLL("user32.dll")

	procHidGetFeature      = hidDLL.NewProc("HidD_GetFeature")
	procHidGetSerialNumber = hidDLL.NewProc("HidD_GetSerialNumberString")
	procHidGetProduct      = hidDLL.NewProc("HidD_GetProductString")
	procHidGetManufacturer = hidDLL.NewProc("HidD_GetManufacturerString")

	procGetOverlappedResult = kernel32DLL.NewProc("GetOverlappedResult")
	procCancelIoEx          = kernel32DLL.NewProc("CancelIoEx")
	procResetEvent          = kernel32DLL.NewProc("ResetEvent")
	procCreateEvent         = kernel32DLL.NewProc("CreateEventW")

	procDeviceIDListSize = cfgmgr32DLL.NewProc("CM_Get_Device_ID_List_SizeW")
	procDeviceIDList     = cfgmgr32DLL.NewProc("CM_Get_Device_ID_ListW")

	procPhysicalCursorPos   = user32DLL.NewProc("GetPhysicalCursorPos")
	procMonitorInfo         = user32DLL.NewProc("GetMonitorInfoW")
	procEnumDisplayMonitors = user32DLL.NewProc("EnumDisplayMonitors")
	procGetWindowRect       = user32DLL.NewProc("GetWindowRect")
	procDPIAwareness        = user32DLL.NewProc("SetProcessDpiAwarenessContext")
)

// hidInterfaceGUID is the device interface class every HID collection exposes.
const hidInterfaceGUID = "{4d1e55b2-f16f-11cf-88cb-001111000030}"

// CM_Get_Device_ID_List flags.
const (
	cmGetIDListFilterPresent uint32 = 0x00000001
	// CM_Get_Device_ID filters by enumerator as well.
	cmGetIDListFilterEnumerator uint32 = 0x00000002
)

// MONITORINFOF_PRIMARY marks the primary monitor in GetMonitorInfoW's dwFlags.
const (
	monitorInfoPrimary uint32 = 1
	// dpiAwarenessPerMonitorV2 is DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
	// which is the handle value -4.
	dpiAwarenessPerMonitorV2 = ^uintptr(3)
)

// createEvent is a manual-reset event, which the overlapped waits need so a
// completed transfer is never mistaken for the next one.
func createEvent() (syscall.Handle, error) {
	handle, _, err := procCreateEvent.Call(0, 1, 0, 0)
	if handle == 0 {
		return 0, errorf(IO, "CreateEventW failed: %v", err)
	}
	return syscall.Handle(handle), nil
}

// hidTransport is one open vendor collection.
type hidTransport struct {
	mu         sync.Mutex
	handle     syscall.Handle
	readEvent  syscall.Handle
	writeEvent syscall.Handle
	closed     bool
}

// openHIDPath opens one interface path read/write with overlapped I/O, which is
// how a read can be given a timeout without a second thread.
func openHIDPath(path string) (Transport, error) {
	name, err := syscall.UTF16PtrFromString(path)
	if err != nil {
		return nil, errorf(Invalid, "HID path: %s", err)
	}
	handle, err := syscall.CreateFile(
		name,
		syscall.GENERIC_READ|syscall.GENERIC_WRITE,
		syscall.FILE_SHARE_READ|syscall.FILE_SHARE_WRITE,
		nil,
		syscall.OPEN_EXISTING,
		syscall.FILE_FLAG_OVERLAPPED,
		0,
	)
	if err != nil {
		return nil, errorf(Disconnected, "CreateFileW(%s): %s", path, err)
	}
	readEvent, err := createEvent()
	if err != nil {
		syscall.CloseHandle(handle)
		return nil, err
	}
	writeEvent, err := createEvent()
	if err != nil {
		syscall.CloseHandle(readEvent)
		syscall.CloseHandle(handle)
		return nil, err
	}
	return &hidTransport{handle: handle, readEvent: readEvent, writeEvent: writeEvent}, nil
}

// Feature performs GET_FEATURE(14).
func (h *hidTransport) Feature() (Report, error) {
	var report Report
	report[0] = FeatureReportID
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return report, errorf(Disconnected, "HID transport is closed")
	}
	ok, _, err := procHidGetFeature.Call(
		uintptr(h.handle),
		uintptr(unsafe.Pointer(&report[0])),
		uintptr(ReportSize),
	)
	if ok == 0 {
		return report, errorf(Disconnected, "HidD_GetFeature failed: %v", err)
	}
	if report[0] != FeatureReportID {
		return report, errorf(Framing, "feature report id %d, expected %d", report[0], FeatureReportID)
	}
	return report, nil
}

// Write sends one 64-byte output report and waits for the transfer to finish.
func (h *hidTransport) Write(report Report) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return errorf(Disconnected, "HID transport is closed")
	}
	overlapped := syscall.Overlapped{}
	overlapped.HEvent = h.writeEvent
	procResetEvent.Call(uintptr(h.writeEvent))
	var written uint32
	err := syscall.WriteFile(h.handle, report[:], &written, &overlapped)
	if err != nil {
		if err != syscall.ERROR_IO_PENDING {
			return errorf(Disconnected, "WriteFile: %s", err)
		}
		completed, err := h.wait(h.writeEvent, &overlapped, &written, syscall.INFINITE)
		if err != nil {
			return err
		}
		if !completed {
			return errorf(Timeout, "HID write did not complete")
		}
	}
	if written != ReportSize {
		return errorf(IO, "short HID write: %d/%d", written, ReportSize)
	}
	return nil
}

// Read waits at most timeout for one 64-byte input report.
func (h *hidTransport) Read(timeout time.Duration) (Report, bool, error) {
	var report Report
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return report, false, errorf(Disconnected, "HID transport is closed")
	}
	overlapped := syscall.Overlapped{}
	overlapped.HEvent = h.readEvent
	procResetEvent.Call(uintptr(h.readEvent))
	var read uint32
	err := syscall.ReadFile(h.handle, report[:], &read, &overlapped)
	if err != nil && err != syscall.ERROR_IO_PENDING {
		return report, false, errorf(Disconnected, "ReadFile: %s", err)
	}
	if err == syscall.ERROR_IO_PENDING {
		milliseconds := uint32(timeout / time.Millisecond)
		completed, err := h.wait(h.readEvent, &overlapped, &read, milliseconds)
		if err != nil {
			return report, false, err
		}
		if !completed {
			return report, false, nil
		}
	}
	if read != ReportSize {
		return report, false, errorf(Framing, "short HID read: %d/%d", read, ReportSize)
	}
	return report, true, nil
}

// wait blocks on an overlapped operation, cancelling it on timeout.
func (h *hidTransport) wait(event syscall.Handle, overlapped *syscall.Overlapped, moved *uint32, milliseconds uint32) (bool, error) {
	status, err := syscall.WaitForSingleObject(event, milliseconds)
	if err != nil {
		return false, errorf(IO, "WaitForSingleObject: %s", err)
	}
	if status == syscall.WAIT_TIMEOUT {
		// Reap the cancelled transfer so the next one starts clean.
		procCancelIoEx.Call(uintptr(h.handle), uintptr(unsafe.Pointer(overlapped)))
		procGetOverlappedResult.Call(
			uintptr(h.handle),
			uintptr(unsafe.Pointer(overlapped)),
			uintptr(unsafe.Pointer(moved)),
			1,
		)
		return false, nil
	}
	ok, _, callErr := procGetOverlappedResult.Call(
		uintptr(h.handle),
		uintptr(unsafe.Pointer(overlapped)),
		uintptr(unsafe.Pointer(moved)),
		0,
	)
	if ok == 0 {
		return false, errorf(Disconnected, "GetOverlappedResult: %v", callErr)
	}
	return true, nil
}

// Close releases the handle and both events.
func (h *hidTransport) Close() error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed {
		return nil
	}
	h.closed = true
	if h.handle != syscall.InvalidHandle {
		syscall.CloseHandle(h.handle)
		h.handle = syscall.InvalidHandle
	}
	syscall.CloseHandle(h.readEvent)
	syscall.CloseHandle(h.writeEvent)
	return nil
}

// List enumerates the vendor collections and reads their identity strings.
func List() ([]Entry, error) {
	return listVendorInterfaces()
}

// ListJSON returns the device list as JSON, the shape gdb_device_list produces.
func ListJSON() (string, error) {
	entries, err := List()
	if err != nil {
		return "", err
	}
	return jsonString(entries)
}

// Open connects to the device matching selector: a serial number, a HID path,
// or a fragment of a serial. An empty selector takes the first device that
// answers. Candidates are tried in order, so a stale interface entry does not
// hide the real one.
func Open(selector string, opts Options) (*Device, error) {
	entries, err := listVendorInterfaces()
	if err != nil {
		return nil, err
	}
	candidates := matchEntries(entries, selector)
	if len(candidates) == 0 {
		if selector == "" {
			return nil, errorf(Disconnected, "no GDBOX device found")
		}
		return nil, errorf(Disconnected, "no GDBOX device matches %q", selector)
	}
	var lastErr error
	for _, candidate := range candidates {
		path := candidate.Path
		transport, err := openHIDPath(path)
		if err != nil {
			lastErr = err
			continue
		}
		device, err := newDevice(transport, targetPath, candidate.Serial, candidate.Product, opts)
		if err != nil {
			lastErr = err
			continue
		}
		device.reopen = func() (Transport, error) { return openHIDPath(path) }
		return device, nil
	}
	return nil, lastErr
}

// matchEntries orders the devices a selector can mean: exact serial or path
// first, then a serial fragment, then everything (with a serial ahead of an
// interface that never answered its identity strings).
func matchEntries(entries []Entry, selector string) []Entry {
	candidates := make([]Entry, 0, len(entries))
	if selector != "" {
		for _, entry := range entries {
			if entry.Serial == selector || entry.Path == selector {
				candidates = append(candidates, entry)
			}
		}
		if len(candidates) > 0 {
			return candidates
		}
		for _, entry := range entries {
			if entry.Serial != "" && strings.Contains(entry.Serial, selector) {
				candidates = append(candidates, entry)
			}
		}
		return candidates
	}
	for _, entry := range entries {
		if entry.Serial != "" {
			candidates = append(candidates, entry)
		}
	}
	for _, entry := range entries {
		if entry.Serial == "" {
			candidates = append(candidates, entry)
		}
	}
	return candidates
}

// listVendorInterfaces walks the PnP instance ids through cfgmgr32 and turns
// each vendor collection into an interface path.
func listVendorInterfaces() ([]Entry, error) {
	// Filtering by the HID enumerator and matching the VID/PID here keeps the
	// call to the documented flag combination; a device-instance-id filter needs
	// the exact instance and would miss every collection.
	filter, err := syscall.UTF16PtrFromString("HID")
	if err != nil {
		return nil, errorf(Invalid, "device filter: %s", err)
	}
	flags := uintptr(cmGetIDListFilterPresent | cmGetIDListFilterEnumerator)
	var size uint32
	status, _, _ := procDeviceIDListSize.Call(
		uintptr(unsafe.Pointer(&size)),
		uintptr(unsafe.Pointer(filter)),
		flags,
	)
	if status != 0 {
		return nil, errorf(IO, "CM_Get_Device_ID_List_SizeW failed: %d", status)
	}
	if size == 0 {
		return nil, nil
	}
	buffer := make([]uint16, size)
	status, _, _ = procDeviceIDList.Call(
		uintptr(unsafe.Pointer(filter)),
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(size),
		flags,
	)
	if status != 0 {
		return nil, errorf(IO, "CM_Get_Device_ID_ListW failed: %d", status)
	}
	entries := make([]Entry, 0, 4)
	for _, instance := range multiSz(buffer) {
		upper := strings.ToUpper(instance)
		if !strings.Contains(upper, `VID_03F0&PID_0024`) {
			continue
		}
		path, ok := interfacePath(instance)
		if !ok {
			continue
		}
		entry := Entry{Path: path, VID: VendorID, PID: ProductID}
		if info, err := readIdentity(path); err == nil {
			entry.Serial = info.Serial
			entry.Product = info.Product
			entry.Manufacturer = info.Manufacturer
		}
		entries = append(entries, entry)
	}
	return entries, nil
}

// multiSz splits a double-NUL terminated UTF-16 list.
func multiSz(buffer []uint16) []string {
	out := make([]string, 0, 4)
	start := 0
	for index, value := range buffer {
		if value != 0 {
			continue
		}
		if index == start {
			break
		}
		out = append(out, syscall.UTF16ToString(buffer[start:index]))
		start = index + 1
	}
	return out
}

// interfacePath turns a PnP instance id into the device interface path:
// HID\VID_03F0&PID_0024&MI_01\7&2c1b&0&0000 becomes
// \\?\hid#vid_03f0&pid_0024&mi_01#7&2c1b&0&0000#{4d1e55b2-...}.
func interfacePath(instance string) (string, bool) {
	parts := strings.Split(instance, `\`)
	if len(parts) != 3 {
		return "", false
	}
	if !strings.EqualFold(parts[0], "HID") {
		return "", false
	}
	// Only the MI_01 collection is the vendor interface; MI_00 is the keyboard
	// and consumer collection, which answers no v3 frames.
	if !strings.HasSuffix(strings.ToUpper(parts[1]), "&MI_01") {
		return "", false
	}
	return `\\?\hid#` + strings.ToLower(parts[1]) + "#" + strings.ToLower(parts[2]) + "#" + hidInterfaceGUID, true
}

// identity is the serial/product/manufacturer pair read from an open handle.
type identity struct {
	Serial       string
	Product      string
	Manufacturer string
}

// readIdentity opens one path just long enough to read its strings.
func readIdentity(path string) (identity, error) {
	transport, err := openHIDPath(path)
	if err != nil {
		return identity{}, err
	}
	hid := transport.(*hidTransport)
	defer transport.Close()
	return identity{
		Serial:       hid.stringProperty(procHidGetSerialNumber),
		Product:      hid.stringProperty(procHidGetProduct),
		Manufacturer: hid.stringProperty(procHidGetManufacturer),
	}, nil
}

// stringProperty reads one UTF-16 device string property.
func (h *hidTransport) stringProperty(proc *syscall.LazyProc) string {
	buffer := make([]uint16, 128)
	ok, _, _ := proc.Call(
		uintptr(h.handle),
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(len(buffer)*2),
	)
	if ok == 0 {
		return ""
	}
	return syscall.UTF16ToString(buffer)
}

// screenTargetFor resolves one absolute move against the live desktop: which
// rectangle it is anchored to, where the pointer is and every monitor.
func screenTargetFor(screen ScreenTarget) (Target, error) {
	enableDPIAwareness()
	return Resolve(systemDesktop{}, screen)
}

// screenAndCursor reports the primary monitor rectangle and the pointer position
// relative to it, in physical pixels.
func screenAndCursor() (Screen, [2]int32, error) {
	target, err := screenTargetFor(PrimaryScreen())
	if err != nil {
		return Screen{}, [2]int32{}, err
	}
	return Screen{Width: target.Rect.Width(), Height: target.Rect.Height()}, [2]int32{
		target.Cursor[0] - target.Rect.Left,
		target.Cursor[1] - target.Rect.Top,
	}, nil
}

// systemDesktop is the live Windows desktop.
type systemDesktop struct{}

// Monitors reports every monitor rectangle, in EnumDisplayMonitors order.
func (systemDesktop) Monitors() ([]Rect, error) {
	found, err := enumerateMonitors()
	if err != nil {
		return nil, err
	}
	rects := make([]Rect, 0, len(found))
	for _, entry := range found {
		rects = append(rects, entry.rect)
	}
	return rects, nil
}

// Primary is the index of the monitor the system flags as primary. That flag is
// the same rectangle MonitorFromPoint((0,0), MONITOR_DEFAULTTOPRIMARY) answers,
// and it does not assume the primary monitor is index 0.
func (systemDesktop) Primary() (int, error) {
	found, err := enumerateMonitors()
	if err != nil {
		return 0, err
	}
	for index, entry := range found {
		if entry.flags&monitorInfoPrimary != 0 {
			return index, nil
		}
	}
	return 0, nil
}

// Cursor is the pointer in virtual-desktop pixels.
func (systemDesktop) Cursor() ([2]int32, error) {
	var cursor point
	ok, _, err := procPhysicalCursorPos.Call(uintptr(unsafe.Pointer(&cursor)))
	if ok == 0 {
		return [2]int32{}, errorf(Unsupported, "GetPhysicalCursorPos failed: %v", err)
	}
	return [2]int32{cursor.X, cursor.Y}, nil
}

// Window is a window's bounding rectangle in virtual-desktop pixels.
func (systemDesktop) Window(hwnd int) (Rect, error) {
	var bounds rect
	ok, _, err := procGetWindowRect.Call(uintptr(hwnd), uintptr(unsafe.Pointer(&bounds)))
	if ok == 0 {
		return Rect{}, errorf(Unsupported, "GetWindowRect failed for window 0x%x: %v", hwnd, err)
	}
	return Rect{Left: bounds.Left, Top: bounds.Top, Right: bounds.Right, Bottom: bounds.Bottom}, nil
}

// monitorEntry is one monitor rectangle plus its flags.
type monitorEntry struct {
	rect  Rect
	flags uint32
}

var (
	// The enumeration callback writes here: syscall.NewCallback cannot capture a
	// closure per call without leaking a stub every time a move reads the
	// pointer, so the buffer is package level and the mutex serialises readers.
	monitorMu     sync.Mutex
	monitorBuffer []monitorEntry

	// monitorEnumProc collects one monitor for EnumDisplayMonitors.
	monitorEnumProc = syscall.NewCallback(func(monitor, _hdc, _clip, _data uintptr) uintptr {
		info := monitorInfo{}
		info.Size = uint32(unsafe.Sizeof(info))
		ok, _, _ := procMonitorInfo.Call(monitor, uintptr(unsafe.Pointer(&info)))
		if ok != 0 {
			monitorBuffer = append(monitorBuffer, monitorEntry{
				rect: Rect{
					Left:   info.Monitor.Left,
					Top:    info.Monitor.Top,
					Right:  info.Monitor.Right,
					Bottom: info.Monitor.Bottom,
				},
				flags: info.Flags,
			})
		}
		return 1
	})
)

// enumerateMonitors is every monitor rectangle and its flags, in
// EnumDisplayMonitors order.
func enumerateMonitors() ([]monitorEntry, error) {
	monitorMu.Lock()
	defer monitorMu.Unlock()
	monitorBuffer = monitorBuffer[:0]
	ok, _, err := procEnumDisplayMonitors.Call(0, 0, monitorEnumProc, 0)
	if ok == 0 {
		return nil, errorf(Unsupported, "EnumDisplayMonitors failed: %v", err)
	}
	if len(monitorBuffer) == 0 {
		return nil, errorf(Unsupported, "the system reports no monitors")
	}
	return append([]monitorEntry(nil), monitorBuffer...), nil
}

// enableDPIAwareness makes the process per-monitor DPI aware so the cursor and
// the monitor rectangle are both in physical pixels.
func enableDPIAwareness() {
	if procDPIAwareness.Find() != nil {
		return
	}
	procDPIAwareness.Call(dpiAwarenessPerMonitorV2)
}

type point struct{ X, Y int32 }

type rect struct{ Left, Top, Right, Bottom int32 }

type monitorInfo struct {
	Size    uint32
	Monitor rect
	Work    rect
	Flags   uint32
}
