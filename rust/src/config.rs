//! User-tunable behavior. Every field has a serde default so an older state file
//! (or one with fields removed) still loads. All lengths are dp.

use serde::{Deserialize, Serialize};

macro_rules! defaults {
    ($($name:ident : $ty:ty = $val:expr),* $(,)?) => {
        $(fn $name() -> $ty { $val })*
    };
}

defaults! {
    d_card_w: f64 = 88.0,
    d_card_h: f64 = 108.0,
    d_snap_gap: f64 = 12.0,
    d_snap_distance: f64 = 24.0,
    d_snap_break: f64 = 32.0,
    d_true: bool = true,
    d_false: bool = false,
    d_drift: f64 = 0.5,
    d_touch_slop: f64 = 8.0,
    d_double_tap_ms: u64 = 300,
    d_double_tap_dist: f64 = 40.0,
    d_long_press_ms: u64 = 380,
    d_edge_zone: f64 = 48.0,
    d_edge_min: f64 = 120.0,
    d_edge_max: f64 = 720.0,
    d_edge_latency: u64 = 120,
    d_zoom_speed: f64 = 1.0,
    d_max_zoom: f64 = 1.5,
    d_dot_spacing: f64 = 80.0,
    d_fit_padding: f64 = 48.0,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub card_w: f64,
    pub card_h: f64,
    pub snap_gap: f64,
    /// Pull distance in screen px (divided by zoom while dragging).
    pub snap_distance: f64,
    pub snap_break_force: f64,
    pub snap_enabled: bool,
    pub snap_same_edge: bool,
    pub snap_centers: bool,
    /// Pan momentum: 0 = off ... 1 = floatiest.
    pub drift: f64,
    pub touch_slop: f64,
    pub double_tap_ms: u64,
    pub double_tap_dist: f64,
    pub long_press_ms: u64,
    pub edge_pan_zone: f64,
    pub edge_pan_min: f64,
    pub edge_pan_max: f64,
    pub edge_pan_latency_ms: u64,
    pub zoom_speed: f64,
    pub max_zoom: f64,
    pub dot_spacing: f64,
    pub fit_padding: f64,
    /// Choosing an app that already has a card in the drawer jumps to its card
    /// instead of launching it again.
    pub drawer_jumps_to_existing: bool,
    /// Keep launching apps from the dock even when a card exists.
    pub dock_launches_existing: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            card_w: d_card_w(),
            card_h: d_card_h(),
            snap_gap: d_snap_gap(),
            snap_distance: d_snap_distance(),
            snap_break_force: d_snap_break(),
            snap_enabled: d_true(),
            snap_same_edge: d_true(),
            snap_centers: d_false(),
            drift: d_drift(),
            touch_slop: d_touch_slop(),
            double_tap_ms: d_double_tap_ms(),
            double_tap_dist: d_double_tap_dist(),
            long_press_ms: d_long_press_ms(),
            edge_pan_zone: d_edge_zone(),
            edge_pan_min: d_edge_min(),
            edge_pan_max: d_edge_max(),
            edge_pan_latency_ms: d_edge_latency(),
            zoom_speed: d_zoom_speed(),
            max_zoom: d_max_zoom(),
            dot_spacing: d_dot_spacing(),
            fit_padding: d_fit_padding(),
            drawer_jumps_to_existing: d_true(),
            dock_launches_existing: d_true(),
        }
    }
}

impl Config {
    /// Clamp values that would break the math (negative sizes, NaN from a
    /// hand-edited file) back into sane ranges.
    pub fn sanitized(mut self) -> Self {
        let def = Config::default();
        let fix = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.card_w = fix(self.card_w, 32.0, 400.0, def.card_w);
        self.card_h = fix(self.card_h, 32.0, 400.0, def.card_h);
        self.snap_gap = fix(self.snap_gap, 0.0, 100.0, def.snap_gap);
        self.snap_distance = fix(self.snap_distance, 0.0, 200.0, def.snap_distance);
        self.snap_break_force = fix(self.snap_break_force, 0.0, 400.0, def.snap_break_force);
        self.drift = fix(self.drift, 0.0, 1.0, def.drift);
        self.touch_slop = fix(self.touch_slop, 1.0, 60.0, def.touch_slop);
        self.double_tap_dist = fix(self.double_tap_dist, 4.0, 200.0, def.double_tap_dist);
        self.edge_pan_zone = fix(self.edge_pan_zone, 0.0, 200.0, def.edge_pan_zone);
        self.edge_pan_min = fix(self.edge_pan_min, 0.0, 5000.0, def.edge_pan_min);
        self.edge_pan_max = fix(self.edge_pan_max, 0.0, 5000.0, def.edge_pan_max).max(self.edge_pan_min);
        self.zoom_speed = fix(self.zoom_speed, 0.1, 4.0, def.zoom_speed);
        self.max_zoom = fix(self.max_zoom, 0.5, 4.0, def.max_zoom);
        self.dot_spacing = fix(self.dot_spacing, 20.0, 400.0, def.dot_spacing);
        self.fit_padding = fix(self.fit_padding, 0.0, 300.0, def.fit_padding);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_empty_object() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_object_keeps_other_defaults() {
        let c: Config = serde_json::from_str(r#"{"drift": 0.9}"#).unwrap();
        assert_eq!(c.drift, 0.9);
        assert_eq!(c.card_w, 88.0);
    }

    #[test]
    fn sanitize_clamps() {
        let c = Config {
            card_w: -5.0,
            drift: 7.0,
            snap_gap: f64::NAN,
            ..Config::default()
        }
        .sanitized();
        assert_eq!(c.card_w, 32.0);
        assert_eq!(c.drift, 1.0);
        assert_eq!(c.snap_gap, 12.0);
    }
}
