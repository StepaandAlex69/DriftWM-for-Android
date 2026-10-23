package dev.drift.launcher;

import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.util.LruCache;

import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Icons are decoded off the main thread; until one arrives callers draw a placeholder. */
final class IconCache {
    interface Listener {
        void onIconsLoaded();
    }

    private final AppRepo repo;
    private final int sizePx;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Set<String> pending = new HashSet<>();
    private final Set<String> failed = new HashSet<>();
    private boolean notifyQueued;
    private Listener listener;

    private final LruCache<String, Bitmap> cache = new LruCache<>(24 * 1024 * 1024) {
        @Override protected int sizeOf(String key, Bitmap value) {
            return value.getByteCount();
        }
    };

    IconCache(AppRepo repo, float density) {
        this.repo = repo;
        this.sizePx = Math.max(48, Math.round(64f * density));
    }

    void setListener(Listener l) {
        listener = l;
    }

    /** Main thread only. Returns null while the icon is loading. */
    Bitmap get(String key) {
        if (key == null) return null;
        Bitmap b = cache.get(key);
        if (b == null && !failed.contains(key) && pending.add(key)) {
            worker.execute(() -> {
                Bitmap loaded = repo.renderIcon(key, sizePx);
                main.post(() -> {
                    pending.remove(key);
                    if (loaded != null) {
                        cache.put(key, loaded);
                    } else {
                        failed.add(key);
                    }
                    queueNotify();
                });
            });
        }
        return b;
    }

    /** Forget everything (an app was updated and its icon may differ). */
    void clear() {
        cache.evictAll();
        failed.clear();
    }

    private void queueNotify() {
        if (notifyQueued) return;
        notifyQueued = true;
        main.post(() -> {
            notifyQueued = false;
            if (listener != null) listener.onIconsLoaded();
        });
    }
}
