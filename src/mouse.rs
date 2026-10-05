use crate::{
    cancel::Cancellation,
    hid::{HidTransport, Session},
    platform::{self, Screen},
    protocol::Status,
};
use anyhow::{Result, ensure};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MouseAction {
    MoveTo {
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    MoveBy {
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    Drag {
        button: u8,
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    Click {
        button: u8,
        count: u8,
    },
    Button {
        button: u8,
        down: bool,
    },
    Wheel {
        vertical: i32,
        horizontal: i32,
    },
    Raw {
        x: i32,
        y: i32,
    },
}
#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub at_ms: u64,
    pub x: i32,
    pub y: i32,
}
fn jerk(t: f64) -> f64 {
    t * t * t * (10.0 + t * (-15.0 + 6.0 * t))
}
fn bezier(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}
pub fn to_hid(pixel: i32, size: i32) -> Result<u16> {
    ensure!(size > 1, "顯示器尺寸無效");
    Ok(
        ((pixel.clamp(0, size - 1) as i64 * 65535 + (size - 1) as i64 / 2) / (size - 1) as i64)
            as u16,
    )
}
pub fn split_delta(mut x: i32, mut y: i32) -> Vec<(i32, i32)> {
    let mut parts = Vec::new();
    while x != 0 || y != 0 {
        let dx = x.clamp(-1024, 1024);
        let dy = y.clamp(-1024, 1024);
        parts.push((dx, dy));
        x -= dx;
        y -= dy;
    }
    parts
}
pub fn trajectory<R: Rng>(
    start: (i32, i32),
    target: (i32, i32),
    screen: Screen,
    duration_ms: u32,
    rng: &mut R,
) -> Result<Vec<Node>> {
    ensure!(screen.width > 1 && screen.height > 1, "顯示器尺寸無效");
    let target = (
        target.0.clamp(0, screen.width - 1),
        target.1.clamp(0, screen.height - 1),
    );
    let dx = (target.0 as f64) - (start.0 as f64);
    let dy = (target.1 as f64) - (start.1 as f64);
    let distance = dx.hypot(dy);
    let duration = if duration_ms == 0 {
        (150.0 + distance * 0.42 + rng.random_range(-25.0..=45.0)).clamp(170.0, 900.0) as u32
    } else {
        duration_ms.clamp(80, 2000)
    };
    if distance < 1.0 {
        return Ok(vec![Node {
            at_ms: 0,
            x: target.0,
            y: target.1,
        }]);
    }
    let nx = -dy / distance;
    let ny = dx / distance;
    let limit = (distance * 0.13).clamp(4.0, 110.0);
    let bend = rng.random_range(-limit..=limit);
    let a1 = rng.random_range(0.20..=0.38);
    let a2 = rng.random_range(0.62..=0.84);
    let bend2 = bend * 0.55 + rng.random_range(-limit * 0.20..=limit * 0.20);
    let c1 = (
        start.0 as f64 + dx * a1 + nx * bend,
        start.1 as f64 + dy * a1 + ny * bend,
    );
    let c2 = (
        start.0 as f64 + dx * a2 + nx * bend2,
        start.1 as f64 + dy * a2 + ny * bend2,
    );
    let phase = rng.random_range(0.0..std::f64::consts::TAU);
    let frequency = rng.random_range(1.1..=2.3);
    let amplitude = (distance * 0.003).clamp(0.15, 1.35);
    let steps = (duration / 8).clamp(10, 180);
    let mut nodes = Vec::new();
    let mut last = start;
    for step in 1..=steps {
        let p = step as f64 / steps as f64;
        let t = jerk(p);
        let jitter = amplitude
            * (std::f64::consts::PI * p).sin()
            * (phase + p * std::f64::consts::TAU * frequency).sin();
        let point = if step == steps {
            target
        } else {
            (
                (bezier(start.0 as f64, c1.0, c2.0, target.0 as f64, t) + nx * jitter).round()
                    as i32,
                (bezier(start.1 as f64, c1.1, c2.1, target.1 as f64, t) + ny * jitter).round()
                    as i32,
            )
        };
        let point = (
            point.0.clamp(0, screen.width - 1),
            point.1.clamp(0, screen.height - 1),
        );
        if point != last || step == steps {
            nodes.push(Node {
                at_ms: duration as u64 * step as u64 / steps as u64,
                x: point.0,
                y: point.1,
            });
            last = point;
        }
    }
    Ok(nodes)
}
pub fn current_node(nodes: &[Node], next: usize, elapsed_ms: u64) -> usize {
    nodes
        .partition_point(|node| node.at_ms <= elapsed_ms)
        .saturating_sub(1)
        .max(next)
        .min(nodes.len().saturating_sub(1))
}
fn set_button<T: HidTransport>(
    session: &mut Session<T>,
    button: u8,
    down: bool,
    cancel: &Cancellation,
) -> Result<()> {
    ensure!((1..=5).contains(&button), "按鍵編號須為 1–5");
    session.command(
        &format!("mouse_button:{button},{}", if down { "down" } else { "up" }),
        "ok:mouse_button",
        cancel,
    )?;
    Ok(())
}
struct ReleaseGuard<'a, T: HidTransport> {
    session: &'a mut Session<T>,
    armed: bool,
}
impl<T: HidTransport> Drop for ReleaseGuard<'_, T> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.session.reset(&Cancellation::default());
        }
    }
}
fn move_to<T: HidTransport>(
    session: &mut Session<T>,
    target: (i32, i32),
    duration: u32,
    cancel: &Cancellation,
) -> Result<()> {
    let (screen, start) = platform::screen_and_cursor()?;
    ensure!(
        start.0 >= 0 && start.0 < screen.width && start.1 >= 0 && start.1 < screen.height,
        "擬人化移動前，請將游標放在主顯示器內"
    );
    let nodes = trajectory(start, target, screen, duration, &mut rand::rng())?;
    let started = Instant::now();
    let mut index = 0;
    while index < nodes.len() {
        cancel.check()?;
        index = current_node(&nodes, index, started.elapsed().as_millis() as u64);
        let node = nodes[index];
        platform::wait_until(started + Duration::from_millis(node.at_ms), cancel)?;
        session.queue(
            &format!(
                "mouse_abs:{},{}",
                to_hid(node.x, screen.width)?,
                to_hid(node.y, screen.height)?
            ),
            "ok:mouse_abs",
            cancel,
        )?;
        index += 1;
    }
    session.finish(cancel)
}
pub fn execute<T: HidTransport>(
    session: &mut Session<T>,
    action: &MouseAction,
    cancel: &Cancellation,
) -> Result<Status> {
    let before = session.status(cancel)?;
    match *action {
        MouseAction::MoveTo { x, y, duration_ms } => move_to(session, (x, y), duration_ms, cancel)?,
        MouseAction::MoveBy { x, y, duration_ms } => {
            let (_, start) = platform::screen_and_cursor()?;
            move_to(
                session,
                (start.0.saturating_add(x), start.1.saturating_add(y)),
                duration_ms,
                cancel,
            )?;
        }
        MouseAction::Drag {
            button,
            x,
            y,
            duration_ms,
        } => {
            let mut guard = ReleaseGuard {
                session,
                armed: true,
            };
            set_button(guard.session, button, true, cancel)?;
            move_to(guard.session, (x, y), duration_ms, cancel)?;
            set_button(guard.session, button, false, cancel)?;
            guard.armed = false;
        }
        MouseAction::Click { button, count } => {
            ensure!(count == 1 || count == 2, "點擊次數須為 1 或 2");
            let mut guard = ReleaseGuard {
                session,
                armed: true,
            };
            for index in 0..count {
                set_button(guard.session, button, true, cancel)?;
                cancel.sleep(Duration::from_millis(55))?;
                set_button(guard.session, button, false, cancel)?;
                if index + 1 < count {
                    cancel.sleep(Duration::from_millis(150))?;
                }
            }
            guard.armed = false;
        }
        MouseAction::Button { button, down } => set_button(session, button, down, cancel)?,
        MouseAction::Raw { x, y } => {
            ensure!(
                x.unsigned_abs() <= 100_000 && y.unsigned_abs() <= 100_000,
                "單次位移限 ±100000"
            );
            for (dx, dy) in split_delta(x, y) {
                session.queue(&format!("mouse_move:{dx},{dy}"), "ok:mouse_move", cancel)?;
            }
            session.finish(cancel)?;
        }
        MouseAction::Wheel {
            vertical,
            horizontal,
        } => {
            ensure!(
                vertical.unsigned_abs() <= 100_000 && horizontal.unsigned_abs() <= 100_000,
                "單次滾輪限 ±100000"
            );
            for (name, amount) in [("mouse_wheel", vertical), ("mouse_hwheel", horizontal)] {
                for (chunk, _) in split_delta(amount, 0) {
                    session.queue(&format!("{name}:{chunk}"), &format!("ok:{name}"), cancel)?;
                }
            }
            session.finish(cancel)?;
        }
    }
    let after = session.status(cancel)?;
    ensure!(
        before.count("tx") == after.count("tx")
            && before.count("hid") == after.count("hid")
            && before.count("fs") == after.count("fs"),
        "操作期間裝置失敗計數增加，不能確認成功：{}",
        after.raw
    );
    Ok(after)
}
#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    #[test]
    fn pixel_endpoints() {
        assert_eq!(to_hid(0, 1920).unwrap(), 0);
        assert_eq!(to_hid(1919, 1920).unwrap(), 65535);
        assert_eq!(to_hid(-1, 1920).unwrap(), 0);
        assert!(to_hid(1, 1).is_err());
    }
    #[test]
    fn delta_boundary_and_conservation() {
        let parts = split_delta(-2049, 2050);
        assert!(
            parts
                .iter()
                .all(|(x, y)| x.abs() <= 1024 && y.abs() <= 1024)
        );
        assert_eq!(parts.iter().map(|p| p.0).sum::<i32>(), -2049);
        assert_eq!(parts.iter().map(|p| p.1).sum::<i32>(), 2050);
    }
    #[test]
    fn trajectory_exact_final_and_deduplication() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let nodes = trajectory(
            (10, 10),
            (1500, 700),
            Screen {
                width: 1920,
                height: 1080,
            },
            500,
            &mut rng,
        )
        .unwrap();
        let end = nodes.last().unwrap();
        assert_eq!((end.x, end.y, end.at_ms), (1500, 700, 500));
        assert!(nodes.windows(2).all(|p| p[0].at_ms < p[1].at_ms));
    }
    #[test]
    fn late_scheduler_skips_old_points() {
        let nodes = vec![
            Node {
                at_ms: 8,
                x: 0,
                y: 0,
            },
            Node {
                at_ms: 16,
                x: 1,
                y: 1,
            },
            Node {
                at_ms: 24,
                x: 2,
                y: 2,
            },
        ];
        assert_eq!(current_node(&nodes, 0, 21), 1);
        assert_eq!(current_node(&nodes, 0, 99), 2);
    }
    struct ClickFake {
        responses: std::collections::VecDeque<Vec<u8>>,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        cancel: Cancellation,
    }
    impl HidTransport for ClickFake {
        fn write(&mut self, report: &[u8]) -> Result<usize> {
            let command = String::from_utf8_lossy(&report[1..])
                .trim_matches(['\0', '\n'])
                .to_owned();
            self.log.lock().unwrap().push(command.clone());
            let line = if command == "status" {
                "ok:status,p=0,tx=0,hid=0,fs=0,mb=00"
            } else if command == "reset" {
                "ok:release_all"
            } else {
                "ok:mouse_button"
            };
            self.responses.push_back(
                [
                    vec![crate::protocol::IN_ID],
                    format!("{line}\n").into_bytes(),
                ]
                .concat(),
            );
            Ok(report.len())
        }
        fn read_timeout(&mut self, buffer: &mut [u8], _: i32) -> Result<usize> {
            let Some(report) = self.responses.pop_front() else {
                return Ok(0);
            };
            buffer[..report.len()].copy_from_slice(&report);
            if self
                .log
                .lock()
                .unwrap()
                .last()
                .is_some_and(|c| c.ends_with(",down"))
            {
                self.cancel.cancel();
            }
            Ok(report.len())
        }
    }
    #[test]
    fn cancelled_click_releases_after_acknowledged_press() {
        let token = Cancellation::default();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut session = Session::new(ClickFake {
            responses: Default::default(),
            log: log.clone(),
            cancel: token.clone(),
        });
        assert!(
            execute(
                &mut session,
                &MouseAction::Click {
                    button: 1,
                    count: 2
                },
                &token
            )
            .is_err()
        );
        let commands = log.lock().unwrap();
        assert_eq!(
            commands.iter().position(|s| s == "mouse_button:1,down"),
            Some(1)
        );
        assert!(commands.iter().skip(2).any(|s| s == "reset"));
    }
}
