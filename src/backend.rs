use crate::{
    cancel::Cancellation,
    firmware::{self, UploadConfig, UploadObserver, UploadOutcome},
    hid::{self, Device, Session},
    latency::{self, LatencyConfig, Mode, Report, Round},
    mouse,
    process::SystemRunner,
    protocol::{Hello, Status},
};
use anyhow::{Result, bail};
use crossbeam_channel::{Receiver, Sender};
use input_protocol::v3::Command;
use rand::Rng;
use std::time::{Duration, Instant};

/// How often a connected device is asked for STATUS while nothing else happens.
/// Five samples per second is enough to watch counters move without stealing
/// bandwidth from the 1 ms input endpoint.
const STATUS_POLL: Duration = Duration::from_millis(150);
/// Diagnostics move slower than the counters: once a second is enough to watch
/// uptime, queue depth and the last fault without adding traffic to the 1 ms poll.
const EXT_POLL: Duration = Duration::from_millis(1000);

#[derive(Debug)]
pub enum AppCommand {
    Scan,
    Connect(Device),
    Disconnect,
    Status,
    Reset,
    Upload(UploadConfig),
    Latency(LatencyConfig),
    Mouse(crate::mouse::MouseRequest),
    TypeText(crate::typing::TypeTextConfig),
    /// Game mode off/on: when disabled the worker stops polling the device and
    /// only keeps the 500 ms heartbeat alive.
    SetDebug(bool),
    Chord(crate::typing::ChordConfig),
}
#[derive(Debug)]
pub enum AppEvent {
    Devices(Vec<Device>),
    Connected(Device, Hello),
    Disconnected,
    Status(Status),
    /// STATUS_EXT block; absent when the device did not answer the query.
    Ext(input_protocol::v3::StatusExt),
    /// SESSION_LIST: one entry per connected owner.
    Sessions(Vec<input_protocol::v3::SessionInfo>),
    Log(String),
    Error(String),
    Progress(usize, String),
    BootChoices(Vec<String>),
    Round(Round),
    Report(Report),
    Upload(UploadOutcome),
    Cleanup(bool),
    Done,
    Stopped,
}
struct Work {
    command: AppCommand,
    cancel: Cancellation,
}
pub struct Backend {
    commands: Sender<Work>,
    pub events: Receiver<AppEvent>,
    pub choices: Sender<String>,
    cancel: Option<Cancellation>,
    exit: Cancellation,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Backend {
    pub fn start() -> Self {
        let (commands, rx) = crossbeam_channel::bounded(1);
        let (events, event_rx) = crossbeam_channel::unbounded();
        let (choices, choice_rx) = crossbeam_channel::bounded(1);
        let exit = Cancellation::default();
        let token = exit.clone();
        let join = std::thread::spawn(move || worker(rx, events, choice_rx, token));
        Self {
            commands,
            events: event_rx,
            choices,
            cancel: None,
            exit,
            join: Some(join),
        }
    }
    pub fn submit(&mut self, command: AppCommand) -> Result<()> {
        let cancel = Cancellation::default();
        self.commands
            .try_send(Work {
                command,
                cancel: cancel.clone(),
            })
            .map_err(|_| anyhow::anyhow!("背景工作忙碌"))?;
        self.cancel = Some(cancel);
        Ok(())
    }
    pub fn cancel(&self) {
        if let Some(token) = &self.cancel {
            token.cancel();
        }
    }
    pub fn shutdown(&self) {
        self.cancel();
        self.exit.cancel();
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(join) = self.join.take() {
            // Wait for the worker's final cleanup instead of detaching it,
            // matching the GUI close grace period.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !join.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if join.is_finished() {
                let _ = join.join();
            }
        }
    }
}
struct Observer<'a> {
    events: &'a Sender<AppEvent>,
    choices: &'a Receiver<String>,
    stage: usize,
}
impl UploadObserver for Observer<'_> {
    fn log(&mut self, line: String) {
        // Every upload line carries its stage, so a failure says *where* it
        // stopped instead of leaving the user to guess from a wall of frames.
        let _ = self
            .events
            .send(AppEvent::Log(format!("[燒錄 {}] {line}", self.stage)));
    }
    fn progress(&mut self, stage: usize, message: &str) {
        self.stage = stage;
        let _ = self.events.send(AppEvent::Progress(stage, message.into()));
        let _ = self
            .events
            .send(AppEvent::Log(format!("[燒錄 {stage}] {message}")));
    }
    fn choose_target(&mut self, candidates: Vec<String>, cancel: &Cancellation) -> Result<String> {
        for _ in self.choices.try_iter() {}
        let _ = self.events.send(AppEvent::BootChoices(candidates.clone()));
        loop {
            cancel.check()?;
            match self.choices.recv_timeout(Duration::from_millis(50)) {
                Ok(choice) => {
                    if candidates.contains(&choice) {
                        return Ok(choice);
                    }
                    bail!("選擇的 bootloader 不在候選清單");
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(_) => bail!("GUI 已關閉"),
            }
        }
    }
}
fn require(session: &mut Option<Session>) -> Result<&mut Session> {
    session
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("請先選取裝置並連線"))
}
fn reconnect_device(old: &Device) -> Result<Device> {
    let devices = hid::enumerate()?;
    if let Some(device) = devices.iter().find(|d| d.path == old.path) {
        return Ok(device.clone());
    }
    let matches: Vec<_> = devices
        .into_iter()
        .filter(|d| {
            !old.serial.is_empty() && d.serial == old.serial && d.vid == old.vid && d.pid == old.pid
        })
        .collect();
    if matches.len() == 1 {
        Ok(matches[0].clone())
    } else {
        bail!("無法唯一識別重新連接的裝置；請重新選取")
    }
}
fn recover(
    selected: &mut Option<Device>,
    session: &mut Option<Session>,
    events: &Sender<AppEvent>,
    debug: bool,
) {
    log_responses(session, events);
    drop(session.take());
    let result = (|| -> Result<()> {
        let device = reconnect_device(
            selected
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("未選取裝置"))?,
        )?;
        let mut reopened = latency::recover_reset(&device)?;
        reopened.set_trace(debug);
        let hello = reopened.hello(&Cancellation::default())?;
        let status = reopened.status(&Cancellation::default())?;
        *selected = Some(device.clone());
        *session = Some(reopened);
        let _ = events.send(AppEvent::Connected(device, hello));
        let _ = events.send(AppEvent::Status(status));
        Ok(())
    })();
    let ok = result.is_ok();
    let _ = events.send(AppEvent::Cleanup(ok));
    if let Err(e) = result {
        let _ = events.send(AppEvent::Log(format!(
            "釋放狀態未知：{e:#}；裝置仍保留 30 秒 failsafe 租約"
        )));
        let _ = events.send(AppEvent::Disconnected);
    }
}
fn log_responses(session: &mut Option<Session>, events: &Sender<AppEvent>) {
    if let Some(session) = session {
        for reply in session.take_events() {
            let _ = events.send(AppEvent::Log(format!(
                "HID {}ms: {}",
                reply.unix_ms, reply.line
            )));
        }
    }
}
fn worker(
    commands: Receiver<Work>,
    events: Sender<AppEvent>,
    choices: Receiver<String>,
    exit: Cancellation,
) {
    let mut selected = None;
    let mut session: Option<Session> = None;
    let mut held = false;
    let mut debug = true;
    let mut heartbeat = Instant::now();
    let mut polled = Instant::now();
    let mut ext_polled = Instant::now();
    // Consecutive idle-poll failures; one is a hiccup, two mean the session is gone.
    let mut misses = 0u32;
    while !exit.is_cancelled() {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(work) => {
                let result = (|| -> Result<()> {
                    match work.command {
                        AppCommand::Scan => {
                            let _ = events.send(AppEvent::Devices(hid::enumerate()?));
                        }
                        AppCommand::Connect(device) => {
                            if let Some(active) = session.as_mut() {
                                let _ = active.reset(&Cancellation::default());
                                let _ = active.close(&Cancellation::default());
                            }
                            drop(session.take());
                            held = false;
                            selected = Some(device.clone());
                            let mut opened = hid::open(&device)?;
                            opened.synchronize(&work.cancel)?;
                            opened.set_trace(debug);
                            let hello = opened.hello(&work.cancel)?;
                            let status = opened.status(&work.cancel)?;
                            let _ = events.send(AppEvent::Connected(device, hello));
                            let _ = events.send(AppEvent::Status(status));
                            session = Some(opened);
                        }
                        AppCommand::Disconnect => {
                            // RELEASE_ALL proves the keys are physically up, CLOSE then
                            // frees the slot so the next program does not have to wait
                            // for the 2 s lease.
                            let confirmed = match session.as_mut() {
                                Some(active) => {
                                    let confirmed = active.reset(&Cancellation::default()).is_ok();
                                    let _ = active.close(&Cancellation::default());
                                    confirmed
                                }
                                None => true,
                            };
                            drop(session.take());
                            selected = None;
                            held = false;
                            let _ = events.send(AppEvent::Cleanup(confirmed));
                            let _ = events.send(AppEvent::Disconnected);
                        }
                        AppCommand::Status => {
                            let status = require(&mut session)?.status(&work.cancel)?;
                            held = status.buttons() != 0;
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::Reset if session.is_none() => {
                            // The emergency-release hotkey can fire with no session;
                            // nothing is held, so this is not an error.
                            let _ = events.send(AppEvent::Log("沒有連線，無需放開".into()));
                        }
                        AppCommand::Reset => {
                            require(&mut session)?.reset(&Cancellation::default())?;
                            held = false;
                            let _ = events.send(AppEvent::Cleanup(true));
                            let status = require(&mut session)?.status(&Cancellation::default())?;
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::Mouse(request) => {
                            let status = mouse::execute(
                                require(&mut session)?,
                                &request.action,
                                &request.profile,
                                &work.cancel,
                            )?;
                            held = status.buttons() != 0;
                            heartbeat = Instant::now();
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::SetDebug(enabled) => {
                            debug = enabled;
                            // Packet log = the SDK's frame trace; off in game mode
                            // so the latency path does not format every frame.
                            if let Some(active) = session.as_mut() {
                                active.set_trace(enabled);
                            }
                            let _ = events.send(AppEvent::Log(format!(
                                "除錯模式{}：{}",
                                if enabled { "開啟" } else { "關閉" },
                                if enabled {
                                    "150 ms STATUS、1 s 診斷、每行 HID 都送給介面"
                                } else {
                                    "只保留 500 ms 心跳，其他輪詢全部停止"
                                }
                            )));
                        }
                        AppCommand::TypeText(config) => {
                            let active = require(&mut session)?;
                            let strokes = crate::typing::strokes(&config.text);
                            let mut rng = rand::rng();
                            let hold = Duration::from_millis(u64::from(config.hold_ms.max(8)));
                            for stroke in strokes {
                                if stroke.shift {
                                    active.input(
                                        Command::Key(crate::typing::LEFT_SHIFT, true),
                                        &work.cancel,
                                    )?;
                                }
                                active.input(Command::Key(stroke.key, true), &work.cancel)?;
                                work.cancel.sleep(hold)?;
                                active.input(Command::Key(stroke.key, false), &work.cancel)?;
                                if stroke.shift {
                                    active.input(
                                        Command::Key(crate::typing::LEFT_SHIFT, false),
                                        &work.cancel,
                                    )?;
                                }
                                let jitter = if config.jitter_ms > 0 {
                                    u64::from(rng.random_range(0..=config.jitter_ms))
                                } else {
                                    0
                                };
                                work.cancel.sleep(Duration::from_millis(
                                    u64::from(config.interval_ms) + jitter,
                                ))?;
                            }
                            let status = active.status(&work.cancel)?;
                            held = status.buttons() != 0;
                            let _ = events.send(AppEvent::Status(status));
                            let _ = events.send(AppEvent::Done);
                        }
                        AppCommand::Chord(config) => {
                            let active = require(&mut session)?;
                            let mut pressed = input_protocol::v3::InputState::default();
                            for usage in &config.usages {
                                pressed.set(
                                    input_protocol::KeyId {
                                        usage_page: 7,
                                        usage: *usage,
                                    },
                                    true,
                                )?;
                            }
                            active.input(Command::Snapshot(pressed), &work.cancel)?;
                            work.cancel
                                .sleep(Duration::from_millis(u64::from(config.hold_ms.max(10))))?;
                            active.input(
                                Command::Snapshot(input_protocol::v3::InputState::default()),
                                &work.cancel,
                            )?;
                            let status = active.status(&work.cancel)?;
                            held = false;
                            let _ = events.send(AppEvent::Status(status));
                            let _ = events.send(AppEvent::Done);
                        }
                        AppCommand::Upload(config) => {
                            if let Some(active) = session.as_mut() {
                                active.reset(&work.cancel)?;
                                let _ = active.close(&Cancellation::default());
                            }
                            drop(session.take());
                            held = false;
                            let mut observer = Observer {
                                events: &events,
                                choices: &choices,
                                stage: 1,
                            };
                            let outcome = firmware::upload(
                                &config,
                                selected.as_ref(),
                                &SystemRunner,
                                &work.cancel,
                                &mut observer,
                            );
                            let _ = events.send(AppEvent::Upload(outcome));
                            if selected.is_some() {
                                recover(&mut selected, &mut session, &events, debug);
                            }
                        }
                        AppCommand::Latency(config) => {
                            let device = selected
                                .clone()
                                .ok_or_else(|| anyhow::anyhow!("請先連線"))?;
                            held = false;
                            let mut report = if config.mode == Mode::Extreme {
                                require(&mut session)?.reset(&work.cancel)?;
                                drop(session.take());
                                latency::run_extreme(
                                    device,
                                    config,
                                    &work.cancel,
                                    &mut |round| {
                                        let _ = events.send(AppEvent::Round(round));
                                    },
                                    &mut |line| {
                                        let _ = events.send(AppEvent::Log(line));
                                    },
                                )?
                            } else {
                                latency::measure(
                                    require(&mut session)?,
                                    device,
                                    config,
                                    &work.cancel,
                                    &mut |round| {
                                        let _ = events.send(AppEvent::Round(round));
                                    },
                                )?
                            };
                            if !report.cleanup_confirmed || session.is_none() {
                                recover(&mut selected, &mut session, &events, debug);
                                report.cleanup_confirmed = session.is_some();
                            }
                            let _ = events.send(AppEvent::Cleanup(report.cleanup_confirmed));
                            let _ = events.send(AppEvent::Report(report));
                        }
                    }
                    Ok(())
                })();
                if debug {
                    log_responses(&mut session, &events);
                }
                if let Err(e) = result {
                    let _ = events.send(AppEvent::Error(format!("{e:#}")));
                    held = false;
                    if selected.is_some() {
                        recover(&mut selected, &mut session, &events, debug);
                    }
                }
                let _ = events.send(AppEvent::Done);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                let result = (|| -> Result<()> {
                    if let Some(active) = session.as_mut() {
                        active.poll()?;
                        // The SDK keeps the 500 ms lease heartbeat on its own
                        // thread, so the worker only polls the device here.
                        if active.needs_heartbeat(held)
                            && heartbeat.elapsed() >= active.heartbeat_interval()
                        {
                            active.input(
                                input_protocol::v3::Command::Heartbeat,
                                &Cancellation::default(),
                            )?;
                            heartbeat = Instant::now();
                        }
                        // Live telemetry: keep the STATUS panel moving even when
                        // the user is not pressing anything.
                        if debug && polled.elapsed() >= STATUS_POLL {
                            let status = active.status(&Cancellation::default())?;
                            held = status.buttons() != 0;
                            polled = Instant::now();
                            let _ = events.send(AppEvent::Status(status));
                        }
                        // Older firmware has no STATUS_EXT; a failure here must not
                        // tear down a working session.
                        if debug && ext_polled.elapsed() >= EXT_POLL {
                            ext_polled = Instant::now();
                            if let Ok(ext) = active.status_ext(&Cancellation::default()) {
                                let _ = events.send(AppEvent::Ext(ext));
                            }
                            if let Ok(sessions) = active.sessions(&Cancellation::default()) {
                                let _ = events.send(AppEvent::Sessions(sessions));
                            }
                        }
                    }
                    Ok(())
                })();
                if debug {
                    log_responses(&mut session, &events);
                }
                match result {
                    Ok(()) => misses = 0,
                    Err(e) => {
                        misses += 1;
                        // One missed poll is usually a busy bus or a reply stolen
                        // by another reader. Tearing the session down for every
                        // hiccup is what makes the device-global failsafe counter
                        // climb, so require two in a row.
                        if misses < 2 {
                            let _ = events.send(AppEvent::Log(format!("略過一次輪詢失敗：{e:#}")));
                        } else {
                            misses = 0;
                            let _ =
                                events.send(AppEvent::Error(format!("連線中斷／心跳失敗：{e:#}")));
                            held = false;
                            recover(&mut selected, &mut session, &events, debug);
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(mut active) = session.take() {
        let confirmed = active.reset(&Cancellation::default()).is_ok();
        let _ = active.close(&Cancellation::default());
        let _ = events.send(AppEvent::Cleanup(confirmed));
    }
    let _ = events.send(AppEvent::Stopped);
}
