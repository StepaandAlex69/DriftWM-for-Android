package dev.drift.launcher;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.Collections;
import java.util.Set;

/**
 * On-disk app previews, one JPEG per package (last capture wins). Written by
 * {@link ThumbService}, read through {@link ThumbCache}. A preview is a still
 * image of the app as it looked at the last capture, never a live view.
 */
final class ThumbStore {
    interface Listener {
        /** Main thread. */
        void onThumbUpdated(String pkg);
    }

    static final int WIDTH_PX = 240;

    /** System-bar heights (px) to cut from captures; set by the launcher from its insets. */
    static volatile int cropTop;
    static volatile int cropBottom;

    private static volatile Set<String> known = Collections.emptySet();
    private static volatile Listener listener;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private ThumbStore() {}

    static void setKnownPackages(Set<String> packages) {
        known = packages;
    }

    static void setListener(Listener l) {
        listener = l;
    }

    /** Only launchable apps get previews: this excludes the keyboard, SystemUI and the launcher itself. */
    static boolean isCapturable(String pkg) {
        return known.contains(pkg);
    }

    static String packageOf(String key) {
        int slash = key.indexOf('/');
        return slash > 0 ? key.substring(0, slash) : key;
    }

    static File dir(Context ctx) {
        File d = new File(ctx.getFilesDir(), "thumbs");
        //noinspection ResultOfMethodCallIgnored
        d.mkdirs();
        return d;
    }

    static File file(Context ctx, String pkg) {
        return new File(dir(ctx), pkg + ".jpg");
    }

    /** Crop system bars, downscale, write atomically. Call off the main thread. */
    static void save(Context ctx, String pkg, Bitmap full) {
        int top = cropTop;
        int bottom = cropBottom;
        int h = full.getHeight() - top - bottom;
        if (top < 0 || bottom < 0 || h < full.getHeight() / 2) {
            top = 0;
            bottom = 0;
            h = full.getHeight();
        }
        Bitmap cropped = Bitmap.createBitmap(full, 0, top, full.getWidth(), h);
        int th = Math.max(1, Math.round(WIDTH_PX * (float) h / full.getWidth()));
        Bitmap scaled = Bitmap.createScaledBitmap(cropped, WIDTH_PX, th, true);
        File dst = file(ctx, pkg);
        File tmp = new File(dst.getPath() + ".tmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            scaled.compress(Bitmap.CompressFormat.JPEG, 85, out);
            out.getFD().sync();
        } catch (IOException e) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            return;
        } finally {
            if (cropped != full) cropped.recycle();
            if (scaled != cropped) scaled.recycle();
        }
        if (!tmp.renameTo(dst)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            return;
        }
        Listener l = listener;
        if (l != null) MAIN.post(() -> l.onThumbUpdated(pkg));
    }
}
