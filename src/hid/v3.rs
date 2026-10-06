use super::*;
use input_protocol::{KeyId, v3::Command};
impl<T: HidTransport> Session<T> {
    pub(super) fn v3_pump(&mut self, timeout: i32) -> Result<()> {
        let mut r = [0; 64];
        let n = match self.transport.read_timeout(&mut r, timeout) {
            Ok(n) => n,
            Err(e) => return self.fail(e),
        };
        if n == 0 {
            return Ok(());
        }
        if n > 64 {
            return self.fail(anyhow::anyhow!("HID 回應長度錯誤"));
        }
        if r[0] == 11 {
            return Ok(());
        } // old v2 responses cannot acknowledge a v3 request
        if let Err(e) = self.binary.as_mut().unwrap().receive(&r[..n]) {
            return self.fail(e);
        }
        Ok(())
    }
    pub(super) fn v3_send(&mut self, command: &Command, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        ensure!(self.valid, "HID session 已失效");
        self.v3_pump(0)?;
        let r = self.binary.as_mut().unwrap().prepare(command)?;
        match self.transport.write(&r) {
            Ok(64) => {}
            Ok(n) => return self.fail(anyhow::anyhow!("HID 短寫入 {n}/64")),
            Err(e) => return self.fail(e),
        }
        if command.expects_reply() {
            let started = Instant::now();
            loop {
                if let Err(e) = cancel.check() {
                    return self.fail(e);
                }
                if started.elapsed() >= Duration::from_secs(1) {
                    return self.fail(anyhow::anyhow!("v3 控制回應逾時"));
                }
                self.v3_pump(10)?;
                if self.binary.as_mut().unwrap().take_reply().is_some() {
                    break;
                }
            }
        }
        Ok(())
    }
    pub fn input(&mut self, command: Command, cancel: &Cancellation) -> Result<()> {
        if self.binary.is_some() {
            return self.v3_send(&command, cancel);
        }
        match command {
            Command::Key(k, down) => self.queue(
                &format!("{}:{}", if down { "d" } else { "u" }, k.legacy_token()?),
                if down {
                    "ok:down|ok:already_down"
                } else {
                    "ok:up|ok:already_up"
                },
                cancel,
            ),
            Command::Snapshot(state) => {
                ensure!(state.consumer == 0, "v2 不支援多媒體鍵");
                let keys = state.pressed();
                ensure!(
                    keys.iter()
                        .filter(|k| k.usage_page == 7 && k.usage < 0xe0)
                        .count()
                        <= 6,
                    "v2 最多 6 個一般鍵"
                );
                let names = keys
                    .into_iter()
                    .map(KeyId::legacy_token)
                    .collect::<Result<Vec<_>>>()?;
                self.queue(
                    &format!("sync:{}", names.join(",")),
                    "ok:sync|ok:sync_unchanged",
                    cancel,
                )?;
                self.input(Command::MouseButtons(state.buttons), cancel)
            }
            Command::MouseMove(x, y) => {
                self.queue(&format!("mouse_move:{x},{y}"), "ok:mouse_move", cancel)
            }
            Command::MouseButtons(buttons) => {
                ensure!(buttons & !31 == 0, "無效滑鼠按鈕");
                for (bit, id) in [(1, 1), (2, 3), (4, 2), (8, 4), (16, 5)] {
                    if (buttons ^ self.buttons) & bit != 0 {
                        self.queue(
                            &format!(
                                "mouse_button:{id},{}",
                                if buttons & bit != 0 { "down" } else { "up" }
                            ),
                            "ok:mouse_button",
                            cancel,
                        )?;
                    }
                }
                self.buttons = buttons;
                Ok(())
            }
            Command::Wheel(v, h) => {
                if v != 0 {
                    self.queue(&format!("mouse_wheel:{v}"), "ok:mouse_wheel", cancel)?;
                }
                if h != 0 {
                    self.queue(&format!("mouse_hwheel:{h}"), "ok:mouse_hwheel", cancel)?;
                }
                Ok(())
            }
            Command::Heartbeat => self.command("ping", "pong", cancel).map(|_| ()),
            Command::Barrier => self.finish(cancel),
            Command::ReleaseAll => self.reset(cancel),
            Command::Bootloader => self
                .command("enter_bootloader", "ok:enter_bootloader", cancel)
                .map(|_| ()),
            Command::Status => self.status(cancel).map(|_| ()),
            Command::Open => anyhow::bail!("v2 使用 hello 建立連線"),
        }
    }
    /// Long pointer waits still renew the lease, even if all intermediate pixels were deduplicated.
    pub fn wait_until(&mut self, deadline: Instant, cancel: &Cancellation) -> Result<()> {
        let interval = self.heartbeat_interval();
        while deadline.saturating_duration_since(Instant::now()) > interval {
            cancel.sleep(interval)?;
            self.input(Command::Heartbeat, cancel)?;
        }
        crate::platform::wait_until(deadline, cancel)
    }
    pub fn needs_heartbeat(&self, held: bool) -> bool {
        self.binary.is_some() || held
    }
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_millis(if self.binary.is_some() { 500 } else { 5000 })
    }
    pub fn key(&mut self, key: KeyId, down: bool, cancel: &Cancellation) -> Result<()> {
        self.input(Command::Key(key, down), cancel)
    }
    pub fn set_button(&mut self, button: u8, down: bool, cancel: &Cancellation) -> Result<()> {
        let bit = match button {
            1 => 1,
            2 => 4,
            3 => 2,
            4 => 8,
            5 => 16,
            _ => anyhow::bail!("無效滑鼠按鈕"),
        };
        let current = self
            .binary
            .as_ref()
            .map(|b| b.state.buttons)
            .unwrap_or(self.buttons);
        self.input(
            Command::MouseButtons(if down { current | bit } else { current & !bit }),
            cancel,
        )?;
        self.finish(cancel)
    }
}
