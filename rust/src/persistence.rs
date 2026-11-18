//! Durable workspace state: versioned JSON, atomic writes, corruption recovery.
//!
//! Migration: a file of an older version is copied to `<file>.v<N>` once (before anything
//! overwrites it) and then loaded in the current format.
//!
//! Write: serialize -> `<file>.tmp` -> fsync -> keep the previous good file as
//! `<file>.bak` -> rename over `<file>`. A crash leaves either the old or the
//! new complete file. Read: try the main file, then the backup; an unparseable
//! or too-new file is moved aside to `<file>.corrupt.<unix-ts>` instead of being
//! overwritten. Unknown fields round-trip so a downgrade never drops new data.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::dock::Dock;

/// Format version written by this build.
/// - v1: five-slot dock stored as a bare array of `null | "key"`.
/// - v2: dock with 3..=8 slots whose entries are apps or folders (`dock::Dock`).
///
/// Older builds reject a file whose version is newer than theirs (it is moved aside to
/// `.corrupt.<ts>`, never half-read), and this build reads every older version.
pub const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardRecord {
    pub key: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Show the captured app preview on this card (false: icon and name only).
    #[serde(default = "default_true")]
    pub preview: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraRecord {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedState {
    pub version: u32,
    #[serde(default)]
    pub saved_at: u64,
    pub camera: CameraRecord,
    /// Bottom to top: z-order is array order.
    #[serde(default)]
    pub cards: Vec<CardRecord>,
    #[serde(default)]
    pub dock: Dock,
    /// Card keys, most recently used first.
    #[serde(default)]
    pub recents: Vec<String>,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub config: Config,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SavedState {
    pub fn empty() -> Self {
        SavedState {
            version: VERSION,
            saved_at: 0,
            camera: CameraRecord {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            cards: Vec::new(),
            dock: Dock::new(),
            recents: Vec::new(),
            selected: None,
            config: Config::default(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    Missing,
    Main,
    Backup,
    /// Both files unusable; started empty.
    Reset,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn parse(bytes: &[u8]) -> Option<SavedState> {
    let state: SavedState = serde_json::from_slice(bytes).ok()?;
    if state.version == 0 || state.version > VERSION {
        return None;
    }
    let finite = |v: f64| v.is_finite();
    if !(finite(state.camera.x) && finite(state.camera.y) && finite(state.camera.zoom))
        || state.camera.zoom <= 0.0
    {
        return None;
    }
    Some(state)
}

fn quarantine(path: &Path) {
    let dest = sibling(path, &format!(".corrupt.{}", now_secs()));
    let _ = fs::rename(path, dest);
}

/// Drop records that would break invariants (non-finite or non-positive geometry,
/// duplicate keys) so one bad entry can't poison the workspace.
pub fn sanitize(state: &mut SavedState) {
    let mut seen = std::collections::HashSet::new();
    state.cards.retain(|c| {
        c.x.is_finite()
            && c.y.is_finite()
            && c.w.is_finite()
            && c.h.is_finite()
            && c.w > 0.0
            && c.h > 0.0
            && !c.key.is_empty()
            && seen.insert(c.key.clone())
    });
    state.dock = std::mem::take(&mut state.dock).normalized();
    state.camera.zoom = state.camera.zoom.clamp(0.01, 10.0);
}

/// Keep a pristine copy of a pre-migration file next to it (once per version).
fn preserve_old_version(path: &Path, bytes: &[u8], version: u32) {
    let copy = sibling(path, &format!(".v{version}"));
    if !copy.exists() {
        let _ = fs::write(copy, bytes);
    }
}

pub fn load(path: &Path) -> (SavedState, LoadSource) {
    let backup = sibling(path, ".bak");
    let mut had_file = false;

    if let Ok(bytes) = fs::read(path) {
        had_file = true;
        match parse(&bytes) {
            Some(mut s) => {
                if s.version < VERSION {
                    preserve_old_version(path, &bytes, s.version);
                    s.version = VERSION;
                }
                sanitize(&mut s);
                return (s, LoadSource::Main);
            }
            None => quarantine(path),
        }
    }
    if let Ok(bytes) = fs::read(&backup) {
        had_file = true;
        if let Some(mut s) = parse(&bytes) {
            if s.version < VERSION {
                preserve_old_version(path, &bytes, s.version);
                s.version = VERSION;
            }
            sanitize(&mut s);
            return (s, LoadSource::Backup);
        }
        quarantine(&backup);
    }
    let source = if had_file {
        LoadSource::Reset
    } else {
        LoadSource::Missing
    };
    (SavedState::empty(), source)
}

pub fn to_json(state: &SavedState) -> String {
    let mut s = state.clone();
    s.version = VERSION;
    s.saved_at = now_secs();
    serde_json::to_string(&s).unwrap_or_default()
}

/// Atomically replace `path` with `json`. Refuses to write something that would
/// not load back (empty or malformed), so a bug can't destroy a good file.
pub fn write_atomic(path: &Path, json: &str) -> std::io::Result<()> {
    if parse(json.as_bytes()).is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "refusing to persist unparseable state",
        ));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, ".tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::copy(path, sibling(path, ".bak"));
    }
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("drift-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> SavedState {
        let mut s = SavedState::empty();
        s.camera = CameraRecord {
            x: -1234.5,
            y: 9.0e8,
            zoom: 0.42,
        };
        s.cards = vec![
            CardRecord {
                key: "com.a".into(),
                label: "A".into(),
                x: 10.0,
                y: -20.0,
                w: 96.0,
                h: 112.0,
            preview: true,
        },
            CardRecord {
                key: "com.b".into(),
                label: "B".into(),
                x: 118.0,
                y: -20.0,
                w: 96.0,
                h: 112.0,
            preview: true,
        },
        ];
        s.dock.assign(0, "com.a");
        s.recents = vec!["com.b".into(), "com.a".into()];
        s.selected = Some("com.b".into());
        s
    }

    fn v1_json() -> String {
        r#"{"version":1,"saved_at":5,"camera":{"x":-40.5,"y":12.0,"zoom":0.8},
            "cards":[{"key":"com.a","label":"A","x":1.0,"y":2.0,"w":96.0,"h":112.0},
                     {"key":"com.b","label":"B","x":3.0,"y":4.0,"w":96.0,"h":112.0,"preview":false}],
            "dock":["com.a",null,"com.b",null,null],
            "recents":["com.b","com.a"],"selected":"com.a"}"#
            .to_owned()
    }

    #[test]
    fn v1_layout_migrates_without_losing_anything() {
        let d = tmpdir("v1");
        let p = d.join("state.json");
        fs::write(&p, v1_json()).unwrap();
        let (s, src) = load(&p);
        assert_eq!(src, LoadSource::Main);
        assert_eq!(s.version, VERSION);
        assert_eq!(s.camera, CameraRecord { x: -40.5, y: 12.0, zoom: 0.8 });
        assert_eq!(s.cards.len(), 2);
        assert!(!s.cards[1].preview);
        assert_eq!(s.dock.count(), 5);
        assert_eq!(s.dock.app_at(0), Some("com.a"));
        assert_eq!(s.dock.app_at(2), Some("com.b"));
        assert_eq!(s.recents, vec!["com.b".to_string(), "com.a".to_string()]);
        assert_eq!(s.selected.as_deref(), Some("com.a"));
        // The untouched original is kept next to it.
        let kept = fs::read_to_string(sibling(&p, ".v1")).unwrap();
        assert_eq!(kept, v1_json());
        // Saving writes v2, and the kept copy is not overwritten later.
        write_atomic(&p, &to_json(&s)).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        assert_eq!(again["version"], 2);
        assert_eq!(fs::read_to_string(sibling(&p, ".v1")).unwrap(), v1_json());
        let (back, _) = load(&p);
        assert_eq!(back.dock, s.dock);
    }

    #[test]
    fn v2_with_folders_round_trips_and_is_not_readable_as_v1() {
        let d = tmpdir("v2");
        let p = d.join("state.json");
        let mut s = sample();
        s.dock = Dock::with_slots(7);
        s.dock.assign(0, "com.a");
        let f = s.dock.create_folder(3).unwrap();
        s.dock.folder_add(f, "com.b");
        s.dock.folder_add(f, "com.c");
        s.dock.folder_rename(f, "Work");
        let empty = s.dock.create_folder(6).unwrap();
        write_atomic(&p, &to_json(&s)).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        assert_eq!(raw["version"], 2, "a v1-only reader must see a newer version and refuse it");
        let (back, _) = load(&p);
        assert_eq!(back.dock, s.dock);
        assert_eq!(back.dock.count(), 7);
        assert_eq!(back.dock.folder(f).unwrap().apps, ["com.b", "com.c"]);
        assert_eq!(back.dock.folder(f).unwrap().name, "Work");
        assert!(back.dock.folder(empty).unwrap().apps.is_empty());
        assert!(!sibling(&p, ".v1").exists(), "no migration copy for a native v2 file");
    }

    #[test]
    fn camera_and_card_order_survive() {
        let d = tmpdir("order");
        let p = d.join("state.json");
        let mut s = sample();
        s.camera = CameraRecord { x: 1e6 + 0.125, y: -2e6, zoom: 0.37 };
        s.cards.reverse();
        write_atomic(&p, &to_json(&s)).unwrap();
        let (back, _) = load(&p);
        assert_eq!(back.camera, s.camera);
        let keys: Vec<_> = back.cards.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["com.b", "com.a"]);
    }

    #[test]
    fn v1_backup_is_migrated_too() {
        let d = tmpdir("v1bak");
        let p = d.join("state.json");
        fs::write(&p, b"{ torn").unwrap();
        fs::write(sibling(&p, ".bak"), v1_json()).unwrap();
        let (s, src) = load(&p);
        assert_eq!(src, LoadSource::Backup);
        assert_eq!(s.version, VERSION);
        assert_eq!(s.dock.app_at(2), Some("com.b"));
    }

    #[test]
    fn round_trip() {
        let d = tmpdir("rt");
        let p = d.join("state.json");
        let s = sample();
        write_atomic(&p, &to_json(&s)).unwrap();
        let (back, src) = load(&p);
        assert_eq!(src, LoadSource::Main);
        assert_eq!(back.cards, s.cards);
        assert_eq!(back.camera, s.camera);
        assert_eq!(back.dock, s.dock);
        assert_eq!(back.recents, s.recents);
        assert_eq!(back.selected, s.selected);
    }

    #[test]
    fn missing_file_is_fresh_start() {
        let d = tmpdir("missing");
        let (s, src) = load(&d.join("nope.json"));
        assert_eq!(src, LoadSource::Missing);
        assert!(s.cards.is_empty());
    }

    #[test]
    fn truncated_file_falls_back_to_backup() {
        let d = tmpdir("trunc");
        let p = d.join("state.json");
        let s1 = sample();
        write_atomic(&p, &to_json(&s1)).unwrap();
        let mut s2 = s1.clone();
        s2.cards.pop();
        write_atomic(&p, &to_json(&s2)).unwrap();
        // Simulate a torn write of the main file.
        let full = fs::read(&p).unwrap();
        fs::write(&p, &full[..full.len() / 2]).unwrap();
        let (back, src) = load(&p);
        assert_eq!(src, LoadSource::Backup);
        assert_eq!(back.cards.len(), 2);
        let corrupt = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".corrupt."));
        assert!(corrupt, "bad file must be kept for recovery");
    }

    #[test]
    fn both_corrupt_resets_but_keeps_files() {
        let d = tmpdir("both");
        let p = d.join("state.json");
        fs::write(&p, b"{not json").unwrap();
        fs::write(sibling(&p, ".bak"), b"").unwrap();
        let (s, src) = load(&p);
        assert_eq!(src, LoadSource::Reset);
        assert!(s.cards.is_empty());
        assert!(!p.exists());
    }

    #[test]
    fn newer_version_is_quarantined_not_misparsed() {
        let d = tmpdir("newer");
        let p = d.join("state.json");
        let mut v = serde_json::to_value(sample()).unwrap();
        v["version"] = serde_json::json!(VERSION + 1);
        fs::write(&p, v.to_string()).unwrap();
        let (_, src) = load(&p);
        assert_eq!(src, LoadSource::Reset);
    }

    #[test]
    fn unknown_fields_survive() {
        let d = tmpdir("extra");
        let p = d.join("state.json");
        let mut v = serde_json::to_value(sample()).unwrap();
        v["future_feature"] = serde_json::json!({"a": [1, 2, 3]});
        fs::write(&p, v.to_string()).unwrap();
        let (s, _) = load(&p);
        assert!(s.extra.contains_key("future_feature"));
        write_atomic(&p, &to_json(&s)).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        assert_eq!(again["future_feature"]["a"][2], 3);
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let d = tmpdir("minimal");
        let p = d.join("state.json");
        fs::write(&p, r#"{"version":1,"camera":{"x":1.0,"y":2.0,"zoom":1.0}}"#).unwrap();
        let (s, src) = load(&p);
        assert_eq!(src, LoadSource::Main);
        assert!(s.cards.is_empty());
        assert_eq!(s.dock.count(), 5);
    }

    #[test]
    fn sanitize_drops_bad_cards_and_duplicates() {
        let mut s = sample();
        s.cards.push(CardRecord {
            key: "com.a".into(),
            label: "dup".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            preview: true,
        });
        s.cards.push(CardRecord {
            key: "com.nan".into(),
            label: "n".into(),
            x: f64::NAN,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            preview: true,
        });
        s.cards.push(CardRecord {
            key: "com.zero".into(),
            label: "z".into(),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 10.0,
            preview: true,
        });
        sanitize(&mut s);
        assert_eq!(s.cards.len(), 2);
    }

    #[test]
    fn refuses_to_write_garbage() {
        let d = tmpdir("garbage");
        let p = d.join("state.json");
        write_atomic(&p, &to_json(&sample())).unwrap();
        assert!(write_atomic(&p, "").is_err());
        assert!(write_atomic(&p, "{}").is_err());
        let (s, src) = load(&p);
        assert_eq!(src, LoadSource::Main);
        assert_eq!(s.cards.len(), 2);
    }

    #[test]
    fn huge_coordinates_round_trip_exactly() {
        let d = tmpdir("huge");
        let p = d.join("state.json");
        let mut s = sample();
        s.cards[0].x = 123456789012.25;
        s.cards[0].y = -98765432109.5;
        write_atomic(&p, &to_json(&s)).unwrap();
        let (back, _) = load(&p);
        assert_eq!(back.cards[0].x, 123456789012.25);
        assert_eq!(back.cards[0].y, -98765432109.5);
    }
}
