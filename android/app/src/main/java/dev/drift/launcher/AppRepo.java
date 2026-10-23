package dev.drift.launcher;

import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Context;
import android.content.pm.LauncherActivityInfo;
import android.content.pm.LauncherApps;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.os.UserHandle;
import android.os.UserManager;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Launchable apps via the public LauncherApps API: no QUERY_ALL_PACKAGES, works
 * for work profiles, and package events arrive through a callback (no polling).
 * A card/dock key is the flattened component name, plus "#serial" for apps in
 * another user profile.
 */
final class AppRepo {
    static final class Entry {
        final String key;
        final String label;
        final ComponentName component;
        final UserHandle user;
        final LauncherActivityInfo info;

        Entry(String key, String label, ComponentName component, UserHandle user, LauncherActivityInfo info) {
            this.key = key;
            this.label = label;
            this.component = component;
            this.user = user;
            this.info = info;
        }
    }

    interface Listener {
        /** Main thread; the list was replaced and pushed to the core. */
        void onAppsChanged();
    }

    private final Context ctx;
    private final LauncherApps launcherApps;
    private final UserManager userManager;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private volatile Map<String, Entry> entries = new HashMap<>();
    private Listener listener;
    private boolean refreshQueued;
    private final Runnable refreshTask = this::refreshNow;

    private final LauncherApps.Callback callback = new LauncherApps.Callback() {
        @Override public void onPackageRemoved(String p, UserHandle u) { scheduleRefresh(); }
        @Override public void onPackageAdded(String p, UserHandle u) { scheduleRefresh(); }
        @Override public void onPackageChanged(String p, UserHandle u) { scheduleRefresh(); }
        @Override public void onPackagesAvailable(String[] p, UserHandle u, boolean replacing) { scheduleRefresh(); }
        @Override public void onPackagesUnavailable(String[] p, UserHandle u, boolean replacing) { scheduleRefresh(); }
    };

    AppRepo(Context context) {
        ctx = context.getApplicationContext();
        launcherApps = (LauncherApps) ctx.getSystemService(Context.LAUNCHER_APPS_SERVICE);
        userManager = (UserManager) ctx.getSystemService(Context.USER_SERVICE);
    }

    void setListener(Listener l) {
        listener = l;
    }

    void start() {
        launcherApps.registerCallback(callback, main);
        scheduleRefresh();
    }

    void stop() {
        launcherApps.unregisterCallback(callback);
    }

    /** Coalesces bursts (installing an app fires several callbacks). */
    void scheduleRefresh() {
        if (refreshQueued) return;
        refreshQueued = true;
        main.postDelayed(refreshTask, 150);
    }

    private void refreshNow() {
        refreshQueued = false;
        worker.execute(() -> {
            Map<String, Entry> map = new HashMap<>();
            List<String> keys = new ArrayList<>();
            List<String> labels = new ArrayList<>();
            UserHandle me = Process.myUserHandle();
            try {
                for (UserHandle user : userManager.getUserProfiles()) {
                    boolean mine = user.equals(me);
                    long serial = userManager.getSerialNumberForUser(user);
                    for (LauncherActivityInfo info : launcherApps.getActivityList(null, user)) {
                        ComponentName cn = info.getComponentName();
                        if (cn.getPackageName().equals(ctx.getPackageName())) continue;
                        String key = cn.flattenToString() + (mine ? "" : "#" + serial);
                        String label = String.valueOf(info.getLabel());
                        if (!mine) {
                            label = String.valueOf(ctx.getPackageManager().getUserBadgedLabel(label, user));
                        }
                        if (map.put(key, new Entry(key, label, cn, user, info)) == null) {
                            keys.add(key);
                            labels.add(label);
                        }
                    }
                }
            } catch (RuntimeException e) {
                return; // keep the previous list rather than publishing a partial one
            }
            String[] k = keys.toArray(new String[0]);
            String[] l = labels.toArray(new String[0]);
            main.post(() -> {
                entries = map;
                Native.setApps(k, l);
                if (listener != null) listener.onAppsChanged();
            });
        });
    }

    Entry get(String key) {
        return entries.get(key);
    }

    String label(String key) {
        Entry e = entries.get(key);
        return e == null ? key : e.label;
    }

    /** Standard Android launch; false when the app cannot be started. */
    boolean launch(String key) {
        Entry e = entries.get(key);
        if (e == null) return false;
        try {
            if (!launcherApps.isActivityEnabled(e.component, e.user)) return false;
            launcherApps.startMainActivity(e.component, e.user, null, null);
            return true;
        } catch (SecurityException | ActivityNotFoundException | IllegalStateException ex) {
            return false;
        }
    }

    /** Renders the app icon; call off the main thread. */
    Bitmap renderIcon(String key, int sizePx) {
        Entry e = entries.get(key);
        if (e == null) return null;
        try {
            Drawable d = e.info.getIcon(0);
            if (d == null) return null;
            Bitmap bmp = Bitmap.createBitmap(sizePx, sizePx, Bitmap.Config.ARGB_8888);
            Canvas c = new Canvas(bmp);
            d.setBounds(0, 0, sizePx, sizePx);
            d.draw(c);
            return bmp;
        } catch (RuntimeException ex) {
            return null;
        }
    }
}
