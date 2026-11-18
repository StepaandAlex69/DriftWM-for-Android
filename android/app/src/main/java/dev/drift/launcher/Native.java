package dev.drift.launcher;

/** JNI surface of the Rust core (libdrift_core.so). All workspace logic lives there. */
final class Native {
    private Native() {}

    static {
        System.loadLibrary("drift_core");
    }

    /** Loads saved state. 0 = none yet, 1 = main file, 2 = backup, 3 = unreadable (reset), -1 = failed. */
    static native int init(String statePath);

    /** Like {@link #init} but does nothing when a workspace is already loaded (for screens other than home). */
    static native int ensureInit(String statePath);

    static native void setViewport(int w, int h, float density, int left, int top, int right, int bottom);

    static native void setApps(String[] keys, String[] labels);

    /** kind: 0 down, 1 move, 2 up, 3 cancel. Coordinates in raw pixels. */
    static native void touch(int kind, int id, float x, float y, long timeMs);

    /** Advances animations; true while another frame is needed. */
    static native boolean tick(long nowMs);

    static native void resetClock();

    /** Fills dst with the display list; returns its length (nothing copied when it exceeds dst.length). */
    static native int render(float[] dst);

    static native int layoutVersion();

    static native String stringTable();

    /** Next pending event ("kind\tfield..."), or null. */
    static native String pollEvent();

    static native String[] searchApps(String query);

    static native void drawerPick(String key);

    static native void drawerClosed();

    static native void dockRemove(int slot);

    static native void dockReplace(int slot);

    static native void onHome();

    /** Home pressed while the launcher is already in front: deselect and go to the home view (and back on the next press). */
    static native void homePressed();

    /** True when something was dismissed (a card selection). */
    static native boolean onBack();

    /** Serialized workspace (cheap). Write it with {@link #writeSnapshot} on a background thread. */
    static native String snapshot();

    /** Atomic write with fsync: never call on the UI thread. */
    static native boolean writeSnapshot(String json);

    /** Snapshot + write in one call; only for background threads. */
    static native boolean save();

    // ---- dock folders and slot count (all state lives in the core) ----

    /** Turns the slot into a folder; returns its id, or -1. */
    static native int dockCreateFolder(int slot);

    /** 0 added, 1 already in the folder, 2 no such folder, 3 not installed. */
    static native int dockAddApp(int folder, String key);

    static native boolean dockRemoveApp(int folder, String key);

    static native boolean dockMoveApp(int folder, int from, int to);

    static native boolean dockRenameFolder(int folder, String name);

    /** Apps in display order; null when the folder no longer exists. */
    static native String[] dockFolderApps(int folder);

    /** The folder's own name ("" = default); null when it no longer exists. */
    static native String dockFolderName(int folder);

    static native int dockCount();

    /** {occupied slots beyond the new count, moved into free slots, items lost, apps lost}. Changes nothing. */
    static native int[] dockPlanResize(int count);

    /** Applies the new count (confirm any loss first); returns the same plan. */
    static native int[] dockSetCount(int count);
}
