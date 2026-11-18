package dev.drift.launcher;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.BitmapShader;
import android.graphics.Shader;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.util.LruCache;

import java.io.File;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ExecutorService;

/**
 * Decodes preview JPEGs on a low-priority thread (RGB_565 to halve memory). Main-thread API.
 * Redraws are batched: a burst of decoded previews causes one {@code onThumbsChanged}.
 */
final class ThumbCache implements ThumbStore.Listener {
    interface Listener {
        void onThumbsChanged();
    }

    /** A decoded preview and the shader that draws it with rounded corners (built once, on first use). */
    static final class Thumb {
        final Bitmap bitmap;
        private BitmapShader shader;

        Thumb(Bitmap b) {
            bitmap = b;
        }

        BitmapShader shader() {
            if (shader == null) shader = new BitmapShader(bitmap, Shader.TileMode.CLAMP, Shader.TileMode.CLAMP);
            return shader;
        }
    }

    private static final long NOTIFY_BATCH_MS = 32;

    private final Context ctx;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Workers.single("drift-thumbs", Process.THREAD_PRIORITY_BACKGROUND);
    private final Set<String> pending = new HashSet<>();
    private final Set<String> missing = new HashSet<>();
    private Listener listener;
    private boolean notifyQueued;
    private final Runnable notifyTask = () -> {
        notifyQueued = false;
        if (listener != null) listener.onThumbsChanged();
    };

    private final LruCache<String, Thumb> cache = new LruCache<>(20 * 1024 * 1024) {
        @Override protected int sizeOf(String key, Thumb value) {
            return value.bitmap.getByteCount();
        }
    };

    ThumbCache(Context context) {
        ctx = context.getApplicationContext();
    }

    void setListener(Listener l) {
        listener = l;
    }

    /** The preview for a package, or null (none captured yet, or still decoding). */
    Thumb get(String pkg) {
        if (pkg == null) return null;
        Thumb t = cache.get(pkg);
        if (t == null && !missing.contains(pkg) && pending.add(pkg)) {
            worker.execute(() -> {
                long t0 = SystemClock.elapsedRealtimeNanos();
                File f = ThumbStore.file(ctx, pkg);
                Bitmap decoded = null;
                if (f.exists()) {
                    BitmapFactory.Options o = new BitmapFactory.Options();
                    o.inPreferredConfig = Bitmap.Config.RGB_565;
                    decoded = BitmapFactory.decodeFile(f.getPath(), o);
                }
                PerfStats.THUMB_LOAD.add(SystemClock.elapsedRealtimeNanos() - t0, 0);
                final Bitmap result = decoded;
                main.post(() -> {
                    pending.remove(pkg);
                    if (result != null) {
                        cache.put(pkg, new Thumb(result));
                    } else {
                        missing.add(pkg);
                    }
                    queueNotify();
                });
            });
        }
        return t;
    }

    @Override public void onThumbUpdated(String pkg) {
        cache.remove(pkg);
        missing.remove(pkg);
        queueNotify();
    }

    private void queueNotify() {
        if (notifyQueued) return;
        notifyQueued = true;
        main.postDelayed(notifyTask, NOTIFY_BATCH_MS);
    }
}
