use crate::{
    backend::{AppCommand, AppEvent, Backend},
    hid::Device,
    latency::{Mode, Report, Round, Statistics},
    mouse::MouseAction,
    protocol::{Hello, Status},
    settings::Settings,
};
use eframe::egui::{self, Color32, RichText};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Device,
    Status,
    Firmware,
    Latency,
    Mouse,
}

/// One STATUS sample. The live panel keeps a short ring of these so it can show
/// per-second rates and a 30 s strip instead of a single frozen number.
#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    received: u32,
    completed: u32,
    pending: u8,
    lease: u16,
    rx: u16,
    tx: u16,
    hid: u16,
    failsafe: u16,
    held: usize,
    buttons: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Counter {
    Received,
    Completed,
    Pending,
    Lease,
    Rx,
    Tx,
    Hid,
    Failsafe,
}

impl Counter {
    /// Monotonic counters wrap at u16/u32; gauges (lease, pending, held) do not.
    fn wraps(self) -> Option<u64> {
        match self {
            Self::Received | Self::Completed => Some(1 << 32),
            Self::Rx | Self::Tx | Self::Hid | Self::Failsafe => Some(1 << 16),
            Self::Pending | Self::Lease => None,
        }
    }
}

impl Sample {
    fn of(status: &Status, at: Instant) -> Self {
        Self {
            at,
            received: status.count("received") as u32,
            completed: status.count("completed") as u32,
            pending: status.count("pending") as u8,
            lease: status.count("lease") as u16,
            rx: status.count("rx") as u16,
            tx: status.count("tx") as u16,
            hid: status.count("hid") as u16,
            failsafe: status.count("fs") as u16,
            held: status.count("p") as usize,
            buttons: status.buttons(),
        }
    }

    fn get(self, counter: Counter) -> u32 {
        match counter {
            Counter::Received => self.received,
            Counter::Completed => self.completed,
            Counter::Pending => u32::from(self.pending),
            Counter::Lease => u32::from(self.lease),
            Counter::Rx => u32::from(self.rx),
            Counter::Tx => u32::from(self.tx),
            Counter::Hid => u32::from(self.hid),
            Counter::Failsafe => u32::from(self.failsafe),
        }
    }
}
pub struct App {
    backend: Backend,
    settings: Settings,
    page: Page,
    devices: Vec<Device>,
    selected: Option<usize>,
    connected: Option<(Device, Hello)>,
    status: Option<Status>,
    samples: VecDeque<Sample>,
    status_at: Option<Instant>,
    logs: VecDeque<String>,
    busy: bool,
    message: String,
    progress: usize,
    choices: Vec<String>,
    cleanup: Option<bool>,
    report: Option<Report>,
    stats: Option<Statistics>,
    rounds: Vec<Round>,
    x: i32,
    y: i32,
    duration: u32,
    button: u8,
    vertical: i32,
    horizontal: i32,
    close_started: Option<Instant>,
    stopped: bool,
}
impl App {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        Self::from_context(&context.egui_ctx)
    }
    fn from_context(context: &egui::Context) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        let mut font_loaded = false;
        #[cfg(windows)]
        {
            if let Some(windir) = std::env::var_os("WINDIR") {
                for name in ["msjh.ttc", "msyh.ttc", "mingliu.ttc"] {
                    if let Ok(bytes) =
                        std::fs::read(std::path::PathBuf::from(&windir).join("Fonts").join(name))
                    {
                        fonts.font_data.insert(
                            "traditional_chinese".into(),
                            egui::FontData::from_owned(bytes).into(),
                        );
                        fonts
                            .families
                            .entry(egui::FontFamily::Proportional)
                            .or_default()
                            .insert(0, "traditional_chinese".into());
                        fonts
                            .families
                            .entry(egui::FontFamily::Monospace)
                            .or_default()
                            .push("traditional_chinese".into());
                        font_loaded = true;
                        break;
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            for path in [
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            ] {
                if let Ok(bytes) = std::fs::read(path) {
                    fonts.font_data.insert(
                        "traditional_chinese".into(),
                        egui::FontData::from_owned(bytes).into(),
                    );
                    fonts
                        .families
                        .entry(egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, "traditional_chinese".into());
                    font_loaded = true;
                    break;
                }
            }
        }
        context.set_fonts(fonts);
        context.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10., 8.);
        });
        let mut app = Self {
            backend: Backend::start(),
            settings: Settings::load(),
            page: Page::Device,
            devices: Vec::new(),
            selected: None,
            connected: None,
            status: None,
            samples: VecDeque::new(),
            status_at: None,
            logs: VecDeque::new(),
            busy: false,
            message: "選擇裝置後連線，或前往韌體燒錄".into(),
            progress: 0,
            choices: Vec::new(),
            cleanup: None,
            report: None,
            stats: None,
            rounds: Vec::new(),
            x: 0,
            y: 0,
            duration: 0,
            button: 1,
            vertical: 0,
            horizontal: 0,
            close_started: None,
            stopped: false,
        };
        app.log("FirmwareManageTool Rust • Windows 11 x64 • protocol=3".into());
        if !font_loaded {
            app.log("未找到系統中文字型；請安裝 Windows 繁中文字型以正常顯示介面".into());
        }
        app
    }
    fn log(&mut self, line: String) {
        if self.logs.len() >= 1000 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }
    fn submit(&mut self, command: AppCommand) {
        if self.busy || self.close_started.is_some() {
            return;
        }
        match self.backend.submit(command) {
            Ok(()) => {
                self.busy = true;
                self.progress = 0;
                self.cleanup = None;
                self.message = "操作進行中…".into();
            }
            Err(e) => self.log(format!("{e:#}")),
        }
    }
    fn events(&mut self) {
        let events: Vec<_> = self.backend.events.try_iter().take(256).collect();
        for event in events {
            match event {
                AppEvent::Devices(devices) => {
                    self.devices = devices;
                    self.selected = None;
                    self.message = format!("找到 {} 個符合的 Vendor HID 裝置", self.devices.len());
                }
                AppEvent::Connected(device, hello) => {
                    self.message = format!("已連線：{}", device.label());
                    self.connected = Some((device, hello));
                }
                AppEvent::Disconnected => {
                    self.connected = None;
                    self.status = None;
                    self.samples.clear();
                    self.status_at = None;
                }
                AppEvent::Status(status) => {
                    let at = Instant::now();
                    self.samples.push_back(Sample::of(&status, at));
                    // 600 samples at ~5 Hz is two minutes of history.
                    while self.samples.len() > 600 {
                        self.samples.pop_front();
                    }
                    self.status_at = Some(at);
                    self.status = Some(status);
                }
                AppEvent::Log(line) => self.log(line),
                AppEvent::Error(error) => {
                    self.message = error.clone();
                    self.log(format!("錯誤：{error}"));
                }
                AppEvent::Progress(stage, message) => {
                    self.progress = stage;
                    self.message = message;
                }
                AppEvent::BootChoices(choices) => self.choices = choices,
                AppEvent::Round(round) => {
                    self.message = format!("已完成第 {} 輪", round.round);
                    self.rounds.push(round);
                }
                AppEvent::Report(report) => {
                    self.message = if let Some(error) = &report.terminal_error {
                        format!("測試停止：{error}")
                    } else if report.cancelled {
                        "測試已取消".into()
                    } else {
                        "測試完成".into()
                    };
                    // Statistics are computed once here; the page must not
                    // re-sort up to 10000 samples on every repaint.
                    self.stats = Some(report.statistics());
                    self.report = Some(report);
                }
                AppEvent::Upload(outcome) => {
                    self.message = outcome.label();
                    self.log(self.message.clone());
                }
                AppEvent::Cleanup(ok) => {
                    self.cleanup = Some(ok);
                    self.log(
                        if ok {
                            "已確認全部放開"
                        } else {
                            "釋放狀態未知；需重新連線確認"
                        }
                        .into(),
                    );
                }
                AppEvent::Done => {
                    self.busy = false;
                    self.choices.clear();
                    if self.message == "操作進行中…" {
                        self.message = "操作完成".into();
                    }
                }
                AppEvent::Stopped => self.stopped = true,
            }
        }
    }
    fn device_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("裝置與連線");
        ui.label("Vendor HID • 03F0:0024 / 6666:6666 • usage FF60:0061");
        if ui
            .add_enabled(!self.busy, egui::Button::new("重新掃描裝置"))
            .clicked()
        {
            self.submit(AppCommand::Scan);
        }
        for (index, device) in self.devices.iter().enumerate() {
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(device.label()).selected(self.selected == Some(index)),
                )
                .clicked()
            {
                self.selected = Some(index);
            }
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && self.selected.is_some(),
                    egui::Button::new("連線選取裝置"),
                )
                .clicked()
                && let Some(device) = self.selected.and_then(|i| self.devices.get(i)).cloned()
            {
                self.submit(AppCommand::Connect(device));
            }
            if ui
                .add_enabled(
                    !self.busy && self.connected.is_some(),
                    egui::Button::new("斷線並放開"),
                )
                .clicked()
            {
                self.submit(AppCommand::Disconnect);
            }
            if ui
                .add_enabled(
                    !self.busy && self.connected.is_some(),
                    egui::Button::new("讀取狀態"),
                )
                .clicked()
            {
                self.submit(AppCommand::Status);
            }
        });
        ui.separator();
        if let Some((device, hello)) = &self.connected {
            ui.label(RichText::new(device.label()).strong());
            ui.label(format!(
                "韌體 {} • 租約 {} ms • 滑鼠 {} • protocol={} • NKRO={} • 多媒體={}",
                hello.firmware,
                hello.lease_ms,
                if hello.mouse { "支援" } else { "不支援" },
                hello.protocol,
                hello.nkro,
                hello.media
            ));
            ui.monospace(&hello.raw);
        } else {
            ui.label("尚未連線");
        }
        if let Some(status) = &self.status {
            ui.label(format!(
                "按下鍵數 {} • 滑鼠位元 {:02X} • failsafe {} • TX 丟棄 {} • HID 失敗 {}",
                status.count("p"),
                status.buttons(),
                status.count("fs"),
                status.count("tx"),
                status.count("hid")
            ));
            ui.monospace(&status.raw);
        }
    }

    /// Per-second rate over roughly the last second of samples.
    fn rate(&self, counter: Counter) -> Option<f64> {
        let latest = *self.samples.back()?;
        let base = self
            .samples
            .iter()
            .rev()
            .find(|sample| latest.at.duration_since(sample.at) >= Duration::from_millis(900))?;
        let seconds = latest.at.duration_since(base.at).as_secs_f64();
        if seconds <= 0.0 {
            return None;
        }
        Some(Self::delta(counter, latest, *base) / seconds)
    }

    /// Difference between two samples, honouring the counter's wrap width.
    fn delta(counter: Counter, newer: Sample, older: Sample) -> f64 {
        let (a, b) = (u64::from(newer.get(counter)), u64::from(older.get(counter)));
        let raw = match counter.wraps() {
            Some(span) if a < b => span - b + a,
            _ if a < b => 0,
            _ => a - b,
        };
        raw as f64
    }

    /// Strip values, oldest first: activity per sample for monotonic counters,
    /// the raw value for gauges.
    fn series(&self, counter: Counter, len: usize) -> Vec<f32> {
        let samples: Vec<Sample> = self.samples.iter().rev().take(len).copied().collect();
        let zero = Sample {
            at: Instant::now(),
            received: 0,
            completed: 0,
            pending: 0,
            lease: 0,
            rx: 0,
            tx: 0,
            hid: 0,
            failsafe: 0,
            held: 0,
            buttons: 0,
        };
        let mut values = Vec::with_capacity(samples.len());
        for (index, sample) in samples.iter().enumerate() {
            let previous = samples.get(index + 1).copied().unwrap_or(zero);
            let value = match counter.wraps() {
                Some(_) => Self::delta(counter, *sample, previous) as f32,
                None => sample.get(counter) as f32,
            };
            values.push(value);
        }
        values.reverse();
        values
    }

    fn status_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("RP2040 即時狀態");
        let Some(status) = self.status.clone() else {
            ui.label(
                "尚未連線。連線後每秒 5 次自動送出 STATUS，這裡會顯示即時計數、租約與 30 秒走勢。",
            );
            return;
        };
        if let Some((device, hello)) = &self.connected {
            ui.label(RichText::new(device.label()).strong());
            let device_id = hello.raw.split("device=").nth(1).unwrap_or("-");
            ui.label(format!(
                "板型 2（XIAO RP2040）• 韌體 {} • protocol {} • 租約 {} ms • NKRO {} • 多媒體 {} • 序號 {}",
                hello.firmware,
                hello.protocol,
                hello.lease_ms,
                if hello.nkro { "有" } else { "無" },
                if hello.media { "有" } else { "無" },
                device_id
            ));
        } else {
            ui.label("裝置已離線，以下是最後一次收到的狀態。");
        }

        let latest = self.samples.back().copied();
        if let Some(sample) = latest {
            let first = self.samples.front().copied().unwrap_or(sample);
            ui.horizontal(|ui| {
                metric(
                    ui,
                    "按住鍵數",
                    sample.held.to_string(),
                    &format!("滑鼠 {:02X}", sample.buttons),
                );
                metric(
                    ui,
                    "租約剩餘",
                    format!("{} ms", sample.lease),
                    "心跳每 500 ms",
                );
                metric(ui, "待送工作", sample.pending.to_string(), "上限 32");
                metric(
                    ui,
                    "HID 失敗",
                    sample.hid.to_string(),
                    &format!("自連線 +{}", Self::delta(Counter::Hid, sample, first)),
                );
                metric(
                    ui,
                    "failsafe",
                    sample.failsafe.to_string(),
                    &format!("自連線 +{}", Self::delta(Counter::Failsafe, sample, first)),
                );
            });
            ui.add_space(4.0);
            let age = sample.at.elapsed().as_secs_f64();
            let health = if sample.hid != first.hid || sample.tx != first.tx {
                RichText::new("傳輸失敗計數增加，請檢查線材／hub").color(Color32::LIGHT_RED)
            } else if sample.failsafe != first.failsafe {
                RichText::new("有其他 session 失效（failsafe 增加）").color(Color32::YELLOW)
            } else {
                RichText::new("傳輸計數穩定").color(Color32::from_rgb(0x6c, 0xd4, 0x8f))
            };
            ui.horizontal(|ui| {
                ui.label(health);
                ui.label(
                    RichText::new(format!(
                        "• 最後更新 {:.1} s 前 • 樣本 {} 筆 • 更新頻率 5 Hz",
                        age,
                        self.samples.len()
                    ))
                    .weak(),
                );
            });
        }

        ui.separator();
        let rows: Vec<(&str, Counter, Color32)> = vec![
            (
                "received",
                Counter::Received,
                Color32::from_rgb(0x7f, 0xa8, 0xd8),
            ),
            (
                "completed",
                Counter::Completed,
                Color32::from_rgb(0x7f, 0xa8, 0xd8),
            ),
            (
                "pending",
                Counter::Pending,
                Color32::from_rgb(0xd8, 0xc0, 0x7f),
            ),
            ("rx", Counter::Rx, Color32::from_rgb(0xd8, 0xa0, 0x7f)),
            ("tx", Counter::Tx, Color32::from_rgb(0xd8, 0xa0, 0x7f)),
            ("hid", Counter::Hid, Color32::from_rgb(0xd8, 0x7f, 0x7f)),
            (
                "failsafe",
                Counter::Failsafe,
                Color32::from_rgb(0xd8, 0x7f, 0x7f),
            ),
            ("lease", Counter::Lease, Color32::from_rgb(0x7f, 0xd8, 0xc0)),
        ];
        let table: Vec<(&str, u32, String, Vec<f32>, Color32)> = rows
            .into_iter()
            .map(|(name, counter, color)| {
                let value = latest.map(|sample| sample.get(counter)).unwrap_or(0);
                let rate = self
                    .rate(counter)
                    .map(|rate| format!("{rate:+.1}/s"))
                    .unwrap_or_else(|| "-".into());
                (name, value, rate, self.series(counter, 150), color)
            })
            .collect();
        egui::Grid::new("live-counters")
            .num_columns(4)
            .spacing([18.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("計數器").weak().small());
                ui.label(RichText::new("目前").weak().small());
                ui.label(RichText::new("每秒").weak().small());
                ui.label(
                    RichText::new("最近 30 秒（單調計數器畫增量、gauge 畫值）")
                        .weak()
                        .small(),
                );
                ui.end_row();
                for (name, value, rate, values, color) in &table {
                    ui.monospace(*name);
                    ui.monospace(value.to_string());
                    ui.monospace(rate);
                    sparkline(ui, values, *color);
                    ui.end_row();
                }
            });

        ui.separator();
        let keys = status
            .fields
            .get("keys")
            .map(String::as_str)
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.label("目前按住：");
            if keys.is_empty() {
                ui.label(RichText::new("（無）").weak());
            } else {
                for key in keys.split(',') {
                    ui.label(RichText::new(key).monospace().strong());
                }
            }
            ui.label(format!(
                "• 滑鼠按鈕 {:02X}",
                latest.map(|s| s.buttons).unwrap_or(0)
            ));
        });
        ui.add_space(4.0);
        ui.collapsing("原始 STATUS", |ui| {
            ui.monospace(&status.raw);
        });
    }

    fn firmware_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("韌體燒錄");
        ui.label("固定目標：Seeed XIAO RP2040（純 Rust：embassy USB、Vendor HID v3）。");
        ui.add_enabled_ui(!self.busy, |ui| {
            text_field(
                ui,
                "cargo 路徑（空白＝從 PATH 找）",
                &mut self.settings.upload.cargo_path,
            );
            text_field(
                ui,
                "firmware 根目錄（空白＝隨附資料夾，需含 app/）",
                &mut self.settings.upload.firmware_path,
            );
            text_field(
                ui,
                "UF2 磁碟（空白＝自動偵測）",
                &mut self.settings.upload.boot_target,
            );
            ui.checkbox(
                &mut self.settings.upload.auto_bootloader,
                "透過選取的 HID 裝置自動進入 bootloader",
            );
            ui.checkbox(
                &mut self.settings.upload.skip_dependencies,
                "跳過 rustup target 安裝（已安裝 thumbv6m-none-eabi）",
            );
            ui.monospace(format!(
                "{} ({}) ← firmware/app",
                crate::firmware::TARGET,
                crate::firmware::APP_CRATE
            ));
            ui.label("無法由 HID 進入下載模式時，請按住 BOOTSEL 再上電。");
            if ui.button("編譯並燒錄").clicked() {
                let _ = self.settings.save();
                self.submit(AppCommand::Upload(self.settings.upload.clone()));
            }
        });
        if self.progress > 0 {
            ui.add(
                egui::ProgressBar::new(self.progress as f32 / 5.)
                    .text(format!("步驟 {}/5", self.progress)),
            );
        }
    }
    fn latency_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("鍵盤延遲量測");
        ui.label("量測主機送出 A 按下指令，至 Windows 觀察到按鍵的端到端耗時；含 USB、韌體、driver 與主機輪詢。");
        ui.add_enabled_ui(!self.busy,|ui| {
            ui.horizontal(|ui| {ui.label("測試輪數");ui.add(egui::DragValue::new(&mut self.settings.latency.rounds).range(1..=10000));});
            egui::ComboBox::from_id_salt("latency-mode").selected_text(self.settings.latency.mode.label()).show_ui(ui,|ui| {for mode in [Mode::Normal,Mode::HighPrecision,Mode::Extreme] {ui.selectable_value(&mut self.settings.latency.mode,mode,mode.label());}});
            if self.settings.latency.mode==Mode::Extreme {ui.label("極限模式只在送出指令時使用 REALTIME／TIME_CRITICAL；等待按鍵時改用高精度優先權，讓 Windows 處理輸入。每階段期限 1 秒。");}
            ui.label("請先放開 A，測試期間不要操作實體鍵盤，並保持 Windows 互動桌面可用。");
            if ui.add_enabled(self.connected.is_some(),egui::Button::new("開始測試")).clicked(){self.rounds.clear();self.report=None;self.stats=None;let _=self.settings.save();self.submit(AppCommand::Latency(self.settings.latency.clone()));}
        });
        if let Some(stats) = &self.stats {
            ui.separator();
            ui.label(format!(
                "成功 {} / 已執行 {} • 要求 {} • 未執行 {} • 失敗 {} • timeout {} • 成功率 {:.1}%",
                stats.successful,
                stats.attempted,
                stats.attempted + stats.unattempted,
                stats.unattempted,
                stats.failed,
                stats.timeouts,
                if stats.attempted == 0 {
                    0.
                } else {
                    stats.successful as f64 / stats.attempted as f64 * 100.
                }
            ));
            egui::Grid::new("latency-statistics")
                .num_columns(4)
                .show(ui, |ui| {
                    for row in [
                        [("最小", stats.min), ("最大", stats.max)],
                        [("平均", stats.average), ("中位數", stats.median)],
                        [("P95", stats.p95), ("P99", stats.p99)],
                        [("樣本標準差", stats.sample_stddev), ("", None)],
                    ] {
                        for (label, value) in row {
                            ui.label(label);
                            ui.monospace(
                                value
                                    .map(|v| format!("{v:.3} ms"))
                                    .unwrap_or_else(|| "—".into()),
                            );
                        }
                        ui.end_row();
                    }
                });
            if stats.successful == 0 {
                ui.colored_label(Color32::LIGHT_RED, "沒有成功樣本，無可用延遲統計。");
            }
            text_field(ui, "CSV 匯出路徑", &mut self.settings.csv_path);
            if ui.button("匯出 CSV").clicked() {
                let path = if self.settings.csv_path.trim().is_empty() {
                    crate::platform::application_dir()
                        .unwrap_or_default()
                        .join("latency.csv")
                } else {
                    self.settings.csv_path.clone().into()
                };
                if let Some(report) = &self.report {
                    match report.export(&path) {
                        Ok(()) => self.log(format!("CSV 已匯出：{}", path.display())),
                        Err(e) => self.log(format!("CSV 匯出失敗：{e:#}")),
                    }
                }
            }
        }
        let rounds: &[Round] = self
            .report
            .as_ref()
            .map(|report| &report.rounds)
            .unwrap_or(&self.rounds);
        egui::ScrollArea::vertical()
            .id_salt("rounds")
            .max_height(180.)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for round in rounds {
                    ui.monospace(format!(
                        "#{:<5} {:<12} {:?} {}",
                        round.round,
                        round
                            .latency_ms
                            .map(|v| format!("{v:.3} ms"))
                            .unwrap_or_else(|| "—".into()),
                        round.outcome,
                        round.error
                    ));
                }
            });
    }
    fn mouse_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("滑鼠測試");
        ui.label("擬人化移動控制主顯示器，座標為實體像素。開始前將游標放在主顯示器內。");
        let ready = !self.busy
            && self
                .connected
                .as_ref()
                .is_some_and(|(_, hello)| hello.mouse);
        ui.add_enabled_ui(ready, |ui| {
            ui.horizontal(|ui| {
                ui.label("X / ΔX");
                ui.add(egui::DragValue::new(&mut self.x).range(-100000..=100000));
                ui.label("Y / ΔY");
                ui.add(egui::DragValue::new(&mut self.y).range(-100000..=100000));
            });
            ui.horizontal(|ui| {
                ui.label("移動時間 ms（0＝自動）");
                ui.add(egui::DragValue::new(&mut self.duration).range(0..=2000));
            });
            ui.horizontal(|ui| {
                if ui.button("擬人化移到 X,Y").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::MoveTo {
                        x: self.x,
                        y: self.y,
                        duration_ms: self.duration,
                    }));
                }
                if ui.button("擬人化相對移動").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::MoveBy {
                        x: self.x,
                        y: self.y,
                        duration_ms: self.duration,
                    }));
                }
                if ui.button("原始相對移動").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Raw {
                        x: self.x,
                        y: self.y,
                    }));
                }
            });
            ui.separator();
            egui::ComboBox::from_id_salt("mouse-button")
                .selected_text(button_label(self.button))
                .show_ui(ui, |ui| {
                    for button in 1..=5 {
                        ui.selectable_value(&mut self.button, button, button_label(button));
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("單擊").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Click {
                        button: self.button,
                        count: 1,
                    }));
                }
                if ui.button("雙擊").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Click {
                        button: self.button,
                        count: 2,
                    }));
                }
                if ui.button("拖曳至 X,Y").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Drag {
                        button: self.button,
                        x: self.x,
                        y: self.y,
                        duration_ms: self.duration,
                    }));
                }
                if ui.button("按住").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Button {
                        button: self.button,
                        down: true,
                    }));
                }
                if ui.button("放開").clicked() {
                    self.submit(AppCommand::Mouse(MouseAction::Button {
                        button: self.button,
                        down: false,
                    }));
                }
            });
            ui.label("手動按住期間每 5 秒心跳；停止、取消、斷線或退出時嘗試全部放開。");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("垂直滾輪");
                ui.add(egui::DragValue::new(&mut self.vertical).range(-100000..=100000));
                ui.label("水平滾輪");
                ui.add(egui::DragValue::new(&mut self.horizontal).range(-100000..=100000));
            });
            if ui.button("傳送滾輪").clicked() {
                self.submit(AppCommand::Mouse(MouseAction::Wheel {
                    vertical: self.vertical,
                    horizontal: self.horizontal,
                }));
            }
        });
        if let Some(status) = &self.status {
            ui.label(format!("最後確認的按鍵位元：{:02X}", status.buttons()));
        }
    }
}
fn text_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
}
fn button_label(button: u8) -> &'static str {
    match button {
        1 => "1 左鍵",
        2 => "2 右鍵",
        3 => "3 中鍵",
        4 => "4 側鍵",
        _ => "5 側鍵",
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.events();
        if ctx.input(|i| i.viewport().close_requested()) && self.close_started.is_none() {
            self.close_started = Some(Instant::now());
            self.backend.shutdown();
            self.message = "正在取消工作並確認按鍵釋放…".into();
        }
        if let Some(start) = self.close_started {
            if self.stopped || start.elapsed() >= Duration::from_secs(5) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::Panel::top("navigation").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("FirmwareManageTool");
                ui.label("Rust • Windows 11 x64");
            });
            ui.horizontal(|ui| {
                for (page, label) in [
                    (Page::Device, "裝置"),
                    (Page::Status, "即時狀態"),
                    (Page::Firmware, "韌體燒錄"),
                    (Page::Latency, "鍵盤延遲"),
                    (Page::Mouse, "滑鼠測試"),
                ] {
                    ui.selectable_value(&mut self.page, page, label);
                }
            });
        });
        egui::Panel::bottom("logs")
            .resizable(true)
            .default_size(200.)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(if self.busy {
                        "● 工作中"
                    } else {
                        "● 就緒"
                    });
                    if ui
                        .add_enabled(self.busy, egui::Button::new("取消操作"))
                        .clicked()
                    {
                        self.backend.cancel();
                        self.message = "正在取消並釋放…".into();
                    }
                    if ui
                        .add_enabled(
                            self.connected.is_some() && self.close_started.is_none(),
                            egui::Button::new("全部放開 / 停止"),
                        )
                        .clicked()
                    {
                        if self.busy {
                            self.backend.cancel();
                        } else {
                            self.submit(AppCommand::Reset);
                        }
                    }
                    if ui.button("清除日誌").clicked() {
                        self.logs.clear();
                    }
                    if ui.button("保存設定").clicked()
                        && let Err(e) = self.settings.save()
                    {
                        self.log(format!("保存失敗：{e:#}"));
                    }
                });
                ui.label(&self.message);
                if let Some(ok) = self.cleanup {
                    ui.colored_label(
                        if ok {
                            Color32::LIGHT_GREEN
                        } else {
                            Color32::LIGHT_RED
                        },
                        if ok {
                            "已確認全部放開"
                        } else {
                            "釋放狀態未知"
                        },
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("operation-logs")
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.monospace(line);
                        }
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                Page::Device => self.device_page(ui),
                Page::Status => self.status_page(ui),
                Page::Firmware => self.firmware_page(ui),
                Page::Latency => self.latency_page(ui),
                Page::Mouse => self.mouse_page(ui),
            });
        });
        if !self.choices.is_empty() {
            let choices = self.choices.clone();
            egui::Window::new("選擇燒錄目標")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("確認實際 bootloader COM／磁碟；選擇後立即繼續上傳。");
                    for choice in choices {
                        if ui.button(&choice).clicked()
                            && self.backend.choices.try_send(choice).is_ok()
                        {
                            self.choices.clear();
                        }
                    }
                    if ui.button("取消燒錄").clicked() {
                        self.backend.cancel();
                        self.choices.clear();
                    }
                });
        }
    }
    fn on_exit(&mut self) {
        self.backend.shutdown();
        let _ = self.settings.save();
    }
}

/// A compact "current value" card for the live panel.
fn metric(ui: &mut egui::Ui, label: &str, value: String, hint: &str) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(112.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).small().weak());
            ui.label(RichText::new(value).heading());
            ui.label(RichText::new(hint).small().weak());
        });
    });
}

/// 30 s strip: filled background, step line, no axes.
fn sparkline(ui: &mut egui::Ui, values: &[f32], color: Color32) {
    let (response, painter) = ui.allocate_painter(egui::vec2(180.0, 22.0), egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    if values.len() < 2 {
        return;
    }
    let max = values.iter().copied().fold(0.0_f32, f32::max).max(1.0);
    let step = rect.width() / (values.len() - 1) as f32;
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = rect.left() + index as f32 * step;
            let y = rect.bottom() - 1.0 - (value / max) * (rect.height() - 3.0);
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.4, color)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_pages_render_without_a_device_and_report_handles_empty_statistics() {
        let context = egui::Context::default();
        let mut app = App::from_context(&context);
        app.report = Some(Report {
            config: crate::latency::LatencyConfig::default(),
            device: Device {
                path: "mock".into(),
                serial: String::new(),
                product: "測試裝置".into(),
                vid: 0,
                pid: 0,
            },
            unix_ms: 0,
            rounds: Vec::new(),
            cancelled: false,
            cleanup_confirmed: true,
            terminal_error: Some("尚未驗證".into()),
        });
        let stats = app.report.as_ref().unwrap().statistics();
        app.stats = Some(stats);
        for page in [
            Page::Device,
            Page::Status,
            Page::Firmware,
            Page::Latency,
            Page::Mouse,
        ] {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080., 780.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| match page {
                        Page::Device => app.device_page(ui),
                        Page::Status => app.status_page(ui),
                        Page::Firmware => app.firmware_page(ui),
                        Page::Latency => app.latency_page(ui),
                        Page::Mouse => app.mouse_page(ui),
                    });
                },
            );
            assert!(!output.shapes.is_empty());
            output.textures_delta.clear();
        }
    }

    /// The live page must render from a populated status, not only from "no
    /// device": that is where the rate and sparkline maths live.
    #[test]
    fn live_status_page_renders_with_samples() {
        use std::collections::BTreeMap;
        let context = egui::Context::default();
        let mut app = App::from_context(&context);
        let mut fields = BTreeMap::new();
        for (key, value) in [
            ("received", "120"),
            ("completed", "119"),
            ("pending", "3"),
            ("lease", "1750"),
            ("rx", "4"),
            ("tx", "1"),
            ("hid", "2"),
            ("fs", "7"),
            ("p", "2"),
            ("mb", "01"),
            ("keys", "a,shift"),
        ] {
            fields.insert(key.to_string(), value.to_string());
        }
        app.status = Some(Status {
            raw: "v3 live".into(),
            fields: fields.clone(),
        });
        let now = Instant::now();
        for index in 0..40u32 {
            app.samples.push_back(Sample {
                at: now - Duration::from_millis(200 * u64::from(40 - index)),
                received: 100 + index,
                completed: 99 + index,
                pending: 3,
                lease: 1750,
                rx: 4,
                tx: 1,
                hid: 2,
                failsafe: 7,
                held: 2,
                buttons: 1,
            });
        }
        app.status_at = Some(now);
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1080., 780.),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.status_page(ui));
            },
        );
        assert!(!output.shapes.is_empty());
        // Rate over one second of samples is ~5/s for `received`.
        let rate = app.rate(Counter::Received).unwrap();
        assert!((4.0..6.0).contains(&rate), "rate {rate}");
        assert_eq!(app.series(Counter::Lease, 40).len(), 40);
        output.textures_delta.clear();
    }
}
