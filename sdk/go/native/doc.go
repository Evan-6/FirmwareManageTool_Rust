// Package gdbnative is a native Go implementation of the GDBOX SDK.
//
// It speaks the vendor HID protocol itself — Feature 14 handshake, OPEN, the
// sequenced command stream, reply matching and the lease heartbeat — instead of
// loading gdb.dll through syscall.NewLazyDLL the way the package gdb binding
// does. The wire format is the one in firmware/protocol-v3.md, shared by
// shared/input-protocol.
//
//	device, err := gdbnative.OpenMock()
//	if err != nil { log.Fatal(err) }
//	defer device.Close()
//	fmt.Println(device.InfoJSON())
//	device.Keyboard().TypeText("hello", 45, 18, 30)
//
// Everything above the Transport interface is portable and headless: tests run
// against MockTransport, which implements the firmware's reply timing, error
// bytes and input state the same way sdk/rust/src/mock.rs does. The Windows HID
// transport lives in transport_windows.go and is the only file that touches a
// DLL; on other platforms the package still builds and the mock still works.
//
// Logging is off by default and process-wide, like sdk/rust/src/log.rs:
// SetLogLevel(LogTrace) with a SetLogSink callback produces the same
// "out opcode=… session=… sequence=… payload=[…]" lines the Rust SDK writes.
// Device.Command is the raw escape hatch: same session and sequence numbering,
// no bookkeeping. Device.SetHandler installs the three callbacks of
// sdk/rust/src/event.rs (state, status, error) next to the single-function
// SetEventHandler.
//
// The flashing sequence of sdk/rust/src/flash.rs lives in flash.go: Build runs
// cargo through the Runner seam and packs the ELF with UFPack (uf2.go), Push
// sends BOOTLOADER, waits for the RPI-RP2 volume, copies and verifies the board
// through a second enumeration. Both take a Cancel token and a Progress sink,
// so no GUI is required and no test needs a drive.
//
// Three files are generated or captured rather than written by hand:
// keys_gen.go (from shared/input-protocol/keys.json), testdata/motion.json
// (node lists captured from sdk/motion/src/lib.rs) and the wire vectors the
// tests read from shared/input-protocol/vectors.json.
package gdbnative
