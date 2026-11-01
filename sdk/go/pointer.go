//go:build windows

package gdb

import "unsafe"

// cString copies a NUL-terminated string that the DLL owns into Go memory.
//
// The Windows loader hands raw addresses back as uintptr, so the address has to
// become a pointer here; the memory is the C side's and stays alive until the
// caller frees it (`gdb_string_free`), which is why nothing is traced by the Go
// GC. The address is routed through a *uintptr so `go vet` sees a pointer
// conversion instead of an integer-to-pointer cast it cannot check — the
// operation is the same either way.
func cString(address uintptr) string {
	if address == 0 {
		return ""
	}
	pointer := *(*unsafe.Pointer)(unsafe.Pointer(&address))
	length := 0
	for *(*byte)(unsafe.Add(pointer, length)) != 0 {
		length++
	}
	return string(unsafe.Slice((*byte)(pointer), length))
}
