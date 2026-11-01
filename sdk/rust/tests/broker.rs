//! Headless broker tests: a mock device behind a real named pipe.
//!
//! Every test names its own pipe (`pid` plus a counter) so a parallel `cargo
//! test` never has two brokers on one name.

#![cfg(windows)]

use gdb::broker::{self, Client, Event, Server, Source};
use gdb::event::ConnectionState;
use gdb::{DeviceOptions, ErrorCode, Manager, MotionOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

fn unique(stem: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    format!(
        "test-{}-{}-{stem}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Starts a broker on a mock device: real pipe, real threads, no hardware.
fn serving(name: &str) -> Server {
    let (device, _handle) = Manager::open_mock_with_handle(DeviceOptions::default()).unwrap();
    Server::new(name, Source::Device(device)).unwrap()
}

fn serving_with_mock(name: &str) -> (Server, gdb::mock::MockHandle) {
    let (device, handle) = Manager::open_mock_with_handle(DeviceOptions::default()).unwrap();
    (Server::new(name, Source::Device(device)).unwrap(), handle)
}

/// A raw client: std's own pipe handle, so a test can send bytes no `Client`
/// would ever produce. A busy pipe is retried, as any Windows pipe client must.
fn raw(name: &str) -> std::fs::File {
    let path = broker::pipe_path(name);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return file,
            Err(error) if error.raw_os_error() == Some(231) => {
                if std::time::Instant::now() >= deadline {
                    panic!("the broker pipe stayed busy: {error}");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("cannot open {path}: {error}"),
        }
    }
}

#[test]
fn availability_matches_reality() {
    let name = unique("availability");
    assert!(!Client::available(&name));
    let error = Client::connect(&name).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Disconnected);

    let server = serving(&name);
    assert!(Client::available(&name));
    drop(server);
    assert!(!Client::available(&name));
}

#[test]
fn three_clients_share_one_device_session() {
    let name = unique("three");
    let (server, mock) = serving_with_mock(&name);

    let clients: Vec<Client> = (0..3).map(|_| Client::connect(&name).unwrap()).collect();

    for client in &clients {
        let info = client.info().unwrap();
        assert_eq!(info.serial, "MOCK");
        assert_eq!(info.firmware, "3.2.0");

        let status = client.status().unwrap();
        assert_eq!(status.lease_ms, 2000);

        let sessions = client.sessions().unwrap();
        assert_eq!(sessions.len(), 1, "every client sees the broker's session");
    }

    // Distinct commands, from three different clients, land on one device.
    for (index, client) in clients.iter().enumerate() {
        client.key(7, 0x04 + index as u16, true).unwrap();
    }
    assert_eq!(mock.sessions().len(), 1);
    assert_eq!(mock.held().keys[0] & 0b0111_0000, 0b0111_0000);

    clients[0].barrier().unwrap();
    clients[0].release_all().unwrap();
    assert_eq!(mock.held().keys[0], 0);

    // Replies are matched per client, not by arrival order.
    let statuses: Vec<_> = clients.iter().map(|c| c.status().unwrap()).collect();
    assert_eq!(statuses.len(), 3);
    assert_eq!(server.clients(), 3);
}

#[test]
fn the_wire_answers_typed_calls() {
    let name = unique("typed");
    let (server, mock) = serving_with_mock(&name);
    let client = Client::connect(&name).unwrap();

    client.mouse_move(10, -5).unwrap();
    client.move_raw(800, -300).unwrap();
    client.mouse_buttons(0b0000_0001).unwrap();
    client.wheel(-120, 0).unwrap();
    assert_eq!(mock.held().buttons, 0b0000_0001);

    let status_ext = client.status_ext().unwrap();
    assert_eq!(status_ext.sessions, 1);

    let packet = client
        .command(&input_protocol::v3::Command::Status)
        .unwrap()
        .expect("STATUS has a reply");
    assert_eq!(packet.opcode, input_protocol::v3::STATUS | 0x80);

    let nothing = client
        .command(&input_protocol::v3::Command::MouseMove(1, 1))
        .unwrap();
    assert!(nothing.is_none(), "input commands have no reply packet");

    client.release_all().unwrap();
    assert_eq!(mock.held().buttons, 0);
    assert_eq!(server.name(), name);
    assert_eq!(server.state(), ConnectionState::Connected);
}

#[test]
fn a_client_that_vanishes_mid_command_does_not_take_the_server_down() {
    let name = unique("vanish");
    let server = serving(&name);

    let client = Arc::new(Client::connect(&name).unwrap());
    let worker_client = client.clone();
    let started = std::time::Instant::now();
    let worker = thread::spawn(move || {
        // 8000 px is eight 1024-px reports paced over 800 ms: the close below
        // lands while the broker is still mid-move.
        worker_client.move_by(8000, 0, MotionOptions::linear(800))
    });
    thread::sleep(Duration::from_millis(120));
    client.close();

    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Disconnected);
    assert!(
        error.message.contains("broker"),
        "unexpected message: {}",
        error.message
    );
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "the call must fail when the client leaves, not when the move ends"
    );

    // The broker is still serving, and the device lock was released.
    let other = Client::connect(&name).unwrap();
    assert_eq!(other.info().unwrap().serial, "MOCK");
    assert!(other.status().is_ok());

    // Give the dead client's thread a moment to leave the roster, then confirm
    // no ghost connections remain.
    let mut clients = server.clients();
    for _ in 0..40 {
        if clients <= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
        clients = server.clients();
    }
    assert_eq!(clients, 1);
}

#[test]
fn events_reach_every_client() {
    let name = unique("events");
    let (server, mock) = serving_with_mock(&name);

    let first = Client::connect(&name).unwrap();
    let second = Client::connect(&name).unwrap();
    let first_events = first.events();
    let second_events = second.events();

    // Both were greeted with the current state.
    for receiver in [&first_events, &second_events] {
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Some(Event::State(ConnectionState::Connected))
        );
    }

    // Force the device to drop a write; the SDK reopens and reports it, and the
    // broker fans that out to everyone.
    mock.set_disconnect_after(Some(mock.writes().len()));
    let _ = first.status();

    for receiver in [&first_events, &second_events] {
        let event = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("every client hears about the state change");
        assert!(
            matches!(
                event,
                Event::State(ConnectionState::Connected) | Event::Error(_)
            ),
            "unexpected event: {event:?}"
        );
    }

    // Both are still able to drive the device afterwards.
    assert!(first.status().is_ok());
    assert!(second.status().is_ok());
    assert_eq!(server.clients(), 2);
}

#[test]
fn a_garbage_line_kills_only_that_client() {
    let name = unique("garbage");
    let server = serving(&name);
    let good = Client::connect(&name).unwrap();

    // Garbage: the broker hangs up on this connection and nobody else.
    let mut stream = BufReader::new(raw(&name));
    stream
        .get_mut()
        .write_all(b"this is not json at all\n")
        .unwrap();
    let mut greeting = String::new();
    stream.read_line(&mut greeting).unwrap();
    assert!(greeting.contains("\"event\":\"state\""), "{greeting}");
    let mut leftover = String::new();
    let read = stream.read_line(&mut leftover).unwrap_or(0);
    assert_eq!(read, 0, "the broker must close a garbage connection");
    drop(stream);

    assert!(good.status().is_ok(), "the good client must survive");

    // A well-formed request for an unknown op is answered, not fatal.
    let mut stream = BufReader::new(raw(&name));
    stream
        .get_mut()
        .write_all(b"{\"id\":7,\"op\":\"nope\",\"args\":{}}\n")
        .unwrap();
    let mut reply = String::new();
    let mut found = false;
    for _ in 0..3 {
        reply.clear();
        if stream.read_line(&mut reply).unwrap_or(0) == 0 {
            break;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply)
            && value.get("id").and_then(|id| id.as_u64()) == Some(7)
        {
            assert_eq!(value.get("ok").and_then(|ok| ok.as_bool()), Some(false));
            assert_eq!(
                value["error"]["code"].as_u64(),
                Some(ErrorCode::Unsupported.as_u8() as u64)
            );
            found = true;
            break;
        }
    }
    assert!(found, "an unknown op must be answered: {reply}");
    drop(stream);

    assert!(good.status().is_ok());
    assert!(server.clients() >= 1);
}

#[test]
fn connect_or_spawn_finds_a_running_server() {
    let name = unique("spawn");
    let _server = serving(&name);

    // The executable is never needed: the pipe is already there.
    let missing = std::path::PathBuf::from(r"Z:\nowhere\no-broker.exe");
    let client = Client::connect_or_spawn(&name, &missing).unwrap();
    assert_eq!(client.info().unwrap().serial, "MOCK");
}

#[test]
fn connect_or_spawn_reports_a_timeout_when_nothing_listens() {
    let name = unique("nospawn");
    let missing = std::path::PathBuf::from(r"Z:\nowhere\no-broker.exe");
    let started = std::time::Instant::now();
    let error = Client::connect_or_spawn(&name, &missing).unwrap_err();
    assert_ne!(error.code(), ErrorCode::Ok);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a failed spawn must not hang"
    );
}

#[test]
fn closing_the_server_ends_clients_with_a_clear_error() {
    let name = unique("shutdown");
    let server = serving(&name);
    let client = Client::connect(&name).unwrap();
    let events = client.events();
    assert!(client.status().is_ok());
    assert_eq!(
        events
            .try_recv()
            .or_else(|| events.recv_timeout(Duration::from_secs(1))),
        Some(Event::State(ConnectionState::Connected))
    );

    server.shutdown();

    let error = client.status().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Disconnected);
    assert!(
        error.message.contains("broker"),
        "unexpected message: {}",
        error.message
    );
    assert!(client.is_closed());
    assert_eq!(
        events.recv_timeout(Duration::from_secs(2)),
        Some(Event::Closed)
    );
    assert!(!Client::available(&name));
    assert_eq!(server.clients(), 0);
}
