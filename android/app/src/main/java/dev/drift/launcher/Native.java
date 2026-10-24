package dev.drift.launcher;

/** JNI surface of the Rust core (libdrift_core.so). All workspace logic lives there. */
final class Native {
    private Native() {}

    static {
        System.loadLibrary("drift_core");
    }

    /** Loads saved state. 0 = none yet, 1 = main file, 2 = backup, 3 = unreadable (reset), -1 = failed. */
    static native int init(String statePath);

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

    /** True when something was dismissed (a card selection). */
    static native boolean onBack();

    static native boolean save();
}
