//! JNI surface. Everything with logic lives in `Bridge` (plain Rust, unit-tested
//! on the host); the `Java_*` functions below only convert types and hold the
//! process-wide instance. The Java side owns no workspace logic.
//!
//! Java class: `dev.drift.launcher.Native` (all methods static).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JObjectArray, JString};
use jni::sys::{jboolean, jfloat, jint, jintArray, jlong, jobjectArray, jstring};

use crate::dock::{FolderAdd, ResizePlan};

use crate::drawer::AppEntry;
use crate::launcher::Launcher;
use crate::persistence::{self, LoadSource};

pub struct Bridge {
    pub launcher: Launcher,
    pub path: PathBuf,
    scratch: Vec<f32>,
    pub load_source: LoadSource,
}

impl Bridge {
    /// Load the saved workspace (falling back to the backup, then to empty).
    pub fn new(path: PathBuf) -> Self {
        let (saved, source) = persistence::load(&path);
        Bridge {
            launcher: Launcher::from_saved(saved),
            path,
            scratch: Vec::with_capacity(1024),
            load_source: source,
        }
    }

    pub fn set_apps(&mut self, keys: Vec<String>, labels: Vec<String>) {
        let apps = keys
            .into_iter()
            .zip(labels)
            .map(|(k, l)| AppEntry::new(k, l))
            .collect();
        self.launcher.set_apps(apps);
    }

    /// Render and copy into `dst`. Returns the display-list length; when it is
    /// larger than `dst.len()` nothing was copied and the caller must grow.
    pub fn render_into(&mut self, dst: &mut [f32]) -> usize {
        let n = self.render_scratch();
        if n <= dst.len() {
            dst[..n].copy_from_slice(&self.scratch);
        }
        n
    }

    /// Build the display list into the internal buffer; returns its length.
    pub fn render_scratch(&mut self) -> usize {
        self.launcher.render(&mut self.scratch);
        self.scratch.len()
    }

    pub fn scratch(&self) -> &[f32] {
        &self.scratch
    }

    /// Serialized state to write, if anything is worth writing.
    pub fn snapshot_json(&self) -> String {
        persistence::to_json(&self.launcher.to_saved())
    }

    pub fn back(&mut self) -> bool {
        if self.launcher.selected.is_some() {
            self.launcher.on_home();
            true
        } else {
            false
        }
    }
}

static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

fn with<R>(default: R, f: impl FnOnce(&mut Bridge) -> R) -> R {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = BRIDGE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_mut().map(f)
    }));
    match r {
        Ok(Some(v)) => v,
        _ => default,
    }
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

fn str_array(env: &mut JNIEnv, arr: &JObjectArray) -> Vec<String> {
    let n = env.get_array_length(arr).unwrap_or(0);
    (0..n)
        .map(|i| {
            match env.get_object_array_element(arr, i) {
                Ok(o) => {
                    let js = JString::from(o);
                    let s = jstr(env, &js);
                    // Local references are a bounded resource on ART: free each one.
                    let _ = env.delete_local_ref(js);
                    s
                }
                Err(_) => String::new(),
            }
        })
        .collect()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_init(
    mut env: JNIEnv,
    _c: JClass,
    path: JString,
) -> jint {
    let path = PathBuf::from(jstr(&mut env, &path));
    let r = catch_unwind(AssertUnwindSafe(|| Bridge::new(path)));
    match r {
        Ok(b) => {
            let code = match b.load_source {
                LoadSource::Missing => 0,
                LoadSource::Main => 1,
                LoadSource::Backup => 2,
                LoadSource::Reset => 3,
            };
            *BRIDGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(b);
            code
        }
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_setViewport(
    _env: JNIEnv,
    _c: JClass,
    w: jint,
    h: jint,
    density: jfloat,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
) {
    with((), |b| {
        b.launcher.set_viewport(
            f64::from(w),
            f64::from(h),
            f64::from(density),
            [f64::from(left), f64::from(top), f64::from(right), f64::from(bottom)],
        )
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_setApps(
    mut env: JNIEnv,
    _c: JClass,
    keys: JObjectArray,
    labels: JObjectArray,
) {
    let keys = str_array(&mut env, &keys);
    let labels = str_array(&mut env, &labels);
    with((), |b| b.set_apps(keys, labels));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_touch(
    _env: JNIEnv,
    _c: JClass,
    kind: jint,
    id: jint,
    x: jfloat,
    y: jfloat,
    t: jlong,
) {
    with((), |b| {
        b.launcher
            .touch(kind, id, f64::from(x), f64::from(y), t.max(0) as u64)
    });
}

/// Advance animations; true while another frame is needed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_tick(
    _env: JNIEnv,
    _c: JClass,
    now_ms: jlong,
) -> jboolean {
    with(false, |b| b.launcher.tick(now_ms.max(0) as u64)).into()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_resetClock(_env: JNIEnv, _c: JClass) {
    with((), |b| b.launcher.reset_clock());
}

/// Fills `dst` with the display list and returns its length (see `Bridge::render_into`).
/// One render per call: the list is built once into the bridge's scratch buffer and copied
/// straight into the Java array when it fits.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_render(
    env: JNIEnv,
    _c: JClass,
    dst: JFloatArray,
) -> jint {
    let cap = env.get_array_length(&dst).unwrap_or(0).max(0) as usize;
    let n = with(0usize, |b| {
        let len = b.render_scratch();
        if len <= cap && len > 0 {
            let _ = env.set_float_array_region(&dst, 0, b.scratch());
        }
        len
    });
    n as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_layoutVersion(
    _env: JNIEnv,
    _c: JClass,
) -> jint {
    with(0, |b| b.launcher.layout_version() as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_stringTable(
    env: JNIEnv,
    _c: JClass,
) -> jstring {
    let s = with(String::new(), |b| b.launcher.string_table());
    env.new_string(s).map_or(std::ptr::null_mut(), |j| j.into_raw())
}

/// Next pending event encoded as `kind\tfield...`, or null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_pollEvent(
    env: JNIEnv,
    _c: JClass,
) -> jstring {
    match with(None, |b| b.launcher.poll_event().map(|e| e.encode())) {
        Some(s) => env.new_string(s).map_or(std::ptr::null_mut(), |j| j.into_raw()),
        None => std::ptr::null_mut(),
    }
}

/// Keys of apps matching `query`, best match first; blank query lists all.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_searchApps(
    mut env: JNIEnv,
    _c: JClass,
    query: JString,
) -> jobjectArray {
    let q = jstr(&mut env, &query);
    let keys: Vec<String> = with(Vec::new(), |b| {
        let apps = b.launcher.apps();
        b.launcher
            .search_apps(&q)
            .into_iter()
            .map(|i| apps[i].key.clone())
            .collect()
    });
    let Ok(string_class) = env.find_class("java/lang/String") else {
        return std::ptr::null_mut();
    };
    let Ok(arr) = env.new_object_array(keys.len() as jint, string_class, JString::default()) else {
        return std::ptr::null_mut();
    };
    for (i, k) in keys.iter().enumerate() {
        if let Ok(s) = env.new_string(k) {
            let _ = env.set_object_array_element(&arr, i as jint, &s);
            let _ = env.delete_local_ref(s);
        }
    }
    arr.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_drawerPick(
    mut env: JNIEnv,
    _c: JClass,
    key: JString,
) {
    let key = jstr(&mut env, &key);
    with((), |b| b.launcher.drawer_pick(&key));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_drawerClosed(_env: JNIEnv, _c: JClass) {
    with((), |b| b.launcher.drawer_closed());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockRemove(
    _env: JNIEnv,
    _c: JClass,
    slot: jint,
) {
    with((), |b| b.launcher.dock_remove(slot.max(0) as usize));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockReplace(
    _env: JNIEnv,
    _c: JClass,
    slot: jint,
) {
    with((), |b| b.launcher.dock_replace(slot.max(0) as usize));
}

/// Load the saved workspace only when nothing is loaded yet (a screen other than the home
/// screen may be the first to run after the process was recreated). Same codes as `init`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_ensureInit(
    env: JNIEnv,
    c: JClass,
    path: JString,
) -> jint {
    let loaded = BRIDGE.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    if loaded {
        return 1;
    }
    Java_dev_drift_launcher_Native_init(env, c, path)
}

/// Turn a dock slot into a folder; returns the folder id, or -1.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockCreateFolder(
    _env: JNIEnv,
    _c: JClass,
    slot: jint,
) -> jint {
    with(-1, |b| {
        b.launcher
            .dock_create_folder(slot.max(0) as usize)
            .map_or(-1, |id| id as jint)
    })
}

/// Add an installed app to a folder through the core (which also moves it out of any
/// other dock place). 0 added, 1 already in the folder, 2 no such folder, 3 not installed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockAddApp(
    mut env: JNIEnv,
    _c: JClass,
    folder: jint,
    key: JString,
) -> jint {
    let key = jstr(&mut env, &key);
    with(2, |b| match b.launcher.dock_add_app(folder.max(0) as u32, &key) {
        FolderAdd::Added => 0,
        FolderAdd::AlreadyInFolder => 1,
        FolderAdd::NoSuchFolder => 2,
        FolderAdd::NotInstalled => 3,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockRemoveApp(
    mut env: JNIEnv,
    _c: JClass,
    folder: jint,
    key: JString,
) -> jboolean {
    let key = jstr(&mut env, &key);
    with(false, |b| b.launcher.dock_remove_app(folder.max(0) as u32, &key)).into()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockMoveApp(
    _env: JNIEnv,
    _c: JClass,
    folder: jint,
    from: jint,
    to: jint,
) -> jboolean {
    with(false, |b| {
        b.launcher
            .dock_move_app(folder.max(0) as u32, from.max(0) as usize, to.max(0) as usize)
    })
    .into()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockRenameFolder(
    mut env: JNIEnv,
    _c: JClass,
    folder: jint,
    name: JString,
) -> jboolean {
    let name = jstr(&mut env, &name);
    with(false, |b| b.launcher.dock_rename_folder(folder.max(0) as u32, &name)).into()
}

/// Apps of a folder in display order; null when the folder no longer exists.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockFolderApps(
    mut env: JNIEnv,
    _c: JClass,
    folder: jint,
) -> jobjectArray {
    let Some(keys) = with(None, |b| b.launcher.dock_folder_apps(folder.max(0) as u32)) else {
        return std::ptr::null_mut();
    };
    let Ok(string_class) = env.find_class("java/lang/String") else {
        return std::ptr::null_mut();
    };
    let Ok(arr) = env.new_object_array(keys.len() as jint, string_class, JString::default()) else {
        return std::ptr::null_mut();
    };
    for (i, k) in keys.iter().enumerate() {
        if let Ok(s) = env.new_string(k) {
            let _ = env.set_object_array_element(&arr, i as jint, &s);
            let _ = env.delete_local_ref(s);
        }
    }
    arr.into_raw()
}

/// The folder's own name (empty = default), or null when it no longer exists.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockFolderName(
    env: JNIEnv,
    _c: JClass,
    folder: jint,
) -> jstring {
    match with(None, |b| b.launcher.dock_folder_name(folder.max(0) as u32)) {
        Some(n) => env.new_string(n).map_or(std::ptr::null_mut(), |j| j.into_raw()),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockCount(_env: JNIEnv, _c: JClass) -> jint {
    with(5, |b| b.launcher.dock.count() as jint)
}

fn plan_array(env: &JNIEnv, p: ResizePlan) -> jintArray {
    let vals = [p.occupied_beyond as jint, p.moved as jint, p.lost_items as jint, p.lost_apps as jint];
    match env.new_int_array(4) {
        Ok(a) => {
            let _ = env.set_int_array_region(&a, 0, &vals);
            a.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// What changing the slot count would do: {occupied slots beyond, moved into free
/// slots, items lost, apps lost}. Changes nothing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockPlanResize(
    env: JNIEnv,
    _c: JClass,
    count: jint,
) -> jintArray {
    let p = with(ResizePlan::default(), |b| b.launcher.dock_resize_plan(count.max(0) as usize));
    plan_array(&env, p)
}

/// Apply a new slot count (the caller has confirmed any loss); returns the same plan.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_dockSetCount(
    env: JNIEnv,
    _c: JClass,
    count: jint,
) -> jintArray {
    let p = with(ResizePlan::default(), |b| b.launcher.dock_set_count(count.max(0) as usize));
    plan_array(&env, p)
}

/// Home pressed while the launcher is already showing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_onHome(_env: JNIEnv, _c: JClass) {
    with((), |b| b.launcher.on_home());
}

/// Home pressed while the launcher is already in front.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_homePressed(_env: JNIEnv, _c: JClass) {
    with((), |b| b.launcher.home_pressed());
}

/// Back pressed: true when something (a selection) was dismissed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_onBack(_env: JNIEnv, _c: JClass) -> jboolean {
    with(false, |b| b.back()).into()
}

/// Serialized workspace, taken under the lock (cheap). The caller writes it with
/// `writeSnapshot` on a background thread: fsync never runs on the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_snapshot(env: JNIEnv, _c: JClass) -> jstring {
    match with(None, |b| Some(b.snapshot_json())) {
        Some(json) => env.new_string(json).map_or(std::ptr::null_mut(), |j| j.into_raw()),
        None => std::ptr::null_mut(),
    }
}

/// Atomically write a snapshot (temp file, fsync, backup, rename). Takes the lock only to
/// read the path, so a slow disk never blocks touch handling.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_writeSnapshot(
    mut env: JNIEnv,
    _c: JClass,
    json: JString,
) -> jboolean {
    let json = jstr(&mut env, &json);
    let path = with(None, |b| Some(b.path.clone()));
    let ok = match path {
        Some(p) if !json.is_empty() => persistence::write_atomic(&p, &json).is_ok(),
        _ => false,
    };
    ok.into()
}

/// Snapshot and write in one call. Kept for callers that already run off the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_drift_launcher_Native_save(_env: JNIEnv, _c: JClass) -> jboolean {
    let job = with(None, |b| Some((b.path.clone(), b.snapshot_json())));
    let ok = match job {
        Some((path, json)) => persistence::write_atomic(&path, &json).is_ok(),
        None => false,
    };
    ok.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::LaunchSource;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("drift-bridge-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("state.json")
    }

    fn keys(n: usize) -> (Vec<String>, Vec<String>) {
        (
            (0..n).map(|i| format!("pkg{i}/.Main")).collect(),
            (0..n).map(|i| format!("App {i}")).collect(),
        )
    }

    #[test]
    fn fresh_start_is_empty_and_missing() {
        let b = Bridge::new(temp_path("fresh"));
        assert_eq!(b.load_source, LoadSource::Missing);
        assert!(b.launcher.cards.is_empty());
    }

    #[test]
    fn workspace_survives_process_restart() {
        let path = temp_path("restart");
        let mut b = Bridge::new(path.clone());
        b.launcher.set_viewport(1080.0, 2400.0, 3.0, [0.0, 72.0, 0.0, 72.0]);
        let (k, l) = keys(6);
        b.set_apps(k.clone(), l.clone());
        for key in &k {
            b.launcher.launch_app(key, LaunchSource::Drawer);
        }
        b.launcher.dock.assign(2, &k[1]);
        let json = b.snapshot_json();
        persistence::write_atomic(&path, &json).unwrap();
        let positions: Vec<_> = b.launcher.cards.iter().map(|c| (c.key.clone(), c.rect)).collect();

        let mut again = Bridge::new(path.clone());
        assert_eq!(again.load_source, LoadSource::Main);
        let restored: Vec<_> = again.launcher.cards.iter().map(|c| (c.key.clone(), c.rect)).collect();
        assert_eq!(positions, restored);
        assert_eq!(again.launcher.dock.app_at(2), Some(k[1].as_str()));
        // Restoring must not launch anything by itself.
        assert!(again.launcher.poll_event().is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupted_state_falls_back_to_backup() {
        let path = temp_path("corrupt");
        let mut b = Bridge::new(path.clone());
        let (k, l) = keys(2);
        b.set_apps(k.clone(), l);
        b.launcher.launch_app(&k[0], LaunchSource::Drawer);
        persistence::write_atomic(&path, &b.snapshot_json()).unwrap();
        b.launcher.launch_app(&k[1], LaunchSource::Drawer);
        persistence::write_atomic(&path, &b.snapshot_json()).unwrap(); // first file becomes .bak
        std::fs::write(&path, b"{ half written").unwrap();
        let again = Bridge::new(path.clone());
        assert_eq!(again.load_source, LoadSource::Backup);
        assert_eq!(again.launcher.cards.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn render_reports_required_length_when_buffer_small() {
        let mut b = Bridge::new(temp_path("render"));
        b.launcher.set_viewport(1080.0, 2400.0, 3.0, [0.0; 4]);
        let (k, l) = keys(3);
        b.set_apps(k.clone(), l);
        b.launcher.launch_app(&k[0], LaunchSource::Drawer);
        let mut tiny = [0f32; 4];
        let need = b.render_into(&mut tiny);
        assert!(need > 4);
        assert_eq!(tiny, [0.0; 4], "nothing copied when it does not fit");
        let mut big = vec![0f32; need];
        assert_eq!(b.render_into(&mut big), need);
        assert!(big[1] > 0.0, "zoom field populated");
    }

    #[test]
    fn back_only_consumes_when_selection_exists() {
        let mut b = Bridge::new(temp_path("back"));
        assert!(!b.back());
        let (k, l) = keys(1);
        b.set_apps(k.clone(), l);
        b.launcher.launch_app(&k[0], LaunchSource::Drawer);
        assert!(b.back());
        assert!(!b.back());
    }

    #[test]
    fn mismatched_label_array_does_not_panic() {
        let mut b = Bridge::new(temp_path("zip"));
        b.set_apps(vec!["a/.A".into(), "b/.B".into()], vec!["A".into()]);
        assert_eq!(b.launcher.apps().len(), 1);
    }
}
