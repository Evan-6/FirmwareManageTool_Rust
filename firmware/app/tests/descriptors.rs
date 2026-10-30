//! The report descriptors are hand-owned now, so this test is the only thing
//! standing between a careless edit and a device Windows no longer recognises.
//!
//! The hashes were taken from the bytes the C++ sketch compiled (and that the
//! TinyUSB golden agreed with) on 2026-10-31, right before the sketch was deleted.

use xiao_firmware::usb_descriptors::{INPUT_REPORT_DESCRIPTOR, VENDOR_REPORT_DESCRIPTOR};

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

#[test]
fn input_descriptor_is_unchanged() {
    assert_eq!(INPUT_REPORT_DESCRIPTOR.len(), 159);
    assert_eq!(
        fnv1a(INPUT_REPORT_DESCRIPTOR),
        0xae14_000f,
        "input report descriptor changed; update the C++/TinyUSB reference before touching it"
    );
    // Report ids the engine actually writes: NKRO 2, relative mouse 3, consumer 5.
    for id in [0x02, 0x03, 0x05] {
        assert!(
            INPUT_REPORT_DESCRIPTOR
                .windows(2)
                .any(|pair| pair == [0x85, id]),
            "report id {id} missing from the input descriptor"
        );
    }
}

#[test]
fn vendor_descriptor_is_unchanged() {
    assert_eq!(VENDOR_REPORT_DESCRIPTOR.len(), 35);
    assert_eq!(
        fnv1a(VENDOR_REPORT_DESCRIPTOR),
        0xaccd_5e98,
        "vendor report descriptor changed; the host protocol depends on ids 12/13/14"
    );
    // Vendor collection: usage page 0xFF60, usage 0x61.
    assert_eq!(&VENDOR_REPORT_DESCRIPTOR[..2], &[0x06, 0x60]);
    assert_eq!(&VENDOR_REPORT_DESCRIPTOR[2..5], &[0xff, 0x09, 0x61]);
    for id in [0x0c, 0x0d, 0x0e] {
        assert!(
            VENDOR_REPORT_DESCRIPTOR
                .windows(2)
                .any(|pair| pair == [0x85, id]),
            "report id {id} missing from the vendor descriptor"
        );
    }
}
