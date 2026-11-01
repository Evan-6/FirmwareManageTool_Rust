//go:build !windows

package gdbnative

// volumes reports that the bootloader drive can only be found on Windows; the
// rest of the flashing sequence still builds and is covered by the tests.
func volumes() ([]string, error) {
	return nil, errorf(Unsupported, "UF2 磁碟偵測僅支援 Windows")
}
