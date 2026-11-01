package gdbnative

import "sort"

// keyCatalog holds every (page, usage) pair in shared/input-protocol/keys.json,
// packed as page<<16|usage and sorted. It is a straight copy of that file (and
// of keys_generated.rs, which the same script produces): regenerate it from
// keys.json with `python scripts/sync-input-protocol.py`, never by hand, because
// the firmware rejects any usage the catalog does not define.
//
// 129 entries: keyboard usages 4..231 on page 7 and the seven consumer usages
// on page 12.
var keyCatalog = []uint32{
	0x00070004, 0x00070005, 0x00070006, 0x00070007, 0x00070008, 0x00070009,
	0x0007000A, 0x0007000B, 0x0007000C, 0x0007000D, 0x0007000E, 0x0007000F,
	0x00070010, 0x00070011, 0x00070012, 0x00070013, 0x00070014, 0x00070015,
	0x00070016, 0x00070017, 0x00070018, 0x00070019, 0x0007001A, 0x0007001B,
	0x0007001C, 0x0007001D, 0x0007001E, 0x0007001F, 0x00070020, 0x00070021,
	0x00070022, 0x00070023, 0x00070024, 0x00070025, 0x00070026, 0x00070027,
	0x00070028, 0x00070029, 0x0007002A, 0x0007002B, 0x0007002C, 0x0007002D,
	0x0007002E, 0x0007002F, 0x00070030, 0x00070031, 0x00070033, 0x00070034,
	0x00070035, 0x00070036, 0x00070037, 0x00070038, 0x00070039, 0x0007003A,
	0x0007003B, 0x0007003C, 0x0007003D, 0x0007003E, 0x0007003F, 0x00070040,
	0x00070041, 0x00070042, 0x00070043, 0x00070044, 0x00070045, 0x00070046,
	0x00070047, 0x00070048, 0x00070049, 0x0007004A, 0x0007004B, 0x0007004C,
	0x0007004D, 0x0007004E, 0x0007004F, 0x00070050, 0x00070051, 0x00070052,
	0x00070053, 0x00070054, 0x00070055, 0x00070056, 0x00070057, 0x00070058,
	0x00070059, 0x0007005A, 0x0007005B, 0x0007005C, 0x0007005D, 0x0007005E,
	0x0007005F, 0x00070060, 0x00070061, 0x00070062, 0x00070063, 0x00070064,
	0x00070065, 0x00070068, 0x00070069, 0x0007006A, 0x0007006B, 0x0007006C,
	0x0007006D, 0x0007006E, 0x0007006F, 0x00070070, 0x00070071, 0x00070072,
	0x00070073, 0x00070087, 0x00070088, 0x00070089, 0x0007008A, 0x0007008B,
	0x000700E0, 0x000700E1, 0x000700E2, 0x000700E3, 0x000700E4, 0x000700E5,
	0x000700E6, 0x000700E7, 0x000C00B5, 0x000C00B6, 0x000C00B7, 0x000C00CD,
	0x000C00E2, 0x000C00E9, 0x000C00EA,
}

// consumers is the consumer-page order of the snapshot bits (protocol-v3.md).
var consumers = [7]uint16{0xE2, 0xE9, 0xEA, 0xCD, 0xB5, 0xB6, 0xB7}

// ValidKey reports whether the catalog defines one usage.
func ValidKey(page, usage uint16) bool {
	wanted := uint32(page)<<16 | uint32(usage)
	index := sort.Search(len(keyCatalog), func(i int) bool { return keyCatalog[i] >= wanted })
	return index < len(keyCatalog) && keyCatalog[index] == wanted
}

// consumerIndex is the bit position of a consumer usage in the snapshot.
func consumerIndex(usage uint16) (uint8, bool) {
	for index, candidate := range consumers {
		if candidate == usage {
			return uint8(index), true
		}
	}
	return 0, false
}
