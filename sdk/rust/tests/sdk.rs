//! End-to-end SDK tests over the in-memory device: no hardware required.

use gdb::manager::Manager;
use gdb::{Button, ConnectionState, DeviceOptions, MotionOptions, TypeTextConfig};

fn open() -> (gdb::Device, gdb::mock::MockHandle) {
    Manager::open_mock_with_handle(DeviceOptions::default()).expect("mock device")
}

#[test]
fn connects_reports_info_and_closes() {
    let (device, handle) = open();
    let info = device.info();
    assert_eq!(info.board, 2);
    assert_eq!(info.firmware, "3.2.0");
    assert_eq!(info.lease_ms, 2000);
    assert_eq!(info.poll_ms, 1);
    assert_eq!(info.capabilities, 15);
    assert_eq!(device.state(), ConnectionState::Connected);

    let status = device.status().expect("status");
    assert_eq!(status.lease_ms, 2000);
    let ext = device.status_ext().expect("status_ext");
    assert_eq!(ext.build, 0x0DB0_0001);
    assert_eq!(ext.boot_count, 1);
    let owners = device.sessions().expect("sessions");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].id, handle.session() as u16);

    device.close().expect("close");
    assert!(handle.is_closed());
    assert_eq!(device.state(), ConnectionState::Disconnected);
}

#[test]
fn keys_chords_and_text_reach_the_device() {
    let (device, handle) = open();
    let keyboard = device.keyboard();
    keyboard.key_down(7, 0x04).unwrap();
    assert!(handle.held().keys[0] & 0x10 != 0);
    keyboard.key_up(7, 0x04).unwrap();
    assert_eq!(handle.held().keys[0] & 0x10, 0);

    keyboard.chord(&[0xE0, 0x06], 10).unwrap();
    // The chord is synchronous: both keys go down and come back up.
    let downs: Vec<u16> = handle
        .writes()
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::KEY && packet.payload[4] == 1)
        .map(|packet| u16::from_le_bytes([packet.payload[2], packet.payload[3]]))
        .collect();
    assert!(downs.contains(&0x06), "{downs:?}");
    assert_eq!(handle.held().keys[0] & 0x40, 0);

    keyboard
        .type_text(&TypeTextConfig {
            text: "hi".to_string(),
            interval_ms: 1,
            hold_ms: 1,
            jitter_ms: 0,
        })
        .unwrap();
    assert_eq!(
        handle.held().keys.iter().filter(|byte| **byte != 0).count(),
        0
    );

    keyboard.long_press(7, 0x2C, 5).unwrap();
    assert_eq!(handle.held().keys[5] & 0x10, 0);
}

#[test]
fn relative_moves_are_split_into_hid_sized_reports() {
    let (device, handle) = open();
    let before = handle.writes().len();
    device
        .mouse()
        .move_by(1500, -1100, MotionOptions::linear(40))
        .unwrap();
    let moves: Vec<_> = handle.writes()[before..]
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::MOUSE_MOVE)
        .map(|packet| {
            (
                i16::from_le_bytes([packet.payload[0], packet.payload[1]]),
                i16::from_le_bytes([packet.payload[2], packet.payload[3]]),
            )
        })
        .collect();
    assert_eq!(moves, vec![(1024, -1024), (476, -76)]);
}

#[test]
fn clicks_drags_and_release_all() {
    let (device, handle) = open();
    let mouse = device.mouse();
    mouse.click(Button::Left, 2).unwrap();
    let buttons: Vec<u8> = handle
        .writes()
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::MOUSE_BUTTONS)
        .map(|packet| packet.payload[0])
        .collect();
    assert_eq!(buttons, vec![1, 0, 1, 0]);

    mouse.button(Button::Right, true).unwrap();
    assert_eq!(handle.held().buttons, 2);
    mouse.release_all().unwrap();
    assert_eq!(handle.held().buttons, 0);
}

#[test]
fn a_failed_operation_reconnects_once_and_keeps_working() {
    let (device, handle) = open();
    let writes = handle.writes().len();
    // Fail the next write, then let the fresh mock accept everything.
    handle.set_disconnect_after(Some(writes));
    let status = device.status().expect("auto reconnect");
    assert_eq!(status.lease_ms, 2000);
    assert_eq!(device.state(), ConnectionState::Connected);
}

#[test]
fn no_hardware_is_required_for_the_mock() {
    let (device, _handle) = open();
    assert_eq!(device.info().serial, "MOCK");
    assert_eq!(device.info().product, "GDBOX mock");
}
