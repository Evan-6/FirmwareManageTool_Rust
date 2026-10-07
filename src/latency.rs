#[cfg(windows)]
use crate::platform;
#[cfg(windows)]
use crate::platform::{Counter, PriorityGuard};
use crate::{
    cancel::Cancellation,
    hid::{self, Device, HidTransport, NativeSession, Session},
    process::{Stream, read_lines, spawn_managed},
};
#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    io::{BufRead, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Normal,
    HighPrecision,
    Extreme,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "一般",
            Self::HighPrecision => "高精度",
            Self::Extreme => "極限 REALTIME（僅送出）",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LatencyConfig {
    pub rounds: u32,
    pub mode: Mode,
}
impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            rounds: 50,
            mode: Mode::Normal,
        }
    }
}
impl LatencyConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!((1..=10_000).contains(&self.rounds), "輪數須為 1–10000");
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundOutcome {
    Success,
    Failed,
    Timeout,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub round: u32,
    pub latency_ms: Option<f64>,
    pub outcome: RoundOutcome,
    pub error: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub config: LatencyConfig,
    pub device: Device,
    pub unix_ms: u128,
    pub rounds: Vec<Round>,
    pub cancelled: bool,
    pub cleanup_confirmed: bool,
    pub terminal_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Statistics {
    pub attempted: usize,
    pub unattempted: usize,
    pub successful: usize,
    pub failed: usize,
    pub timeouts: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub average: Option<f64>,
    pub median: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub sample_stddev: Option<f64>,
}
impl Report {
    pub fn statistics(&self) -> Statistics {
        let mut values: Vec<_> = self
            .rounds
            .iter()
            .filter(|r| r.outcome == RoundOutcome::Success)
            .filter_map(|r| r.latency_ms)
            .collect();
        values.sort_by(f64::total_cmp);
        let n = values.len();
        let average = if n == 0 {
            None
        } else {
            Some(values.iter().sum::<f64>() / n as f64)
        };
        let median = if n == 0 {
            None
        } else if n % 2 == 0 {
            Some((values[n / 2 - 1] + values[n / 2]) / 2.0)
        } else {
            Some(values[n / 2])
        };
        let percentile = |p: f64| {
            if n == 0 {
                None
            } else {
                Some(values[((n as f64 * p).ceil() as usize).saturating_sub(1)])
            }
        };
        Statistics {
            attempted: self.rounds.len(),
            unattempted: (self.config.rounds as usize).saturating_sub(self.rounds.len()),
            successful: n,
            failed: self
                .rounds
                .iter()
                .filter(|r| r.outcome == RoundOutcome::Failed)
                .count(),
            timeouts: self
                .rounds
                .iter()
                .filter(|r| r.outcome == RoundOutcome::Timeout)
                .count(),
            min: values.first().copied(),
            max: values.last().copied(),
            average,
            median,
            p95: percentile(0.95),
            p99: percentile(0.99),
            sample_stddev: if n > 1 {
                Some(
                    (values
                        .iter()
                        .map(|v| (v - average.unwrap()).powi(2))
                        .sum::<f64>()
                        / (n - 1) as f64)
                        .sqrt(),
                )
            } else {
                None
            },
        }
    }
    pub fn csv(&self) -> String {
        fn quote(s: &str) -> String {
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        let mut csv = String::from(
            "\u{feff}unix_ms,mode,device,device_path,requested_rounds,round,outcome,latency_ms,error,cleanup_confirmed,cancelled\r\n",
        );
        // An empty report still exports its conditions and terminal failure.
        if self.rounds.is_empty() {
            csv.push_str(&format!(
                "{},{},{},{},{},,Failed,,{},{},{}\r\n",
                self.unix_ms,
                quote(self.config.mode.label()),
                quote(&self.device.label()),
                quote(&self.device.path),
                self.config.rounds,
                quote(self.terminal_error.as_deref().unwrap_or("無成功樣本")),
                self.cleanup_confirmed,
                self.cancelled
            ));
        }
        for round in &self.rounds {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{:?},{},{},{},{}\r\n",
                self.unix_ms,
                quote(self.config.mode.label()),
                quote(&self.device.label()),
                quote(&self.device.path),
                self.config.rounds,
                round.round,
                round.outcome,
                round
                    .latency_ms
                    .map(|v| format!("{v:.6}"))
                    .unwrap_or_default(),
                quote(&round.error),
                self.cleanup_confirmed,
                self.cancelled
            ));
        }
        csv
    }
    pub fn export(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.csv())?;
        Ok(())
    }
}
#[cfg(windows)]
fn wait_key(target: bool, counter: &Counter, mode: Mode, cancel: &Cancellation) -> Result<()> {
    let start = counter.ticks();
    let mut spins = 0u32;
    loop {
        cancel.check()?;
        if platform::key_a_down() == target {
            return Ok(());
        }
        if counter.millis(counter.ticks() - start) >= 1000.0 {
            bail!(
                "等待 A {}逾時（1 秒）",
                if target { "按下" } else { "放開" }
            );
        }
        if mode == Mode::Normal {
            std::thread::sleep(Duration::from_micros(50));
        } else {
            std::hint::spin_loop();
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(2048) {
                std::thread::yield_now();
            }
        }
    }
}
pub fn measure<T: HidTransport>(
    session: &mut Session<T>,
    device: Device,
    config: LatencyConfig,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Round),
) -> Result<Report> {
    config.validate()?;
    #[cfg(not(windows))]
    {
        let _ = (session, device, config, cancel, progress);
        bail!("實體延遲量測僅支援 Windows");
    }
    #[cfg(windows)]
    {
        let mut report = Report {
            config: config.clone(),
            device,
            unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            rounds: Vec::new(),
            cancelled: false,
            cleanup_confirmed: false,
            terminal_error: None,
        };
        let counter = Counter::new()?;
        let setup = (|| -> Result<()> {
            session.reset(cancel)?;
            wait_key(false, &counter, config.mode, cancel)?;
            cancel.sleep(Duration::from_millis(500))?;
            Ok(())
        })();
        if let Err(e) = setup {
            report.terminal_error = Some(format!("{e:#}"));
        } else {
            let before = session.status(cancel)?;
            for round in 1..=config.rounds {
                if cancel.is_cancelled() {
                    report.cancelled = true;
                    break;
                }
                let result = (|| -> Result<f64> {
                    wait_key(false, &counter, config.mode, cancel)
                        .context("量測前確認 A 已放開")?;
                    cancel.sleep(Duration::from_millis(80))?;
                    let duration;
                    {
                        // Windows must be able to process input while we observe it.
                        // REALTIME polling can starve the very input thread we await.
                        let _observer_priority =
                            PriorityGuard::set(false, config.mode != Mode::Normal)
                                .context("設定觀察執行緒優先權")?;
                        let send_priority = if config.mode == Mode::Extreme {
                            Some(
                                PriorityGuard::set(true, false)
                                    .context("設定 REALTIME 送出優先權")?,
                            )
                        } else {
                            None
                        };
                        let start = counter.ticks();
                        let sent = session.key(
                            input_protocol::KeyId::from_token("a").unwrap(),
                            true,
                            cancel,
                        );
                        // Restore both process and thread before polling, including
                        // when the write failed. Keep restoration inside QPC timing.
                        drop(send_priority);
                        sent.context("送出 A 按下")?;
                        wait_key(true, &counter, config.mode, cancel)
                            .context("等待 Windows 觀察到 A 按下")?;
                        let end = counter.ticks();
                        duration = counter.millis(end - start);
                    }
                    session.finish(cancel).context("確認 A 按下的 USB 屏障")?;
                    session
                        .key(
                            input_protocol::KeyId::from_token("a").unwrap(),
                            false,
                            cancel,
                        )
                        .context("送出 A 放開")?;
                    session.finish(cancel).context("確認 A 放開的 USB 屏障")?;
                    wait_key(false, &counter, config.mode, cancel)
                        .context("等待 Windows 觀察到 A 放開")?;
                    let after = session.status(cancel).context("讀取量測後的裝置狀態")?;
                    after
                        .check_transport_since(&before)
                        .context("檢查量測期間的裝置傳輸")?;
                    Ok(duration)
                })();
                let record = match result {
                    Ok(ms) => Round {
                        round,
                        latency_ms: Some(ms),
                        outcome: RoundOutcome::Success,
                        error: String::new(),
                    },
                    Err(e) => {
                        let error = format!("{e:#}");
                        let outcome = if error.contains("逾時") {
                            RoundOutcome::Timeout
                        } else {
                            RoundOutcome::Failed
                        };
                        Round {
                            round,
                            latency_ms: None,
                            outcome,
                            error,
                        }
                    }
                };
                let failed = record.outcome != RoundOutcome::Success;
                progress(record.clone());
                report.rounds.push(record);
                if failed {
                    report.terminal_error = Some(report.rounds.last().unwrap().error.clone());
                    report.cancelled = cancel.is_cancelled();
                    break;
                }
            }
        }
        report.cancelled |= cancel.is_cancelled();
        report.cleanup_confirmed = session.reset(&Cancellation::default()).is_ok();
        Ok(report)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub device: Device,
    pub config: LatencyConfig,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerEvent {
    Round(Round),
    Report(Report),
    Error(String),
}
/// Internal child entrypoint. stdout is exclusively newline-delimited JSON.
pub fn worker_entry() -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    input.read_line(&mut line)?;
    let request: WorkerRequest = serde_json::from_str(&line)?;
    request.config.validate()?;
    let cancel = Cancellation::default();
    let token = cancel.clone();
    drop(input);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) if line == "cancel" => {
                    token.cancel();
                    break;
                }
                Err(_) => {
                    token.cancel();
                    break;
                }
                _ => {}
            }
        }
        token.cancel();
    });
    let emit = |event: WorkerEvent| {
        if let Ok(json) = serde_json::to_string(&event) {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{json}");
            let _ = out.flush();
        }
    };
    let result = (|| -> Result<Report> {
        let mut session = hid::open(&request.device)?;
        session.synchronize(&cancel)?;
        session.hello(&cancel)?;
        let mut report = measure(
            &mut session,
            request.device.clone(),
            request.config,
            &cancel,
            &mut |round| emit(WorkerEvent::Round(round)),
        )?;
        drop(session);
        if !report.cleanup_confirmed {
            report.cleanup_confirmed = recover_reset(&request.device).is_ok();
        }
        Ok(report)
    })();
    match result {
        Ok(report) => emit(WorkerEvent::Report(report)),
        Err(e) => {
            let cleanup = recover_reset(&request.device);
            emit(WorkerEvent::Error(format!(
                "{e:#}；清理：{}",
                if cleanup.is_ok() {
                    "已確認釋放"
                } else {
                    "釋放狀態未知"
                }
            )));
        }
    }
    Ok(())
}
pub fn recover_reset(device: &Device) -> Result<NativeSession> {
    let mut session = hid::open(device)?;
    let token = Cancellation::default();
    session.synchronize(&token)?;
    session.hello(&token)?;
    Ok(session)
}
pub fn run_extreme(
    device: Device,
    config: LatencyConfig,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Round),
    log: &mut dyn FnMut(String),
) -> Result<Report> {
    config.validate()?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--latency-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut managed = spawn_managed(&mut command)?;
    let mut stdin = managed.child.stdin.take().unwrap();
    let request = WorkerRequest { device, config };
    writeln!(stdin, "{}", serde_json::to_string(&request)?)?;
    stdin.flush()?;
    let (tx, rx) = crossbeam_channel::bounded(256);
    let readers = [
        read_lines(
            managed.child.stdout.take().unwrap(),
            Stream::Stdout,
            tx.clone(),
        ),
        read_lines(
            managed.child.stderr.take().unwrap(),
            Stream::Stderr,
            tx.clone(),
        ),
    ];
    drop(tx);
    let started = Instant::now();
    let mut last_activity = Instant::now();
    let mut cancelling = None;
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut observed = Vec::new();
    let mut report = None;
    let mut error = None;
    let mut exited = false;
    loop {
        for line in rx.try_iter().take(128) {
            match line.stream {
                Stream::Stderr => log(line.text),
                Stream::Stdout => match serde_json::from_str::<WorkerEvent>(&line.text) {
                    Ok(WorkerEvent::Round(round)) => {
                        last_activity = Instant::now();
                        observed.push(round.clone());
                        progress(round);
                    }
                    Ok(WorkerEvent::Report(value)) => {
                        report = Some(value);
                        last_activity = Instant::now();
                    }
                    Ok(WorkerEvent::Error(value)) => {
                        error = Some(value);
                        last_activity = Instant::now();
                    }
                    Err(e) => {
                        error = Some(format!("量測子程序 IPC 格式錯誤：{e}"));
                    }
                },
            }
        }
        if cancel.is_cancelled() && cancelling.is_none() {
            let _ = writeln!(stdin, "cancel");
            let _ = stdin.flush();
            cancelling = Some(Instant::now());
        }
        if cancelling.is_some_and(|at| at.elapsed() >= Duration::from_secs(2))
            || last_activity.elapsed() > Duration::from_secs(8)
            || started.elapsed() > Duration::from_secs(request.config.rounds as u64 * 4 + 10)
        {
            managed.terminate_tree()?;
            managed.child.wait()?;
            error = Some(
                if cancel.is_cancelled() {
                    "量測已取消；超過 2 秒，子程序已終止"
                } else {
                    "量測子程序監督期限到期"
                }
                .into(),
            );
            exited = true;
        }
        if exited || managed.child.try_wait()?.is_some() {
            managed.terminate_tree()?;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stdin);
    // Read final JSON already in the pipe after observing process exit.
    for line in rx.iter() {
        match line.stream {
            Stream::Stderr => log(line.text),
            Stream::Stdout => match serde_json::from_str::<WorkerEvent>(&line.text) {
                Ok(WorkerEvent::Report(value)) => report = Some(value),
                Ok(WorkerEvent::Round(round)) => {
                    observed.push(round.clone());
                    progress(round);
                }
                Ok(WorkerEvent::Error(value)) => error = Some(value),
                Err(e) => error = Some(format!("IPC 格式錯誤：{e}")),
            },
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let missing = report.is_none();
    let mut report = report.unwrap_or(Report {
        config: request.config,
        device: request.device,
        unix_ms,
        rounds: observed,
        cancelled: cancel.is_cancelled(),
        cleanup_confirmed: false,
        terminal_error: None,
    });
    if let Some(error) = error.or_else(|| missing.then(|| "量測子程序結束但未回傳報告".into()))
    {
        report.cleanup_confirmed = false;
        report.terminal_error = Some(error);
    }
    report.cancelled |= cancel.is_cancelled();
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn report(values: &[f64]) -> Report {
        Report {
            config: LatencyConfig::default(),
            device: Device {
                path: "test,\"device".into(),
                serial: "中文".into(),
                product: "fake".into(),
                vid: 0,
                pid: 0,
            },
            unix_ms: 0,
            rounds: values
                .iter()
                .enumerate()
                .map(|(i, &value)| Round {
                    round: i as u32 + 1,
                    latency_ms: Some(value),
                    outcome: RoundOutcome::Success,
                    error: String::new(),
                })
                .collect(),
            cancelled: false,
            cleanup_confirmed: true,
            terminal_error: None,
        }
    }
    #[test]
    fn zero_single_and_percentiles() {
        let empty = report(&[]).statistics();
        assert_eq!(empty.average, None);
        assert_eq!(report(&[3.0]).statistics().sample_stddev, None);
        let stats = report(&[1., 2., 3., 4.]).statistics();
        assert_eq!(stats.median, Some(2.5));
        assert_eq!(stats.p95, Some(4.));
        assert!((stats.sample_stddev.unwrap() - 1.290994).abs() < 0.00001);
    }
    #[test]
    fn csv_quotes_device_conditions() {
        let csv = report(&[1.5]).csv();
        assert!(csv.contains("\"test,\"\"device\""));
        assert!(csv.contains("1.500000"));
        assert!(csv.contains("cleanup_confirmed"));
    }
    #[test]
    fn invalid_rounds_are_rejected() {
        assert!(
            LatencyConfig {
                rounds: 0,
                mode: Mode::Normal
            }
            .validate()
            .is_err()
        );
        assert!(
            LatencyConfig {
                rounds: 10001,
                mode: Mode::Extreme
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn early_stop_counts_only_attempted_rounds() {
        let mut measured = report(&[1.0; 15]);
        measured.rounds.push(Round {
            round: 16,
            latency_ms: None,
            outcome: RoundOutcome::Timeout,
            error: "等待 A 按下逾時".into(),
        });
        let stats = measured.statistics();
        assert_eq!(stats.attempted, 16);
        assert_eq!(stats.unattempted, 34);
        assert_eq!(stats.successful, 15);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.timeouts, 1);
        assert_eq!(stats.average, Some(1.0));
        let empty = report(&[]).statistics();
        assert_eq!(empty.attempted, 0);
        assert_eq!(empty.unattempted, 50);
    }
}
