use crate::{
    cancel::Cancellation,
    firmware::{self, UploadConfig, UploadObserver, UploadOutcome},
    hid::{self, Device, NativeSession},
    latency::{self, LatencyConfig, Mode, Report, Round},
    mouse::{self, MouseAction},
    process::SystemRunner,
    protocol::{Hello, Status},
};
use anyhow::{Result, bail};
use crossbeam_channel::{Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum AppCommand {
    Scan,
    Connect(Device),
    Disconnect,
    Status,
    Reset,
    Upload(UploadConfig),
    Latency(LatencyConfig),
    Mouse(MouseAction),
}
#[derive(Debug)]
pub enum AppEvent {
    Devices(Vec<Device>),
    Connected(Device, Hello),
    Disconnected,
    Status(Status),
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
        if self.join.as_ref().is_some_and(|h| h.is_finished()) {
            let _ = self.join.take().unwrap().join();
        }
    }
}
struct Observer<'a> {
    events: &'a Sender<AppEvent>,
    choices: &'a Receiver<String>,
}
impl UploadObserver for Observer<'_> {
    fn log(&mut self, line: String) {
        let _ = self.events.send(AppEvent::Log(line));
    }
    fn progress(&mut self, stage: usize, message: &str) {
        let _ = self.events.send(AppEvent::Progress(stage, message.into()));
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
fn require(session: &mut Option<NativeSession>) -> Result<&mut NativeSession> {
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
    session: &mut Option<NativeSession>,
    events: &Sender<AppEvent>,
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
fn log_responses(session: &mut Option<NativeSession>, events: &Sender<AppEvent>) {
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
    let mut session: Option<NativeSession> = None;
    let mut held = false;
    let mut heartbeat = Instant::now();
    while !exit.is_cancelled() {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(work) => {
                let result = (|| -> Result<()> {
                    match work.command {
                        AppCommand::Scan => {
                            let _ = events.send(AppEvent::Devices(hid::enumerate()?));
                        }
                        AppCommand::Connect(device) => {
                            if session.is_some() {
                                require(&mut session)?.reset(&Cancellation::default())?;
                            }
                            drop(session.take());
                            held = false;
                            selected = Some(device.clone());
                            let mut opened = hid::open(&device)?;
                            opened.synchronize(&work.cancel)?;
                            let hello = opened.hello(&work.cancel)?;
                            let status = opened.status(&work.cancel)?;
                            let _ = events.send(AppEvent::Connected(device, hello));
                            let _ = events.send(AppEvent::Status(status));
                            session = Some(opened);
                        }
                        AppCommand::Disconnect => {
                            if session.is_some() {
                                require(&mut session)?.reset(&Cancellation::default())?;
                            }
                            drop(session.take());
                            selected = None;
                            held = false;
                            let _ = events.send(AppEvent::Cleanup(true));
                            let _ = events.send(AppEvent::Disconnected);
                        }
                        AppCommand::Status => {
                            let status = require(&mut session)?.status(&work.cancel)?;
                            held = status.buttons() != 0;
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::Reset => {
                            require(&mut session)?.reset(&Cancellation::default())?;
                            held = false;
                            let _ = events.send(AppEvent::Cleanup(true));
                            let status = require(&mut session)?.status(&Cancellation::default())?;
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::Mouse(action) => {
                            let status =
                                mouse::execute(require(&mut session)?, &action, &work.cancel)?;
                            held = status.buttons() != 0;
                            heartbeat = Instant::now();
                            let _ = events.send(AppEvent::Status(status));
                        }
                        AppCommand::Upload(config) => {
                            if session.is_some() {
                                require(&mut session)?.reset(&work.cancel)?;
                            }
                            drop(session.take());
                            held = false;
                            let mut observer = Observer {
                                events: &events,
                                choices: &choices,
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
                                recover(&mut selected, &mut session, &events);
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
                                recover(&mut selected, &mut session, &events);
                                report.cleanup_confirmed = session.is_some();
                            }
                            let _ = events.send(AppEvent::Cleanup(report.cleanup_confirmed));
                            let _ = events.send(AppEvent::Report(report));
                        }
                    }
                    Ok(())
                })();
                log_responses(&mut session, &events);
                if let Err(e) = result {
                    let _ = events.send(AppEvent::Error(format!("{e:#}")));
                    held = false;
                    if selected.is_some() {
                        recover(&mut selected, &mut session, &events);
                    }
                }
                let _ = events.send(AppEvent::Done);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                let result = (|| -> Result<()> {
                    if let Some(active) = session.as_mut() {
                        active.poll()?;
                        if active.needs_heartbeat(held)
                            && heartbeat.elapsed() >= active.heartbeat_interval()
                        {
                            active.input(
                                input_protocol::v3::Command::Heartbeat,
                                &Cancellation::default(),
                            )?;
                            heartbeat = Instant::now();
                        }
                    }
                    Ok(())
                })();
                log_responses(&mut session, &events);
                if let Err(e) = result {
                    let _ = events.send(AppEvent::Error(format!("連線中斷／心跳失敗：{e:#}")));
                    held = false;
                    recover(&mut selected, &mut session, &events);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(mut active) = session.take() {
        let confirmed = active.reset(&Cancellation::default()).is_ok();
        let _ = events.send(AppEvent::Cleanup(confirmed));
    }
    let _ = events.send(AppEvent::Stopped);
}
