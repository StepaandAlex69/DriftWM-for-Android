package dev.drift.launcher;

import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.util.LruCache;

import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;

/**
 * Icons for the canvas, the dock, folders and the app list. Main-thread API; callers draw a
 * placeholder until {@link #get} returns a bitmap.
 *
 * Loading order for a key: memory, then the disk cache (no PackageManager involved, so a cold
 * start does not wait for the app list), then a fresh render through {@link AppRepo}. A
 * render is checked for obvious garbage (empty or solid black) and only stored when it is
 * not. A key whose load did not work is retried after the next app list update and every
 * time the launcher comes back to the front, never cached as a permanent failure.
 */
final class IconCache {
    interface Listener {
        void onIconsLoaded();
    }

    private static final long NOTIFY_BATCH_MS = 24;

    private final AppRepo repo;
    private final IconStore store;
    private final int sizePx;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Workers.single("drift-icons", Process.THREAD_PRIORITY_BACKGROUND);
    private final Set<String> pending = new HashSet<>();
    /** Loads that failed although the app is known: retried on resume / next list update. */
    private final Set<String> failed = new HashSet<>();
    /** Keys asked for before the app list arrived: asked again once it is in, not on every frame. */
    private final Set<String> awaiting = new HashSet<>();
    private boolean notifyQueued;
    private Listener listener;
    private final Runnable notifyTask = () -> {
        notifyQueued = false;
        if (listener != null) listener.onIconsLoaded();
    };

    private final LruCache<String, Bitmap> cache = new LruCache<>(24 * 1024 * 1024) {
        @Override protected int sizeOf(String key, Bitmap value) {
            return value.getByteCount();
        }
    };

    IconCache(android.content.Context ctx, AppRepo repo, float density) {
        this.repo = repo;
        this.sizePx = Math.max(48, Math.round(64f * density));
        this.store = new IconStore(ctx, sizePx);
    }

    IconStore store() {
        return store;
    }

    void setListener(Listener l) {
        listener = l;
    }

    /** Main thread only. Returns null while the icon is loading. */
    Bitmap get(String key) {
        if (key == null) return null;
        Bitmap b = cache.get(key);
        if (b == null && !failed.contains(key) && !awaiting.contains(key) && pending.add(key)) {
            worker.execute(() -> load(key));
        }
        return b;
    }

    private void load(String key) {
        long t0 = SystemClock.elapsedRealtimeNanos();
        Bitmap bmp = store.load(key); // disk first: needs no PackageManager
        boolean known = true;
        if (bmp == null) {
            known = repo.get(key) != null;
            if (known) {
                Bitmap fresh = repo.renderIcon(key, sizePx);
                if (fresh != null && !looksBroken(fresh)) {
                    bmp = fresh;
                    store.save(key, fresh);
                }
            }
        }
        PerfStats.ICON_LOAD.add(SystemClock.elapsedRealtimeNanos() - t0, 0);
        final Bitmap result = bmp;
        final boolean appKnown = known;
        main.post(() -> {
            pending.remove(key);
            if (result != null) {
                cache.put(key, result);
            } else if (appKnown) {
                failed.add(key); // retried on resume or on the next app list
            } else {
                awaiting.add(key); // the app list is not in yet: asked again when it arrives
            }
            queueNotify();
        });
    }

    /** True for results that cannot be a real icon: nothing drawn, or an opaque black blob. */
    static boolean looksBroken(Bitmap b) {
        int w = b.getWidth();
        int h = b.getHeight();
        if (w < 4 || h < 4) return true;
        int visible = 0;
        int black = 0;
        int total = 0;
        for (int y = 0; y < 16; y++) {
            for (int x = 0; x < 16; x++) {
                int p = b.getPixel((x * (w - 1)) / 15, (y * (h - 1)) / 15);
                int a = p >>> 24;
                if (a < 8) continue;
                total++;
                visible++;
                if (a > 200 && ((p >> 16) & 255) < 8 && ((p >> 8) & 255) < 8 && (p & 255) < 8) black++;
            }
        }
        if (visible == 0) return true;
        return total > 20 && black * 100 >= total * 97;
    }

    /** The app list was read: drop memory entries of packages whose icons changed, retry failures. */
    void onAppsUpdated(Set<String> changedGroups) {
        boolean wasWaiting = !awaiting.isEmpty();
        awaiting.clear();
        if (!changedGroups.isEmpty()) {
            for (String key : cache.snapshot().keySet()) {
                if (changedGroups.contains(IconStore.groupOf(key))) cache.remove(key);
            }
        }
        if (wasWaiting || !failed.isEmpty() || !changedGroups.isEmpty()) {
            failed.clear();
            queueNotify();
        }
    }

    /** Back in the launcher: give failed icons another chance. */
    void retryFailed() {
        if (!failed.isEmpty()) {
            failed.clear();
            queueNotify();
        }
    }

    /** Reconcile versions on the calling (worker) thread; returns the groups to refresh. */
    Set<String> reconcile(Map<String, String> versions) {
        return store.reconcile(versions);
    }

    private void queueNotify() {
        if (notifyQueued) return;
        notifyQueued = true;
        main.postDelayed(notifyTask, NOTIFY_BATCH_MS); // one redraw for a burst of icons
    }
}
