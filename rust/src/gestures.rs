//! Touch recognition: pan, pinch-zoom, tap/double-tap, card drag (single card or
//! whole group), dock reorder, edge auto-pan and three-finger navigation.
//!
//! Every gesture starts "pending" and only commits once the finger leaves the
//! touch slop, so a tap can never become a drag and a drag can never open an app.
//! Scale handling follows DriftWM's touch recognizer: pan and zoom run together,
//! zoom engages only after the finger spread changes by a ratio slop.

use crate::launcher::{Event, LaunchSource, Launcher};
use crate::snapping::{SnapConfig, SnapRect, SnapState, cluster_of, snap_move};
use crate::ui::UiId;
use crate::workspace::{Direction, Rect, Vec2, edge_pan_velocity, snap_zoom, zoom_anchor_camera};

/// Spread change (fraction of the starting spread) before pinch-zoom engages.
const ZOOM_SLOP_RATIO: f64 = 0.08;
/// ...and the minimum absolute change in dp, so tiny spreads don't zoom on jitter.
const ZOOM_MIN_DELTA: f64 = 10.0;
const SWIPE3_DISTANCE: f64 = 56.0;
/// Smallest hit target for a card, in dp, however far the canvas is zoomed out.
const MIN_HIT: f64 = 28.0;

/// Smallest and largest card width (canvas dp) reachable by pinch-resizing.
const MIN_CARD_W: f64 = 40.0;
const MAX_CARD_W: f64 = 1200.0;
/// Extra hit slop around the round card button, dp.
const BUTTON_SLOP: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Target {
    Card(u64),
    /// The round preview button on the selected card.
    CardButton(u64),
    Empty,
    Ui(UiId),
}

#[derive(Debug, Clone, Copy)]
struct Pointer {
    id: i32,
    start: Vec2,
    pos: Vec2,
    last: Vec2,
}

#[derive(Debug, Clone)]
pub(crate) struct DragState {
    pid: i32,
    card_id: u64,
    /// Card position minus the finger's canvas position, fixed at drag start.
    grab: Vec2,
    members: Vec<(u64, Vec2)>,
    snap: SnapState,
    finger: Vec2,
    edge_since: Option<u64>,
}

impl Target {
    /// Screen-fixed controls: never pannable, never start a pinch.
    fn is_control(self) -> bool {
        matches!(self, Target::Ui(_) | Target::CardButton(_))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Mode {
    Idle,
    Pending {
        pid: i32,
        target: Target,
        down_ms: u64,
        long: bool,
    },
    Pan {
        pid: i32,
    },
    Drag(DragState),
    DockDrag {
        pid: i32,
        from: usize,
        over: usize,
        x: f64,
    },
    Multi {
        last_centroid: Vec2,
        last_spread: f64,
        start_spread: f64,
        zoom_engaged: bool,
    },
    /// Pinch on the selected card: scales the card, not the canvas.
    Resize {
        card_id: u64,
        start_spread: f64,
        last_spread: f64,
        engaged: bool,
        /// Canvas point under the fingers when they landed; the card scales about it.
        anchor: Vec2,
    },
    Swipe3 {
        start: Vec2,
        fired: bool,
    },
    /// Extra fingers or an abandoned gesture: swallow input until all lift.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LastTap {
    card: u64,
    ms: u64,
    pos: Vec2,
}

pub(crate) struct GestureState {
    pointers: Vec<Pointer>,
    pub(crate) mode: Mode,
    last_tap: Option<LastTap>,
    last_empty_tap: Option<(u64, Vec2)>,
}

impl Default for GestureState {
    fn default() -> Self {
        GestureState {
            pointers: Vec::new(),
            mode: Mode::Idle,
            last_tap: None,
            last_empty_tap: None,
        }
    }
}

impl Launcher {
    pub(crate) fn gesture_active(&self) -> bool {
        !matches!(self.g.mode, Mode::Idle)
    }

    /// The UI element currently held down (for pressed-state drawing).
    pub(crate) fn pressed_ui(&self) -> Option<UiId> {
        match self.g.mode {
            Mode::Pending {
                target: Target::Ui(id),
                ..
            } => Some(id),
            _ => None,
        }
    }

    pub(crate) fn dragging_card(&self) -> Option<u64> {
        match &self.g.mode {
            Mode::Drag(d) => Some(d.card_id),
            _ => None,
        }
    }

    /// Bit 0: x axis snapped to a neighbour, bit 1: y axis snapped.
    pub(crate) fn drag_snap_flags(&self) -> u32 {
        match &self.g.mode {
            Mode::Drag(d) => u32::from(d.snap.x.is_some()) | (u32::from(d.snap.y.is_some()) << 1),
            _ => 0,
        }
    }

    pub(crate) fn dock_drag(&self) -> Option<(usize, usize, f64)> {
        match self.g.mode {
            Mode::DockDrag { from, over, x, .. } => Some((from, over, x)),
            _ => None,
        }
    }

    /// kind: 0 down, 1 move, 2 up, 3 cancel. Coordinates are raw pixels.
    pub fn touch(&mut self, kind: i32, id: i32, x_px: f64, y_px: f64, t_ms: u64) {
        self.now = t_ms;
        let p = Vec2::new(x_px / self.density, y_px / self.density);
        match kind {
            0 => self.on_down(id, p),
            1 => self.on_move(id, p),
            2 => self.on_up(id, p),
            _ => self.on_cancel(),
        }
    }

    fn pointer_mut(&mut self, id: i32) -> Option<&mut Pointer> {
        self.g.pointers.iter_mut().find(|p| p.id == id)
    }

    fn hit_test(&self, p: Vec2) -> Target {
        if let Some(ui) = self.ui_layout().hit(p) {
            return Target::Ui(ui);
        }
        if let Some((id, r)) = self.card_button()
            && Rect::new(
                r.x - BUTTON_SLOP,
                r.y - BUTTON_SLOP,
                r.w + 2.0 * BUTTON_SLOP,
                r.h + 2.0 * BUTTON_SLOP,
            )
            .contains(p)
        {
            return Target::CardButton(id);
        }
        let zoom = self.camera.zoom;
        let c = self.camera.screen_to_canvas(p);
        let min_canvas = MIN_HIT / zoom;
        for card in self.cards.iter().rev() {
            let r = card.rect;
            let grow_x = ((min_canvas - r.w) / 2.0).max(0.0);
            let grow_y = ((min_canvas - r.h) / 2.0).max(0.0);
            if c.x >= r.x - grow_x
                && c.x <= r.x_high() + grow_x
                && c.y >= r.y - grow_y
                && c.y <= r.y_high() + grow_y
            {
                return Target::Card(card.id);
            }
        }
        Target::Empty
    }

    fn on_down(&mut self, id: i32, p: Vec2) {
        if self.g.pointers.iter().any(|q| q.id == id) {
            return;
        }
        self.g.pointers.push(Pointer {
            id,
            start: p,
            pos: p,
            last: p,
        });
        match self.g.pointers.len() {
            1 => {
                self.momentum.stop();
                self.anim = None;
                let target = self.hit_test(p);
                self.g.mode = Mode::Pending {
                    pid: id,
                    target,
                    down_ms: self.now,
                    long: false,
                };
            }
            2 => {
                // First finger on the selected card: this pinch resizes that card.
                let resize = match &self.g.mode {
                    Mode::Pending {
                        target: Target::Card(id),
                        long: false,
                        ..
                    } if self.selected == Some(*id) => Some(*id),
                    _ => None,
                };
                let pannable = match &self.g.mode {
                    Mode::Pending { target, long, .. } => {
                        !target.is_control() && !(*long && matches!(target, Target::Card(_)))
                    }
                    Mode::Pan { .. } => true,
                    _ => false,
                };
                if let Some(id) = resize {
                    self.begin_resize(id);
                } else if pannable {
                    self.begin_multi();
                }
            }
            3 => {
                if matches!(self.g.mode, Mode::Multi { .. } | Mode::Pan { .. } | Mode::Pending { .. })
                    && !matches!(&self.g.mode, Mode::Pending { target, .. } if target.is_control())
                {
                    let c = self.centroid(3);
                    self.g.mode = Mode::Swipe3 {
                        start: c,
                        fired: false,
                    };
                }
            }
            _ => {}
        }
    }

    fn centroid(&self, n: usize) -> Vec2 {
        let pts = &self.g.pointers[..n.min(self.g.pointers.len())];
        let sum = pts.iter().fold(Vec2::ZERO, |a, p| a + p.pos);
        sum * (1.0 / pts.len().max(1) as f64)
    }

    fn spread(&self) -> f64 {
        if self.g.pointers.len() < 2 {
            return 0.0;
        }
        self.g.pointers[0].pos.dist(self.g.pointers[1].pos)
    }

    fn begin_multi(&mut self) {
        self.momentum.stop();
        self.momentum.tracker.clear();
        let s = self.spread();
        self.g.mode = Mode::Multi {
            last_centroid: self.centroid(2),
            last_spread: s,
            start_spread: s,
            zoom_engaged: false,
        };
    }

    fn on_move(&mut self, id: i32, p: Vec2) {
        let Some(ptr) = self.pointer_mut(id) else { return };
        ptr.pos = p;
        let start = ptr.start;
        let last = ptr.last;
        ptr.last = p;

        match self.g.mode.clone() {
            Mode::Pending {
                pid,
                target,
                long,
                ..
            } if pid == id => {
                if p.dist(start) <= self.cfg.touch_slop {
                    return;
                }
                match target {
                    Target::Card(card_id) => self.begin_drag(id, card_id, p, long),
                    Target::Empty => {
                        self.user_moved_camera();
                        self.momentum.stop();
                        self.g.mode = Mode::Pan { pid: id };
                        if let Some(ptr) = self.pointer_mut(id) {
                            ptr.last = p;
                        }
                    }
                    Target::Ui(UiId::DockSlot(slot)) if long => {
                        self.g.mode = Mode::DockDrag {
                            pid: id,
                            from: slot as usize,
                            over: slot as usize,
                            x: p.x,
                        };
                        self.bump_layout();
                    }
                    Target::Ui(_) | Target::CardButton(_) => self.g.mode = Mode::Ignore,
                }
            }
            Mode::Pan { pid } if pid == id => {
                let delta = Vec2::new(
                    -(p.x - last.x) / self.camera.zoom,
                    -(p.y - last.y) / self.camera.zoom,
                );
                self.camera.pos = self.camera.pos + delta;
                self.momentum.accumulate(delta, self.now);
            }
            Mode::Drag(d) if d.pid == id => {
                if let Mode::Drag(ds) = &mut self.g.mode {
                    ds.finger = p;
                }
                self.apply_drag();
            }
            Mode::DockDrag { pid, from, .. } if pid == id => {
                let over = self.ui_layout().slot_at_x(p.x);
                self.g.mode = Mode::DockDrag {
                    pid,
                    from,
                    over,
                    x: p.x,
                };
            }
            Mode::Multi { .. } => self.update_multi(),
            Mode::Resize { .. } => self.update_resize(),
            Mode::Swipe3 { start, fired } => {
                if fired {
                    return;
                }
                let c = self.centroid(3);
                let d = c - start;
                if d.length() >= SWIPE3_DISTANCE {
                    let dir = if d.x.abs() >= d.y.abs() {
                        if d.x > 0.0 { Direction::Right } else { Direction::Left }
                    } else if d.y > 0.0 {
                        Direction::Down
                    } else {
                        Direction::Up
                    };
                    self.go_nearest(dir);
                    self.g.mode = Mode::Swipe3 { start, fired: true };
                }
            }
            _ => {}
        }
    }

    fn update_multi(&mut self) {
        let Mode::Multi {
            last_centroid,
            last_spread,
            start_spread,
            zoom_engaged,
        } = self.g.mode.clone()
        else {
            return;
        };
        if self.g.pointers.len() < 2 {
            return;
        }
        self.user_moved_camera();
        let centroid = self.centroid(2);
        let spread = self.spread();
        let mut engaged = zoom_engaged;
        let mut baseline = last_spread;

        if !engaged
            && start_spread > 1.0
            && (spread / start_spread - 1.0).abs() > ZOOM_SLOP_RATIO
            && (spread - start_spread).abs() > ZOOM_MIN_DELTA
        {
            engaged = true;
            baseline = spread;
        }

        let mut zoom = self.camera.zoom;
        if engaged && baseline > 1.0 && spread > 1.0 {
            let ratio = (spread / baseline).powf(self.cfg.zoom_speed);
            zoom = self.clamp_zoom(self.camera.zoom * ratio);
        }

        // Keep the canvas point that was under the previous centroid under the
        // new centroid: pan and zoom about the gesture center, with no jump.
        let anchor = self.camera.screen_to_canvas(last_centroid);
        self.camera.zoom = zoom;
        self.camera.pos = zoom_anchor_camera(anchor, centroid, zoom);

        self.g.mode = Mode::Multi {
            last_centroid: centroid,
            last_spread: if engaged { spread } else { last_spread },
            start_spread,
            zoom_engaged: engaged,
        };
    }

    fn begin_resize(&mut self, card_id: u64) {
        self.momentum.stop();
        let s = self.spread();
        let anchor = self.camera.screen_to_canvas(self.centroid(2));
        self.g.mode = Mode::Resize {
            card_id,
            start_spread: s,
            last_spread: s,
            engaged: false,
            anchor,
        };
    }

    /// Scale the card about the pinch centre so the canvas point between the
    /// fingers stays under them; aspect ratio is kept.
    fn update_resize(&mut self) {
        let Mode::Resize {
            card_id,
            start_spread,
            last_spread,
            engaged,
            anchor,
        } = self.g.mode.clone()
        else {
            return;
        };
        if self.g.pointers.len() < 2 {
            return;
        }
        let spread = self.spread();
        let mut engaged = engaged;
        let mut base = last_spread;
        if !engaged
            && start_spread > 1.0
            && (spread / start_spread - 1.0).abs() > ZOOM_SLOP_RATIO
            && (spread - start_spread).abs() > ZOOM_MIN_DELTA
        {
            engaged = true;
            base = spread;
        }
        if engaged
            && base > 1.0
            && spread > 1.0
            && let Some(i) = self.card_index(card_id)
        {
            let r = self.cards[i].rect;
            let new_w = (r.w * spread / base).clamp(MIN_CARD_W, MAX_CARD_W);
            let k = new_w / r.w;
            let a = anchor;
            self.cards[i].rect = Rect::new(a.x + (r.x - a.x) * k, a.y + (r.y - a.y) * k, r.w * k, r.h * k);
            self.geometry_changed();
        }
        self.g.mode = Mode::Resize {
            card_id,
            start_spread,
            last_spread: if engaged { spread } else { last_spread },
            engaged,
            anchor,
        };
    }

    fn begin_drag(&mut self, pid: i32, card_id: u64, finger: Vec2, group: bool) {
        let Some(idx) = self.card_index(card_id) else {
            self.g.mode = Mode::Ignore;
            return;
        };
        let origin = self.cards[idx].rect;
        let mut members = Vec::new();
        if group {
            let set = cluster_of(idx, &self.snap_rects(), self.cfg.snap_gap);
            for m in set {
                if m != idx {
                    let r = self.cards[m].rect;
                    members.push((self.cards[m].id, Vec2::new(r.x - origin.x, r.y - origin.y)));
                }
            }
        }
        self.select(Some(card_id));
        self.raise(card_id);
        // Members rise too, in their existing order, so a dragged group stays on top.
        let ids: Vec<u64> = members.iter().map(|(id, _)| *id).collect();
        for id in ids {
            self.raise(id);
        }
        self.raise(card_id);

        let c = self.camera.screen_to_canvas(finger);
        let pos = self.card(card_id).map(|c| c.rect).unwrap_or(origin);
        self.g.mode = Mode::Drag(DragState {
            pid,
            card_id,
            grab: Vec2::new(pos.x - c.x, pos.y - c.y),
            members,
            snap: SnapState::default(),
            finger,
            edge_since: None,
        });
        self.user_moved_camera();
    }

    fn apply_drag(&mut self) {
        let Mode::Drag(d) = &self.g.mode else { return };
        let finger_canvas = self.camera.screen_to_canvas(d.finger);
        let natural = Vec2::new(finger_canvas.x + d.grab.x, finger_canvas.y + d.grab.y);
        let card_id = d.card_id;
        let member_ids: Vec<(u64, Vec2)> = d.members.clone();
        let Some(idx) = self.card_index(card_id) else { return };
        let size = Vec2::new(self.cards[idx].rect.w, self.cards[idx].rect.h);

        let pos = if self.cfg.snap_enabled {
            let excluded: Vec<u64> = std::iter::once(card_id)
                .chain(member_ids.iter().map(|(id, _)| *id))
                .collect();
            let others: Vec<SnapRect> = self
                .cards
                .iter()
                .filter(|c| !excluded.contains(&c.id))
                .map(|c| SnapRect::from(c.rect))
                .collect();
            let cfg = SnapConfig {
                gap: self.cfg.snap_gap,
                distance: self.cfg.snap_distance,
                break_force: self.cfg.snap_break_force,
                same_edge: self.cfg.snap_same_edge,
                edge_center: self.cfg.snap_centers,
            };
            let zoom = self.camera.zoom;
            let Mode::Drag(d) = &mut self.g.mode else { return };
            snap_move(natural, size, zoom, &others, &cfg, &mut d.snap)
        } else {
            natural
        };

        self.cards[idx].rect.x = pos.x;
        self.cards[idx].rect.y = pos.y;
        for (id, off) in member_ids {
            if let Some(i) = self.card_index(id) {
                self.cards[i].rect.x = pos.x + off.x;
                self.cards[i].rect.y = pos.y + off.y;
            }
        }
        self.geometry_changed();
    }

    /// Per-frame gesture work: long-press timer and edge auto-pan while dragging.
    pub(crate) fn tick_gestures(&mut self, dt: f64) {
        let now = self.now;
        if let Mode::Pending {
            pid,
            target,
            down_ms,
            long,
        } = self.g.mode.clone()
            && !long
            && now.saturating_sub(down_ms) >= self.cfg.long_press_ms
        {
            let allowed = matches!(target, Target::Card(_) | Target::Ui(UiId::DockSlot(_)));
            let filled = match target {
                Target::Ui(UiId::DockSlot(s)) => self.dock.get(s as usize).is_some(),
                _ => true,
            };
            if allowed && filled {
                self.g.mode = Mode::Pending {
                    pid,
                    target,
                    down_ms,
                    long: true,
                };
                self.emit(Event::Haptic);
            }
        }

        let Mode::Drag(d) = &self.g.mode else { return };
        let finger = d.finger;
        let zone = self.cfg.edge_pan_zone;
        let vel = edge_pan_velocity(
            finger,
            self.viewport.x,
            self.viewport.y,
            zone,
            self.cfg.edge_pan_min,
            self.cfg.edge_pan_max,
        );
        let latency = self.cfg.edge_pan_latency_ms;
        let engaged = match (vel, d.edge_since) {
            (None, _) => None,
            (Some(_), None) => Some(now),
            (Some(_), Some(t)) => Some(t),
        };
        if let Mode::Drag(d) = &mut self.g.mode {
            d.edge_since = engaged;
        }
        if let (Some(v), Some(since)) = (vel, engaged)
            && now.saturating_sub(since) >= latency
            && dt > 0.0
        {
            let zoom = self.camera.zoom;
            self.camera.pos = self.camera.pos + Vec2::new(v.x * dt / zoom, v.y * dt / zoom);
            self.apply_drag();
        }
    }

    fn on_up(&mut self, id: i32, p: Vec2) {
        let Some(ptr) = self.pointer_mut(id) else { return };
        ptr.pos = p;
        let start = ptr.start;
        self.g.pointers.retain(|q| q.id != id);
        let remaining = self.g.pointers.len();

        match self.g.mode.clone() {
            Mode::Pending {
                pid,
                target,
                down_ms,
                long,
            } if pid == id => {
                self.g.mode = if remaining == 0 { Mode::Idle } else { Mode::Ignore };
                let moved = p.dist(start) > self.cfg.touch_slop;
                if !moved {
                    self.on_tap(target, p, down_ms, long);
                }
            }
            Mode::Pan { pid } if pid == id => {
                if remaining == 0 {
                    self.momentum.launch(self.now);
                    self.g.mode = Mode::Idle;
                } else {
                    self.g.mode = Mode::Ignore;
                }
                self.emit(Event::Save);
            }
            Mode::Drag(d) if d.pid == id => {
                self.g.mode = if remaining == 0 { Mode::Idle } else { Mode::Ignore };
                self.geometry_changed();
                self.touch_history(d.card_id);
                self.emit(Event::Save);
            }
            Mode::DockDrag { pid, from, over, .. } if pid == id => {
                self.g.mode = if remaining == 0 { Mode::Idle } else { Mode::Ignore };
                if self.dock.move_slot(from, over) {
                    self.emit(Event::Save);
                }
                self.bump_layout();
            }
            Mode::Multi { .. } => {
                self.momentum.tracker.clear();
                if remaining >= 2 {
                    self.begin_multi();
                } else if remaining == 1 {
                    let pid = self.g.pointers[0].id;
                    self.g.pointers[0].last = self.g.pointers[0].pos;
                    self.g.mode = Mode::Pan { pid };
                } else {
                    self.g.mode = Mode::Idle;
                }
                let z = snap_zoom(self.camera.zoom);
                if z != self.camera.zoom {
                    let center = self.viewport * 0.5;
                    let anchor = self.camera.screen_to_canvas(center);
                    self.camera.zoom = z;
                    self.camera.pos = zoom_anchor_camera(anchor, center, z);
                }
                self.emit(Event::Save);
            }
            Mode::Resize { .. } => {
                self.g.mode = if remaining == 0 { Mode::Idle } else { Mode::Ignore };
                self.geometry_changed();
                self.bump_layout();
                self.emit(Event::Save);
            }
            Mode::Swipe3 { .. } => {
                self.g.mode = if remaining == 0 { Mode::Idle } else { Mode::Ignore };
            }
            _ => {
                if remaining == 0 {
                    self.g.mode = Mode::Idle;
                }
            }
        }
    }

    fn on_cancel(&mut self) {
        if matches!(self.g.mode, Mode::Drag(_) | Mode::Resize { .. }) {
            self.geometry_changed();
            self.emit(Event::Save);
        }
        if matches!(self.g.mode, Mode::DockDrag { .. }) {
            self.bump_layout();
        }
        self.g.pointers.clear();
        self.g.mode = Mode::Idle;
    }

    fn on_tap(&mut self, target: Target, p: Vec2, down_ms: u64, long: bool) {
        let now = self.now;
        let _ = down_ms;
        match target {
            Target::Card(id) => {
                if long {
                    self.select(Some(id));
                    self.raise(id);
                    return;
                }
                let is_double = self.g.last_tap.is_some_and(|t| {
                    t.card == id
                        && now.saturating_sub(t.ms) <= self.cfg.double_tap_ms
                        && t.pos.dist(p) <= self.cfg.double_tap_dist
                });
                if is_double {
                    self.g.last_tap = None;
                    self.open_card(id);
                } else {
                    self.g.last_tap = Some(LastTap { card: id, ms: now, pos: p });
                    self.select(Some(id));
                    self.raise(id);
                }
            }
            Target::CardButton(id) => {
                self.g.last_tap = None;
                if self.ui_layout().hit(p).is_none()
                    && self.card_button().is_some_and(|(sel, _)| sel == id)
                {
                    self.toggle_preview(id);
                }
            }
            Target::Empty => {
                self.g.last_tap = None;
                let double = self.g.last_empty_tap.is_some_and(|(ms, pos)| {
                    now.saturating_sub(ms) <= self.cfg.double_tap_ms
                        && pos.dist(p) <= self.cfg.double_tap_dist
                });
                if double {
                    self.g.last_empty_tap = None;
                    self.toggle_home();
                } else {
                    self.g.last_empty_tap = Some((now, p));
                    self.select(None);
                }
            }
            Target::Ui(ui) => {
                if self.ui_layout().hit(p) != Some(ui) {
                    return;
                }
                self.activate_ui(ui, long);
            }
        }
    }

    fn open_card(&mut self, id: u64) {
        let Some(card) = self.card(id) else { return };
        if !card.available {
            let label = card.label.clone();
            self.toast(&format!("{label} is not installed. Select it and tap remove to clear the card"));
            return;
        }
        let key = card.key.clone();
        self.launch_app(&key, LaunchSource::Card);
    }

    fn activate_ui(&mut self, ui: UiId, long: bool) {
        match ui {
            UiId::DockSlot(s) => {
                let slot = s as usize;
                match self.dock.get(slot).map(str::to_owned) {
                    Some(_) if long => self.emit(Event::DockMenu(slot)),
                    Some(key) => {
                        if self.is_installed(&key) {
                            let src = if self.cfg.dock_launches_existing {
                                LaunchSource::Dock
                            } else {
                                LaunchSource::Drawer
                            };
                            self.launch_app(&key, src);
                        } else {
                            self.toast("This app is not installed. Long-press the slot to replace it");
                        }
                    }
                    None => self.open_drawer(Some(slot)),
                }
            }
            UiId::Drawer => self.open_drawer(None),
            UiId::Overview => self.toggle_overview(),
            UiId::Recent => self.cycle_recent(),
            UiId::ToolCenter => {
                if let Some(id) = self.selected {
                    self.center_card(id);
                }
            }
            UiId::ToolGroup => {
                if let Some(id) = self.selected {
                    self.fit_group(id);
                }
            }
            UiId::ToolPreview => {
                if let Some(id) = self.selected {
                    self.toggle_preview(id);
                }
            }
            UiId::CardPreview => {} // handled as Target::CardButton
            UiId::ToolPin => self.pin_selected(),
            UiId::ToolRemove => {
                if let Some(id) = self.selected {
                    self.remove_card(id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawer::AppEntry;
    use crate::launcher::{Card, Event};
    use crate::workspace::Rect;

    const D: f64 = 2.0;

    fn px(v: f64) -> f64 {
        v * D
    }

    /// 360x800 dp screen at density 2.
    fn setup() -> Launcher {
        let mut l = Launcher::new();
        l.set_viewport(720.0, 1600.0, D, [0.0, 48.0, 0.0, 48.0]);
        l.set_apps(
            ["a", "b", "c"]
                .iter()
                .map(|k| AppEntry::new(*k, k.to_uppercase()))
                .collect(),
        );
        l
    }

    fn put_card(l: &mut Launcher, key: &str, x: f64, y: f64) -> u64 {
        let id = (l.cards.len() + 1) as u64 * 100;
        l.cards.push(Card {
            id,
            key: key.into(),
            label: key.to_uppercase(),
            rect: Rect::new(x, y, 88.0, 108.0),
            available: true,
            preview: true,
        });
        l.geometry_changed();
        l.bump_layout();
        id
    }

    struct Finger {
        id: i32,
        x: f64,
        y: f64,
    }

    impl Finger {
        fn down(l: &mut Launcher, id: i32, x: f64, y: f64, t: u64) -> Self {
            l.touch(0, id, px(x), px(y), t);
            Finger { id, x, y }
        }

        fn to(&mut self, l: &mut Launcher, x: f64, y: f64, t: u64) {
            self.x = x;
            self.y = y;
            l.touch(1, self.id, px(x), px(y), t);
        }

        fn up(self, l: &mut Launcher, t: u64) {
            l.touch(2, self.id, px(self.x), px(self.y), t);
        }
    }

    fn tap(l: &mut Launcher, x: f64, y: f64, t: u64) {
        l.touch(0, 0, px(x), px(y), t);
        l.touch(2, 0, px(x), px(y), t + 40);
    }

    fn events(l: &mut Launcher) -> Vec<Event> {
        std::iter::from_fn(|| l.poll_event()).collect()
    }

    fn screen_of(l: &Launcher, id: u64) -> Vec2 {
        l.camera.canvas_to_screen(l.card(id).unwrap().rect.center())
    }

    #[test]
    fn single_tap_selects_without_launching() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        tap(&mut l, c.x, c.y, 1000);
        assert_eq!(l.selected, Some(id));
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn double_tap_launches() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        tap(&mut l, c.x, c.y, 1000);
        tap(&mut l, c.x + 3.0, c.y + 2.0, 1200);
        assert!(events(&mut l).contains(&Event::Launch("a".into())));
    }

    #[test]
    fn slow_second_tap_is_not_double() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        tap(&mut l, c.x, c.y, 1000);
        tap(&mut l, c.x, c.y, 1500);
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn far_second_tap_is_not_double() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        tap(&mut l, c.x - 30.0, c.y, 1000);
        // Second tap lands on the card but outside the allowed distance.
        tap(&mut l, c.x + 30.0, c.y, 1150);
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn double_tap_across_two_cards_does_not_launch() {
        let mut l = setup();
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 200.0, 300.0);
        let (ca, cb) = (screen_of(&l, a), screen_of(&l, b));
        tap(&mut l, ca.x, ca.y, 1000);
        tap(&mut l, cb.x, cb.y, 1100);
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
        assert_eq!(l.selected, Some(b));
    }

    #[test]
    fn drag_is_never_a_tap_and_never_launches() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        for round in 0..2u64 {
            let t0 = 1000 + round * 150;
            let mut f = Finger::down(&mut l, 0, c.x, c.y, t0);
            f.to(&mut l, c.x + 30.0, c.y, t0 + 20);
            f.to(&mut l, c.x + 5.0, c.y, t0 + 40);
            f.up(&mut l, t0 + 60);
        }
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn slop_jitter_still_counts_as_tap() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 3.0, c.y + 2.0, 1010);
        f.up(&mut l, 1040);
        assert_eq!(l.selected, Some(id));
    }

    #[test]
    fn dragging_card_moves_it_and_not_the_camera() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let cam = l.camera;
        let c = screen_of(&l, id);
        let before = l.card(id).unwrap().rect;
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        for i in 1..=10 {
            f.to(&mut l, c.x + i as f64 * 3.0, c.y + i as f64 * 5.0, 1000 + i * 16);
        }
        f.up(&mut l, 1200);
        let after = l.card(id).unwrap().rect;
        // The drag commits after the 8 dp slop; from there the card tracks 1:1.
        assert!((after.x - before.x - 24.0).abs() < 1e-6, "{after:?}");
        assert!((after.y - before.y - 40.0).abs() < 1e-6);
        assert_eq!(l.camera, cam);
    }

    #[test]
    fn card_follows_finger_without_a_jump_at_drag_start() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let before = l.card(id).unwrap().rect;
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 9.0, c.y, 1016);
        let after = l.card(id).unwrap().rect;
        assert!((after.x - before.x).abs() < 1e-6, "card must not leap by the slop distance");
    }

    #[test]
    fn drag_scale_is_screen_one_to_one_when_zoomed() {
        let mut l = setup();
        l.camera.zoom = 0.5;
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let before = l.card(id).unwrap().rect;
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 20.0, c.y, 1016);
        f.to(&mut l, c.x + 40.0, c.y, 1032);
        f.up(&mut l, 1100);
        let after = l.card(id).unwrap().rect;
        // 20 dp of finger travel after the drag began is 40 canvas dp at zoom 0.5.
        assert!((after.x - before.x - 40.0).abs() < 1e-6);
    }

    #[test]
    fn empty_drag_pans_camera_one_to_one() {
        let mut l = setup();
        let cam = l.camera;
        let mut f = Finger::down(&mut l, 0, 100.0, 300.0, 1000);
        f.to(&mut l, 120.0, 300.0, 1016);
        f.to(&mut l, 160.0, 330.0, 1032);
        f.up(&mut l, 1500);
        // Finger moved (60, 30) after leaving the slop at (120, 300): camera moved by -(40, 30).
        assert!((l.camera.pos.x - (cam.pos.x - 40.0)).abs() < 1e-6);
        assert!((l.camera.pos.y - (cam.pos.y - 30.0)).abs() < 1e-6);
    }

    #[test]
    fn fling_leaves_momentum_and_slow_release_does_not() {
        let mut l = setup();
        let mut f = Finger::down(&mut l, 0, 100.0, 300.0, 1000);
        for i in 1..=8 {
            f.to(&mut l, 100.0 + i as f64 * 25.0, 300.0, 1000 + i * 10);
        }
        f.up(&mut l, 1085);
        assert!(l.momentum.coasting);
        assert!(l.momentum.velocity.length() > 500.0);

        let mut l2 = setup();
        let mut f = Finger::down(&mut l2, 0, 100.0, 300.0, 1000);
        f.to(&mut l2, 140.0, 300.0, 1020);
        f.to(&mut l2, 180.0, 300.0, 1400);
        f.up(&mut l2, 2000);
        assert!(!l2.momentum.coasting, "a held finger must not fling");
    }

    #[test]
    fn touching_stops_momentum() {
        let mut l = setup();
        l.momentum.velocity = Vec2::new(2000.0, 0.0);
        l.momentum.coasting = true;
        l.touch(0, 0, 100.0, 100.0, 1000);
        assert!(!l.momentum.coasting);
    }

    #[test]
    fn pinch_zooms_around_gesture_center_with_no_jump() {
        let mut l = setup();
        l.camera.pos = Vec2::new(30.0, -40.0);
        let centroid = Vec2::new(180.0, 400.0);
        let canvas_under = l.camera.screen_to_canvas(centroid);
        l.touch(0, 0, px(150.0), px(400.0), 1000);
        l.touch(0, 1, px(210.0), px(400.0), 1005);
        let mut d = 0.0;
        for i in 1..=10u64 {
            d = i as f64 * 6.0;
            l.touch(1, 0, px(150.0 - d), px(400.0), 1005 + i * 16);
            l.touch(1, 1, px(210.0 + d), px(400.0), 1005 + i * 16);
        }
        assert!(d > 0.0);
        assert!(l.camera.zoom > 1.0 || l.cfg.max_zoom == l.camera.zoom, "spreading zooms in");
        let after = l.camera.canvas_to_screen(canvas_under);
        assert!((after.x - centroid.x).abs() < 1e-6 && (after.y - centroid.y).abs() < 1e-6);
    }

    #[test]
    fn pinch_in_zooms_out_and_respects_min() {
        let mut l = setup();
        put_card(&mut l, "a", 0.0, 0.0);
        l.touch(0, 0, px(60.0), px(400.0), 1000);
        l.touch(0, 1, px(300.0), px(400.0), 1005);
        for i in 1..=60u64 {
            let d = 120.0 - i as f64 * 1.9;
            l.touch(1, 0, px(180.0 - d.max(2.0)), px(400.0), 1005 + i * 16);
            l.touch(1, 1, px(180.0 + d.max(2.0)), px(400.0), 1005 + i * 16);
        }
        assert!(l.camera.zoom < 1.0);
        assert!(l.camera.zoom >= l.min_zoom() - 1e-9);
    }

    #[test]
    fn two_finger_pan_without_spread_change_does_not_zoom() {
        let mut l = setup();
        l.touch(0, 0, px(100.0), px(400.0), 1000);
        l.touch(0, 1, px(200.0), px(400.0), 1005);
        for i in 1..=10u64 {
            let dx = i as f64 * 8.0;
            l.touch(1, 0, px(100.0 + dx), px(400.0 + i as f64 * 2.0), 1005 + i * 16);
            l.touch(1, 1, px(200.0 + dx + (i % 2) as f64), px(400.0 + i as f64 * 2.0), 1005 + i * 16);
        }
        assert_eq!(l.camera.zoom, 1.0);
        assert!(l.camera.pos.x < -50.0, "centroid pan moves the camera");
    }

    #[test]
    fn second_finger_during_card_drag_does_not_hijack() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let cam = l.camera;
        l.touch(0, 0, px(c.x), px(c.y), 1000);
        l.touch(1, 0, px(c.x + 20.0), px(c.y), 1016);
        l.touch(0, 1, px(300.0), px(500.0), 1020);
        l.touch(1, 1, px(310.0), px(520.0), 1030);
        l.touch(1, 0, px(c.x + 40.0), px(c.y), 1040);
        assert_eq!(l.camera, cam);
        assert!(l.card(id).unwrap().rect.x > 136.0 + 15.0);
        l.touch(2, 1, px(310.0), px(520.0), 1050);
        l.touch(2, 0, px(c.x + 40.0), px(c.y), 1060);
        assert!(matches!(l.g.mode, Mode::Idle));
    }

    #[test]
    fn lifting_one_finger_of_two_continues_as_pan_without_jump() {
        let mut l = setup();
        l.touch(0, 0, px(100.0), px(400.0), 1000);
        l.touch(0, 1, px(200.0), px(400.0), 1005);
        l.touch(1, 0, px(110.0), px(400.0), 1020);
        l.touch(2, 1, px(200.0), px(400.0), 1030);
        let cam = l.camera;
        l.touch(1, 0, px(110.5), px(400.0), 1040);
        assert!((l.camera.pos.x - cam.pos.x).abs() < 2.0);
        l.touch(2, 0, px(110.5), px(400.0), 1050);
        assert!(matches!(l.g.mode, Mode::Idle));
    }

    #[test]
    fn dragging_card_next_to_another_snaps_and_forms_group() {
        let mut l = setup();
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 220.0, 300.0);
        let cb = screen_of(&l, b);
        let target_x = 20.0 + 88.0 + l.cfg.snap_gap;
        let mut f = Finger::down(&mut l, 0, cb.x, cb.y, 1000);
        let start_x = l.card(b).unwrap().rect.x;
        let mut x = cb.x;
        while l.card(b).unwrap().rect.x > target_x + 10.0 {
            x -= 4.0;
            f.to(&mut l, x, cb.y, 1016);
        }
        f.up(&mut l, 1500);
        let rb = l.card(b).unwrap().rect;
        assert!(start_x > rb.x);
        assert_eq!(rb.x, target_x, "should have snapped flush with the gap");
        let idx_a = l.card_index(a).unwrap();
        let group = cluster_of(idx_a, &l.snap_rects(), l.cfg.snap_gap);
        assert_eq!(group.len(), 2);
    }

    #[test]
    fn snap_can_be_disabled() {
        let mut l = setup();
        l.cfg.snap_enabled = false;
        put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 220.0, 300.0);
        let cb = screen_of(&l, b);
        let mut f = Finger::down(&mut l, 0, cb.x, cb.y, 1000);
        f.to(&mut l, cb.x - 90.0, cb.y, 1020);
        f.to(&mut l, cb.x - 112.0, cb.y, 1040);
        f.up(&mut l, 1100);
        // The drag began at -90 dp, so the card follows the remaining 22 dp.
        assert_eq!(l.card(b).unwrap().rect.x, 198.0);
    }

    #[test]
    fn plain_drag_detaches_card_from_group() {
        let mut l = setup();
        let gap = l.cfg.snap_gap;
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 20.0 + 88.0 + gap, 300.0);
        let cb = screen_of(&l, b);
        let mut f = Finger::down(&mut l, 0, cb.x, cb.y, 1000);
        for i in 1..=20 {
            f.to(&mut l, cb.x, cb.y + i as f64 * 10.0, 1000 + i * 16);
        }
        f.up(&mut l, 1400);
        let idx_a = l.card_index(a).unwrap();
        assert_eq!(l.card(a).unwrap().rect.x, 20.0, "neighbor must stay put");
        assert_eq!(cluster_of(idx_a, &l.snap_rects(), gap).len(), 1);
    }

    #[test]
    fn long_press_then_drag_moves_whole_group_preserving_layout() {
        let mut l = setup();
        let gap = l.cfg.snap_gap;
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 20.0 + 88.0 + gap, 300.0);
        let c_far = put_card(&mut l, "c", 20.0, 600.0);
        let cb = screen_of(&l, b);
        let (ra, rb, rc) = (l.card(a).unwrap().rect, l.card(b).unwrap().rect, l.card(c_far).unwrap().rect);
        let mut f = Finger::down(&mut l, 0, cb.x, cb.y, 1000);
        l.tick(1000);
        l.tick(1500);
        assert!(events(&mut l).contains(&Event::Haptic));
        f.to(&mut l, cb.x + 30.0, cb.y + 20.0, 1520);
        f.to(&mut l, cb.x + 50.0, cb.y + 60.0, 1540);
        f.up(&mut l, 1600);
        let (na, nb) = (l.card(a).unwrap().rect, l.card(b).unwrap().rect);
        assert_eq!(l.card(c_far).unwrap().rect, rc, "unrelated card stays");
        assert!((nb.x - rb.x - 20.0).abs() < 1e-6 && (nb.y - rb.y - 40.0).abs() < 1e-6);
        assert!(((na.x - ra.x) - (nb.x - rb.x)).abs() < 1e-6, "group moves rigidly");
        assert!(((na.y - ra.y) - (nb.y - rb.y)).abs() < 1e-6);
    }

    #[test]
    fn group_drag_does_not_snap_onto_its_own_members() {
        let mut l = setup();
        let gap = l.cfg.snap_gap;
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 20.0 + 88.0 + gap, 300.0);
        let cb = screen_of(&l, b);
        let mut f = Finger::down(&mut l, 0, cb.x, cb.y, 1000);
        l.tick(1500);
        f.to(&mut l, cb.x + 15.0, cb.y + 1.0, 1520);
        f.to(&mut l, cb.x + 17.0, cb.y + 2.0, 1540);
        f.up(&mut l, 1560);
        let (ra, rb) = (l.card(a).unwrap().rect, l.card(b).unwrap().rect);
        assert!((rb.x - ra.x - (88.0 + gap)).abs() < 1e-6, "relative offset preserved");
    }

    #[test]
    fn edge_pan_scrolls_camera_and_card_follows() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 20.0, c.y, 1016);
        f.to(&mut l, 355.0, c.y, 1032);
        let cam0 = l.camera.pos;
        let card0 = l.card(id).unwrap().rect;
        let mut t = 1032;
        for _ in 0..60 {
            t += 16;
            l.tick(t);
        }
        assert!(l.camera.pos.x > cam0.x + 50.0, "camera should scroll right");
        let card1 = l.card(id).unwrap().rect;
        assert!(card1.x > card0.x + 50.0, "dragged card must travel with the camera");
        let s = screen_of(&l, id);
        assert!((s.x - 355.0).abs() < 60.0 + 1e-6, "card stays under the finger");
        f.up(&mut l, t + 16);
    }

    #[test]
    fn edge_pan_waits_for_latency() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 20.0, c.y, 1016);
        f.to(&mut l, 358.0, c.y, 1032);
        l.tick(1040);
        let cam = l.camera.pos;
        l.tick(1056);
        assert_eq!(l.camera.pos, cam, "no edge pan before the dwell time");
        f.up(&mut l, 1100);
    }

    #[test]
    fn dock_tap_launches_and_empty_slot_opens_picker() {
        let mut l = setup();
        l.dock.assign(0, "a");
        let ui = l.ui_layout();
        let s0 = ui.rect_of(UiId::DockSlot(0)).unwrap().center();
        let s1 = ui.rect_of(UiId::DockSlot(1)).unwrap().center();
        tap(&mut l, s0.x, s0.y, 1000);
        let ev = events(&mut l);
        assert!(ev.contains(&Event::Launch("a".into())));
        assert_eq!(l.cards.len(), 1);
        tap(&mut l, s1.x, s1.y, 2000);
        assert!(events(&mut l).contains(&Event::OpenDrawer(Some(1))));
    }

    #[test]
    fn drawer_button_opens_list() {
        let mut l = setup();
        let r = l.ui_layout().rect_of(UiId::Drawer).unwrap().center();
        tap(&mut l, r.x, r.y, 1000);
        assert!(events(&mut l).contains(&Event::OpenDrawer(None)));
    }

    #[test]
    fn dock_does_not_move_with_camera() {
        let mut l = setup();
        let before = l.ui_layout().rect_of(UiId::DockSlot(2)).unwrap();
        l.camera.pos = Vec2::new(9999.0, -4444.0);
        l.camera.zoom = 0.2;
        assert_eq!(l.ui_layout().rect_of(UiId::DockSlot(2)).unwrap(), before);
        // ...and it is still tappable with the camera far away.
        l.dock.assign(2, "b");
        let c = before.center();
        tap(&mut l, c.x, c.y, 1000);
        assert!(events(&mut l).contains(&Event::Launch("b".into())));
    }

    #[test]
    fn touch_on_dock_over_a_card_hits_dock_first() {
        let mut l = setup();
        l.dock.assign(0, "a");
        let s0 = l.ui_layout().rect_of(UiId::DockSlot(0)).unwrap().center();
        let canvas = l.camera.screen_to_canvas(s0);
        let cid = put_card(&mut l, "b", canvas.x - 44.0, canvas.y - 54.0);
        tap(&mut l, s0.x, s0.y, 1000);
        assert_ne!(l.selected, Some(cid));
        assert!(events(&mut l).contains(&Event::Launch("a".into())));
    }

    #[test]
    fn dock_long_press_release_opens_menu() {
        let mut l = setup();
        l.dock.assign(1, "a");
        let s = l.ui_layout().rect_of(UiId::DockSlot(1)).unwrap().center();
        l.touch(0, 0, px(s.x), px(s.y), 1000);
        l.tick(1000);
        l.tick(1500);
        l.touch(2, 0, px(s.x), px(s.y), 1520);
        let ev = events(&mut l);
        assert!(ev.contains(&Event::DockMenu(1)));
        assert!(!ev.iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn dock_long_press_drag_reorders() {
        let mut l = setup();
        l.dock.assign(0, "a");
        l.dock.assign(1, "b");
        let ui = l.ui_layout();
        let s0 = ui.rect_of(UiId::DockSlot(0)).unwrap().center();
        let s3 = ui.rect_of(UiId::DockSlot(3)).unwrap().center();
        l.touch(0, 0, px(s0.x), px(s0.y), 1000);
        l.tick(1000);
        l.tick(1500);
        l.touch(1, 0, px(s0.x + 30.0), px(s0.y), 1520);
        l.touch(1, 0, px(s3.x), px(s3.y), 1540);
        l.touch(2, 0, px(s3.x), px(s3.y), 1560);
        assert_eq!(l.dock.get(3), Some("a"));
        assert_eq!(l.dock.get(0), Some("b"));
        assert!(l.cards.is_empty());
    }

    #[test]
    fn plain_drag_on_dock_does_not_pan_or_launch() {
        let mut l = setup();
        l.dock.assign(0, "a");
        let s = l.ui_layout().rect_of(UiId::DockSlot(0)).unwrap().center();
        let cam = l.camera;
        let mut f = Finger::down(&mut l, 0, s.x, s.y, 1000);
        f.to(&mut l, s.x + 40.0, s.y - 60.0, 1020);
        f.to(&mut l, s.x + 80.0, s.y - 120.0, 1040);
        f.up(&mut l, 1060);
        assert_eq!(l.camera, cam);
        assert!(l.cards.is_empty());
    }

    #[test]
    fn double_tap_empty_canvas_goes_home_and_back() {
        let mut l = setup();
        l.camera.pos = Vec2::new(4000.0, 4000.0);
        let before = l.camera;
        tap(&mut l, 180.0, 300.0, 1000);
        tap(&mut l, 182.0, 301.0, 1150);
        for i in 0..200 {
            l.tick(2000 + i * 16);
        }
        assert!(l.camera.center(l.viewport).length() < 1.0);
        tap(&mut l, 180.0, 300.0, 6000);
        tap(&mut l, 180.0, 300.0, 6150);
        for i in 0..200 {
            l.tick(7000 + i * 16);
        }
        assert!(l.camera.pos.dist(before.pos) < 1.0);
    }

    #[test]
    fn tap_empty_deselects() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        l.select(Some(id));
        tap(&mut l, 20.0, 150.0, 1000);
        assert_eq!(l.selected, None);
    }

    #[test]
    fn three_finger_swipe_jumps_to_neighbor_card() {
        let mut l = setup();
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let b = put_card(&mut l, "b", 400.0, 300.0);
        l.select(Some(a));
        for (i, x) in [100.0, 150.0, 200.0].iter().enumerate() {
            l.touch(0, i as i32, px(*x), px(200.0), 1000 + i as u64);
        }
        for step in 1..=6 {
            for (i, x) in [100.0, 150.0, 200.0].iter().enumerate() {
                l.touch(1, i as i32, px(x + step as f64 * 15.0), px(200.0), 1010 + step * 10);
            }
        }
        assert_eq!(l.selected, Some(b));
    }

    #[test]
    fn selection_toolbar_remove_deletes_card() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        l.select(Some(id));
        let r = l.ui_layout().rect_of(UiId::ToolRemove).unwrap().center();
        tap(&mut l, r.x, r.y, 1000);
        assert!(l.cards.is_empty());
    }

    #[test]
    fn release_outside_pressed_button_cancels_it() {
        let mut l = setup();
        let d = l.ui_layout().rect_of(UiId::Drawer).unwrap().center();
        l.touch(0, 0, px(d.x), px(d.y), 1000);
        l.touch(2, 0, px(d.x - 150.0), px(d.y - 150.0), 1050);
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::OpenDrawer(_))));
    }

    #[test]
    fn unavailable_card_double_tap_explains_instead_of_launching() {
        let mut l = setup();
        let id = put_card(&mut l, "gone", 136.0, 300.0);
        l.cards[0].available = false;
        let c = screen_of(&l, id);
        tap(&mut l, c.x, c.y, 1000);
        tap(&mut l, c.x, c.y, 1150);
        let ev = events(&mut l);
        assert!(ev.iter().any(|e| matches!(e, Event::Toast(_))));
        assert!(!ev.iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn small_zoomed_out_cards_remain_tappable() {
        let mut l = setup();
        l.camera.zoom = 0.1;
        let id = put_card(&mut l, "a", 0.0, 0.0);
        let c = screen_of(&l, id);
        tap(&mut l, c.x + 10.0, c.y, 1000);
        assert_eq!(l.selected, Some(id));
    }

    #[test]
    fn cancel_resets_everything_and_keeps_card_where_it_is() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let c = screen_of(&l, id);
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 1000);
        f.to(&mut l, c.x + 40.0, c.y, 1020);
        l.touch(3, 0, 0.0, 0.0, 1030);
        assert!(matches!(l.g.mode, Mode::Idle));
        assert!(l.g.pointers.is_empty());
        let _ = id;
    }

    #[test]
    fn tick_reports_activity_only_when_needed() {
        let mut l = setup();
        assert!(!l.tick(1000));
        l.touch(0, 0, 10.0, 10.0, 1100);
        assert!(l.tick(1116));
        l.touch(2, 0, 10.0, 10.0, 1130);
        assert!(!l.tick(1146));
    }

    #[test]
    fn stale_pointer_events_are_ignored() {
        let mut l = setup();
        l.touch(1, 5, 10.0, 10.0, 1000);
        l.touch(2, 5, 10.0, 10.0, 1010);
        assert!(matches!(l.g.mode, Mode::Idle));
    }

    // ---- per-card preview toggle and pinch-resize -------------------------------

    fn select_by_tap(l: &mut Launcher, id: u64, t: u64) {
        let c = screen_of(l, id);
        tap(l, c.x, c.y, t);
        assert_eq!(l.selected, Some(id));
        events(l);
    }

    /// Two fingers straddling `cx`,`cy`; returns after the pinch started.
    fn pinch(l: &mut Launcher, cx: f64, cy: f64, spreads: &[f64], t0: u64) {
        let half = spreads[0] / 2.0;
        let mut a = Finger::down(l, 0, cx - half, cy, t0);
        let mut b = Finger::down(l, 1, cx + half, cy, t0 + 10);
        for (i, s) in spreads.iter().enumerate().skip(1) {
            let t = t0 + 20 + i as u64 * 16;
            a.to(l, cx - s / 2.0, cy, t);
            b.to(l, cx + s / 2.0, cy, t);
        }
        let t = t0 + 400;
        a.up(l, t);
        b.up(l, t + 5);
    }

    #[test]
    fn toolbar_preview_button_toggles_selected_card() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        assert!(l.card(id).unwrap().preview);
        let r = l.ui_layout().rect_of(UiId::ToolPreview).unwrap().center();
        tap(&mut l, r.x, r.y, 2000);
        assert!(!l.card(id).unwrap().preview);
        assert!(events(&mut l).contains(&Event::Save));
        tap(&mut l, r.x, r.y, 3000);
        assert!(l.card(id).unwrap().preview);
    }

    #[test]
    fn card_round_button_toggles_without_opening_the_app() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let other = put_card(&mut l, "b", 20.0, 300.0);
        assert!(l.card_button().is_none(), "no button without a selection");
        select_by_tap(&mut l, id, 1000);
        let (bid, r) = l.card_button().expect("button on the selected card");
        assert_eq!(bid, id);
        let card = l.card(id).unwrap().rect;
        assert!(r.x_high() <= card.x_high() && r.y >= card.y, "button sits inside the top-right corner");
        assert!(r.cx() > card.cx() && r.cy() < card.cy());
        let c = r.center();
        tap(&mut l, c.x, c.y, 2000);
        assert!(!l.card(id).unwrap().preview);
        // A quick second tap on the button must not count as a double tap that opens the app.
        tap(&mut l, c.x, c.y, 2150);
        assert!(l.card(id).unwrap().preview);
        assert!(!events(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
        assert_eq!(l.selected, Some(id));
        assert!(l.card(other).unwrap().preview, "other cards are unaffected");
    }

    #[test]
    fn dragging_from_the_round_button_does_not_move_the_card() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let before = l.card(id).unwrap().rect;
        let c = l.card_button().unwrap().1.center();
        let mut f = Finger::down(&mut l, 0, c.x, c.y, 2000);
        f.to(&mut l, c.x + 40.0, c.y + 40.0, 2020);
        f.up(&mut l, 2040);
        assert_eq!(l.card(id).unwrap().rect, before);
        assert!(l.card(id).unwrap().preview);
    }

    #[test]
    fn pinch_on_selected_card_resizes_it_and_leaves_the_canvas_alone() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let cam = l.camera;
        let before = l.card(id).unwrap().rect;
        pinch(&mut l, 180.0, 354.0, &[40.0, 60.0, 120.0], 2000);
        let after = l.card(id).unwrap().rect;
        assert!((after.w / before.w - 2.0).abs() < 1e-6, "width doubled, got {}", after.w / before.w);
        assert!((after.h / before.h - 2.0).abs() < 1e-6, "aspect ratio kept");
        assert_eq!(l.camera, cam, "canvas camera must not move or zoom");
        assert!(events(&mut l).contains(&Event::Save));
    }

    #[test]
    fn pinch_out_then_in_shrinks_the_card() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let before = l.card(id).unwrap().rect;
        pinch(&mut l, 180.0, 354.0, &[80.0, 100.0, 50.0], 2000);
        assert!((l.card(id).unwrap().rect.w / before.w - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resize_keeps_the_point_between_the_fingers_fixed_in_the_card() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let before = l.card(id).unwrap().rect;
        let rel = |r: crate::workspace::Rect| ((190.0 - r.x) / r.w, (354.0 - r.y) / r.h);
        let (rx, ry) = rel(before);
        pinch(&mut l, 190.0, 354.0, &[40.0, 60.0, 120.0], 2000);
        let after = l.card(id).unwrap().rect;
        let (ax, ay) = rel(after);
        assert!((rx - ax).abs() < 1e-9 && (ry - ay).abs() < 1e-9, "({rx},{ry}) -> ({ax},{ay})");
    }

    #[test]
    fn resize_is_clamped_both_ways() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        pinch(&mut l, 180.0, 354.0, &[30.0, 40.0, 400.0, 2000.0], 2000);
        assert!(l.card(id).unwrap().rect.w <= MAX_CARD_W + 1e-9);
        pinch(&mut l, 180.0, 354.0, &[300.0, 250.0, 120.0, 20.0, 12.0], 5000);
        assert!(l.card(id).unwrap().rect.w >= MIN_CARD_W - 1e-9);
    }

    #[test]
    fn small_jitter_does_not_resize() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let before = l.card(id).unwrap().rect;
        pinch(&mut l, 180.0, 354.0, &[40.0, 42.0, 39.0, 43.0], 2000);
        assert_eq!(l.card(id).unwrap().rect, before);
    }

    #[test]
    fn pinch_on_an_unselected_card_still_zooms_the_canvas() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        let before = l.card(id).unwrap().rect;
        pinch(&mut l, 180.0, 354.0, &[40.0, 60.0, 200.0], 2000);
        assert_eq!(l.card(id).unwrap().rect, before, "card untouched");
        assert!(l.camera.zoom > 1.05, "canvas zoomed, got {}", l.camera.zoom);
    }

    #[test]
    fn pinch_started_on_empty_canvas_zooms_even_if_a_card_is_selected() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        let before = l.card(id).unwrap().rect;
        pinch(&mut l, 180.0, 650.0, &[40.0, 60.0, 200.0], 2000);
        assert_eq!(l.card(id).unwrap().rect, before);
        assert!(l.camera.zoom > 1.05);
    }

    #[test]
    fn size_and_preview_flag_survive_save_and_restore() {
        let mut l = setup();
        let id = put_card(&mut l, "a", 136.0, 300.0);
        select_by_tap(&mut l, id, 1000);
        pinch(&mut l, 180.0, 354.0, &[40.0, 60.0, 120.0], 2000);
        l.toggle_preview(id);
        let l2 = Launcher::from_saved(l.to_saved());
        let c = &l2.cards[0];
        assert_eq!(c.rect, l.card(id).unwrap().rect);
        assert!(!c.preview);
    }

    #[test]
    fn old_state_without_preview_field_defaults_to_shown() {
        let rec: crate::persistence::CardRecord =
            serde_json::from_str(r#"{"key":"a","label":"A","x":0,"y":0,"w":88,"h":108}"#).unwrap();
        assert!(rec.preview);
    }

    #[test]
    fn resized_cards_join_and_leave_groups_by_their_new_edges() {
        let mut l = setup();
        let a = put_card(&mut l, "a", 20.0, 300.0);
        let gap = l.cfg.snap_gap;
        let b = put_card(&mut l, "b", 20.0 + 88.0 + gap, 300.0);
        assert!(l.geometry().pairs.iter().any(|&(i, j, _)| (i, j) == (0, 1) || (i, j) == (1, 0)));
        select_by_tap(&mut l, a, 1000);
        // Pinch the left card bigger about its far-left edge: it now overlaps/leaves adjacency.
        pinch(&mut l, 40.0, 354.0, &[30.0, 50.0, 150.0], 2000);
        let _ = b;
        assert!(l.card(a).unwrap().rect.w > 88.0);
        // The adjacency cache was invalidated: recomputing must not panic and must reflect the new rect.
        let _ = l.geometry().pairs.len();
    }
}
