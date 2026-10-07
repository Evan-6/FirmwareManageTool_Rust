use super::*;
struct BinaryFake(input_protocol::testing::UsbModel);
impl HidTransport for BinaryFake {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        Ok(Some(input_protocol::testing::UsbModel::feature()))
    }
    fn write(&mut self, r: &[u8]) -> Result<usize> {
        self.0.write(r)
    }
    fn read_timeout(&mut self, r: &mut [u8], _: i32) -> Result<usize> {
        Ok(self.0.read(r))
    }
}
#[test]
fn binary_session_negotiates_and_preserves_physical_keys_without_ack() {
    use input_protocol::{KeyId, v3};
    let cancel = Cancellation::default();
    let mut s = Session::new(BinaryFake(Default::default()));
    s.synchronize(&cancel).unwrap();
    assert_eq!(s.hello(&cancel).unwrap().protocol, 3);
    for token in [
        "1",
        "kp_1",
        "enter",
        "kp_enter",
        "a",
        "b",
        "c",
        "d",
        "e",
        "f",
        "g",
        "volume_up",
    ] {
        s.key(KeyId::from_token(token).unwrap(), true, &cancel)
            .unwrap();
    }
    assert_eq!(s.transport.0.state.pressed().len(), 12);
    assert!(!s.transport.0.writes.iter().any(|p| p.opcode == v3::BARRIER));
    s.finish(&cancel).unwrap();
    assert_eq!(s.transport.0.writes.last().unwrap().opcode, v3::BARRIER);
    s.reset(&cancel).unwrap();
    assert!(s.status(&cancel).unwrap().released());
}
#[test]
fn binary_fault_short_write_timeout_and_cancel_invalidate() {
    use input_protocol::{KeyId, v3};
    let c = Cancellation::default();
    for kind in 0..4 {
        let mut s = Session::new(BinaryFake(Default::default()));
        s.synchronize(&c).unwrap();
        let result = match kind {
            0 => {
                s.transport.0.fault(5);
                s.poll()
            }
            1 => {
                s.transport.0.short = true;
                s.key(KeyId::from_token("a").unwrap(), true, &c)
            }
            2 => {
                s.transport.0.silent = true;
                s.finish(&c)
            }
            _ => {
                let token = Cancellation::default();
                token.cancel();
                s.input(v3::Command::Barrier, &token)
            }
        };
        assert!(result.is_err());
        // Cancellation before a write need not invalidate an untouched connection.
        if kind != 3 {
            assert!(!s.is_valid());
        }
    }
}

#[test]
fn idle_wait_renews_binary_lease_without_an_input_ack() {
    let cancel = Cancellation::default();
    let mut session = Session::new(BinaryFake(Default::default()));
    session.synchronize(&cancel).unwrap();
    session
        .wait_until(Instant::now() + Duration::from_millis(510), &cancel)
        .unwrap();
    assert_eq!(
        session.transport.0.writes.last().unwrap().opcode,
        input_protocol::v3::HEARTBEAT
    );
    session.finish(&cancel).unwrap();
}

#[test]
fn binary_shared_reports_ignore_other_clients_replies_and_faults() {
    use input_protocol::v3::{self, Packet};
    let cancel = Cancellation::default();
    let mut session = Session::new(BinaryFake(Default::default()));
    session.synchronize(&cancel).unwrap();
    let own_id = session.binary.as_ref().unwrap().session;
    for opcode in [v3::EVENT, v3::BARRIER | 128, v3::OPEN | 128] {
        session.transport.0.reads.push_back(
            Packet {
                opcode,
                flags: 0,
                session: own_id.wrapping_add(1),
                sequence: 2,
                payload: vec![5],
            }
            .encode(v3::IN_ID)
            .unwrap(),
        );
    }
    session.finish(&cancel).unwrap();
    assert!(session.is_valid());
    session.transport.0.fault(5);
    assert!(session.poll().is_err());
    assert!(!session.is_valid());
}

// An expired GUI/peer session raises the device-global failsafe counter while
// the latency worker's own session remains healthy. Exercise real v3 I/O and
// the status check used by both latency and mouse operations.
struct PeerExpiryFake {
    inner: BinaryFake,
    failsafe: u16,
    tx_errors: u16,
    hid_errors: u16,
}
impl HidTransport for PeerExpiryFake {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        self.inner.feature()
    }
    fn write(&mut self, r: &[u8]) -> Result<usize> {
        self.inner.write(r)
    }
    fn read_timeout(&mut self, r: &mut [u8], timeout: i32) -> Result<usize> {
        use input_protocol::v3;
        let n = self.inner.read_timeout(r, timeout)?;
        if n > 0 {
            let mut packet = v3::Packet::decode(&r[..n], v3::IN_ID)?;
            if packet.opcode == v3::STATUS | 128 {
                packet.payload[14..16].copy_from_slice(&self.tx_errors.to_le_bytes());
                packet.payload[16..18].copy_from_slice(&self.hid_errors.to_le_bytes());
                packet.payload[18..20].copy_from_slice(&self.failsafe.to_le_bytes());
                r[..64].copy_from_slice(&packet.encode(v3::IN_ID)?);
            }
        }
        Ok(n)
    }
}
#[test]
fn peer_lease_expiry_does_not_fail_an_active_measurement() {
    use input_protocol::v3::{self, Packet};
    let cancel = Cancellation::default();
    let mut session = Session::new(PeerExpiryFake {
        inner: BinaryFake(Default::default()),
        failsafe: 0,
        tx_errors: 0,
        hid_errors: 0,
    });
    session.synchronize(&cancel).unwrap();
    let before = session.status(&cancel).unwrap();
    let own_id = session.binary.as_ref().unwrap().session;
    session.transport.inner.0.reads.push_back(
        Packet {
            opcode: v3::EVENT,
            flags: 0,
            session: own_id.wrapping_add(1),
            sequence: 3,
            payload: vec![5],
        }
        .encode(v3::IN_ID)
        .unwrap(),
    );
    session.transport.failsafe = 1;
    session.finish(&cancel).unwrap();
    let after = session.status(&cancel).unwrap();
    assert_eq!(after.count("fs"), before.count("fs") + 1);
    after.check_transport_since(&before).unwrap();
    assert!(session.is_valid());

    // Shared transport errors and an own-session lease fault still fail.
    session.transport.tx_errors = 1;
    assert!(
        session
            .status(&cancel)
            .unwrap()
            .check_transport_since(&before)
            .is_err()
    );
    session.transport.tx_errors = 0;
    session.transport.hid_errors = 1;
    assert!(
        session
            .status(&cancel)
            .unwrap()
            .check_transport_since(&before)
            .is_err()
    );
    session.transport.inner.0.fault(5);
    assert!(session.status(&cancel).is_err());
    assert!(!session.is_valid());
}

struct Unsupported;
impl HidTransport for Unsupported {
    fn write(&mut self, _: &[u8]) -> Result<usize> {
        panic!("unsupported device must not receive commands")
    }
    fn read_timeout(&mut self, _: &mut [u8], _: i32) -> Result<usize> {
        Ok(0)
    }
}
#[test]
fn missing_v3_feature_rejects_without_sending_input() {
    let mut s = Session::new(Unsupported);
    assert!(
        s.synchronize(&Cancellation::default())
            .unwrap_err()
            .to_string()
            .contains("不支援")
    );
    assert!(!s.is_valid());
}
#[test]
fn buttons_use_standard_left_right_middle_and_side_bits() {
    let token = Cancellation::default();
    let mut s = Session::new(BinaryFake(Default::default()));
    s.synchronize(&token).unwrap();
    for n in 1..=5 {
        s.set_button(n, true, &token).unwrap();
        assert_eq!(s.transport.0.state.buttons, 1 << (n - 1));
        s.set_button(n, false, &token).unwrap();
    }
}
