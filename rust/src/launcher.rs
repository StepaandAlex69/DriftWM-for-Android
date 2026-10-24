//! The workspace: cards on an infinite canvas, the camera, the dock, recents and
//! every user-level action. The Android layer owns no logic; it forwards touches,
//! draws the display list from `render`, and executes the `Event`s emitted here.

use std::collections::{HashSet, VecDeque};

use crate::config::Config;
use crate::dock::Dock;
use crate::drawer::{AppEntry, sort_apps};
use crate::gestures::GestureState;
use crate::persistence::{CameraRecord, CardRecord, SavedState};
use crate::placement::choose_position;
use crate::snapping::{self, Side, SnapRect, cluster_of};
use crate::ui::{Insets, UiLayout, layout};
use crate::workspace::{
    Camera, CameraAnimation, Direction, MomentumState, Rect, Vec2, bounding_box, dynamic_min_zoom,
    find_nearest, visible_fraction, zoom_to_fit,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: u64,
    pub key: String,
    pub label: String,
    pub rect: Rect,
    /// False when the app is no longer launchable (uninstalled, disabled, profile gone).
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Start this app through the Android launcher APIs.
    Launch(String),
    /// Show the all-apps list; `Some(slot)` means "pick an app for this dock slot".
    OpenDrawer(Option<usize>),
    /// Offer replace/remove for a dock slot.
    DockMenu(usize),
    Toast(String),
    Haptic,
    /// Workspace changed in a way worth persisting.
    Save,
}

impl Event {
    /// Wire format for the JNI layer: `kind` or `kind\tfield...`.
    pub fn encode(&self) -> String {
        match self {
            Event::Launch(k) => format!("launch\t{k}"),
            Event::OpenDrawer(None) => "drawer\t-1".to_owned(),
            Event::OpenDrawer(Some(s)) => format!("drawer\t{s}"),
            Event::DockMenu(s) => format!("dockmenu\t{s}"),
            Event::Toast(t) => format!("toast\t{t}"),
            Event::Haptic => "haptic".to_owned(),
            Event::Save => "save".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSource {
    Dock,
    Drawer,
    Card,
}

pub struct Launcher {
    pub cfg: Config,
    pub cards: Vec<Card>,
    pub camera: Camera,
    pub viewport: Vec2,
    pub insets: Insets,
    pub density: f64,
    pub dock: Dock,
    pub selected: Option<u64>,
    pub(crate) history: Vec<u64>,
    pub(crate) apps: Vec<AppEntry>,
    pub(crate) events: VecDeque<Event>,
    pub(crate) momentum: MomentumState,
    pub(crate) anim: Option<CameraAnimation>,
    pub(crate) overview_return: Option<Camera>,
    pub(crate) home_return: Option<Camera>,
    pub(crate) cycle_idx: usize,
    pub(crate) cycle_ms: u64,
    pub(crate) g: GestureState,
    pub(crate) now: u64,
    pub(crate) last_tick: Option<u64>,
    pub(crate) layout_version: u32,
    pub(crate) geom: Option<Geometry>,
    pub(crate) drawer_pick: Option<usize>,
    next_id: u64,
}

/// Adjacency derived from card rectangles; rebuilt lazily after any move.
#[derive(Clone)]
pub(crate) struct Geometry {
    pub pairs: Vec<(usize, usize, Side)>,
    pub groups: Vec<usize>,
}

impl Default for Launcher {
    fn default() -> Self {
        Launcher::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        let cfg = Config::default();
        Launcher {
            momentum: MomentumState::new(cfg.drift),
            cfg,
            cards: Vec::new(),
            camera: Camera::default(),
            viewport: Vec2::new(360.0, 800.0),
            insets: Insets::default(),
            density: 1.0,
            dock: Dock::new(),
            selected: None,
            history: Vec::new(),
            apps: Vec::new(),
            events: VecDeque::new(),
            anim: None,
            overview_return: None,
            home_return: None,
            cycle_idx: 0,
            cycle_ms: 0,
            g: GestureState::default(),
            now: 0,
            last_tick: None,
            layout_version: 1,
            geom: None,
            drawer_pick: None,
            next_id: 1,
        }
    }

    pub fn from_saved(saved: SavedState) -> Self {
        let mut l = Launcher::new();
        l.cfg = saved.config.sanitized();
        l.momentum = MomentumState::new(l.cfg.drift);
        l.dock = saved.dock.normalized();
        l.camera = Camera {
            pos: Vec2::new(saved.camera.x, saved.camera.y),
            zoom: saved.camera.zoom,
        };
        for c in saved.cards {
            let id = l.alloc_id();
            l.cards.push(Card {
                id,
                key: c.key,
                label: c.label,
                rect: Rect::new(c.x, c.y, c.w, c.h),
                available: true,
            });
        }
        for key in &saved.recents {
            if let Some(c) = l.cards.iter().find(|c| &c.key == key) {
                let id = c.id;
                if !l.history.contains(&id) {
                    l.history.push(id);
                }
            }
        }
        l.selected = saved
            .selected
            .and_then(|k| l.cards.iter().find(|c| c.key == k).map(|c| c.id));
        l
    }

    pub fn to_saved(&self) -> SavedState {
        let mut s = SavedState::empty();
        s.camera = CameraRecord {
            x: self.camera.pos.x,
            y: self.camera.pos.y,
            zoom: self.camera.zoom,
        };
        s.cards = self
            .cards
            .iter()
            .map(|c| CardRecord {
                key: c.key.clone(),
                label: c.label.clone(),
                x: c.rect.x,
                y: c.rect.y,
                w: c.rect.w,
                h: c.rect.h,
            })
            .collect();
        s.dock = self.dock.clone();
        s.recents = self
            .history
            .iter()
            .filter_map(|id| self.card(*id).map(|c| c.key.clone()))
            .collect();
        s.selected = self.selected.and_then(|id| self.card(id).map(|c| c.key.clone()));
        s.config = self.cfg.clone();
        s
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn card(&self, id: u64) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id)
    }

    pub fn card_index(&self, id: u64) -> Option<usize> {
        self.cards.iter().position(|c| c.id == id)
    }

    pub fn card_index_by_key(&self, key: &str) -> Option<usize> {
        self.cards.iter().position(|c| c.key == key)
    }

    pub(crate) fn emit(&mut self, e: Event) {
        if e == Event::Save && self.events.back() == Some(&Event::Save) {
            return;
        }
        self.events.push_back(e);
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub(crate) fn toast(&mut self, msg: &str) {
        self.emit(Event::Toast(msg.to_owned()));
    }

    pub(crate) fn geometry_changed(&mut self) {
        self.geom = None;
    }

    pub(crate) fn bump_layout(&mut self) {
        self.layout_version = self.layout_version.wrapping_add(1);
    }

    pub(crate) fn snap_rects(&self) -> Vec<SnapRect> {
        self.cards.iter().map(|c| SnapRect::from(c.rect)).collect()
    }

    pub(crate) fn geometry(&mut self) -> &Geometry {
        if self.geom.is_none() {
            let (pairs, groups) = snapping::adjacency(&self.snap_rects(), self.cfg.snap_gap);
            self.geom = Some(Geometry { pairs, groups });
        }
        self.geom.as_ref().unwrap()
    }

    pub fn set_viewport(&mut self, w_px: f64, h_px: f64, density: f64, insets_px: [f64; 4]) {
        let density = if density.is_finite() && density > 0.1 { density } else { 1.0 };
        let new = Vec2::new((w_px / density).max(1.0), (h_px / density).max(1.0));
        if (new.x - self.viewport.x).abs() > 0.5 || (new.y - self.viewport.y).abs() > 0.5 {
            let center = self.camera.center(self.viewport);
            self.viewport = new;
            self.camera = Camera::centered_on(center, self.camera.zoom, new);
        }
        self.density = density;
        self.insets = Insets {
            left: insets_px[0] / density,
            top: insets_px[1] / density,
            right: insets_px[2] / density,
            bottom: insets_px[3] / density,
        };
        self.cfg_changed();
        self.bump_layout();
    }

    /// Re-derive anything that depends on `cfg` after a change.
    pub fn cfg_changed(&mut self) {
        self.cfg = std::mem::take(&mut self.cfg).sanitized();
        self.momentum.drift = self.cfg.drift;
        self.geometry_changed();
    }

    pub fn ui_layout(&self) -> UiLayout {
        layout(self.viewport, self.insets, self.selected.is_some())
    }

    pub fn min_zoom(&self) -> f64 {
        let bbox = bounding_box(self.cards.iter().map(|c| &c.rect));
        dynamic_min_zoom(bbox, self.viewport, self.cfg.fit_padding).min(0.4)
    }

    pub fn clamp_zoom(&self, z: f64) -> f64 {
        z.clamp(self.min_zoom().min(self.cfg.max_zoom), self.cfg.max_zoom)
    }

    pub fn view_rect(&self) -> Rect {
        self.camera.visible_rect(self.viewport)
    }

    /// Any direct manipulation of the camera cancels programmatic motion and the
    /// "go back" targets, which would otherwise yank the view somewhere stale.
    pub(crate) fn user_moved_camera(&mut self) {
        self.anim = None;
        self.overview_return = None;
        self.home_return = None;
    }

    pub fn animate_to(&mut self, center: Vec2, zoom: f64) {
        self.momentum.stop();
        self.anim = Some(CameraAnimation {
            target_center: center,
            target_zoom: self.clamp_zoom(zoom),
        });
    }

    pub fn select(&mut self, id: Option<u64>) {
        if self.selected != id {
            self.selected = id;
            self.bump_layout();
        }
    }

    /// Bring a card to the top of the z-order.
    pub fn raise(&mut self, id: u64) {
        if let Some(i) = self.card_index(id)
            && i + 1 != self.cards.len()
        {
            let c = self.cards.remove(i);
            self.cards.push(c);
            self.geometry_changed();
            self.bump_layout();
        }
    }

    pub(crate) fn touch_history(&mut self, id: u64) {
        self.history.retain(|h| *h != id);
        self.history.insert(0, id);
        self.history.truncate(64);
    }

    /// Pan (and zoom in if needed) so the card is comfortably on screen.
    pub fn reveal_card(&mut self, id: u64) {
        let Some(c) = self.card(id) else { return };
        let rect = c.rect;
        if visible_fraction(&rect, &self.view_rect()) >= 0.9 {
            return;
        }
        let zoom = self.camera.zoom.max(0.6);
        self.animate_to(rect.center(), zoom);
    }

    pub fn center_card(&mut self, id: u64) {
        if let Some(c) = self.card(id) {
            let center = c.rect.center();
            self.animate_to(center, 1.0);
        }
    }

    pub fn fit_group(&mut self, id: u64) {
        let Some(i) = self.card_index(id) else { return };
        let members = cluster_of(i, &self.snap_rects(), self.cfg.snap_gap);
        let Some(bbox) = bounding_box(members.iter().map(|&m| &self.cards[m].rect)) else {
            return;
        };
        let zoom = zoom_to_fit(&bbox, self.viewport, self.cfg.fit_padding, 1.0);
        self.animate_to(bbox.center(), zoom);
    }

    pub fn toggle_overview(&mut self) {
        if let Some(back) = self.overview_return.take() {
            let c = back.center(self.viewport);
            self.animate_to(c, back.zoom);
            return;
        }
        let Some(bbox) = bounding_box(self.cards.iter().map(|c| &c.rect)) else {
            self.toast("Nothing on the canvas yet");
            return;
        };
        let back = self.camera;
        let zoom = zoom_to_fit(&bbox, self.viewport, self.cfg.fit_padding, 1.0);
        self.animate_to(bbox.center(), zoom);
        self.overview_return = Some(back);
        self.home_return = None;
    }

    /// Double tap on empty canvas: jump to the origin view, and back.
    pub fn toggle_home(&mut self) {
        if let Some(back) = self.home_return.take() {
            let c = back.center(self.viewport);
            self.animate_to(c, back.zoom);
            return;
        }
        let at_home = self.camera.center(self.viewport).length() < 4.0
            && (self.camera.zoom - 1.0).abs() < 0.01;
        if at_home {
            return;
        }
        let back = self.camera;
        self.animate_to(Vec2::ZERO, 1.0);
        self.home_return = Some(back);
        self.overview_return = None;
    }

    /// Alt-Tab style: each press within 2.5 s steps one card further back in use.
    pub fn cycle_recent(&mut self) {
        let ids: Vec<u64> = self.history.iter().copied().filter(|h| self.card(*h).is_some()).collect();
        if ids.len() < 2 {
            self.toast("No other recent cards");
            return;
        }
        if self.now.saturating_sub(self.cycle_ms) > 2500 {
            self.cycle_idx = 0;
        }
        self.cycle_idx = (self.cycle_idx + 1) % ids.len();
        self.cycle_ms = self.now;
        let id = ids[self.cycle_idx];
        self.select(Some(id));
        self.raise(id);
        self.center_card(id);
    }

    /// Jump to the nearest card in a direction from the selection (or view center).
    pub fn go_nearest(&mut self, dir: Direction) {
        let origin = self
            .selected
            .and_then(|id| self.card(id))
            .map_or_else(|| self.camera.center(self.viewport), |c| c.rect.center());
        let skip = self.selected;
        let target = find_nearest(
            origin,
            dir,
            self.cards.iter().map(|c| (c.id, c.rect.center())),
            skip,
        );
        if let Some(id) = target {
            self.select(Some(id));
            self.raise(id);
            self.reveal_card(id);
            self.center_card_keep_zoom(id);
        }
    }

    fn center_card_keep_zoom(&mut self, id: u64) {
        if let Some(c) = self.card(id) {
            let center = c.rect.center();
            self.animate_to(center, self.camera.zoom);
        }
    }

    pub fn remove_card(&mut self, id: u64) {
        if let Some(i) = self.card_index(id) {
            self.cards.remove(i);
            self.history.retain(|h| *h != id);
            if self.selected == Some(id) {
                self.selected = None;
            }
            self.geometry_changed();
            self.bump_layout();
            self.emit(Event::Save);
        }
    }

    pub fn pin_selected(&mut self) {
        let Some(card) = self.selected.and_then(|id| self.card(id)) else { return };
        let key = card.key.clone();
        if self.dock.position_of(&key).is_some() {
            self.toast("Already in the dock");
        } else if let Some(slot) = self.dock.first_empty() {
            self.dock.assign(slot, &key);
            self.bump_layout();
            self.emit(Event::Save);
        } else {
            self.toast("Dock is full: long-press a slot to replace it");
        }
    }

    pub fn is_installed(&self, key: &str) -> bool {
        self.apps.iter().any(|a| a.key == key)
    }

    /// Replace the installed-app list (already filtered to launchable apps).
    pub fn set_apps(&mut self, mut apps: Vec<AppEntry>) {
        sort_apps(&mut apps);
        let installed: HashSet<&str> = apps.iter().map(|a| a.key.as_str()).collect();
        let mut changed = false;
        for c in &mut self.cards {
            let now = installed.contains(c.key.as_str());
            if c.available != now {
                c.available = now;
                changed = true;
            }
            if let Some(a) = apps.iter().find(|a| a.key == c.key)
                && a.label != c.label
            {
                c.label = a.label.clone();
                changed = true;
            }
        }
        self.apps = apps;
        if changed {
            self.bump_layout();
            self.emit(Event::Save);
        }
    }

    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    pub fn search_apps(&self, query: &str) -> Vec<usize> {
        crate::drawer::search(&self.apps, query)
    }

    /// Launch an app through the launcher UI, creating and placing its card the
    /// first time. Opening from the drawer an app that already has a card only
    /// brings that card into view (configurable); the user double-taps to open.
    pub fn launch_app(&mut self, key: &str, source: LaunchSource) {
        let Some(app) = self.apps.iter().find(|a| a.key == key).cloned() else {
            self.toast("This app is not available");
            return;
        };

        if let Some(i) = self.card_index_by_key(key) {
            let id = self.cards[i].id;
            self.select(Some(id));
            self.raise(id);
            let jump_only = source == LaunchSource::Drawer && self.cfg.drawer_jumps_to_existing;
            if jump_only {
                self.center_card(id);
            } else {
                self.touch_history(id);
                self.reveal_card(id);
                self.emit(Event::Launch(key.to_owned()));
            }
            self.emit(Event::Save);
            return;
        }

        let rects: Vec<Rect> = self.cards.iter().map(|c| c.rect).collect();
        let focus = self
            .selected
            .and_then(|id| self.card_index(id))
            .or_else(|| self.history.first().and_then(|id| self.card_index(*id)));
        let pos = choose_position(
            &rects,
            focus,
            &self.view_rect(),
            self.cfg.card_w,
            self.cfg.card_h,
            self.cfg.snap_gap,
        );
        let id = self.alloc_id();
        self.cards.push(Card {
            id,
            key: key.to_owned(),
            label: app.label,
            rect: Rect::new(pos.x, pos.y, self.cfg.card_w, self.cfg.card_h),
            available: true,
        });
        self.geometry_changed();
        self.bump_layout();
        self.select(Some(id));
        self.touch_history(id);
        self.reveal_card(id);
        self.emit(Event::Launch(key.to_owned()));
        self.emit(Event::Save);
    }

    pub fn drawer_pick(&mut self, key: &str) {
        match self.drawer_pick.take() {
            Some(slot) => {
                if self.is_installed(key) {
                    self.dock.assign(slot, key);
                    self.bump_layout();
                    self.emit(Event::Save);
                } else {
                    self.toast("This app is not available");
                }
            }
            None => self.launch_app(key, LaunchSource::Drawer),
        }
    }

    pub fn open_drawer(&mut self, pick_slot: Option<usize>) {
        self.drawer_pick = pick_slot;
        self.emit(Event::OpenDrawer(pick_slot));
    }

    pub fn drawer_closed(&mut self) {
        self.drawer_pick = None;
    }

    pub fn dock_remove(&mut self, slot: usize) {
        if self.dock.remove(slot) {
            self.bump_layout();
            self.emit(Event::Save);
        }
    }

    pub fn dock_replace(&mut self, slot: usize) {
        self.open_drawer(Some(slot));
    }

    /// Back/Home pressed while already on the launcher.
    pub fn on_home(&mut self) {
        self.select(None);
    }

    /// Advance animations. Returns true while another frame is needed.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        self.now = now_ms;
        let dt = match self.last_tick {
            Some(prev) => (now_ms.saturating_sub(prev) as f64 / 1000.0).min(0.05),
            None => 0.0,
        };
        self.last_tick = Some(now_ms);

        self.tick_gestures(dt);

        let mut active = false;
        if let Some(delta) = self.momentum.tick(dt) {
            self.camera.pos = self.camera.pos + delta;
        }
        if self.momentum.coasting {
            active = true;
        }
        if let Some(anim) = self.anim {
            let mut cam = self.camera;
            if anim.step(&mut cam, self.viewport, dt.max(1.0 / 240.0)) {
                active = true;
            } else {
                self.anim = None;
                self.emit(Event::Save);
            }
            self.camera = cam;
        }
        active || self.gesture_active()
    }

    /// Call when the activity goes to background so a stale `last_tick` doesn't
    /// produce a long first step.
    pub fn reset_clock(&mut self) {
        self.last_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn apps(keys: &[&str]) -> Vec<AppEntry> {
        keys.iter().map(|k| AppEntry::new(*k, k.to_uppercase())).collect()
    }

    pub fn launcher() -> Launcher {
        let mut l = Launcher::new();
        l.set_viewport(1080.0, 2400.0, 3.0, [0.0, 72.0, 0.0, 72.0]);
        l.set_apps(apps(&["a", "b", "c", "d", "e", "f", "g"]));
        l
    }

    fn drain(l: &mut Launcher) -> Vec<Event> {
        std::iter::from_fn(|| l.poll_event()).collect()
    }

    #[test]
    fn first_launch_creates_card_and_launches() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Drawer);
        assert_eq!(l.cards.len(), 1);
        let ev = drain(&mut l);
        assert!(ev.contains(&Event::Launch("a".into())));
        assert!(ev.contains(&Event::Save));
        assert_eq!(l.selected, Some(l.cards[0].id));
        let card = l.cards[0].rect;
        assert!(visible_fraction(&card, &l.view_rect()) > 0.99);
    }

    #[test]
    fn second_launch_never_duplicates() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Drawer);
        drain(&mut l);
        l.launch_app("a", LaunchSource::Dock);
        l.launch_app("a", LaunchSource::Card);
        assert_eq!(l.cards.len(), 1);
        let launches = drain(&mut l).into_iter().filter(|e| matches!(e, Event::Launch(_))).count();
        assert_eq!(launches, 2);
    }

    #[test]
    fn drawer_selecting_existing_card_only_navigates() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Drawer);
        l.camera.pos = Vec2::new(50000.0, 50000.0);
        drain(&mut l);
        l.launch_app("a", LaunchSource::Drawer);
        let ev = drain(&mut l);
        assert!(!ev.iter().any(|e| matches!(e, Event::Launch(_))));
        assert!(l.anim.is_some(), "camera should fly to the card");
        for t in (0..120).map(|i| i * 16) {
            l.tick(t);
        }
        assert!(visible_fraction(&l.cards[0].rect, &l.view_rect()) > 0.99);
    }

    #[test]
    fn drawer_can_be_configured_to_launch_existing() {
        let mut l = launcher();
        l.cfg.drawer_jumps_to_existing = false;
        l.launch_app("a", LaunchSource::Drawer);
        drain(&mut l);
        l.launch_app("a", LaunchSource::Drawer);
        assert!(drain(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn unknown_app_does_not_create_card() {
        let mut l = launcher();
        l.launch_app("ghost", LaunchSource::Dock);
        assert!(l.cards.is_empty());
        assert!(matches!(drain(&mut l)[0], Event::Toast(_)));
    }

    #[test]
    fn new_cards_never_overlap_and_stay_in_view() {
        let mut l = launcher();
        for k in ["a", "b", "c", "d", "e", "f", "g"] {
            l.launch_app(k, LaunchSource::Drawer);
        }
        for i in 0..l.cards.len() {
            for j in (i + 1)..l.cards.len() {
                assert!(!l.cards[i].rect.intersects(&l.cards[j].rect));
            }
        }
        // Newly created cards cluster: at least one adjacent pair exists.
        assert!(!l.geometry().pairs.is_empty());
    }

    #[test]
    fn app_removed_marks_card_unavailable_and_it_can_be_dismissed() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Drawer);
        l.launch_app("b", LaunchSource::Drawer);
        l.set_apps(apps(&["b", "c"]));
        assert!(!l.cards[0].available);
        assert!(l.cards[1].available);
        let id = l.cards[0].id;
        l.remove_card(id);
        assert_eq!(l.cards.len(), 1);
        assert_ne!(l.selected, Some(id));
    }

    #[test]
    fn reinstalling_restores_availability() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Drawer);
        l.set_apps(apps(&["b"]));
        assert!(!l.cards[0].available);
        l.set_apps(apps(&["a", "b"]));
        assert!(l.cards[0].available);
    }

    #[test]
    fn save_events_coalesce() {
        let mut l = launcher();
        l.emit(Event::Save);
        l.emit(Event::Save);
        assert_eq!(drain(&mut l).len(), 1);
    }

    #[test]
    fn pin_selected_fills_dock_then_reports_full() {
        let mut l = launcher();
        for k in ["a", "b", "c", "d", "e", "f"] {
            l.launch_app(k, LaunchSource::Drawer);
            l.pin_selected();
        }
        assert_eq!(l.dock.slots().iter().filter(|s| s.is_some()).count(), 5);
        let ev = drain(&mut l);
        assert!(ev.iter().any(|e| matches!(e, Event::Toast(t) if t.contains("full"))));
        l.launch_app("a", LaunchSource::Dock);
        l.pin_selected();
        assert!(drain(&mut l).iter().any(|e| matches!(e, Event::Toast(t) if t.contains("Already"))));
    }

    #[test]
    fn drawer_pick_assigns_slot_instead_of_launching() {
        let mut l = launcher();
        l.open_drawer(Some(3));
        l.drawer_pick("c");
        assert_eq!(l.dock.get(3), Some("c"));
        assert!(l.cards.is_empty());
        assert!(!drain(&mut l).iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn overview_round_trip() {
        let mut l = launcher();
        for k in ["a", "b", "c"] {
            l.launch_app(k, LaunchSource::Drawer);
        }
        l.cards[0].rect.x -= 3000.0;
        l.geometry_changed();
        let before = l.camera;
        l.toggle_overview();
        for t in (0..200).map(|i| i * 16) {
            l.tick(t);
        }
        assert!(l.camera.zoom < before.zoom);
        let view = l.view_rect();
        for c in &l.cards {
            assert!(visible_fraction(&c.rect, &view) > 0.99, "overview must show every card");
        }
        l.toggle_overview();
        for t in (200..400).map(|i| i * 16) {
            l.tick(t);
        }
        assert!((l.camera.zoom - before.zoom).abs() < 1e-3);
        assert!(l.camera.pos.dist(before.pos) < 0.5);
    }

    #[test]
    fn home_toggle_returns() {
        let mut l = launcher();
        l.camera.pos = Vec2::new(5000.0, 3000.0);
        let before = l.camera;
        l.toggle_home();
        for t in (0..200).map(|i| i * 16) {
            l.tick(t);
        }
        assert!(l.camera.center(l.viewport).length() < 1.0);
        l.toggle_home();
        for t in (200..400).map(|i| i * 16) {
            l.tick(t);
        }
        assert!(l.camera.pos.dist(before.pos) < 0.5);
    }

    #[test]
    fn cycle_recent_walks_history() {
        let mut l = launcher();
        for k in ["a", "b", "c"] {
            l.launch_app(k, LaunchSource::Dock);
        }
        l.now = 10_000;
        l.cycle_recent();
        assert_eq!(l.card(l.selected.unwrap()).unwrap().key, "b");
        l.now = 10_500;
        l.cycle_recent();
        assert_eq!(l.card(l.selected.unwrap()).unwrap().key, "a");
    }

    #[test]
    fn go_nearest_picks_directional_neighbor() {
        let mut l = launcher();
        for k in ["a", "b"] {
            l.launch_app(k, LaunchSource::Dock);
        }
        let a = l.cards.iter().find(|c| c.key == "a").unwrap().id;
        let b = l.cards.iter().find(|c| c.key == "b").unwrap().id;
        l.cards.iter_mut().find(|c| c.id == a).unwrap().rect = Rect::new(0.0, 0.0, 88.0, 108.0);
        l.cards.iter_mut().find(|c| c.id == b).unwrap().rect = Rect::new(300.0, 0.0, 88.0, 108.0);
        l.select(Some(a));
        l.go_nearest(Direction::Right);
        assert_eq!(l.selected, Some(b));
        l.go_nearest(Direction::Up);
        assert_eq!(l.selected, Some(b));
    }

    #[test]
    fn fit_group_covers_whole_cluster() {
        let mut l = launcher();
        l.cards = vec![
            Card { id: 1, key: "a".into(), label: "A".into(), rect: Rect::new(0.0, 0.0, 88.0, 108.0), available: true },
            Card { id: 2, key: "b".into(), label: "B".into(), rect: Rect::new(100.0, 0.0, 88.0, 108.0), available: true },
            Card { id: 3, key: "c".into(), label: "C".into(), rect: Rect::new(5000.0, 0.0, 88.0, 108.0), available: true },
        ];
        l.fit_group(1);
        for t in (0..200).map(|i| i * 16) {
            l.tick(t);
        }
        let view = l.view_rect();
        assert!(visible_fraction(&l.cards[0].rect, &view) > 0.99);
        assert!(visible_fraction(&l.cards[1].rect, &view) > 0.99);
        assert_eq!(visible_fraction(&l.cards[2].rect, &view), 0.0);
    }

    #[test]
    fn saved_state_round_trip() {
        let mut l = launcher();
        for k in ["a", "b", "c"] {
            l.launch_app(k, LaunchSource::Dock);
        }
        l.dock.assign(1, "b");
        l.camera = Camera { pos: Vec2::new(-77.5, 1.0e7), zoom: 0.31 };
        let saved = l.to_saved();
        let json = crate::persistence::to_json(&saved);
        let dir = std::env::temp_dir().join(format!("drift-l-{}", std::process::id()));
        let p = dir.join("s.json");
        crate::persistence::write_atomic(&p, &json).unwrap();
        let (loaded, _) = crate::persistence::load(&p);
        let l2 = Launcher::from_saved(loaded);
        assert_eq!(l2.cards.len(), 3);
        assert_eq!(l2.camera, l.camera);
        assert_eq!(l2.dock, l.dock);
        let keys = |l: &Launcher| l.cards.iter().map(|c| (c.key.clone(), c.rect)).collect::<Vec<_>>();
        assert_eq!(keys(&l), keys(&l2), "positions and z-order must survive");
        assert_eq!(
            l2.selected.and_then(|id| l2.card(id)).map(|c| c.key.clone()),
            l.selected.and_then(|id| l.card(id)).map(|c| c.key.clone())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restored_cards_do_not_launch_anything() {
        let mut l = launcher();
        l.launch_app("a", LaunchSource::Dock);
        let saved = l.to_saved();
        let mut l2 = Launcher::from_saved(saved);
        assert!(l2.poll_event().is_none());
        l2.set_apps(apps(&["a"]));
        assert!(!l2.events.iter().any(|e| matches!(e, Event::Launch(_))));
    }

    #[test]
    fn viewport_resize_keeps_center() {
        let mut l = launcher();
        l.camera = Camera::centered_on(Vec2::new(123.0, -45.0), 0.8, l.viewport);
        l.set_viewport(2400.0, 1080.0, 3.0, [0.0; 4]);
        let c = l.camera.center(l.viewport);
        assert!((c.x - 123.0).abs() < 1e-6 && (c.y + 45.0).abs() < 1e-6);
    }

    #[test]
    fn many_cards_stay_non_overlapping() {
        let mut l = launcher();
        let keys: Vec<String> = (0..80).map(|i| format!("app{i}")).collect();
        l.set_apps(keys.iter().map(|k| AppEntry::new(k.clone(), k.clone())).collect());
        for k in &keys {
            l.launch_app(k, LaunchSource::Drawer);
        }
        assert_eq!(l.cards.len(), 80);
        for i in 0..l.cards.len() {
            for j in (i + 1)..l.cards.len() {
                assert!(!l.cards[i].rect.intersects(&l.cards[j].rect), "{i} overlaps {j}");
            }
        }
    }
}
