//! Pure motion math shared by the GUI and every GDBOX SDK.
//!
//! No I/O, no clock, no platform calls: callers hand in a screen, a start point,
//! a target and an RNG. The trajectory is a list of host-side targets sampled at
//! `at_ms`; only relative deltas ever cross the USB channel.

use serde::{Deserialize, Serialize};

/// Deterministic RNG contract. Implementations must return `[0, 1)`.
pub trait Rng {
    fn unit(&mut self) -> f64;
}

/// Standard normal sample (Box–Muller, two uniforms, no state kept between
/// calls so a seed still replays exactly).
pub fn normal(rng: &mut impl Rng) -> f64 {
    let u1 = rng.unit().max(1e-12);
    let u2 = rng.unit();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Log-normal sample with the given median and log-space sigma. Human timing is
/// right-skewed (long pauses, few short ones), which a uniform spread cannot
/// reproduce; `sigma` 0.2–0.4 matches keystroke and click distributions.
pub fn lognormal(rng: &mut impl Rng, median: f64, sigma: f64) -> f64 {
    (median.max(1e-9).ln() + sigma.max(0.0) * normal(rng)).exp()
}

/// SplitMix64: tiny, seedable, identical on every platform.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn from_entropy() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self(nanos ^ (std::process::id() as u64).rotate_left(32) ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Rng for SplitMix64 {
    fn unit(&mut self) -> f64 {
        // 53 significant bits, the same value every language can reproduce.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub width: i32,
    pub height: i32,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub at_ms: u64,
    pub x: i32,
    pub y: i32,
}

/// How progress maps to time. The first three variants are the GUI's original
/// profiles and keep their serialized names so stored settings stay valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Ease {
    #[default]
    Jerk,
    EaseInOut,
    Linear,
    EaseIn,
    EaseOut,
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

impl Ease {
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            // The original "jerk" curve: smooth at both ends, no overshoot.
            Self::Jerk => t * t * t * (10.0 + t * (-15.0 + 6.0 * t)),
            Self::EaseIn => t * t * t,
            Self::EaseOut => {
                let u = 1.0 - t;
                1.0 - u * u * u
            }
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier_ease(t, x1, y1, x2, y2),
        }
    }
}

/// Undo the x axis of a cubic bezier (Newton, fixed iteration count so the
/// result does not depend on floating-point convergence luck), then read y.
fn cubic_bezier_ease(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let x1 = x1.clamp(0.0, 1.0);
    let x2 = x2.clamp(0.0, 1.0);
    let curve_x = |u: f64| bezier(0.0, x1, x2, 1.0, u);
    let curve_y = |u: f64| bezier(0.0, y1, y2, 1.0, u);
    let mut u = t;
    for _ in 0..8 {
        let x = curve_x(u) - t;
        if x.abs() < 1e-9 {
            break;
        }
        let slope = (curve_x((u + 1e-6).min(1.0)) - curve_x((u - 1e-6).max(0.0))) / 2e-6;
        if slope.abs() < 1e-9 {
            break;
        }
        u = (u - x / slope).clamp(0.0, 1.0);
    }
    curve_y(u).clamp(0.0, 1.0)
}

fn bezier(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}

/// Human-ish movement profile. Field names and defaults are part of the SDK API.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HumanProfile {
    /// 0 = straight line, 1 = the original bow, 2 = exaggerated arc.
    pub curve: f64,
    /// Micro-jitter amplitude in pixels (0 = perfectly smooth).
    pub jitter_px: f64,
    /// Percent of the distance the pointer overshoots before correcting.
    pub overshoot_pct: f64,
    /// Pause at the end, the way a hand settles before clicking (ms).
    pub settle_ms: u32,
    /// How progress maps to time.
    pub ease: Ease,
    /// Ignore steps smaller than this, which looks like a coarser sensor.
    pub min_step_px: i32,
    /// How much of every field is re-sampled per move, 0..=1. At 0 the fields
    /// are used literally, so two identical moves produce identical paths (that
    /// is what makes them look scripted); at 1 the curve, the overshoot and the
    /// settle time are drawn fresh each time.
    pub organic: f64,
}

impl Default for HumanProfile {
    fn default() -> Self {
        Self {
            curve: 1.0,
            jitter_px: 1.0,
            overshoot_pct: 0.0,
            settle_ms: 0,
            ease: Ease::Jerk,
            min_step_px: 1,
            organic: 0.6,
        }
    }
}

impl HumanProfile {
    /// A perfectly straight, constant-speed path with no jitter or overshoot.
    pub fn linear() -> Self {
        Self {
            curve: 0.0,
            jitter_px: 0.0,
            overshoot_pct: 0.0,
            settle_ms: 0,
            ease: Ease::Linear,
            min_step_px: 1,
            organic: 0.0,
        }
    }

    /// The defaults, spelled out: a curved, lightly jittered path that
    /// occasionally overshoots a long move and settles before a click.
    pub fn organic() -> Self {
        Self::default()
    }
}

/// Splits one relative move into HID-sized chunks (±1024 per axis).
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

/// Distance-adaptive duration used when the caller passes `duration_ms = 0`.
///
/// Fitts' law shape (`a + b·log2(1 + D/W)`, W ≈ a key-sized target) so a short
/// correction stays quick and a long traverse takes longer, times a log-normal
/// factor: human move times are right-skewed, not uniform.
pub fn natural_duration(distance: f64, rng: &mut impl Rng) -> u32 {
    let base = 120.0 + 140.0 * (1.0 + distance.max(0.0) / 120.0).log2();
    (base * lognormal(rng, 1.0, 0.18)).clamp(170.0, 900.0) as u32
}

/// Samples a human-ish path from `start` to `target` on `screen`.
///
/// `duration_ms = 0` means "pick a natural duration". The returned nodes are
/// host coordinates; `at_ms` is relative to the movement start.
pub fn trajectory_with(
    start: (i32, i32),
    target: (i32, i32),
    screen: Screen,
    duration_ms: u32,
    profile: &HumanProfile,
    rng: &mut impl Rng,
) -> crate::Result<Vec<Node>> {
    if screen.width <= 1 || screen.height <= 1 {
        return Err(MotionError::InvalidScreen);
    }
    let target = (
        target.0.clamp(0, screen.width - 1),
        target.1.clamp(0, screen.height - 1),
    );
    let dx = f64::from(target.0) - f64::from(start.0);
    let dy = f64::from(target.1) - f64::from(start.1);
    let distance = dx.hypot(dy);
    let duration = if duration_ms == 0 {
        natural_duration(distance, rng)
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
    // Everything below is drawn per move. A fixed curvature, a fixed overshoot
    // and a fixed settle are repeated identically on every move, which is the
    // clearest sign of a scripted pointer; `organic` decides how much of each
    // value is re-sampled (0 keeps the literal values, for tests and for
    // callers that want a straight line).
    let organic = profile.organic.clamp(0.0, 1.0);
    let curve = profile.curve.clamp(0.0, 2.0) * (1.0 + (rng.unit() * 2.0 - 1.0) * 0.35 * organic);
    let overshoot_pct = if profile.overshoot_pct > 0.0 {
        profile.overshoot_pct
    } else if distance > 350.0 && rng.unit() < 0.35 * organic {
        1.0 + rng.unit() * 2.0 * organic
    } else {
        0.0
    };
    let settle_ms = if profile.settle_ms > 0 {
        (f64::from(profile.settle_ms) * (1.0 + (rng.unit() * 2.0 - 1.0) * 0.3 * organic))
            .clamp(1.0, 400.0) as u32
    } else {
        0
    };
    let over = (distance * overshoot_pct / 100.0).clamp(0.0, 160.0);
    let aim = if over > 0.5 {
        (
            (f64::from(target.0) + dx / distance * over).clamp(0.0, f64::from(screen.width - 1)),
            (f64::from(target.1) + dy / distance * over).clamp(0.0, f64::from(screen.height - 1)),
        )
    } else {
        (f64::from(target.0), f64::from(target.1))
    };
    let limit = (distance * 0.13).clamp(4.0, 110.0) * curve;
    let bend = (rng.unit() * 2.0 - 1.0) * limit;
    let a1 = 0.20 + rng.unit() * 0.18;
    let a2 = 0.62 + rng.unit() * 0.22;
    let bend2 = bend * 0.55 + (rng.unit() * 2.0 - 1.0) * limit * 0.20;
    let c1 = (
        f64::from(start.0) + dx * a1 + nx * bend,
        f64::from(start.1) + dy * a1 + ny * bend,
    );
    let c2 = (
        f64::from(start.0) + dx * a2 + nx * bend2,
        f64::from(start.1) + dy * a2 + ny * bend2,
    );
    let phase = rng.unit() * std::f64::consts::TAU;
    let frequency = 1.1 + rng.unit() * 1.2;
    let amplitude = profile.jitter_px.clamp(0.0, 24.0);
    let steps = (duration / 8).clamp(10, 180);
    let mut nodes = Vec::new();
    let mut last = start;
    for step in 1..=steps {
        let p = f64::from(step) / f64::from(steps);
        let t = profile.ease.apply(p);
        let jitter = amplitude
            * (std::f64::consts::PI * p).sin()
            * (phase + p * std::f64::consts::TAU * frequency).sin();
        let point = if step == steps {
            (aim.0.round() as i32, aim.1.round() as i32)
        } else {
            (
                (bezier(f64::from(start.0), c1.0, c2.0, aim.0, t) + nx * jitter).round() as i32,
                (bezier(f64::from(start.1), c1.1, c2.1, aim.1, t) + ny * jitter).round() as i32,
            )
        };
        let point = (
            point.0.clamp(0, screen.width - 1),
            point.1.clamp(0, screen.height - 1),
        );
        let step_gap = (point.0 - last.0).abs() + (point.1 - last.1).abs();
        if step_gap >= profile.min_step_px.max(1) || step == steps {
            nodes.push(Node {
                at_ms: u64::from(duration) * step as u64 / steps as u64,
                x: point.0,
                y: point.1,
            });
            last = point;
        }
    }
    if over > 0.5 {
        let settle_at = nodes.last().map(|node| node.at_ms).unwrap_or(0) + 90;
        nodes.push(Node {
            at_ms: settle_at,
            x: target.0,
            y: target.1,
        });
    }
    if profile.settle_ms > 0 {
        let last_at = nodes.last().map(|node| node.at_ms).unwrap_or(0);
        nodes.push(Node {
            at_ms: last_at + u64::from(settle_ms),
            x: target.0,
            y: target.1,
        });
    }
    Ok(nodes)
}

/// Picks the node that should be active at `elapsed_ms`, never moving backwards.
pub fn current_node(nodes: &[Node], next: usize, elapsed_ms: u64) -> usize {
    nodes
        .partition_point(|node| node.at_ms <= elapsed_ms)
        .saturating_sub(1)
        .max(next)
        .min(nodes.len().saturating_sub(1))
}

/// The error type is deliberately tiny: the SDK maps it to its unified codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionError {
    InvalidScreen,
}

pub type Result<T> = std::result::Result<T, MotionError>;

impl std::fmt::Display for MotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScreen => write!(f, "screen size is invalid"),
        }
    }
}

impl std::error::Error for MotionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_profile_hits_the_target_and_stays_on_the_line() {
        let nodes = trajectory_with(
            (100, 100),
            (500, 300),
            Screen::default(),
            400,
            &HumanProfile::linear(),
            &mut SplitMix64::new(7),
        )
        .unwrap();
        let last = nodes.last().unwrap();
        assert_eq!((last.x, last.y), (500, 300));
        for node in &nodes {
            // y = 0.5x + 50 for this pair.
            let expected = node.x / 2 + 50;
            assert!((node.y - expected).abs() <= 1, "{node:?}");
        }
    }

    #[test]
    fn the_same_seed_produces_the_same_path() {
        let profile = HumanProfile::default();
        let a = trajectory_with(
            (10, 20),
            (900, 500),
            Screen::default(),
            300,
            &profile,
            &mut SplitMix64::new(42),
        )
        .unwrap();
        let b = trajectory_with(
            (10, 20),
            (900, 500),
            Screen::default(),
            300,
            &profile,
            &mut SplitMix64::new(42),
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn easing_curves_are_monotonic_and_pinned_at_both_ends() {
        for ease in [
            Ease::Linear,
            Ease::Jerk,
            Ease::EaseIn,
            Ease::EaseOut,
            Ease::EaseInOut,
            Ease::CubicBezier {
                x1: 0.25,
                y1: 0.1,
                x2: 0.25,
                y2: 1.0,
            },
        ] {
            assert!((ease.apply(0.0)).abs() < 1e-6, "{ease:?}");
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-6, "{ease:?}");
            let mut previous = 0.0;
            for step in 0..=100 {
                let value = ease.apply(f64::from(step) / 100.0);
                assert!(value + 1e-6 >= previous, "{ease:?} at {step}");
                previous = value;
            }
        }
    }

    #[test]
    fn overshoot_and_settle_extend_the_path_without_missing_the_target() {
        let profile = HumanProfile {
            overshoot_pct: 12.0,
            settle_ms: 40,
            ..HumanProfile::default()
        };
        let nodes = trajectory_with(
            (0, 0),
            (400, 0),
            Screen::default(),
            200,
            &profile,
            &mut SplitMix64::new(1),
        )
        .unwrap();
        let last = nodes.last().unwrap();
        assert_eq!((last.x, last.y), (400, 0));
        let overshot = nodes.iter().any(|node| node.x > 400 || node.x < 0);
        assert!(overshot, "expected an overshoot node");
    }

    #[test]
    fn split_delta_covers_large_moves_in_hid_sized_chunks() {
        assert_eq!(split_delta(0, 0), Vec::<(i32, i32)>::new());
        assert_eq!(split_delta(1025, -1), vec![(1024, -1), (1, 0)]);
    }

    #[test]
    fn lognormal_is_right_skewed_around_its_median() {
        let mut rng = SplitMix64::new(11);
        let samples: Vec<f64> = (0..4000).map(|_| lognormal(&mut rng, 100.0, 0.3)).collect();
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let mut sorted = samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        assert!(
            mean > median * 1.02,
            "a log-normal has a mean above its median, mean={mean} median={median}"
        );
        assert!(sorted[0] >= 0.0);
    }

    #[test]
    fn natural_duration_grows_with_distance_and_stays_in_bounds() {
        let mut near = SplitMix64::new(3);
        let mut far = SplitMix64::new(3);
        let near_times: Vec<u32> = (0..60)
            .map(|_| natural_duration(120.0, &mut near))
            .collect();
        let far_times: Vec<u32> = (0..60)
            .map(|_| natural_duration(1800.0, &mut far))
            .collect();
        let average = |values: &[u32]| values.iter().sum::<u32>() as f64 / values.len() as f64;
        assert!(
            average(&far_times) > average(&near_times) * 1.3,
            "Fitts' law: far moves take longer (near {}, far {})",
            average(&near_times),
            average(&far_times)
        );
        assert!(near_times.iter().all(|ms| (170..=900).contains(ms)));
        assert!(far_times.iter().all(|ms| (170..=900).contains(ms)));
    }

    #[test]
    fn organic_moves_do_not_repeat_one_path() {
        let profile = HumanProfile::organic();
        let a = trajectory_with(
            (10, 20),
            (900, 500),
            Screen::default(),
            260,
            &profile,
            &mut SplitMix64::new(1),
        )
        .unwrap();
        let b = trajectory_with(
            (10, 20),
            (900, 500),
            Screen::default(),
            260,
            &profile,
            &mut SplitMix64::new(2),
        )
        .unwrap();
        assert_ne!(a, b, "two seeds must not produce the identical path");
        // The endpoint still lands on the target, overshoot or not.
        let last = b.last().unwrap();
        assert_eq!((last.x, last.y), (900, 500));
    }

    #[test]
    fn organic_curvature_varies_between_moves() {
        let profile = HumanProfile::organic();
        // Sagitta of the path: how far the middle of the path sits off the
        // straight line. A fixed curve repeats the same value every time.
        let mut sagittas = Vec::new();
        for seed in 0..8u64 {
            let nodes = trajectory_with(
                (0, 0),
                (800, 0),
                Screen::default(),
                300,
                &profile,
                &mut SplitMix64::new(seed),
            )
            .unwrap();
            let middle = &nodes[nodes.len() / 2];
            sagittas.push(middle.y.abs());
        }
        let first = sagittas[0];
        assert!(
            sagittas.iter().any(|value| *value != first),
            "organic curvature must differ between moves: {sagittas:?}"
        );
    }

    #[test]
    fn linear_profiles_never_overshoot_or_jitter() {
        let mut rng = SplitMix64::new(9);
        for _ in 0..20 {
            let nodes = trajectory_with(
                (0, 0),
                (1200, 0),
                Screen::default(),
                200,
                &HumanProfile::linear(),
                &mut rng,
            )
            .unwrap();
            assert!(nodes.iter().all(|node| node.x <= 1200 && node.y == 0));
        }
    }
}
