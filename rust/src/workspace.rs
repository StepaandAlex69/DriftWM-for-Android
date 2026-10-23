//! World coordinates, camera, zoom, momentum and camera animation.
//!
//! Derived from DriftWM `canvas.rs` (GPL-3.0-or-later, Copyright (C) 2026 Klim
//! Kostiuk). Smithay types are replaced with plain `Vec2`; all units are dp.
//! screen = (canvas - camera) * zoom  =>  canvas = screen / zoom + camera.

use std::collections::VecDeque;
use std::ops::{Add, Mul, Sub};

pub const MIN_ZOOM_FLOOR: f64 = 0.03;
pub const MAX_ZOOM: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn dist(self, other: Vec2) -> f64 {
        (self - other).length()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn x_high(&self) -> f64 {
        self.x + self.w
    }

    pub fn y_high(&self) -> f64 {
        self.y + self.h
    }

    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }

    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(self.cx(), self.cy())
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x_high() && p.y >= self.y && p.y <= self.y_high()
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x_high() && o.x < self.x_high() && self.y < o.y_high() && o.y < self.y_high()
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.x_high().max(o.x_high()) - x,
            self.y_high().max(o.y_high()) - y,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn to_unit_vec(self) -> (f64, f64) {
        match self {
            Direction::Up => (0.0, -1.0),
            Direction::Down => (0.0, 1.0),
            Direction::Left => (-1.0, 0.0),
            Direction::Right => (1.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Canvas position of the viewport's top-left corner.
    pub pos: Vec2,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            pos: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn screen_to_canvas(&self, screen: Vec2) -> Vec2 {
        Vec2::new(
            screen.x / self.zoom + self.pos.x,
            screen.y / self.zoom + self.pos.y,
        )
    }

    pub fn canvas_to_screen(&self, canvas: Vec2) -> Vec2 {
        Vec2::new(
            (canvas.x - self.pos.x) * self.zoom,
            (canvas.y - self.pos.y) * self.zoom,
        )
    }

    pub fn visible_rect(&self, viewport: Vec2) -> Rect {
        Rect::new(
            self.pos.x,
            self.pos.y,
            viewport.x / self.zoom,
            viewport.y / self.zoom,
        )
    }

    pub fn center(&self, viewport: Vec2) -> Vec2 {
        Vec2::new(
            self.pos.x + viewport.x / (2.0 * self.zoom),
            self.pos.y + viewport.y / (2.0 * self.zoom),
        )
    }

    pub fn centered_on(center: Vec2, zoom: f64, viewport: Vec2) -> Camera {
        Camera {
            pos: Vec2::new(
                center.x - viewport.x / (2.0 * zoom),
                center.y - viewport.y / (2.0 * zoom),
            ),
            zoom,
        }
    }
}

/// Camera position that keeps `anchor_canvas` at `anchor_screen` after a zoom change.
pub fn zoom_anchor_camera(anchor_canvas: Vec2, anchor_screen: Vec2, new_zoom: f64) -> Vec2 {
    Vec2::new(
        anchor_canvas.x - anchor_screen.x / new_zoom,
        anchor_canvas.y - anchor_screen.y / new_zoom,
    )
}

/// Fraction of `rect` visible inside `view` (0.0-1.0).
pub fn visible_fraction(rect: &Rect, view: &Rect) -> f64 {
    let area = rect.w * rect.h;
    if area <= 0.0 {
        return 0.0;
    }
    let iw = (rect.x_high().min(view.x_high()) - rect.x.max(view.x)).max(0.0);
    let ih = (rect.y_high().min(view.y_high()) - rect.y.max(view.y)).max(0.0);
    (iw * ih) / area
}

pub fn bounding_box<'a>(rects: impl Iterator<Item = &'a Rect>) -> Option<Rect> {
    rects.fold(None, |acc, r| Some(acc.map_or(*r, |a| a.union(r))))
}

/// Zoom that fits `bbox` into `viewport` with `padding` screen px on each side.
pub fn zoom_to_fit(bbox: &Rect, viewport: Vec2, padding: f64, max_zoom: f64) -> f64 {
    let avail_w = (viewport.x - padding * 2.0).max(1.0);
    let avail_h = (viewport.y - padding * 2.0).max(1.0);
    let zx = avail_w / bbox.w.max(1.0);
    let zy = avail_h / bbox.h.max(1.0);
    zx.min(zy).clamp(MIN_ZOOM_FLOOR, max_zoom)
}

/// Minimum zoom: half the zoom that fits every card, so the user can always
/// zoom out past the overview. Uses a virtual 5x5 box when the canvas is empty.
pub fn dynamic_min_zoom(bbox: Option<Rect>, viewport: Vec2, padding: f64) -> f64 {
    let bbox = bbox.unwrap_or(Rect::new(-2.0, -2.0, 5.0, 5.0));
    let fit = zoom_to_fit(&bbox, viewport, padding, MAX_ZOOM);
    (fit * 0.5).max(MIN_ZOOM_FLOOR)
}

/// Snap zoom to 1.0 inside a +-0.05 dead zone.
pub fn snap_zoom(z: f64) -> f64 {
    if (z - 1.0).abs() < 0.05 { 1.0 } else { z }
}

/// Nearest item in a 90 degree cone from `origin`; score = dist / cos(angle).
pub fn find_nearest<W: PartialEq + Copy>(
    origin: Vec2,
    dir: Direction,
    items: impl Iterator<Item = (W, Vec2)>,
    skip: Option<W>,
) -> Option<W> {
    let (ux, uy) = dir.to_unit_vec();
    let mut best: Option<(W, f64)> = None;
    for (item, center) in items {
        if skip == Some(item) {
            continue;
        }
        let dx = center.x - origin.x;
        let dy = center.y - origin.y;
        let dot = dx * ux + dy * uy;
        let cross = (dx * uy - dy * ux).abs();
        if dot > 0.0 && cross <= dot {
            let score = (dx * dx + dy * dy) / dot;
            if best.is_none_or(|(_, d)| score < d) {
                best = Some((item, score));
            }
        }
    }
    best.map(|(w, _)| w)
}

const VELOCITY_WINDOW_MS: u64 = 80;

/// Sliding-window velocity tracker: total displacement over the last 80 ms.
#[derive(Clone, Default)]
pub struct VelocityTracker {
    samples: VecDeque<(u64, Vec2)>,
}

impl VelocityTracker {
    pub fn push(&mut self, time_ms: u64, delta: Vec2) {
        self.samples.push_back((time_ms, delta));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| time_ms.saturating_sub(*t) > VELOCITY_WINDOW_MS)
        {
            self.samples.pop_front();
        }
    }

    /// Drop samples older than the window relative to `now_ms`. A finger that
    /// rested before lifting must not fling with stale speed.
    pub fn expire(&mut self, now_ms: u64) {
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| now_ms.saturating_sub(*t) > VELOCITY_WINDOW_MS)
        {
            self.samples.pop_front();
        }
    }

    /// px/sec; zero with fewer than two samples.
    pub fn launch_velocity(&self) -> Vec2 {
        if self.samples.len() < 2 {
            return Vec2::ZERO;
        }
        let first = self.samples.front().unwrap().0;
        let last = self.samples.back().unwrap().0;
        let elapsed_ms = last.saturating_sub(first);
        if elapsed_ms == 0 {
            return Vec2::ZERO;
        }
        let elapsed = elapsed_ms as f64 / 1000.0;
        let total = self
            .samples
            .iter()
            .fold(Vec2::ZERO, |acc, (_, d)| acc + *d);
        Vec2::new(total.x / elapsed, total.y / elapsed)
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

const MOMENTUM_STOP_THRESHOLD: f64 = 15.0;

#[derive(Clone)]
pub struct MomentumState {
    pub velocity: Vec2,
    pub tracker: VelocityTracker,
    /// 0 = off ... 1 = floatiest.
    pub drift: f64,
    pub coasting: bool,
}

impl MomentumState {
    pub fn new(drift: f64) -> Self {
        Self {
            velocity: Vec2::ZERO,
            tracker: VelocityTracker::default(),
            drift,
            coasting: false,
        }
    }

    pub fn accumulate(&mut self, delta: Vec2, time_ms: u64) {
        self.tracker.push(time_ms, delta);
        self.coasting = false;
    }

    /// Start coasting from the recent motion; `now_ms` is the release time.
    pub fn launch(&mut self, now_ms: u64) {
        self.tracker.expire(now_ms);
        self.velocity = self.tracker.launch_velocity();
        self.coasting = self.drift > 0.0 && self.velocity.length() >= MOMENTUM_STOP_THRESHOLD;
        self.tracker.clear();
    }

    /// Returns the canvas delta to apply this frame.
    pub fn tick(&mut self, dt_secs: f64) -> Option<Vec2> {
        if !self.coasting {
            return None;
        }
        let speed = self.velocity.length();
        if speed < MOMENTUM_STOP_THRESHOLD {
            self.stop();
            return None;
        }
        let retention = speed_dependent_drift(self.drift, speed);
        let decay = retention.powf(dt_secs * 60.0);
        let delta = self.velocity * dt_secs;
        self.velocity = self.velocity * decay;
        Some(delta)
    }

    pub fn stop(&mut self) {
        self.velocity = Vec2::ZERO;
        self.tracker.clear();
        self.coasting = false;
    }
}

/// Per-frame (60 fps) velocity retention. Log-spaced half-life across the knob;
/// gentle scrolls stop sooner than hard flings.
pub fn speed_dependent_drift(drift: f64, speed: f64) -> f64 {
    if drift <= 0.0 {
        return 0.0;
    }
    const FLING_HALFLIFE_MIN: f64 = 0.05;
    const FLING_HALFLIFE_MAX: f64 = 2.3;
    const SLOW_COAST_RATIO: f64 = 0.28;
    let fling = FLING_HALFLIFE_MIN * (FLING_HALFLIFE_MAX / FLING_HALFLIFE_MIN).powf(drift.min(1.0));
    let t = (speed / 2500.0).min(1.0);
    let half_life = fling * SLOW_COAST_RATIO.powf(1.0 - t);
    0.5_f64.powf(1.0 / (60.0 * half_life)).min(0.995)
}

/// Edge-pan velocity (screen px/sec) from the finger's depth into the edge zone;
/// deeper is faster (quadratic ramp). `None` outside the zone.
pub fn edge_pan_velocity(
    screen_pos: Vec2,
    view_w: f64,
    view_h: f64,
    zone: f64,
    pan_min: f64,
    pan_max: f64,
) -> Option<Vec2> {
    let dl = screen_pos.x;
    let dr = view_w - screen_pos.x;
    let dt = screen_pos.y;
    let db = view_h - screen_pos.y;
    let min_dist = dl.min(dr).min(dt).min(db);
    if zone <= 0.0 || min_dist >= zone {
        return None;
    }
    let t = ((zone - min_dist) / zone).clamp(0.0, 1.0);
    let speed = pan_min + (pan_max - pan_min) * t * t;
    let (mut vx, mut vy) = (0.0, 0.0);
    if dl < zone {
        vx -= speed * ((zone - dl) / zone);
    }
    if dr < zone {
        vx += speed * ((zone - dr) / zone);
    }
    if dt < zone {
        vy -= speed * ((zone - dt) / zone);
    }
    if db < zone {
        vy += speed * ((zone - db) / zone);
    }
    let len = vx.hypot(vy);
    if len > speed {
        vx = vx / len * speed;
        vy = vy / len * speed;
    }
    Some(Vec2::new(vx, vy))
}

/// Smooth camera move toward a target center + zoom. The center eases linearly
/// and the zoom in log space, so a combined pan+zoom never drifts off target.
#[derive(Debug, Clone, Copy)]
pub struct CameraAnimation {
    pub target_center: Vec2,
    pub target_zoom: f64,
}

const ANIM_RATE: f64 = 11.0;

impl CameraAnimation {
    /// Advance `cam`; returns true while the animation is still running.
    pub fn step(&self, cam: &mut Camera, viewport: Vec2, dt_secs: f64) -> bool {
        let k = 1.0 - (-ANIM_RATE * dt_secs).exp();
        let center = cam.center(viewport);
        let new_center = Vec2::new(
            center.x + (self.target_center.x - center.x) * k,
            center.y + (self.target_center.y - center.y) * k,
        );
        let lz = cam.zoom.ln();
        let tz = self.target_zoom.ln();
        let new_zoom = (lz + (tz - lz) * k).exp();
        let px = 0.25 / new_zoom;
        let done = (self.target_center.x - new_center.x).abs() < px
            && (self.target_center.y - new_center.y).abs() < px
            && (tz - new_zoom.ln()).abs() < 0.002;
        if done {
            *cam = Camera::centered_on(self.target_center, self.target_zoom, viewport);
            return false;
        }
        *cam = Camera::centered_on(new_center, new_zoom, viewport);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn screen_canvas_round_trip() {
        let cam = Camera {
            pos: v(-340.5, 1200.0),
            zoom: 0.37,
        };
        for p in [v(0.0, 0.0), v(100.0, 250.0), v(-30.0, 999.0)] {
            let back = cam.canvas_to_screen(cam.screen_to_canvas(p));
            assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
        }
    }

    #[test]
    fn huge_coordinates_stay_precise() {
        let cam = Camera {
            pos: v(1.0e9, -1.0e9),
            zoom: 1.0,
        };
        let c = cam.screen_to_canvas(v(10.0, 20.0));
        assert_eq!(c, v(1.0e9 + 10.0, -1.0e9 + 20.0));
    }

    #[test]
    fn zoom_anchor_keeps_point_fixed() {
        let mut cam = Camera {
            pos: v(50.0, 70.0),
            zoom: 1.0,
        };
        let anchor_screen = v(200.0, 300.0);
        let anchor_canvas = cam.screen_to_canvas(anchor_screen);
        cam.zoom = 0.4;
        cam.pos = zoom_anchor_camera(anchor_canvas, anchor_screen, cam.zoom);
        let after = cam.canvas_to_screen(anchor_canvas);
        assert!((after.x - 200.0).abs() < 1e-9 && (after.y - 300.0).abs() < 1e-9);
    }

    #[test]
    fn center_round_trip() {
        let vp = v(400.0, 800.0);
        let cam = Camera::centered_on(v(10.0, -20.0), 0.5, vp);
        let c = cam.center(vp);
        assert!((c.x - 10.0).abs() < 1e-9 && (c.y + 20.0).abs() < 1e-9);
    }

    #[test]
    fn fit_clamps() {
        let vp = v(400.0, 800.0);
        let small = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(zoom_to_fit(&small, vp, 40.0, MAX_ZOOM), MAX_ZOOM);
        let huge = Rect::new(0.0, 0.0, 1.0e9, 1.0e9);
        assert_eq!(zoom_to_fit(&huge, vp, 40.0, MAX_ZOOM), MIN_ZOOM_FLOOR);
        let wide = Rect::new(0.0, 0.0, 720.0, 100.0);
        assert!((zoom_to_fit(&wide, vp, 40.0, MAX_ZOOM) - 320.0 / 720.0).abs() < 1e-9);
    }

    #[test]
    fn snap_zoom_dead_zone() {
        assert_eq!(snap_zoom(1.04), 1.0);
        assert_eq!(snap_zoom(0.9), 0.9);
    }

    #[test]
    fn visible_fraction_cases() {
        let view = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(visible_fraction(&Rect::new(10.0, 10.0, 20.0, 20.0), &view), 1.0);
        assert_eq!(visible_fraction(&Rect::new(200.0, 0.0, 20.0, 20.0), &view), 0.0);
        assert!((visible_fraction(&Rect::new(90.0, 0.0, 20.0, 10.0), &view) - 0.5).abs() < 1e-9);
        assert_eq!(visible_fraction(&Rect::new(0.0, 0.0, 0.0, 5.0), &view), 0.0);
    }

    #[test]
    fn nearest_prefers_aligned() {
        let items = vec![(1, v(100.0, 0.0)), (2, v(80.0, 70.0)), (3, v(-100.0, 0.0))];
        let r = find_nearest(Vec2::ZERO, Direction::Right, items.into_iter(), None);
        assert_eq!(r, Some(1));
    }

    #[test]
    fn nearest_outside_cone_and_skip() {
        let items = vec![(1, v(10.0, 100.0)), (2, v(100.0, 0.0))];
        assert_eq!(
            find_nearest(Vec2::ZERO, Direction::Up, items.clone().into_iter(), None),
            None
        );
        assert_eq!(
            find_nearest(Vec2::ZERO, Direction::Right, items.into_iter(), Some(2)),
            None
        );
    }

    #[test]
    fn velocity_from_samples() {
        let mut t = VelocityTracker::default();
        t.push(0, v(0.0, 0.0));
        t.push(10, v(10.0, 0.0));
        t.push(20, v(10.0, 0.0));
        let vel = t.launch_velocity();
        assert!((vel.x - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn velocity_window_evicts_old() {
        let mut t = VelocityTracker::default();
        t.push(0, v(1000.0, 0.0));
        t.push(500, v(1.0, 0.0));
        t.push(510, v(1.0, 0.0));
        assert!(t.launch_velocity().x < 300.0);
    }

    #[test]
    fn resting_finger_does_not_fling() {
        let mut m = MomentumState::new(0.5);
        m.accumulate(v(0.0, 0.0), 0);
        m.accumulate(v(60.0, 0.0), 16);
        m.accumulate(v(60.0, 0.0), 32);
        m.launch(600);
        assert!(!m.coasting);
    }

    #[test]
    fn momentum_decays_and_stops() {
        let mut m = MomentumState::new(0.5);
        m.accumulate(v(0.0, 0.0), 0);
        m.accumulate(v(40.0, 0.0), 16);
        m.accumulate(v(40.0, 0.0), 32);
        m.launch(32);
        assert!(m.coasting);
        let mut total = 0.0;
        for _ in 0..2000 {
            match m.tick(1.0 / 60.0) {
                Some(d) => total += d.x,
                None => break,
            }
        }
        assert!(!m.coasting);
        assert!(total > 10.0);
    }

    #[test]
    fn momentum_disabled_when_drift_zero() {
        let mut m = MomentumState::new(0.0);
        m.accumulate(v(0.0, 0.0), 0);
        m.accumulate(v(100.0, 0.0), 10);
        m.launch(10);
        assert!(m.tick(0.016).is_none());
    }

    #[test]
    fn momentum_is_frame_rate_independent() {
        let run = |dt: f64| {
            let mut m = MomentumState::new(0.5);
            m.velocity = v(2000.0, 0.0);
            m.coasting = true;
            let mut x = 0.0;
            let mut t = 0.0;
            while t < 0.5 {
                if let Some(d) = m.tick(dt) {
                    x += d.x;
                }
                t += dt;
            }
            x
        };
        let a = run(1.0 / 60.0);
        let b = run(1.0 / 120.0);
        assert!((a - b).abs() / a < 0.05, "{a} vs {b}");
    }

    #[test]
    fn edge_pan_zone() {
        assert!(edge_pan_velocity(v(200.0, 400.0), 400.0, 800.0, 48.0, 100.0, 800.0).is_none());
        let l = edge_pan_velocity(v(5.0, 400.0), 400.0, 800.0, 48.0, 100.0, 800.0).unwrap();
        assert!(l.x < 0.0 && l.y == 0.0);
        let r = edge_pan_velocity(v(395.0, 400.0), 400.0, 800.0, 48.0, 100.0, 800.0).unwrap();
        assert!(r.x > 0.0);
        let deeper = edge_pan_velocity(v(1.0, 400.0), 400.0, 800.0, 48.0, 100.0, 800.0).unwrap();
        let shallower = edge_pan_velocity(v(40.0, 400.0), 400.0, 800.0, 48.0, 100.0, 800.0).unwrap();
        assert!(deeper.x.abs() > shallower.x.abs());
        let corner = edge_pan_velocity(v(1.0, 1.0), 400.0, 800.0, 48.0, 100.0, 800.0).unwrap();
        assert!(corner.length() <= 800.0 + 1e-9);
    }

    #[test]
    fn animation_converges() {
        let vp = v(400.0, 800.0);
        let mut cam = Camera::default();
        let anim = CameraAnimation {
            target_center: v(1000.0, -500.0),
            target_zoom: 0.5,
        };
        let mut steps = 0;
        while anim.step(&mut cam, vp, 1.0 / 60.0) {
            steps += 1;
            assert!(steps < 600);
        }
        let c = cam.center(vp);
        assert!((c.x - 1000.0).abs() < 1e-6 && (c.y + 500.0).abs() < 1e-6);
        assert!((cam.zoom - 0.5).abs() < 1e-9);
    }

    #[test]
    fn dynamic_min_zoom_is_half_fit() {
        let vp = v(400.0, 800.0);
        let bbox = Rect::new(0.0, 0.0, 720.0, 100.0);
        let z = dynamic_min_zoom(Some(bbox), vp, 40.0);
        assert!((z - 160.0 / 720.0).abs() < 1e-9);
    }
}
