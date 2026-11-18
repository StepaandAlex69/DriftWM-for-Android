//! The quick-access dock: 3 to 8 screen-fixed slots, each empty, an app, or a
//! folder of apps. Pure slot model; geometry lives in `ui`, gestures in `gestures`.
//!
//! An app key appears at most once in the whole dock (in a slot or inside one
//! folder), so "move" and "add" never duplicate. A folder is never deleted
//! implicitly: removing its last app leaves an empty folder in its slot.
//!
//! Persisted form (state v2): `{"slots":[null | {"type":"app","key":..} |
//! {"type":"folder","id":..,"name":..,"apps":[..]}], "next_folder_id":N}`.
//! The v1 form, a bare array of `null | "key"`, is still read.

use serde::{Deserialize, Serialize};

pub const MIN_SLOTS: usize = 3;
pub const MAX_SLOTS: usize = 8;
pub const DEFAULT_SLOTS: usize = 5;
/// Longest folder name kept (characters). Empty means "use the default name".
pub const FOLDER_NAME_MAX: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: u32,
    /// User-chosen name; empty means the localized default is shown.
    pub name: String,
    /// Explicit display order; never derived from insertion history.
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockItem {
    App(String),
    Folder(Folder),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dock {
    slots: Vec<Option<DockItem>>,
    next_folder_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderAdd {
    Added,
    AlreadyInFolder,
    NoSuchFolder,
    /// Decided by the launcher (the dock does not know what is installed).
    NotInstalled,
}

/// What shrinking the dock would do. `moved` items fit into free slots that
/// stay; `lost_items` slots (and the `lost_apps` apps in them, folder contents
/// included) would leave the dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResizePlan {
    pub occupied_beyond: usize,
    pub moved: usize,
    pub lost_items: usize,
    pub lost_apps: usize,
}

// ---- persisted representation ------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ItemRepr {
    App { key: String },
    Folder {
        id: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        apps: Vec<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct DockV2 {
    slots: Vec<Option<ItemRepr>>,
    #[serde(default)]
    next_folder_id: u32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DockRepr {
    V2(DockV2),
    /// State v1: five slots, each `null` or an app key.
    Legacy(Vec<Option<String>>),
}

impl From<DockRepr> for Dock {
    fn from(r: DockRepr) -> Dock {
        let (slots, next): (Vec<Option<DockItem>>, u32) = match r {
            DockRepr::Legacy(v) => (v.into_iter().map(|s| s.map(DockItem::App)).collect(), 1),
            DockRepr::V2(v) => (
                v.slots
                    .into_iter()
                    .map(|s| {
                        s.map(|i| match i {
                            ItemRepr::App { key } => DockItem::App(key),
                            ItemRepr::Folder { id, name, apps } => DockItem::Folder(Folder { id, name, apps }),
                        })
                    })
                    .collect(),
                v.next_folder_id,
            ),
        };
        Dock {
            slots,
            next_folder_id: next,
        }
        .normalized()
    }
}

impl From<Dock> for DockV2 {
    fn from(d: Dock) -> DockV2 {
        DockV2 {
            slots: d
                .slots
                .into_iter()
                .map(|s| {
                    s.map(|i| match i {
                        DockItem::App(key) => ItemRepr::App { key },
                        DockItem::Folder(f) => ItemRepr::Folder {
                            id: f.id,
                            name: f.name,
                            apps: f.apps,
                        },
                    })
                })
                .collect(),
            next_folder_id: d.next_folder_id,
        }
    }
}

impl Serialize for Dock {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        DockV2::from(self.clone()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Dock {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Dock, D::Error> {
        Ok(Dock::from(DockRepr::deserialize(d)?))
    }
}

impl Default for Dock {
    fn default() -> Self {
        Dock::new()
    }
}

fn clean_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(FOLDER_NAME_MAX).collect()
}

impl Dock {
    pub fn new() -> Self {
        Dock::with_slots(DEFAULT_SLOTS)
    }

    pub fn with_slots(n: usize) -> Self {
        Dock {
            slots: vec![None; n.clamp(MIN_SLOTS, MAX_SLOTS)],
            next_folder_id: 1,
        }
    }

    /// Repair data of any shape: slot count into 3..=8 (extra entries beyond 8
    /// are dropped, missing ones empty), every app key only once, unique folder ids,
    /// clean folder names.
    pub fn normalized(mut self) -> Self {
        self.slots.truncate(MAX_SLOTS);
        if self.slots.len() < MIN_SLOTS {
            self.slots.resize(DEFAULT_SLOTS.max(self.slots.len()), None);
        }
        let mut seen: Vec<String> = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        let mut max_id = 0u32;
        for slot in &mut self.slots {
            match slot {
                Some(DockItem::App(k)) => {
                    if k.is_empty() || seen.contains(k) {
                        *slot = None;
                    } else {
                        seen.push(k.clone());
                    }
                }
                Some(DockItem::Folder(f)) => {
                    f.name = clean_name(&f.name);
                    f.apps.retain(|k| {
                        if k.is_empty() || seen.contains(k) {
                            false
                        } else {
                            seen.push(k.clone());
                            true
                        }
                    });
                    if f.id == 0 || ids.contains(&f.id) {
                        f.id = 0; // reassigned below, once the largest id is known
                    } else {
                        ids.push(f.id);
                        max_id = max_id.max(f.id);
                    }
                }
                None => {}
            }
        }
        let mut next = self.next_folder_id.max(max_id.saturating_add(1)).max(1);
        for slot in &mut self.slots {
            if let Some(DockItem::Folder(f)) = slot
                && f.id == 0
            {
                f.id = next;
                next += 1;
            }
        }
        self.next_folder_id = next;
        self
    }

    pub fn count(&self) -> usize {
        self.slots.len()
    }

    pub fn slots(&self) -> &[Option<DockItem>] {
        &self.slots
    }

    pub fn get(&self, i: usize) -> Option<&DockItem> {
        self.slots.get(i).and_then(|s| s.as_ref())
    }

    /// The app key in slot `i`, when the slot holds an app (not a folder).
    pub fn app_at(&self, i: usize) -> Option<&str> {
        match self.get(i) {
            Some(DockItem::App(k)) => Some(k),
            _ => None,
        }
    }

    pub fn is_empty_slot(&self, i: usize) -> bool {
        i < self.slots.len() && self.slots[i].is_none()
    }

    pub fn folder(&self, id: u32) -> Option<&Folder> {
        self.slots.iter().find_map(|s| match s {
            Some(DockItem::Folder(f)) if f.id == id => Some(f),
            _ => None,
        })
    }

    fn folder_mut(&mut self, id: u32) -> Option<&mut Folder> {
        self.slots.iter_mut().find_map(|s| match s {
            Some(DockItem::Folder(f)) if f.id == id => Some(f),
            _ => None,
        })
    }

    pub fn folder_slot(&self, id: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Some(DockItem::Folder(f)) if f.id == id))
    }

    /// Slot holding `key` as a plain app.
    pub fn position_of(&self, key: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Some(DockItem::App(k)) if k == key))
    }

    /// True when `key` is anywhere in the dock: in a slot or inside a folder.
    pub fn contains(&self, key: &str) -> bool {
        self.slots.iter().any(|s| match s {
            Some(DockItem::App(k)) => k == key,
            Some(DockItem::Folder(f)) => f.apps.iter().any(|a| a == key),
            None => false,
        })
    }

    pub fn first_empty(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    /// Take `key` out of wherever it is in the dock (slot or folder). True if found.
    fn detach(&mut self, key: &str) -> bool {
        for s in &mut self.slots {
            match s {
                Some(DockItem::App(k)) if k == key => {
                    *s = None;
                    return true;
                }
                Some(DockItem::Folder(f)) => {
                    if let Some(p) = f.apps.iter().position(|a| a == key) {
                        f.apps.remove(p);
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Put app `key` into `slot`, replacing an app there. Refuses folders (a
    /// folder is never overwritten) and out-of-range slots. If `key` is pinned
    /// elsewhere it moves; when it was a plain slot app it swaps with the target.
    pub fn assign(&mut self, slot: usize, key: &str) -> bool {
        if slot >= self.slots.len() || matches!(self.slots[slot], Some(DockItem::Folder(_))) {
            return false;
        }
        if let Some(from) = self.position_of(key) {
            self.slots.swap(from, slot);
        } else {
            self.detach(key); // out of a folder, if it was in one
            self.slots[slot] = Some(DockItem::App(key.to_owned()));
        }
        true
    }

    /// Empty a slot (app or folder). The caller confirms with the user first when
    /// a folder still holds apps.
    pub fn remove(&mut self, slot: usize) -> bool {
        match self.slots.get_mut(slot) {
            Some(s) if s.is_some() => {
                *s = None;
                true
            }
            _ => false,
        }
    }

    /// Drag-reorder: move the entry at `from` to `to`, shifting the ones between.
    pub fn move_slot(&mut self, from: usize, to: usize) -> bool {
        let n = self.slots.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let item = self.slots.remove(from);
        self.slots.insert(to, item);
        true
    }

    // ---- folders ----------------------------------------------------------------

    /// New folder in `slot`. An app in the slot becomes the folder's first app; an
    /// empty slot gets an empty folder. A folder slot is left alone (None).
    pub fn create_folder(&mut self, slot: usize) -> Option<u32> {
        let cur = self.slots.get(slot)?.clone();
        let apps = match cur {
            Some(DockItem::Folder(_)) => return None,
            Some(DockItem::App(k)) => vec![k],
            None => Vec::new(),
        };
        let id = self.next_folder_id;
        self.next_folder_id += 1;
        self.slots[slot] = Some(DockItem::Folder(Folder {
            id,
            name: String::new(),
            apps,
        }));
        Some(id)
    }

    /// Append `key` to the folder (moving it out of its old place in the dock).
    pub fn folder_add(&mut self, id: u32, key: &str) -> FolderAdd {
        let Some(f) = self.folder(id) else {
            return FolderAdd::NoSuchFolder;
        };
        if f.apps.iter().any(|a| a == key) {
            return FolderAdd::AlreadyInFolder;
        }
        self.detach(key);
        if let Some(f) = self.folder_mut(id) {
            f.apps.push(key.to_owned());
        }
        FolderAdd::Added
    }

    /// Take an app out of the folder; the folder stays, even when it becomes empty.
    pub fn folder_remove(&mut self, id: u32, key: &str) -> bool {
        let Some(f) = self.folder_mut(id) else { return false };
        match f.apps.iter().position(|a| a == key) {
            Some(p) => {
                f.apps.remove(p);
                true
            }
            None => false,
        }
    }

    /// Reorder inside a folder: the app at `from` ends up at index `to`.
    pub fn folder_move(&mut self, id: u32, from: usize, to: usize) -> bool {
        let Some(f) = self.folder_mut(id) else { return false };
        let n = f.apps.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let k = f.apps.remove(from);
        f.apps.insert(to, k);
        true
    }

    /// Blank resets to the default name. Returns false for an unknown folder.
    pub fn folder_rename(&mut self, id: u32, name: &str) -> bool {
        let clean = clean_name(name);
        match self.folder_mut(id) {
            Some(f) => {
                f.name = clean;
                true
            }
            None => false,
        }
    }

    // ---- slot count ---------------------------------------------------------------

    pub fn resize_plan(&self, new_count: usize) -> ResizePlan {
        let n = new_count.clamp(MIN_SLOTS, MAX_SLOTS);
        let mut plan = ResizePlan::default();
        if n >= self.slots.len() {
            return plan;
        }
        let free = self.slots[..n].iter().filter(|s| s.is_none()).count();
        let mut free_left = free;
        for s in &self.slots[n..] {
            if let Some(item) = s {
                plan.occupied_beyond += 1;
                if free_left > 0 {
                    free_left -= 1;
                    plan.moved += 1;
                } else {
                    plan.lost_items += 1;
                    plan.lost_apps += match item {
                        DockItem::App(_) => 1,
                        DockItem::Folder(f) => f.apps.len(),
                    };
                }
            }
        }
        plan
    }

    /// Change the slot count. Growing adds empty slots at the end. Shrinking first
    /// moves items from the removed slots into free slots that remain (keeping
    /// their order); only what cannot fit leaves the dock. The caller shows
    /// `resize_plan` and asks for confirmation before shrinking an occupied dock.
    pub fn set_count(&mut self, new_count: usize) -> ResizePlan {
        let n = new_count.clamp(MIN_SLOTS, MAX_SLOTS);
        let plan = self.resize_plan(n);
        if n >= self.slots.len() {
            self.slots.resize(n, None);
            return plan;
        }
        let tail: Vec<DockItem> = self.slots.split_off(n).into_iter().flatten().collect();
        let mut tail = tail.into_iter();
        for s in &mut self.slots {
            if s.is_none() {
                match tail.next() {
                    Some(item) => *s = Some(item),
                    None => break,
                }
            }
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(d: &Dock, i: usize) -> Option<&str> {
        d.app_at(i)
    }

    #[test]
    fn default_has_five_and_bounds_are_enforced() {
        assert_eq!(Dock::new().count(), 5);
        assert_eq!(Dock::with_slots(1).count(), MIN_SLOTS);
        assert_eq!(Dock::with_slots(99).count(), MAX_SLOTS);
        assert_eq!(Dock::default().normalized().count(), 5);
    }

    #[test]
    fn assign_and_replace() {
        let mut d = Dock::new();
        assert!(d.assign(2, "a"));
        assert!(d.assign(2, "b"));
        assert_eq!(app(&d, 2), Some("b"));
        assert_eq!(d.position_of("a"), None);
        assert!(!d.assign(5, "x"));
    }

    #[test]
    fn assigning_pinned_app_moves_it() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(3, "b");
        d.assign(3, "a");
        assert_eq!(app(&d, 3), Some("a"));
        assert_eq!(app(&d, 0), Some("b"));
    }

    #[test]
    fn remove_and_first_empty() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(1, "b");
        assert_eq!(d.first_empty(), Some(2));
        assert!(d.remove(0));
        assert!(!d.remove(0));
        assert_eq!(d.first_empty(), Some(0));
    }

    #[test]
    fn reorder() {
        let mut d = Dock::new();
        for (i, k) in ["a", "b", "c"].iter().enumerate() {
            d.assign(i, k);
        }
        assert!(d.move_slot(0, 2));
        assert_eq!(app(&d, 0), Some("b"));
        assert_eq!(app(&d, 1), Some("c"));
        assert_eq!(app(&d, 2), Some("a"));
        assert!(!d.move_slot(1, 1));
        assert!(!d.move_slot(0, 9));
    }

    #[test]
    fn normalize_fixes_bad_data() {
        let d = Dock {
            slots: vec![
                Some(DockItem::App("a".into())),
                Some(DockItem::App("a".into())),
                None,
                None,
                None,
                Some(DockItem::App("z".into())),
            ],
            next_folder_id: 0,
        }
        .normalized();
        assert_eq!(d.count(), 6, "a valid length is kept");
        assert_eq!(d.get(1), None, "duplicate dropped");
        let short = Dock {
            slots: vec![Some(DockItem::App("a".into()))],
            next_folder_id: 1,
        }
        .normalized();
        assert_eq!(short.count(), 5);
        let long = Dock {
            slots: vec![None; 20],
            next_folder_id: 1,
        }
        .normalized();
        assert_eq!(long.count(), MAX_SLOTS);
    }

    // ---- folders ---------------------------------------------------------------

    #[test]
    fn create_folder_from_app_keeps_it_first() {
        let mut d = Dock::new();
        d.assign(1, "a");
        let id = d.create_folder(1).unwrap();
        let f = d.folder(id).unwrap();
        assert_eq!(f.apps, vec!["a".to_string()]);
        assert_eq!(d.folder_slot(id), Some(1));
        assert!(d.create_folder(1).is_none(), "a folder is not wrapped in another folder");
        assert!(d.create_folder(99).is_none());
    }

    #[test]
    fn create_folder_in_empty_slot_is_empty_but_stays() {
        let mut d = Dock::new();
        let id = d.create_folder(2).unwrap();
        assert!(d.folder(id).unwrap().apps.is_empty());
        assert!(d.get(2).is_some(), "an empty folder still occupies its slot");
        assert_eq!(d.first_empty(), Some(0));
    }

    #[test]
    fn folder_add_moves_app_out_of_its_old_place() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(1, "b");
        let id = d.create_folder(2).unwrap();
        assert_eq!(d.folder_add(id, "a"), FolderAdd::Added);
        assert_eq!(d.get(0), None, "no duplicate left in the slot");
        assert_eq!(d.folder_add(id, "a"), FolderAdd::AlreadyInFolder);
        assert_eq!(d.folder_add(99, "b"), FolderAdd::NoSuchFolder);
        assert!(d.contains("a") && d.contains("b"));
        assert_eq!(d.folder(id).unwrap().apps, vec!["a".to_string()]);
    }

    #[test]
    fn folder_add_from_another_folder_moves_it() {
        let mut d = Dock::new();
        let f1 = d.create_folder(0).unwrap();
        let f2 = d.create_folder(1).unwrap();
        d.folder_add(f1, "a");
        d.folder_add(f2, "a");
        assert!(d.folder(f1).unwrap().apps.is_empty());
        assert_eq!(d.folder(f2).unwrap().apps, vec!["a".to_string()]);
    }

    #[test]
    fn removing_last_app_keeps_the_folder() {
        let mut d = Dock::new();
        d.assign(0, "a");
        let id = d.create_folder(0).unwrap();
        assert!(d.folder_remove(id, "a"));
        assert!(!d.folder_remove(id, "a"));
        assert!(d.folder(id).is_some());
        assert!(matches!(d.get(0), Some(DockItem::Folder(_))));
    }

    #[test]
    fn folder_order_is_explicit() {
        let mut d = Dock::new();
        let id = d.create_folder(0).unwrap();
        for k in ["a", "b", "c", "d"] {
            d.folder_add(id, k);
        }
        assert!(d.folder_move(id, 0, 2));
        assert_eq!(d.folder(id).unwrap().apps, ["b", "c", "a", "d"]);
        assert!(d.folder_move(id, 3, 0));
        assert_eq!(d.folder(id).unwrap().apps, ["d", "b", "c", "a"]);
        assert!(!d.folder_move(id, 1, 1));
        assert!(!d.folder_move(id, 0, 9));
        assert!(!d.folder_move(77, 0, 1));
    }

    #[test]
    fn rename_cleans_and_blank_resets() {
        let mut d = Dock::new();
        let id = d.create_folder(0).unwrap();
        assert!(d.folder_rename(id, "  Games\n "));
        assert_eq!(d.folder(id).unwrap().name, "Games");
        assert!(d.folder_rename(id, &"x".repeat(100)));
        assert_eq!(d.folder(id).unwrap().name.chars().count(), FOLDER_NAME_MAX);
        assert!(d.folder_rename(id, "   "));
        assert_eq!(d.folder(id).unwrap().name, "");
        assert!(!d.folder_rename(42, "n"));
    }

    #[test]
    fn assign_never_overwrites_a_folder() {
        let mut d = Dock::new();
        let id = d.create_folder(1).unwrap();
        d.folder_add(id, "a");
        assert!(!d.assign(1, "b"));
        assert_eq!(d.folder(id).unwrap().apps.len(), 1);
    }

    #[test]
    fn assign_pulls_an_app_out_of_a_folder() {
        let mut d = Dock::new();
        let id = d.create_folder(0).unwrap();
        d.folder_add(id, "a");
        assert!(d.assign(3, "a"));
        assert_eq!(app(&d, 3), Some("a"));
        assert!(d.folder(id).unwrap().apps.is_empty());
    }

    #[test]
    fn folder_slots_reorder_like_apps() {
        let mut d = Dock::new();
        d.assign(0, "a");
        let id = d.create_folder(1).unwrap();
        assert!(d.move_slot(1, 0));
        assert_eq!(d.folder_slot(id), Some(0));
        assert_eq!(app(&d, 1), Some("a"));
    }

    // ---- slot count -------------------------------------------------------------

    #[test]
    fn grow_adds_empty_slots_without_touching_content() {
        let mut d = Dock::new();
        d.assign(4, "a");
        let plan = d.set_count(8);
        assert_eq!(plan, ResizePlan::default());
        assert_eq!(d.count(), 8);
        assert_eq!(app(&d, 4), Some("a"));
    }

    #[test]
    fn shrinking_an_empty_tail_needs_no_confirmation() {
        let mut d = Dock::new();
        d.assign(0, "a");
        let plan = d.resize_plan(3);
        assert_eq!(plan.occupied_beyond, 0);
        d.set_count(3);
        assert_eq!(d.count(), 3);
        assert_eq!(app(&d, 0), Some("a"));
    }

    #[test]
    fn shrinking_moves_items_into_free_slots_first() {
        let mut d = Dock::new();
        d.assign(1, "b");
        d.assign(3, "d");
        d.assign(4, "e");
        // keep 3: slots 0 and 2 are free, so d and e both fit.
        let plan = d.resize_plan(3);
        assert_eq!(plan, ResizePlan { occupied_beyond: 2, moved: 2, lost_items: 0, lost_apps: 0 });
        d.set_count(3);
        assert_eq!(app(&d, 0), Some("d"));
        assert_eq!(app(&d, 1), Some("b"));
        assert_eq!(app(&d, 2), Some("e"));
    }

    #[test]
    fn shrinking_reports_what_will_be_lost_including_folder_contents() {
        let mut d = Dock::with_slots(6);
        for (i, k) in ["a", "b", "c"].iter().enumerate() {
            d.assign(i, k);
        }
        let id = d.create_folder(4).unwrap();
        d.folder_add(id, "x");
        d.folder_add(id, "y");
        d.assign(5, "z");
        let plan = d.resize_plan(3);
        assert_eq!(plan.occupied_beyond, 2);
        assert_eq!(plan.lost_items, 2);
        assert_eq!(plan.lost_apps, 3, "two apps in the folder plus z");
        let applied = d.set_count(3);
        assert_eq!(applied, plan);
        assert_eq!(d.count(), 3);
        assert!(!d.contains("x") && !d.contains("z"));
        assert!(d.contains("a") && d.contains("c"));
    }

    #[test]
    fn clamps_requested_count() {
        let mut d = Dock::new();
        d.set_count(1);
        assert_eq!(d.count(), MIN_SLOTS);
        d.set_count(100);
        assert_eq!(d.count(), MAX_SLOTS);
    }

    // ---- serialization ----------------------------------------------------------

    #[test]
    fn v2_round_trip_keeps_everything() {
        let mut d = Dock::with_slots(7);
        d.assign(0, "a");
        let id = d.create_folder(2).unwrap();
        d.folder_add(id, "x");
        d.folder_add(id, "y");
        d.folder_rename(id, "Games");
        let empty = d.create_folder(5).unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let back: Dock = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.count(), 7);
        assert_eq!(back.folder(id).unwrap().apps, ["x", "y"]);
        assert!(back.folder(empty).unwrap().apps.is_empty());
    }

    #[test]
    fn v1_array_is_read_as_apps() {
        let back: Dock = serde_json::from_str(r#"["a", null, "b", null, null]"#).unwrap();
        assert_eq!(back.count(), 5);
        assert_eq!(app(&back, 0), Some("a"));
        assert_eq!(app(&back, 2), Some("b"));
        let mut back = back;
        assert!(back.create_folder(4).is_some(), "ids are usable after a v1 read");
    }

    #[test]
    fn folder_ids_stay_unique_after_load() {
        let json = r#"{"slots":[
            {"type":"folder","id":3,"name":"A","apps":["x"]},
            {"type":"folder","id":3,"name":"B","apps":["y"]},
            {"type":"folder","id":0,"name":"C","apps":[]},
            null],"next_folder_id":1}"#;
        let mut d: Dock = serde_json::from_str(json).unwrap();
        let mut ids = Vec::new();
        for s in d.slots() {
            if let Some(DockItem::Folder(f)) = s {
                ids.push(f.id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        let new = d.create_folder(3).unwrap();
        assert!(!ids.contains(&new));
    }

    #[test]
    fn duplicate_keys_across_folders_are_dropped_on_load() {
        let json = r#"{"slots":[
            {"type":"app","key":"a"},
            {"type":"folder","id":1,"name":"","apps":["a","b"]},
            null,null,null],"next_folder_id":2}"#;
        let d: Dock = serde_json::from_str(json).unwrap();
        assert_eq!(d.folder(1).unwrap().apps, ["b"]);
    }

    #[test]
    fn unknown_item_types_fail_the_whole_parse_instead_of_losing_data_silently() {
        let json = r#"{"slots":[{"type":"widget"}]}"#;
        assert!(serde_json::from_str::<Dock>(json).is_err());
    }
}
