//! Screen-fixed interface geometry: dock, all-apps button, quick-nav buttons and
//! the selection toolbar. None of these are canvas objects, so they never move
//! with the camera. All coordinates are dp in screen space.

use crate::dock::SLOTS;
use crate::workspace::{Rect, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiId {
    DockSlot(u8),
    Drawer,
    Overview,
    Recent,
    ToolCenter,
    ToolGroup,
    ToolPin,
    ToolRemove,
    /// Show/hide the app preview on the selected card (toolbar).
    ToolPreview,
    /// Round button in the selected card's top-right corner (not part of `layout`).
    CardPreview,
}

impl UiId {
    pub fn kind(self) -> &'static str {
        match self {
            UiId::DockSlot(_) => "slot",
            UiId::Drawer => "drawer",
            UiId::Overview => "overview",
            UiId::Recent => "recent",
            UiId::ToolCenter => "center",
            UiId::ToolGroup => "group",
            UiId::ToolPin => "pin",
            UiId::ToolRemove => "remove",
            UiId::ToolPreview => "preview",
            UiId::CardPreview => "cardpreview",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Insets {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct UiItem {
    pub id: UiId,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct UiLayout {
    pub items: Vec<UiItem>,
    /// Background pill behind the five dock slots.
    pub dock_pill: Rect,
}

const MARGIN: f64 = 12.0;
const BOTTOM_PAD: f64 = 10.0;
const DRAWER_BTN: f64 = 52.0;
const ROW_GAP: f64 = 10.0;
const PILL_PAD: f64 = 6.0;
const SLOT_GAP: f64 = 4.0;
const MAX_SLOT: f64 = 60.0;
const NAV_BTN: f64 = 40.0;
const TOOL_W: f64 = 64.0;
const TOOL_H: f64 = 40.0;
const TOOL_GAP: f64 = 8.0;

pub fn layout(viewport: Vec2, insets: Insets, has_selection: bool) -> UiLayout {
    let left = insets.left + MARGIN;
    let right = viewport.x - insets.right - MARGIN;
    let bottom = viewport.y - insets.bottom - BOTTOM_PAD;

    let pill_avail = (right - left) - ROW_GAP - DRAWER_BTN;
    let slot = ((pill_avail - 2.0 * PILL_PAD - SLOT_GAP * (SLOTS as f64 - 1.0)) / SLOTS as f64)
        .clamp(24.0, MAX_SLOT);
    let pill_w = slot * SLOTS as f64 + SLOT_GAP * (SLOTS as f64 - 1.0) + 2.0 * PILL_PAD;
    let pill_h = slot + 2.0 * PILL_PAD;
    let pill = Rect::new(left, bottom - pill_h, pill_w, pill_h);

    let mut items = Vec::new();
    for i in 0..SLOTS {
        items.push(UiItem {
            id: UiId::DockSlot(i as u8),
            rect: Rect::new(
                pill.x + PILL_PAD + i as f64 * (slot + SLOT_GAP),
                pill.y + PILL_PAD,
                slot,
                slot,
            ),
        });
    }
    let drawer = Rect::new(
        right - DRAWER_BTN,
        pill.y + (pill_h - DRAWER_BTN) / 2.0,
        DRAWER_BTN,
        DRAWER_BTN,
    );
    items.push(UiItem {
        id: UiId::Drawer,
        rect: drawer,
    });

    let nav_x = drawer.x + (DRAWER_BTN - NAV_BTN) / 2.0;
    let nav_top = drawer.y.min(pill.y) - ROW_GAP - NAV_BTN;
    items.push(UiItem {
        id: UiId::Overview,
        rect: Rect::new(nav_x, nav_top, NAV_BTN, NAV_BTN),
    });
    items.push(UiItem {
        id: UiId::Recent,
        rect: Rect::new(nav_x, nav_top - ROW_GAP - NAV_BTN, NAV_BTN, NAV_BTN),
    });

    if has_selection {
        let tools = [
            UiId::ToolCenter,
            UiId::ToolGroup,
            UiId::ToolPreview,
            UiId::ToolPin,
            UiId::ToolRemove,
        ];
        let n = tools.len() as f64;
        // Shrink the buttons on narrow screens so the row always fits.
        let tool_w = ((right - left - TOOL_GAP * (n - 1.0)) / n).clamp(40.0, TOOL_W);
        let total = tool_w * n + TOOL_GAP * (n - 1.0);
        let x0 = ((viewport.x - total) / 2.0).max(left);
        let y = insets.top + MARGIN;
        for (i, id) in tools.into_iter().enumerate() {
            items.push(UiItem {
                id,
                rect: Rect::new(x0 + i as f64 * (tool_w + TOOL_GAP), y, tool_w, TOOL_H),
            });
        }
    }

    UiLayout {
        items,
        dock_pill: pill,
    }
}

impl UiLayout {
    pub fn hit(&self, p: Vec2) -> Option<UiId> {
        self.items.iter().find(|i| i.rect.contains(p)).map(|i| i.id)
    }

    pub fn rect_of(&self, id: UiId) -> Option<Rect> {
        self.items.iter().find(|i| i.id == id).map(|i| i.rect)
    }

    /// Dock slot whose column contains screen x, clamped to the pill.
    pub fn slot_at_x(&self, x: f64) -> usize {
        let mut best = 0usize;
        let mut best_d = f64::MAX;
        for it in &self.items {
            if let UiId::DockSlot(i) = it.id {
                let d = (it.rect.cx() - x).abs();
                if d < best_d {
                    best_d = d;
                    best = i as usize;
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(w: f64, h: f64, sel: bool) -> UiLayout {
        layout(
            Vec2::new(w, h),
            Insets {
                left: 0.0,
                top: 24.0,
                right: 0.0,
                bottom: 24.0,
            },
            sel,
        )
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.intersects(b)
    }

    #[test]
    fn five_slots_and_drawer_corner() {
        let l = lay(360.0, 800.0, false);
        let slots = l.items.iter().filter(|i| matches!(i.id, UiId::DockSlot(_))).count();
        assert_eq!(slots, 5);
        let d = l.rect_of(UiId::Drawer).unwrap();
        assert!(d.x_high() <= 360.0 - 12.0 + 1e-9);
        assert!(d.y_high() <= 800.0 - 24.0, "drawer must clear the nav inset");
        assert!(d.cx() > 360.0 / 2.0);
    }

    #[test]
    fn nothing_overlaps_on_narrow_and_wide_screens() {
        for (w, h) in [(320.0, 640.0), (360.0, 800.0), (412.0, 915.0), (800.0, 1280.0)] {
            for sel in [false, true] {
                let l = lay(w, h, sel);
                for (i, a) in l.items.iter().enumerate() {
                    assert!(a.rect.x >= 0.0 && a.rect.x_high() <= w, "{:?} off-screen", a.id);
                    assert!(a.rect.y >= 0.0 && a.rect.y_high() <= h, "{:?} off-screen", a.id);
                    for b in l.items.iter().skip(i + 1) {
                        assert!(!overlaps(&a.rect, &b.rect), "{:?} overlaps {:?} on {w}x{h}", a.id, b.id);
                    }
                }
            }
        }
    }

    #[test]
    fn toolbar_only_with_selection() {
        assert!(lay(360.0, 800.0, false).rect_of(UiId::ToolRemove).is_none());
        assert!(lay(360.0, 800.0, true).rect_of(UiId::ToolRemove).is_some());
    }

    #[test]
    fn hit_testing() {
        let l = lay(360.0, 800.0, false);
        let s2 = l.rect_of(UiId::DockSlot(2)).unwrap();
        assert_eq!(l.hit(s2.center()), Some(UiId::DockSlot(2)));
        assert_eq!(l.hit(Vec2::new(180.0, 300.0)), None);
        assert_eq!(l.slot_at_x(s2.cx() + 1.0), 2);
        assert_eq!(l.slot_at_x(-500.0), 0);
        assert_eq!(l.slot_at_x(5000.0), 4);
    }

    #[test]
    fn dock_respects_side_insets() {
        let l = layout(
            Vec2::new(800.0, 360.0),
            Insets {
                left: 48.0,
                top: 0.0,
                right: 48.0,
                bottom: 0.0,
            },
            false,
        );
        let first = l.rect_of(UiId::DockSlot(0)).unwrap();
        assert!(first.x >= 48.0);
        assert!(l.rect_of(UiId::Drawer).unwrap().x_high() <= 800.0 - 48.0);
    }
}
