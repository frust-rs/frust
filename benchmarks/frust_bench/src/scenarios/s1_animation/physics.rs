//! The bubble simulation — a 1:1 port of the Flutter repro's
//! `bubble_physics.dart`/`bubble.dart` (flutter/flutter#180958): same
//! constants, same per-frame update order (center gravity → pairwise
//! collisions → touch repulsion → friction → wall constraints → integrate →
//! small-velocity zeroing), same settled detection (every bubble below the
//! velocity threshold for 30 consecutive frames stops the animation until a
//! touch wakes it).
//!
//! Pure data + math: no Frust, no clock. One [`BubblePhysics::update`] call
//! is one frame's step, exactly like the repro's per-vsync `Ticker` callback —
//! the chart widget calls it once per painted frame.

use kurbo::Point;

/// One animated bubble: a ticker symbol, a fixed performance percentage
/// (colors the bubble and its `+x.x%` label), and mutable position/velocity.
pub struct Bubble {
    pub symbol: &'static str,
    /// Signed percentage in `-20.0..20.0`; fixed at init.
    pub performance: f64,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    /// Fixed at init: a fraction of `S = min(play_width, play_height)` (the
    /// SafeArea-inset play region), per the S1 size-parity spec below — not an
    /// absolute logical-pixel value.
    pub radius: f64,
}

impl Bubble {
    /// Current speed (velocity magnitude), the settle-detection input.
    pub fn speed(&self) -> f64 {
        (self.vx * self.vx + self.vy * self.vy).sqrt()
    }
}

/// The repro's 30-symbol ticker list, cycled across bubbles.
const SYMBOLS: [&str; 30] = [
    "BTC", "ETH", "SOL", "BNB", "XRP", "ADA", "DOGE", "AVAX", "DOT", "LINK", "MATIC", "UNI",
    "SHIB", "LTC", "ATOM", "TRX", "XLM", "ETC", "FIL", "NEAR", "APT", "OP", "ARB", "SUI", "INJ",
    "SEI", "FTM", "ALGO", "VET", "SAND",
];

// Physics constants — verbatim from the Dart repro.
const FRICTION: f64 = 0.90;
const CENTER_PULL: f64 = 0.001;
const COLLISION_STRENGTH: f64 = 0.08;
const TOUCH_REPULSION: f64 = 0.9;
const TOUCH_RADIUS: f64 = 200.0;
const WALL_BOUNCE: f64 = 0.35;
const SETTLED_VELOCITY_THRESHOLD: f64 = 0.1;
const SETTLED_FRAME_COUNT: u32 = 30;

// --- S1 size-parity spec (canonical) -------------------------------------
//
// Contract home: `benchmarks/flutter_bench/lib/bench/datasets.dart`'s S1
// section — these constants mirror it byte-for-byte.
//
// Bubble radius and the initial cluster spread are FRACTIONS of
// `S = min(play_width, play_height)` — the SafeArea-inset play region on both
// apps — not absolute logical pixels. This makes the bubble-size-to-play-area
// ratio (hence collision density and settle behavior) identical across frust
// and Flutter regardless of any residual difference in each shell's reported
// logical size, which is what makes S1 a valid head-to-head. The velocity
// constants above are dimensionally consistent and stay byte-identical between
// the two apps, so a given `S` reproduces a byte-identical simulation; the four
// per-bubble RNG draws (performance, radius, x, y) keep their original order so
// `seed 42` yields the same sequence on both sides. Fractions are anchored so a
// phone-sized play area (`S ≈ 380–400` px on the reference OnePlus 9) resolves
// to the original ~20–60 px radii.
const RADIUS_MIN_FRAC: f64 = 0.05;
const RADIUS_SPAN_FRAC: f64 = 0.10;
const CLUSTER_SPAN_FRAC: f64 = 0.5;

/// splitmix64 — a tiny deterministic PRNG so `seed 42` reproduces the same
/// layout every run (the repro seeds Dart's `Random(42)`; the exact sequence
/// differs, the deterministic-given-a-seed property is what matters).
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`, from the top 53 bits.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The simulation: the bubble set plus the active touch point and the
/// settled-state tracker.
#[derive(Default)]
pub struct BubblePhysics {
    pub bubbles: Vec<Bubble>,
    touch: Option<Point>,
    width: f64,
    height: f64,
    center_x: f64,
    center_y: f64,
    consecutive_settled_frames: u32,
    /// Whether the animation is fully settled (30 consecutive below-threshold
    /// frames with no active touch).
    pub is_fully_settled: bool,
}

impl BubblePhysics {
    /// Set (or update) the field size; clamps existing bubbles inside it.
    pub fn set_size(&mut self, width: f64, height: f64) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.width = width;
        self.height = height;
        self.center_x = width / 2.0;
        self.center_y = height / 2.0;
        for b in &mut self.bubbles {
            b.x = b.x.clamp(b.radius, self.width - b.radius);
            b.y = b.y.clamp(b.radius, self.height - b.radius);
        }
    }

    /// (Re)seed `count` bubbles clustered around the center, per the S1
    /// size-parity spec: performance in `-20..20`, radius and cluster spread as
    /// FRACTIONS of `S = min(width, height)` (the SafeArea-inset play region),
    /// so the bubble-to-play-area ratio matches the Flutter side regardless of
    /// absolute logical size. Requires [`set_size`](Self::set_size) to have run
    /// first (the chart widget's `layout` pass guarantees this).
    pub fn initialize_bubbles(&mut self, count: usize, seed: u64) {
        self.bubbles.clear();
        self.consecutive_settled_frames = 0;
        self.is_fully_settled = false;
        let s = self.width.min(self.height);
        let mut rng = SplitMix64(seed);
        for i in 0..count {
            let performance = rng.next_f64() * 40.0 - 20.0;
            let radius = (RADIUS_MIN_FRAC + rng.next_f64() * RADIUS_SPAN_FRAC) * s;
            let x =
                self.center_x + (rng.next_f64() * CLUSTER_SPAN_FRAC - CLUSTER_SPAN_FRAC / 2.0) * s;
            let y =
                self.center_y + (rng.next_f64() * CLUSTER_SPAN_FRAC - CLUSTER_SPAN_FRAC / 2.0) * s;
            self.bubbles.push(Bubble {
                symbol: SYMBOLS[i % SYMBOLS.len()],
                performance,
                x,
                y,
                vx: 0.0,
                vy: 0.0,
                radius,
            });
        }
    }

    /// The active touch point (in field coordinates), if any.
    pub fn touch(&self) -> Option<Point> {
        self.touch
    }

    /// Set or clear the active touch point.
    pub fn set_touch(&mut self, touch: Option<Point>) {
        self.touch = touch;
    }

    /// One frame's physics step, in the repro's exact phase order.
    pub fn update(&mut self) {
        self.apply_center_gravity();
        self.apply_collisions();
        self.apply_touch_repulsion();
        self.apply_friction();
        self.apply_wall_constraints();
        for b in &mut self.bubbles {
            b.x += b.vx;
            b.y += b.vy;
        }
        for b in &mut self.bubbles {
            if b.vx.abs() < 0.05 {
                b.vx = 0.0;
            }
            if b.vy.abs() < 0.05 {
                b.vy = 0.0;
            }
        }
    }

    fn apply_center_gravity(&mut self) {
        for b in &mut self.bubbles {
            b.vx += (self.center_x - b.x) * CENTER_PULL;
            b.vy += (self.center_y - b.y) * CENTER_PULL;
        }
    }

    fn apply_collisions(&mut self) {
        for i in 0..self.bubbles.len() {
            for j in (i + 1)..self.bubbles.len() {
                let (dx, dy, distance, min_distance) = {
                    let (a, b) = (&self.bubbles[i], &self.bubbles[j]);
                    let dx = b.x - a.x;
                    let dy = b.y - a.y;
                    ((dx), (dy), (dx * dx + dy * dy).sqrt(), a.radius + b.radius)
                };
                if distance < min_distance && distance > 0.0 {
                    let nx = dx / distance;
                    let ny = dy / distance;
                    let force = (min_distance - distance) * COLLISION_STRENGTH;
                    self.bubbles[i].vx -= nx * force;
                    self.bubbles[i].vy -= ny * force;
                    self.bubbles[j].vx += nx * force;
                    self.bubbles[j].vy += ny * force;
                }
            }
        }
    }

    fn apply_touch_repulsion(&mut self) {
        let Some(touch) = self.touch else { return };
        for b in &mut self.bubbles {
            let dx = b.x - touch.x;
            let dy = b.y - touch.y;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < TOUCH_RADIUS && distance > 0.0 {
                let force = (1.0 - distance / TOUCH_RADIUS) * TOUCH_REPULSION;
                b.vx += (dx / distance) * force;
                b.vy += (dy / distance) * force;
            }
        }
    }

    fn apply_friction(&mut self) {
        for b in &mut self.bubbles {
            b.vx *= FRICTION;
            b.vy *= FRICTION;
        }
    }

    fn apply_wall_constraints(&mut self) {
        for b in &mut self.bubbles {
            if b.x - b.radius < 0.0 {
                b.x = b.radius;
                b.vx = b.vx.abs() * WALL_BOUNCE;
            }
            if b.x + b.radius > self.width {
                b.x = self.width - b.radius;
                b.vx = -b.vx.abs() * WALL_BOUNCE;
            }
            if b.y - b.radius < 0.0 {
                b.y = b.radius;
                b.vy = b.vy.abs() * WALL_BOUNCE;
            }
            if b.y + b.radius > self.height {
                b.y = self.height - b.radius;
                b.vy = -b.vy.abs() * WALL_BOUNCE;
            }
        }
    }

    /// Whether the next frame still needs to run — the repro's `needsRepaint`:
    /// `true` while any bubble moves (or a touch is active); flips to `false`
    /// only after [`SETTLED_FRAME_COUNT`] consecutive below-threshold frames.
    /// Mutates the settle tracker, so call it exactly once per frame.
    pub fn needs_repaint(&mut self) -> bool {
        if self.touch.is_some() {
            self.consecutive_settled_frames = 0;
            self.is_fully_settled = false;
            return true;
        }
        let all_settled = self
            .bubbles
            .iter()
            .all(|b| b.speed() < SETTLED_VELOCITY_THRESHOLD);
        if all_settled {
            self.consecutive_settled_frames += 1;
            if self.consecutive_settled_frames >= SETTLED_FRAME_COUNT {
                self.is_fully_settled = true;
                return false;
            }
        } else {
            self.consecutive_settled_frames = 0;
            self.is_fully_settled = false;
        }
        true
    }

    /// Restart settle tracking (touch, play-after-pause, reset).
    pub fn wake_from_settled(&mut self) {
        self.consecutive_settled_frames = 0;
        self.is_fully_settled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> BubblePhysics {
        let mut p = BubblePhysics::default();
        p.set_size(800.0, 600.0);
        p
    }

    #[test]
    fn seeded_init_is_deterministic() {
        let mut a = field();
        let mut b = field();
        a.initialize_bubbles(60, 42);
        b.initialize_bubbles(60, 42);
        assert_eq!(a.bubbles.len(), 60);
        for (x, y) in a.bubbles.iter().zip(&b.bubbles) {
            assert_eq!(
                (x.x, x.y, x.radius, x.performance),
                (y.x, y.y, y.radius, y.performance)
            );
        }
        // Radii scale with the play area per the size-parity spec: for
        // `field()`'s 800×600 area, `S = 600`, so radius ∈
        // `[0.05·600, 0.15·600] = [30, 90]`. Performance stays in `-20..20`.
        let s = 800.0_f64.min(600.0);
        let r_lo = RADIUS_MIN_FRAC * s;
        let r_hi = (RADIUS_MIN_FRAC + RADIUS_SPAN_FRAC) * s;
        for bb in &a.bubbles {
            assert!((r_lo..=r_hi).contains(&bb.radius));
            assert!((-20.0..20.0).contains(&bb.performance));
        }
    }

    #[test]
    fn overlapping_bubbles_separate() {
        let mut p = field();
        p.initialize_bubbles(2, 7);
        // Force a deep overlap dead-center.
        p.bubbles[0].x = 400.0;
        p.bubbles[0].y = 300.0;
        p.bubbles[1].x = 405.0;
        p.bubbles[1].y = 300.0;
        let initial = 5.0;
        for _ in 0..120 {
            p.update();
        }
        let dx = p.bubbles[1].x - p.bubbles[0].x;
        let dy = p.bubbles[1].y - p.bubbles[0].y;
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(
            dist > initial,
            "collision force must push overlapping bubbles apart"
        );
    }

    #[test]
    fn bubbles_stay_inside_walls() {
        // Constraints clamp BEFORE integration (the repro's phase order), so a
        // bubble may transiently overshoot a wall by at most the frame's own
        // velocity — the bound asserted here.
        let mut p = field();
        p.initialize_bubbles(60, 42);
        for _ in 0..600 {
            p.update();
        }
        for b in &p.bubbles {
            let (tx, ty) = (b.vx.abs() + 1e-9, b.vy.abs() + 1e-9);
            assert!(b.x >= b.radius - tx && b.x <= 800.0 - b.radius + tx);
            assert!(b.y >= b.radius - ty && b.y <= 600.0 - b.radius + ty);
        }
    }

    #[test]
    fn small_cluster_settles_and_touch_wakes() {
        let mut p = field();
        p.initialize_bubbles(2, 3);
        // A two-bubble system loses energy fast; it must settle well within
        // the repro's practical horizon.
        let mut settled = false;
        for _ in 0..2_000 {
            if !p.needs_repaint() {
                settled = true;
                break;
            }
            p.update();
        }
        assert!(settled, "two bubbles must reach the fully-settled state");
        assert!(p.is_fully_settled);

        // An active touch forces animation back on.
        p.set_touch(Some(Point::new(400.0, 300.0)));
        assert!(p.needs_repaint());
        assert!(!p.is_fully_settled);
    }

    #[test]
    fn touch_repels_nearby_bubbles() {
        let mut p = field();
        p.initialize_bubbles(1, 1);
        p.bubbles[0].x = 400.0;
        p.bubbles[0].y = 300.0;
        p.set_touch(Some(Point::new(390.0, 300.0)));
        p.update();
        assert!(
            p.bubbles[0].vx > 0.0 || p.bubbles[0].x > 400.0,
            "a touch just left of the bubble must push it right"
        );
    }
}
