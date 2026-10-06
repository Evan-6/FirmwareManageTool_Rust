use super::*;
pub struct Fake {
    pub reads: VecDeque<Vec<u8>>,
    pub writes: Vec<Vec<u8>>,
    pub short: bool,
    pub reads_done: usize,
}
impl Fake {
    pub fn lines(lines: &[&str]) -> Self {
        Self {
            reads: lines
                .iter()
                .map(|line| {
                    let mut r = vec![protocol::IN_ID];
                    r.extend_from_slice(format!("{line}\n").as_bytes());
                    r
                })
                .collect(),
            writes: Vec::new(),
            short: false,
            reads_done: 0,
        }
    }
}
impl HidTransport for Fake {
    fn write(&mut self, report: &[u8]) -> Result<usize> {
        self.writes.push(report.to_vec());
        Ok(if self.short { 1 } else { report.len() })
    }
    fn read_timeout(&mut self, buffer: &mut [u8], timeout: i32) -> Result<usize> {
        if self.reads_done >= self.writes.len() {
            return Ok(0);
        }
        if let Some(report) = self.reads.pop_front() {
            self.reads_done += 1;
            buffer[..report.len()].copy_from_slice(&report);
            Ok(report.len())
        } else {
            std::thread::sleep(Duration::from_millis(timeout.max(0) as u64));
            Ok(0)
        }
    }
}
#[test]
fn short_write_invalidates() {
    let mut fake = Fake::lines(&[]);
    fake.short = true;
    let mut s = Session::new(fake);
    assert!(s.queue("ping", "pong", &Cancellation::default()).is_err());
    assert!(!s.is_valid());
}
#[test]
fn final_error_and_failsafe_are_preserved() {
    for line in ["err:hid_send_failed", "warn:failsafe_release_all"] {
        let mut s = Session::new(Fake::lines(&[line]));
        assert!(s.command("ping", "pong", &Cancellation::default()).is_err());
        assert_eq!(s.take_events()[0].line, line);
        assert!(!s.is_valid());
    }
}
#[test]
fn ordered_ack_and_last_drain() {
    let mut s = Session::new(Fake::lines(&["ok:mouse_move", "ok:mouse_move"]));
    let c = Cancellation::default();
    s.queue("mouse_move:1,1", "ok:mouse_move", &c).unwrap();
    s.queue("mouse_move:2,2", "ok:mouse_move", &c).unwrap();
    s.finish(&c).unwrap();
    assert_eq!(s.take_events().len(), 2);
}
#[test]
fn timeout_and_cancel_are_bounded() {
    let mut s = Session::new(Fake::lines(&[]));
    let c = Cancellation::default();
    s.queue("ping", "pong", &c).unwrap();
    c.cancel();
    assert!(s.finish(&c).is_err());
    assert!(!s.is_valid());
    let mut s = Session::new(Fake::lines(&[]));
    let start = Instant::now();
    assert!(s.command("ping", "pong", &Cancellation::default()).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}
struct Fragmented {
    writes: usize,
    reads: usize,
}
impl HidTransport for Fragmented {
    fn write(&mut self, report: &[u8]) -> Result<usize> {
        self.writes += 1;
        Ok(report.len())
    }
    fn read_timeout(&mut self, buffer: &mut [u8], _: i32) -> Result<usize> {
        let data: &[u8] = if self.writes == 1 && self.reads == 0 {
            self.reads += 1;
            b"\x0bok:mo"
        } else if self.writes == 2 && self.reads == 1 {
            self.reads += 1;
            b"\x0buse_move\r\nok:mouse_move\n"
        } else {
            return Ok(0);
        };
        buffer[..data.len()].copy_from_slice(data);
        Ok(data.len())
    }
}
#[test]
fn session_handles_split_and_combined_delayed_ack() {
    let mut s = Session::new(Fragmented {
        writes: 0,
        reads: 0,
    });
    let token = Cancellation::default();
    s.queue("mouse_move:1,1", "ok:mouse_move", &token).unwrap();
    s.queue("mouse_move:2,2", "ok:mouse_move", &token).unwrap();
    s.finish(&token).unwrap();
    assert_eq!(
        s.take_events()
            .iter()
            .map(|e| e.line.as_str())
            .collect::<Vec<_>>(),
        ["ok:mouse_move", "ok:mouse_move"]
    );
}
struct Disconnected;
impl HidTransport for Disconnected {
    fn write(&mut self, _: &[u8]) -> Result<usize> {
        Ok(64)
    }
    fn read_timeout(&mut self, _: &mut [u8], _: i32) -> Result<usize> {
        anyhow::bail!("device disconnected")
    }
}
#[test]
fn disconnect_invalidates_and_a_fresh_connection_resets() {
    let token = Cancellation::default();
    let mut old = Session::new(Disconnected);
    assert!(old.command("ping", "pong", &token).is_err());
    assert!(!old.is_valid());
    let mut fresh = Session::new(Fake::lines(&[
        "ok:release_all",
        "ok:status,p=0,tx=0,hid=0,fs=0,mb=00",
    ]));
    fresh.reset(&token).unwrap();
    assert!(fresh.is_valid());
}
struct Startup {
    reads: VecDeque<Vec<u8>>,
}
impl HidTransport for Startup {
    fn write(&mut self, report: &[u8]) -> Result<usize> {
        let line = if report[1..].starts_with(b"reset") {
            b"\x0bok:mouse_move\nok:release_all\n".as_slice()
        } else {
            b"\x0bok:status,p=0,tx=0,hid=0,fs=0,mb=00\n".as_slice()
        };
        self.reads.push_back(line.to_vec());
        Ok(report.len())
    }
    fn read_timeout(&mut self, buffer: &mut [u8], _: i32) -> Result<usize> {
        if let Some(report) = self.reads.pop_front() {
            buffer[..report.len()].copy_from_slice(&report);
            Ok(report.len())
        } else {
            Ok(0)
        }
    }
}
#[test]
fn reconnect_discards_old_fragments_and_does_not_mistake_move_ack_for_reset() {
    let mut session = Session::new(Startup {
        reads: VecDeque::from([b"\x0bold partial".to_vec()]),
    });
    session.synchronize(&Cancellation::default()).unwrap();
    assert!(session.is_valid());
    let events = session.take_events();
    assert!(events.iter().any(|e| e.line == "ok:mouse_move"));
    assert!(events.iter().any(|e| e.line == "ok:release_all"));
}

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
