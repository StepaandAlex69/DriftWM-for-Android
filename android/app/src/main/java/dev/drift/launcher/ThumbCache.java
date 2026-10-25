package dev.drift.launcher;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.util.LruCache;

import java.io.File;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Decodes preview JPEGs off the main thread (RGB_565 to halve memory). Main-thread API. */
final class ThumbCache implements ThumbStore.Listener {
    interface Listener {
        void onThumbsChanged();
    }

    private final Context ctx;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Set<String> pending = new HashSet<>();
    private final Set<String> missing = new HashSet<>();
    private Listener listener;

    private final LruCache<String, Bitmap> cache = new LruCache<>(20 * 1024 * 1024) {
        @Override protected int sizeOf(String key, Bitmap value) {
            return value.getByteCount();
        }
    };

    ThumbCache(Context context) {
        ctx = context.getApplicationContext();
    }

    void setListener(Listener l) {
        listener = l;
    }

    /** The preview for a package, or null (none captured yet, or still decoding). */
    Bitmap get(String pkg) {
        if (pkg == null) return null;
        Bitmap b = cache.get(pkg);
        if (b == null && !missing.contains(pkg) && pending.add(pkg)) {
            worker.execute(() -> {
                File f = ThumbStore.file(ctx, pkg);
                Bitmap decoded = null;
                if (f.exists()) {
                    BitmapFactory.Options o = new BitmapFactory.Options();
                    o.inPreferredConfig = Bitmap.Config.RGB_565;
                    decoded = BitmapFactory.decodeFile(f.getPath(), o);
                }
                Bitmap result = decoded;
                main.post(() -> {
                    pending.remove(pkg);
                    if (result != null) {
                        cache.put(pkg, result);
                    } else {
                        missing.add(pkg);
                    }
                    if (listener != null) listener.onThumbsChanged();
                });
            });
        }
        return b;
    }

    @Override public void onThumbUpdated(String pkg) {
        cache.remove(pkg);
        missing.remove(pkg);
        if (listener != null) listener.onThumbsChanged();
    }
}
