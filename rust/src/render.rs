//! Display list: the only thing the Android view needs to draw a frame.
//!
//! The list is a flat `f32` buffer (no per-frame allocation on the JVM side)
//! with every coordinate already in screen dp. Cards outside the screen are
//! culled here so a large canvas costs nothing to draw.
//!
//! Layout (see `HEADER` for the leading fields):
//!   header (fields 17..21 are the canvas origin on screen, each coordinate split into
//!   an f32 high part and an f32 remainder so a background tiled on the canvas stays
//!   exact far from the origin), then `n_cards * CARD_STRIDE`, `n_links * LINK_STRIDE`, `n_ui * UI_STRIDE`.

use crate::dock::DockItem;
use crate::launcher::{LIFT_RISE_DP, LIFT_SCALE, Launcher};
use crate::ui::UiId;
use crate::workspace::{Rect, Vec2};

pub const HEADER: usize = 21;
pub const CARD_STRIDE: usize = 7;
pub const LINK_STRIDE: usize = 4;
pub const UI_STRIDE: usize = 7;

pub const CARD_SELECTED: u32 = 1;
pub const CARD_DRAGGING: u32 = 2;
pub const CARD_UNAVAILABLE: u32 = 4;
pub const CARD_SNAP_X: u32 = 8;
pub const CARD_SNAP_Y: u32 = 16;
pub const CARD_NO_PREVIEW: u32 = 32;

pub const UI_PRESSED: u32 = 1;
pub const UI_FILLED: u32 = 2;
pub const UI_DRAG_OVER: u32 = 4;
pub const UI_DRAG_SOURCE: u32 = 8;
/// The dock slot holds a folder (UI_FILLED is set for any occupied slot).
pub const UI_FOLDER: u32 = 16;

/// Dots closer than this (dp) are skipped by doubling the spacing, so the grid
/// stays readable and cheap at every zoom level.
const MIN_DOT_PITCH: f64 = 14.0;
/// Cards this far (dp) beyond the screen edge are still drawn so shadows and
/// group links don't pop in.
const CULL_MARGIN: f64 = 24.0;

fn ui_code(id: UiId) -> (f32, f32) {
    match id {
        UiId::DockSlot(i) => (0.0, f32::from(i)),
        UiId::Drawer => (1.0, 0.0),
        UiId::Overview => (2.0, 0.0),
        UiId::Recent => (3.0, 0.0),
        UiId::ToolCenter => (4.0, 0.0),
        UiId::ToolGroup => (5.0, 0.0),
        UiId::ToolPin => (6.0, 0.0),
        UiId::ToolRemove => (7.0, 0.0),
        UiId::ToolPreview => (8.0, 0.0),
        UiId::CardPreview => (9.0, 0.0),
    }
}

impl Launcher {
    /// Version of the card/dock string tables; changes whenever order, labels
    /// or dock contents change (not on plain movement).
    pub fn layout_version(&self) -> u32 {
        self.layout_version
    }

    /// Strings the view needs next to the numeric display list, one record per
    /// line: `c\tkey\tlabel\tavail` for each card in z-order (same order as the
    /// display list), `d\tslot\tkey` for each dock slot holding an app, and
    /// `f\tslot\tid\tname\tcount\tk1\tk2\tk3\tk4` for each folder (name empty = default,
    /// `kN` the first four apps for the dock tile's mini grid). `p\tn` carries the slot count.
    pub fn string_table(&self) -> String {
        let mut s = String::new();
        for c in &self.cards {
            s.push_str(&format!("c\t{}\t{}\t{}\n", c.key, c.label.replace(['\t', '\n'], " "), u8::from(c.available)));
        }
        s.push_str(&format!("p\t{}\n", self.dock.count()));
        for (i, slot) in self.dock.slots().iter().enumerate() {
            match slot {
                Some(DockItem::App(k)) => s.push_str(&format!("d\t{i}\t{k}\n")),
                Some(DockItem::Folder(f)) => {
                    let first: Vec<&str> = (0..4).map(|n| f.apps.get(n).map_or("", String::as_str)).collect();
                    s.push_str(&format!(
                        "f\t{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        f.id,
                        f.name,
                        f.apps.len(),
                        first[0],
                        first[1],
                        first[2],
                        first[3]
                    ));
                }
                None => {}
            }
        }
        s
    }

    /// Fill `out` with the display list for the current camera. `out` is cleared
    /// first; its capacity is reused across frames.
    pub fn render(&mut self, out: &mut Vec<f32>) {
        out.clear();
        let cam = self.camera;
        let vp = self.viewport;
        let zoom = cam.zoom;

        // Background: dots at a spacing that doubles until readable.
        let mut spacing = self.cfg.dot_spacing * zoom;
        while spacing < MIN_DOT_PITCH && spacing > 0.0 {
            spacing *= 2.0;
        }
        let origin = cam.canvas_to_screen(Vec2::ZERO);
        let ox = origin.x.rem_euclid(spacing);
        let oy = origin.y.rem_euclid(spacing);
        let split = |v: f64| {
            let hi = v as f32;
            (hi, (v - f64::from(hi)) as f32)
        };
        let (origin_x_hi, origin_x_lo) = split(origin.x);
        let (origin_y_hi, origin_y_lo) = split(origin.y);
        let dot_r = (1.1 + 0.9 * zoom.clamp(0.0, 1.5)).min(2.4);

        let layout = self.ui_layout();
        let pressed = self.pressed_ui();
        let dragging = self.dragging_card();
        let snap_flags = self.drag_snap_flags();
        let dock_drag = self.dock_drag();

        let _ = self.geometry();
        let geom = self.geom.as_ref().expect("geometry was just built");
        let (pairs, groups) = (&geom.pairs, &geom.groups);
        let mut sizes = vec![0u32; self.cards.len()];
        for &g in groups {
            sizes[g] += 1;
        }

        let screen = Rect::new(-CULL_MARGIN, -CULL_MARGIN, vp.x + 2.0 * CULL_MARGIN, vp.y + 2.0 * CULL_MARGIN);
        let sr = |r: &Rect| {
            let p = cam.canvas_to_screen(Vec2::new(r.x, r.y));
            Rect::new(p.x, p.y, r.w * zoom, r.h * zoom)
        };

        let mut visible = vec![false; self.cards.len()];
        let mut card_data: Vec<f32> = Vec::new();
        let mut n_cards = 0usize;
        for (i, c) in self.cards.iter().enumerate() {
            let mut r = sr(&c.rect);
            let lift = self.lift_of(c.id);
            if lift > 0.0 {
                // Picked-up card: eased, no overshoot. A lone card grows a little; a group only
                // rises, so its members keep their exact relative positions.
                let k = if self.lift.single { 1.0 + LIFT_SCALE * lift } else { 1.0 };
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                r = Rect::new(cx - r.w * k / 2.0, cy - r.h * k / 2.0 - LIFT_RISE_DP * lift, r.w * k, r.h * k);
            }
            if !r.intersects(&screen) {
                continue;
            }
            visible[i] = true;
            let mut flags = 0u32;
            if self.selected == Some(c.id) {
                flags |= CARD_SELECTED;
            }
            if lift > 0.0 {
                flags |= CARD_DRAGGING;
            }
            if dragging == Some(c.id) {
                if snap_flags & 1 != 0 {
                    flags |= CARD_SNAP_X;
                }
                if snap_flags & 2 != 0 {
                    flags |= CARD_SNAP_Y;
                }
            }
            if !c.available {
                flags |= CARD_UNAVAILABLE;
            }
            if !c.preview {
                flags |= CARD_NO_PREVIEW;
            }
            let grouped = sizes[groups[i]] > 1;
            card_data.extend_from_slice(&[
                i as f32,
                r.x as f32,
                r.y as f32,
                r.w as f32,
                r.h as f32,
                flags as f32,
                if grouped { (groups[i] % 6) as f32 } else { -1.0 },
            ]);
            n_cards += 1;
        }

        let mut link_data: Vec<f32> = Vec::new();
        let mut n_links = 0usize;
        for &(a, b, _) in pairs {
            if !(visible[a] || visible[b]) {
                continue;
            }
            let pa = cam.canvas_to_screen(self.cards[a].rect.center());
            let pb = cam.canvas_to_screen(self.cards[b].rect.center());
            link_data.extend_from_slice(&[pa.x as f32, pa.y as f32, pb.x as f32, pb.y as f32]);
            n_links += 1;
        }

        let mut ui_data: Vec<f32> = Vec::new();
        for item in &layout.items {
            let (kind, slot) = ui_code(item.id);
            let mut flags = 0u32;
            if pressed == Some(item.id) {
                flags |= UI_PRESSED;
            }
            if item.id == UiId::ToolPreview
                && self.selected.and_then(|id| self.card(id)).is_some_and(|c| c.preview)
            {
                flags |= UI_FILLED; // preview currently shown
            }
            if let UiId::DockSlot(i) = item.id {
                match self.dock.get(i as usize) {
                    Some(DockItem::Folder(_)) => flags |= UI_FILLED | UI_FOLDER,
                    Some(DockItem::App(_)) => flags |= UI_FILLED,
                    None => {}
                }
                if let Some((from, over, _)) = dock_drag {
                    if from == i as usize {
                        flags |= UI_DRAG_SOURCE;
                    }
                    if over == i as usize && over != from {
                        flags |= UI_DRAG_OVER;
                    }
                }
            }
            let r = item.rect;
            ui_data.extend_from_slice(&[kind, slot, r.x as f32, r.y as f32, r.w as f32, r.h as f32, flags as f32]);
        }
        if let Some((id, r)) = self.card_button() {
            let (kind, slot) = ui_code(UiId::CardPreview);
            let on = self.card(id).is_some_and(|c| c.preview);
            let flags = if on { UI_FILLED } else { 0 };
            ui_data.extend_from_slice(&[kind, slot, r.x as f32, r.y as f32, r.w as f32, r.h as f32, flags as f32]);
        }
        let n_ui = ui_data.len() / UI_STRIDE;

        let pill = layout.dock_pill;
        let (dd_from, dd_over, dd_x) = dock_drag.map_or((-1.0, -1.0, 0.0), |(f, o, x)| (f as f32, o as f32, x as f32));
        out.extend_from_slice(&[
            self.layout_version as f32,
            zoom as f32,
            spacing as f32,
            ox as f32,
            oy as f32,
            dot_r as f32,
            n_cards as f32,
            n_links as f32,
            n_ui as f32,
            pill.x as f32,
            pill.y as f32,
            pill.w as f32,
            pill.h as f32,
            dd_from,
            dd_over,
            dd_x,
            self.selected.is_some() as u8 as f32,
            origin_x_hi,
            origin_x_lo,
            origin_y_hi,
            origin_y_lo,
        ]);
        debug_assert_eq!(out.len(), HEADER);
        out.extend_from_slice(&card_data);
        out.extend_from_slice(&link_data);
        out.extend_from_slice(&ui_data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawer::AppEntry;
    use crate::launcher::LaunchSource;

    fn launcher() -> Launcher {
        let mut l = Launcher::new();
        l.set_viewport(1080.0, 2400.0, 3.0, [0.0, 72.0, 0.0, 72.0]);
        l.set_apps((0..30).map(|i| AppEntry::new(format!("p{i}"), format!("App {i}"))).collect());
        l
    }

    fn count(buf: &[f32]) -> (usize, usize, usize) {
        (buf[6] as usize, buf[7] as usize, buf[8] as usize)
    }

    #[test]
    fn buffer_length_matches_header_counts() {
        let mut l = launcher();
        for i in 0..5 {
            l.launch_app(&format!("p{i}"), LaunchSource::Drawer);
        }
        let mut b = Vec::new();
        l.render(&mut b);
        let (c, k, u) = count(&b);
        assert_eq!(b.len(), HEADER + c * CARD_STRIDE + k * LINK_STRIDE + u * UI_STRIDE);
        assert_eq!(u, 5 + 3 + 5 + 1, "five slots, drawer, overview, recent + 5 toolbar buttons + the card's round button");
        l.select(None);
        l.render(&mut b);
        let (c, k, u) = count(&b);
        assert_eq!(u, 5 + 3, "no toolbar without a selection");
        assert_eq!(b.len(), HEADER + c * CARD_STRIDE + k * LINK_STRIDE + u * UI_STRIDE);
    }

    #[test]
    fn origin_is_exact_even_far_from_the_canvas_origin() {
        let mut l = launcher();
        l.camera.zoom = 0.75;
        l.camera.pos = Vec2::new(-1_234_567.891_234_5, 7_654_321.123_456_7);
        let want = l.camera.canvas_to_screen(Vec2::ZERO);
        let mut b = Vec::new();
        l.render(&mut b);
        let x = f64::from(b[17]) + f64::from(b[18]);
        let y = f64::from(b[19]) + f64::from(b[20]);
        assert!((x - want.x).abs() < 1e-4 && (y - want.y).abs() < 1e-4, "{x} {y} vs {want:?}");
        // a plain f32 would be off by far more than that at this distance
        assert!((f64::from(b[17]) - want.x).abs() > 1e-4);
    }

    #[test]
    fn far_cards_are_culled() {
        let mut l = launcher();
        l.launch_app("p0", LaunchSource::Drawer);
        l.camera.pos = Vec2::new(1.0e7, -3.0e6);
        let mut b = Vec::new();
        l.render(&mut b);
        assert_eq!(count(&b).0, 0);
    }

    #[test]
    fn dots_stay_readable_when_zoomed_far_out() {
        let mut l = launcher();
        l.camera.zoom = 0.03;
        let mut b = Vec::new();
        l.render(&mut b);
        assert!(f64::from(b[2]) >= MIN_DOT_PITCH);
        assert!(b[3] >= 0.0 && b[3] < b[2] && b[4] >= 0.0 && b[4] < b[2]);
    }

    #[test]
    fn string_table_matches_card_order() {
        let mut l = launcher();
        l.launch_app("p3", LaunchSource::Drawer);
        l.launch_app("p1", LaunchSource::Drawer);
        let t = l.string_table();
        let cards: Vec<&str> = t.lines().filter(|x| x.starts_with("c\t")).collect();
        assert_eq!(cards.len(), 2);
        assert!(cards[0].contains("p3") && cards[1].contains("p1"));
    }

    #[test]
    fn adjacent_cards_produce_a_link_and_group_colour() {
        let mut l = launcher();
        l.launch_app("p0", LaunchSource::Drawer);
        l.launch_app("p1", LaunchSource::Drawer);
        let r0 = l.cards[0].rect;
        l.cards[1].rect = Rect::new(r0.x_high() + l.cfg.snap_gap, r0.y, r0.w, r0.h);
        l.geometry_changed();
        l.camera = crate::workspace::Camera::centered_on(r0.center(), 1.0, l.viewport);
        let mut b = Vec::new();
        l.render(&mut b);
        let (c, k, _) = count(&b);
        assert_eq!((c, k), (2, 1));
        assert!(b[HEADER + 6] >= 0.0, "grouped card carries a group colour");
    }

    #[test]
    fn string_table_lists_dock_apps_folders_and_slot_count() {
        let mut l = launcher();
        l.dock.assign(0, "p1");
        l.dock_set_count(6);
        let id = l.dock_create_folder(2).unwrap();
        for k in ["p2", "p3", "p4", "p5", "p6"] {
            l.dock_add_app(id, k);
        }
        l.dock_rename_folder(id, "Tools");
        let empty = l.dock_create_folder(4).unwrap();
        let t = l.string_table();
        assert!(t.lines().any(|x| x == "p\t6"), "{t}");
        assert!(t.contains("\nd\t0\tp1\n"));
        assert!(t.contains(&format!("\nf\t2\t{id}\tTools\t5\tp2\tp3\tp4\tp5\n")), "{t}");
        assert!(t.contains(&format!("\nf\t4\t{empty}\t\t0\t\t\t\t\n")), "{t}");
    }

    #[test]
    fn dock_slot_flags_distinguish_apps_folders_and_empty_slots() {
        let mut l = launcher();
        l.dock.assign(0, "p1");
        l.dock_create_folder(1).unwrap();
        let mut b = Vec::new();
        l.render(&mut b);
        let (c, k, u) = count(&b);
        let ui = HEADER + c * CARD_STRIDE + k * LINK_STRIDE;
        let flags = |slot: usize| -> u32 {
            (0..u)
                .map(|i| ui + i * UI_STRIDE)
                .find(|&o| b[o] == 0.0 && b[o + 1] as usize == slot)
                .map(|o| b[o + 6] as u32)
                .unwrap()
        };
        assert_eq!(flags(0) & (UI_FILLED | UI_FOLDER), UI_FILLED);
        assert_eq!(flags(1) & (UI_FILLED | UI_FOLDER), UI_FILLED | UI_FOLDER);
        assert_eq!(flags(2) & (UI_FILLED | UI_FOLDER), 0);
    }

    #[test]
    fn the_dock_draws_exactly_the_configured_number_of_slots() {
        let mut l = launcher();
        for n in 3..=8usize {
            l.dock_set_count(n);
            let mut b = Vec::new();
            l.render(&mut b);
            let (c, k, u) = count(&b);
            let ui = HEADER + c * CARD_STRIDE + k * LINK_STRIDE;
            let slots = (0..u).filter(|i| b[ui + i * UI_STRIDE] == 0.0).count();
            assert_eq!(slots, n);
        }
    }
}
