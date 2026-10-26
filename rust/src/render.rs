//! Display list: the only thing the Android view needs to draw a frame.
//!
//! The list is a flat `f32` buffer (no per-frame allocation on the JVM side)
//! with every coordinate already in screen dp. Cards outside the screen are
//! culled here so a large canvas costs nothing to draw.
//!
//! Layout (see `HEADER` for the leading fields):
//!   header, then `n_cards * CARD_STRIDE`, `n_links * LINK_STRIDE`, `n_ui * UI_STRIDE`.

use crate::launcher::Launcher;
use crate::ui::UiId;
use crate::workspace::{Rect, Vec2};

pub const HEADER: usize = 17;
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
    /// line: `c\tkey\tlabel\tavail` for each card in z-order, `d\tslot\tkey`
    /// for each filled dock slot. Cards appear in the same order as the list.
    pub fn string_table(&self) -> String {
        let mut s = String::new();
        for c in &self.cards {
            s.push_str(&format!("c\t{}\t{}\t{}\n", c.key, c.label.replace(['\t', '\n'], " "), u8::from(c.available)));
        }
        for (i, slot) in self.dock.slots().iter().enumerate() {
            if let Some(k) = slot {
                s.push_str(&format!("d\t{i}\t{k}\n"));
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
        let dot_r = (1.1 + 0.9 * zoom.clamp(0.0, 1.5)).min(2.4);

        let layout = self.ui_layout();
        let pressed = self.pressed_ui();
        let dragging = self.dragging_card();
        let snap_flags = self.drag_snap_flags();
        let dock_drag = self.dock_drag();

        let (pairs, groups) = {
            let g = self.geometry();
            (g.pairs.clone(), g.groups.clone())
        };
        let mut sizes = vec![0u32; self.cards.len()];
        for &g in &groups {
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
            let r = sr(&c.rect);
            if !r.intersects(&screen) {
                continue;
            }
            visible[i] = true;
            let mut flags = 0u32;
            if self.selected == Some(c.id) {
                flags |= CARD_SELECTED;
            }
            if dragging == Some(c.id) {
                flags |= CARD_DRAGGING;
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
        for &(a, b, _) in &pairs {
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
                if self.dock.get(i as usize).is_some() {
                    flags |= UI_FILLED;
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
}
