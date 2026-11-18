package dev.drift.launcher;

import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.content.pm.LauncherActivityInfo;
import android.content.pm.LauncherApps;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.os.UserHandle;
import android.os.UserManager;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;

/**
 * Launchable apps via the public LauncherApps API: no QUERY_ALL_PACKAGES, works for work
 * profiles, and package events arrive through a callback.
 *
 * Reading the whole list is the expensive part, so it is done only when needed:
 *  - once at start,
 *  - when LauncherApps reports a change (install, update, removal, availability),
 *  - on returning to the launcher, but not more than once per {@link #FULL_POLL_MS}.
 * It runs on a low-priority worker, and the result reaches the core only when the keys or
 * labels really differ from what the core already has.
 *
 * A card/dock key is the flattened component name, plus "#serial" for apps in another profile.
 */
final class AppRepo {
    static final long FULL_POLL_MS = 60_000;

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
        /** Main thread; the list in the core was replaced (it really changed). */
        void onAppsChanged();
    }

    private final Context ctx;
    private final LauncherApps launcherApps;
    private final UserManager userManager;
    private final PackageManager pm;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Workers.single("drift-apps", Process.THREAD_PRIORITY_BACKGROUND);
    private volatile Map<String, Entry> entries = new HashMap<>();
    private Listener listener;
    private IconCache icons;
    private boolean refreshQueued;
    private boolean refreshRunning;
    private boolean refreshAgain;
    private long lastFullRead = -1_000_000_000L;
    private String[] pushedKeys;
    private String[] pushedLabels;
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
        pm = ctx.getPackageManager();
    }

    void setListener(Listener l) {
        listener = l;
    }

    void attachIcons(IconCache c) {
        icons = c;
    }

    void start() {
        launcherApps.registerCallback(callback, main);
        scheduleRefresh();
    }

    void stop() {
        launcherApps.unregisterCallback(callback);
    }

    /** A package event arrived (or the first read is due). Coalesces bursts. */
    void scheduleRefresh() {
        if (refreshQueued) return;
        refreshQueued = true;
        main.postDelayed(refreshTask, 150);
    }

    /** Back in the launcher: events kept the list current, so a full read is only a once-a-minute safety net. */
    void refreshIfStale() {
        if (SystemClock.elapsedRealtime() - lastFullRead >= FULL_POLL_MS) scheduleRefresh();
    }

    private void refreshNow() {
        refreshQueued = false;
        if (refreshRunning) {
            refreshAgain = true; // something changed while reading: read once more afterwards
            return;
        }
        refreshRunning = true;
        worker.execute(this::readAll);
    }

    private void readAll() {
        long t0 = SystemClock.elapsedRealtimeNanos();
        Map<String, Entry> map = new HashMap<>();
        List<String> keys = new ArrayList<>();
        List<String> labels = new ArrayList<>();
        Map<String, String> versions = new HashMap<>();
        UserHandle me = Process.myUserHandle();
        boolean ok = true;
        try {
            for (UserHandle user : userManager.getUserProfiles()) {
                boolean mine = user.equals(me);
                long serial = userManager.getSerialNumberForUser(user);
                for (LauncherActivityInfo info : launcherApps.getActivityList(null, user)) {
                    ComponentName cn = info.getComponentName();
                    // Hide the launcher itself, but keep its diagnostics screen reachable from Drift.
                    if (cn.getPackageName().equals(ctx.getPackageName())
                        && !cn.getClassName().equals(DiagnosticsActivity.class.getName())) continue;
                    String key = cn.flattenToString() + (mine ? "" : "#" + serial);
                    String label = String.valueOf(info.getLabel());
                    if (!mine) label = String.valueOf(pm.getUserBadgedLabel(label, user));
                    if (map.put(key, new Entry(key, label, cn, user, info)) == null) {
                        keys.add(key);
                        labels.add(label);
                    }
                    String group = IconStore.groupOf(key);
                    if (!versions.containsKey(group)) versions.put(group, versionOf(info, mine));
                }
            }
        } catch (RuntimeException e) {
            ok = false; // keep the previous list rather than publishing a partial one
        }
        final boolean complete = ok;
        final String[] k = keys.toArray(new String[0]);
        final String[] l = labels.toArray(new String[0]);
        // Icon versions are reconciled here, before the list is published, so icons rendered
        // for the new list can already be stored on disk.
        final Set<String> changedIcons = complete && icons != null ? icons.reconcile(versions) : new HashSet<>();
        final Set<String> pkgs = new HashSet<>();
        for (Entry en : map.values()) {
            // Our own screens are never preview targets.
            if (!en.component.getPackageName().equals(ctx.getPackageName())) pkgs.add(en.component.getPackageName());
        }
        PerfStats.APPS_FETCH.add(SystemClock.elapsedRealtimeNanos() - t0, 0);
        main.post(() -> {
            refreshRunning = false;
            if (complete) {
                lastFullRead = SystemClock.elapsedRealtime();
                entries = map;
                boolean differs = !Arrays.equals(k, pushedKeys) || !Arrays.equals(l, pushedLabels);
                if (differs) {
                    pushedKeys = k;
                    pushedLabels = l;
                    ThumbStore.setKnownPackages(pkgs);
                    Native.setApps(k, l);
                }
                if (icons != null) icons.onAppsUpdated(changedIcons);
                if (differs && listener != null) listener.onAppsChanged();
            }
            if (refreshAgain) {
                refreshAgain = false;
                scheduleRefresh();
            }
        });
    }

/**
 * Version of the package an icon was drawn from, or null when it cannot be read. Never "0":
 * an unreadable version must not look like a real one.
 */
    private String versionOf(LauncherActivityInfo info, boolean mine) {
        try {
            if (mine) {
                PackageInfo pi = pm.getPackageInfo(
                    info.getComponentName().getPackageName(), 0
              );
                return pi.getLongVersionCode() + ":" + pi.lastUpdateTime;
            }

            ApplicationInfo ai = info.getApplicationInfo();
            if (ai == null) return null;

            PackageInfo pi = pm.getPackageInfo(ai.packageName, 0);
            return pi.getLongVersionCode() + ":" + info.getFirstInstallTime();

        } catch (PackageManager.NameNotFoundException | RuntimeException e) {
            return null;
        }
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
