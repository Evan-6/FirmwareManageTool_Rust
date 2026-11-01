//go:build windows

package gdbnative

import (
	"strings"
	"syscall"
	"unsafe"
)

// Bootloader volume detection, on kernel32 directly: the RP2040 bootloader
// mounts as a FAT volume labelled RPI-RP2, so the label of every logical drive
// is the whole test. No PowerShell, no WMI — this runs while a user waits for a
// reboot.

var (
	procGetLogicalDrives     = kernel32DLL.NewProc("GetLogicalDrives")
	procGetVolumeInformation = kernel32DLL.NewProc("GetVolumeInformationW")
)

// bootloaderLabel is the volume label the RP2040 bootloader publishes.
const bootloaderLabel = "RPI-RP2"

// volumes reports every mounted RP2040 bootloader drive root, as "E:\".
func volumes() ([]string, error) {
	mask, _, callErr := procGetLogicalDrives.Call()
	if mask == 0 {
		return nil, errorf(IO, "GetLogicalDrives failed: %v", callErr)
	}
	drives := make([]string, 0, 2)
	for bit := 0; bit < 26; bit++ {
		if mask&(1<<uint(bit)) == 0 {
			continue
		}
		root := string(rune('A'+bit)) + `:\`
		// An empty card reader or a volume that will not answer is skipped:
		// it cannot be the board.
		label, err := volumeLabel(root)
		if err != nil || !strings.EqualFold(label, bootloaderLabel) {
			continue
		}
		drives = append(drives, root)
	}
	return drives, nil
}

// volumeLabel is GetVolumeInformationW's lpVolumeNameBuffer, trimmed.
func volumeLabel(root string) (string, error) {
	name, err := syscall.UTF16PtrFromString(root)
	if err != nil {
		return "", errorf(Invalid, "volume root %q: %s", root, err)
	}
	buffer := make([]uint16, 256)
	ok, _, callErr := procGetVolumeInformation.Call(
		uintptr(unsafe.Pointer(name)),
		0, // lpVolumeNameBuffer
		0, // nVolumeNameSize
		0, // lpVolumeSerialNumber
		0, // lpMaximumComponentLength
		0, // lpFileSystemFlags
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(len(buffer)*2),
	)
	if ok == 0 {
		return "", errorf(IO, "GetVolumeInformationW(%s) failed: %v", root, callErr)
	}
	return syscall.UTF16ToString(buffer), nil
}
