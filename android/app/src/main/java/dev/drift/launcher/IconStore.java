package dev.drift.launcher;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Icons on disk (cache dir), one PNG per component under a folder per package, and the version
 * of every package they were drawn from in SharedPreferences "drift_icons".
 *
 * Rules that keep the cache honest:
 *  - A cached icon is used as is (even before PackageManager has answered), so a cold start
 *    shows icons immediately; it is verified when the app list arrives.
 *  - Only packages whose version really changed lose their icons. A version that could not
 *    be read is never stored (not even as 0) and never causes a reset.
 *  - Icons are written to disk only for a package whose version is known, so every stored
 *    file can be invalidated later.
 *  - A different icon size (density change) clears everything once.
 */
final class IconStore {
    private static final String PREFS = "drift_icons";
    private static final String K_SIZE = "size_px";
    private static final String V_PREFIX = "v:";

    private final File root;
    private final SharedPreferences prefs;
    /** Mirror of the stored versions; touched by the worker threads only through synchronized methods. */
    private final Map<String, String> versions = new HashMap<>();

    IconStore(Context ctx, int sizePx) {
        root = new File(ctx.getCacheDir(), "icons");
        prefs = ctx.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        for (Map.Entry<String, ?> e : prefs.getAll().entrySet()) {
            if (e.getKey().startsWith(V_PREFIX) && e.getValue() instanceof String) {
                versions.put(e.getKey().substring(V_PREFIX.length()), (String) e.getValue());
            }
        }
        if (prefs.getInt(K_SIZE, -1) != sizePx) {
            deleteAll();
            prefs.edit().clear().putInt(K_SIZE, sizePx).apply();
            versions.clear();
        }
    }

    /** "pkg" for the owner, "pkg#serial" for a work profile: icons of one app in two profiles stay apart. */
    static String groupOf(String key) {
        int slash = key.indexOf('/');
        String pkg = slash > 0 ? key.substring(0, slash) : key;
        int hash = key.lastIndexOf('#');
        return hash > slash ? pkg + key.substring(hash) : pkg;
    }

    private static String hex(String s) {
        try {
            byte[] d = MessageDigest.getInstance("SHA-1").digest(s.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder(40);
            for (byte b : d) sb.append(Character.forDigit((b >> 4) & 15, 16)).append(Character.forDigit(b & 15, 16));
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            return Integer.toHexString(s.hashCode());
        }
    }

    private File fileFor(String key) {
        return new File(new File(root, groupOf(key)), hex(key) + ".png");
    }

    /** Cached icon, or null. A damaged file is removed. Safe on any thread. */
    Bitmap load(String key) {
        File f = fileFor(key);
        if (!f.isFile()) return null;
        BitmapFactory.Options o = new BitmapFactory.Options();
        o.inPreferredConfig = Bitmap.Config.ARGB_8888;
        Bitmap b = BitmapFactory.decodeFile(f.getPath(), o);
        if (b == null) {
            //noinspection ResultOfMethodCallIgnored
            f.delete();
        }
        return b;
    }

    synchronized boolean hasVersion(String key) {
        return versions.containsKey(groupOf(key));
    }

    /** Stores the icon unless its package has no known version. A crash mid-write leaves an undecodable file, treated as a miss. */
    void save(String key, Bitmap bmp) {
        if (!hasVersion(key)) return;
        File f = fileFor(key);
        File dir = f.getParentFile();
        if (dir == null || (!dir.isDirectory() && !dir.mkdirs())) return;
        File tmp = new File(f.getPath() + ".tmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            if (!bmp.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                //noinspection ResultOfMethodCallIgnored
                tmp.delete();
                return;
            }
        } catch (IOException e) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            return;
        }
        if (!tmp.renameTo(f)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
        }
    }

    /**
     * The app list was read: {@code current} maps package group to its version string, or to null
     * when the version could not be read. Returns the groups whose icons must be redrawn.
     */
    synchronized Set<String> reconcile(Map<String, String> current) {
        Set<String> changed = new HashSet<>();
        SharedPreferences.Editor ed = prefs.edit();
        for (Map.Entry<String, String> e : current.entrySet()) {
            String group = e.getKey();
            String now = e.getValue();
            if (now == null || now.isEmpty()) continue; // unknown: neither store nor reset
            String old = versions.get(group);
            if (now.equals(old)) continue;
            if (old != null) changed.add(group); // updated app: its icon may differ
            deleteGroup(group);
            versions.put(group, now);
            ed.putString(V_PREFIX + group, now);
        }
        // Uninstalled packages: drop their files and versions (only when the list was complete).
        List<String> gone = new ArrayList<>();
        for (String group : versions.keySet()) {
            if (!current.containsKey(group)) gone.add(group);
        }
        for (String group : gone) {
            deleteGroup(group);
            versions.remove(group);
            ed.remove(V_PREFIX + group);
        }
        ed.apply();
        return changed;
    }

    private void deleteGroup(String group) {
        File dir = new File(root, group);
        File[] files = dir.listFiles();
        if (files != null) {
            for (File f : files) {
                //noinspection ResultOfMethodCallIgnored
                f.delete();
            }
        }
        //noinspection ResultOfMethodCallIgnored
        dir.delete();
    }

    private void deleteAll() {
        File[] groups = root.listFiles();
        if (groups == null) return;
        for (File g : groups) {
            File[] files = g.listFiles();
            if (files != null) {
                for (File f : files) {
                    //noinspection ResultOfMethodCallIgnored
                    f.delete();
                }
            }
            //noinspection ResultOfMethodCallIgnored
            g.delete();
        }
    }
}
