package dev.drift.launcher;

import android.accessibilityservice.AccessibilityService;
import android.content.ComponentName;
import android.content.Context;
import android.graphics.Bitmap;
import android.hardware.HardwareBuffer;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.Settings;
import android.text.TextUtils;
import android.view.Display;
import android.view.accessibility.AccessibilityEvent;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Captures a still preview of the app in front, for the cards on the canvas.
 *
 * Android gives a normal launcher no way to read other apps' windows; the only
 * non-root route is AccessibilityService.takeScreenshot (API 30+). This service
 * therefore asks for nothing else: it cannot read screen content (no
 * canRetrieveWindowContent), only learns which package is in front and when the
 * user interacted, and then screenshots ~1 s after the last interaction, so the
 * stored preview is "the last thing you saw in that app". Windows marked secure
 * (banking, password managers) are not captured by the system. Images stay on
 * the device, in the app's private storage.
 */
public final class ThumbService extends AccessibilityService {
    private static final long SETTLE_MS = 900;
    private static final long MIN_INTERVAL_MS = 1300;

    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService io = Executors.newSingleThreadExecutor();
    private final Runnable shoot = this::capture;
    private volatile String front;
    private long lastShot;

    static boolean isEnabled(Context ctx) {
        String enabled = Settings.Secure.getString(ctx.getContentResolver(),
            Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        if (enabled == null) return false;
        String me = new ComponentName(ctx, ThumbService.class).flattenToString();
        TextUtils.SimpleStringSplitter split = new TextUtils.SimpleStringSplitter(':');
        split.setString(enabled);
        while (split.hasNext()) {
            if (me.equalsIgnoreCase(split.next())) return true;
        }
        return false;
    }

    @Override public void onAccessibilityEvent(AccessibilityEvent e) {
        CharSequence cs = e.getPackageName();
        if (cs == null) return;
        String pkg = cs.toString();
        if (!ThumbStore.isCapturable(pkg)) {
            if (pkg.equals(getPackageName())) {
                // Back on the launcher: whatever was pending belongs to an app that is gone.
                main.removeCallbacks(shoot);
                front = null;
            }
            return; // keyboard, SystemUI, our own windows: not an app preview
        }
        front = pkg;
        main.removeCallbacks(shoot);
        main.postDelayed(shoot, SETTLE_MS);
    }

    private void capture() {
        final String target = front;
        if (target == null) return;
        long wait = MIN_INTERVAL_MS - (SystemClock.uptimeMillis() - lastShot);
        if (wait > 0) {
            main.postDelayed(shoot, wait);
            return;
        }
        lastShot = SystemClock.uptimeMillis();
        takeScreenshot(Display.DEFAULT_DISPLAY, io, new TakeScreenshotCallback() {
            @Override public void onSuccess(ScreenshotResult result) {
                HardwareBuffer hb = result.getHardwareBuffer();
                try {
                    if (!target.equals(front)) return; // user already left the app
                    Bitmap hw = Bitmap.wrapHardwareBuffer(hb, result.getColorSpace());
                    if (hw == null) return;
                    Bitmap soft = hw.copy(Bitmap.Config.ARGB_8888, false);
                    hw.recycle();
                    if (soft == null) return;
                    try {
                        ThumbStore.save(ThumbService.this, target, soft);
                    } finally {
                        soft.recycle();
                    }
                } catch (RuntimeException ignored) {
                    // a failed preview must never take the service down
                } finally {
                    hb.close();
                }
            }

            @Override public void onFailure(int errorCode) {
                // Rate limit, secure window, display off: keep the previous preview.
            }
        });
    }

    @Override public void onInterrupt() {
        main.removeCallbacks(shoot);
    }

    @Override public void onDestroy() {
        main.removeCallbacks(shoot);
        io.shutdown();
        super.onDestroy();
    }
}
