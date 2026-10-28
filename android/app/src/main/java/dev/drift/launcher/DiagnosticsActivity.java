package dev.drift.launcher;

import android.app.Activity;
import android.app.role.RoleManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.content.res.Resources;
import android.database.Cursor;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.util.List;
import java.util.Locale;

/**
 * "Is system navigation provided independently of the home app on this firmware?"
 *
 * Recents and the swipe-between-apps gesture are not implemented by a home app.
 * On Android they are served by a component the manufacturer ships (AOSP: the
 * Quickstep service inside the system launcher's package, bound by SystemUI via
 * config_recentsComponentName). Whether that component keeps working when a
 * different app holds the home role is decided by the firmware. This screen
 * prints what the firmware declares so a tester can send one report instead of
 * guessing. Nothing here changes any setting.
 */
public final class DiagnosticsActivity extends Activity {
    private static final String QUICKSTEP_ACTION = "android.intent.action.QUICKSTEP_SERVICE";

    private TextView body;
    private String report = "";

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        float d = getResources().getDisplayMetrics().density;
        int p = Math.round(16 * d);

        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setBackgroundColor(0xFF101216);
        col.setOnApplyWindowInsetsListener((v, wi) -> {
            android.graphics.Insets b = wi.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(p + b.left, p + b.top, p + b.right, p + b.bottom);
            return WindowInsets.CONSUMED;
        });

        TextView head = new TextView(this);
        head.setText("Drift: system navigation check\nRun this while Drift is the selected home app.");
        head.setTextColor(0xFF7AA2FF);
        head.setTextSize(15);
        col.addView(head);

        body = new TextView(this);
        body.setText("Collecting...");
        body.setTextColor(0xFFE6E8EC);
        body.setTextSize(11);
        body.setTypeface(Typeface.MONOSPACE);
        body.setTextIsSelectable(true);
        ScrollView sv = new ScrollView(this);
        sv.addView(body);
        col.addView(sv, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        Button copy = new Button(this);
        copy.setText("Copy");
        copy.setOnClickListener(v -> {
            ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("Drift report", report));
            Toast.makeText(this, "Copied", Toast.LENGTH_SHORT).show();
        });
        Button share = new Button(this);
        share.setText("Share");
        share.setOnClickListener(v -> {
            Intent i = new Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, report);
            startActivity(Intent.createChooser(i, "Send report"));
        });
        row.addView(copy, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        row.addView(share, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        col.addView(row);
        setContentView(col);

        new Thread(() -> {
            String r;
            try {
                r = collect();
            } catch (RuntimeException e) {
                r = "Report failed: " + e;
            }
            final String text = r;
            runOnUiThread(() -> {
                report = text;
                body.setText(text);
            });
        }, "drift-diagnostics").start();
    }

    // ---- report ----------------------------------------------------------------

    private static void section(StringBuilder sb, String title) {
        sb.append("\n== ").append(title).append(" ==\n");
    }

    private static void kv(StringBuilder sb, String k, Object v) {
        sb.append(k).append(": ").append(v).append('\n');
    }

    @SuppressWarnings("deprecation")
    private String collect() {
        StringBuilder sb = new StringBuilder();
        PackageManager pm = getPackageManager();

        section(sb, "Device");
        kv(sb, "manufacturer / brand", Build.MANUFACTURER + " / " + Build.BRAND);
        kv(sb, "model / device", Build.MODEL + " / " + Build.DEVICE);
        kv(sb, "android", Build.VERSION.RELEASE + " (API " + Build.VERSION.SDK_INT + "), patch " + Build.VERSION.SECURITY_PATCH);
        kv(sb, "display id", Build.DISPLAY);
        kv(sb, "fingerprint", Build.FINGERPRINT);
        sb.append("vendor properties (emui/harmony/hw/honor/magic):\n").append(vendorProps());

        section(sb, "Navigation settings");
        kv(sb, "Settings.Secure navigation_mode", Settings.Secure.getInt(getContentResolver(), "navigation_mode", -1)
            + "  (0 = 3 buttons, 1 = 2 buttons, 2 = gestures, -1 = not set here)");
        Integer cfgMode = systemInt("config_navBarInteractionMode");
        kv(sb, "config_navBarInteractionMode", cfgMode == null ? "not exposed" : cfgMode);
        sb.append("settings with nav/gesture/swipe/recent/launcher in the name:\n").append(settingsDump());

        section(sb, "Home role");
        boolean ours = false;
        try {
            RoleManager rm = getSystemService(RoleManager.class);
            ours = rm != null && rm.isRoleHeld(RoleManager.ROLE_HOME);
            kv(sb, "Drift holds the home role", ours);
        } catch (RuntimeException e) {
            kv(sb, "Drift holds the home role", "unknown (" + e.getClass().getSimpleName() + ")");
        }
        Intent home = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME);
        ResolveInfo def = pm.resolveActivity(home, PackageManager.MATCH_DEFAULT_ONLY);
        String defPkg = def == null || def.activityInfo == null ? null : def.activityInfo.packageName;
        kv(sb, "resolved default home", defPkg == null ? "none" : defPkg + "/" + def.activityInfo.name);
        List<ResolveInfo> homes = pm.queryIntentActivities(home, PackageManager.MATCH_ALL);
        sb.append("all home apps:\n");
        for (ResolveInfo ri : homes) {
            sb.append("  ").append(ri.activityInfo.packageName).append(version(pm, ri.activityInfo.packageName)).append('\n');
        }

        section(sb, "Recents / gesture provider");
        String recents = systemString("config_recentsComponentName");
        kv(sb, "config_recentsComponentName", recents == null || recents.isEmpty() ? "not exposed / empty" : recents);
        ComponentName rc = recents == null ? null : ComponentName.unflattenFromString(recents);
        String recentsPkg = rc == null ? null : rc.getPackageName();
        if (recentsPkg != null) {
            kv(sb, "recents package", recentsPkg + version(pm, recentsPkg));
            kv(sb, "recents package is also the default home", recentsPkg.equals(defPkg));
        }
        List<ResolveInfo> qs = pm.queryIntentServices(new Intent(QUICKSTEP_ACTION), PackageManager.MATCH_ALL);
        sb.append("services answering ").append(QUICKSTEP_ACTION).append(":\n");
        if (qs.isEmpty()) sb.append("  none\n");
        boolean quickstepInRecentsPkg = false;
        for (ResolveInfo ri : qs) {
            sb.append("  ").append(ri.serviceInfo.packageName).append('/').append(ri.serviceInfo.name).append('\n');
            if (ri.serviceInfo.packageName.equals(recentsPkg)) quickstepInRecentsPkg = true;
        }

        section(sb, "Accessibility");
        kv(sb, "Drift previews service enabled", ThumbService.isEnabled(this));
        kv(sb, "enabled services", Settings.Secure.getString(getContentResolver(), Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES));

        section(sb, "Reading this report");
        sb.append(verdict(ours, defPkg, recentsPkg, quickstepInRecentsPkg, !qs.isEmpty()));
        sb.append("\nDrift itself does not hide system bars, exclude gestures, touch the task stack or call\n");
        sb.append("any recents API; it only holds the home role.\n");
        return sb.toString();
    }

    private String verdict(boolean ours, String defPkg, String recentsPkg, boolean quickstepInRecentsPkg, boolean anyQuickstep) {
        if (!ours) {
            return "Drift is not the selected home app right now, so this report describes the stock setup.\n"
                + "Select Drift as home (Settings > Apps > Default apps > Home app) and run it again.\n";
        }
        if (recentsPkg == null) {
            return "This firmware does not expose the stock recents component. Recents and gestures are then\n"
                + "implemented by the manufacturer's own system UI, and how they behave when another app\n"
                + "holds the home role is decided by that firmware, not by the home app.\n";
        }
        if (quickstepInRecentsPkg) {
            return "Stock Quickstep layout: the gesture/recents service lives in " + recentsPkg + ", not in the home app.\n"
                + "AOSP keeps it working for any home app through a fallback recents activity in that same\n"
                + "package, so with this layout system navigation should not depend on Drift. If it still\n"
                + "misbehaves, send this report: that would point to a firmware bug.\n";
        }
        String where = anyQuickstep ? "in another package" : "nowhere on this device";
        return "The recents package is " + recentsPkg + " but the stock gesture service (QUICKSTEP_SERVICE) is " + where + ".\n"
            + "So recents and swipe-between-apps here are the manufacturer's own implementation, tied to its\n"
            + "system launcher (" + recentsPkg + (defPkg != null ? ", default home now: " + defPkg : "") + ").\n"
            + "Whether that implementation still works when a third-party app holds the home role is a firmware\n"
            + "decision. An app cannot add it through its manifest or window code.\n";
    }

    private static String version(PackageManager pm, String pkg) {
        try {
            PackageInfo pi = pm.getPackageInfo(pkg, 0);
            boolean system = pi.applicationInfo != null
                && (pi.applicationInfo.flags & android.content.pm.ApplicationInfo.FLAG_SYSTEM) != 0;
            return " (v" + pi.versionName + (system ? ", system" : "") + ")";
        } catch (PackageManager.NameNotFoundException | RuntimeException e) {
            return " (not visible)";
        }
    }

    private static Integer systemInt(String name) {
        try {
            int id = Resources.getSystem().getIdentifier(name, "integer", "android");
            return id == 0 ? null : Resources.getSystem().getInteger(id);
        } catch (RuntimeException e) {
            return null;
        }
    }

    private static String systemString(String name) {
        try {
            int id = Resources.getSystem().getIdentifier(name, "string", "android");
            return id == 0 ? null : Resources.getSystem().getString(id);
        } catch (RuntimeException e) {
            return null;
        }
    }

    private static String vendorProps() {
        StringBuilder sb = new StringBuilder();
        try {
            Process p = new ProcessBuilder("getprop").redirectErrorStream(true).start();
            try (BufferedReader r = new BufferedReader(new InputStreamReader(p.getInputStream()))) {
                for (String line; (line = r.readLine()) != null; ) {
                    String l = line.toLowerCase(Locale.ROOT);
                    if (l.contains("emui") || l.contains("harmony") || l.contains("hw_") || l.contains("huawei")
                        || l.contains("honor") || l.contains("magic") || l.contains("build.version.release")) {
                        sb.append("  ").append(line).append('\n');
                    }
                }
            }
        } catch (java.io.IOException | RuntimeException e) {
            sb.append("  unavailable\n");
        }
        return sb.length() == 0 ? "  none\n" : sb.toString();
    }

    private String settingsDump() {
        StringBuilder sb = new StringBuilder();
        for (String table : new String[] {"secure", "global", "system"}) {
            try (Cursor c = getContentResolver().query(Uri.parse("content://settings/" + table),
                new String[] {"name", "value"}, null, null, null)) {
                if (c == null) continue;
                while (c.moveToNext()) {
                    String n = c.getString(0);
                    String l = n == null ? "" : n.toLowerCase(Locale.ROOT);
                    if (l.contains("navigation") || l.contains("gesture") || l.contains("swipe")
                        || l.contains("recent") || l.contains("launcher") || l.contains("navbar")) {
                        sb.append("  ").append(table).append('.').append(n).append(" = ").append(c.getString(1)).append('\n');
                    }
                }
            } catch (RuntimeException e) {
                sb.append("  ").append(table).append(": not readable\n");
            }
        }
        return sb.length() == 0 ? "  none\n" : sb.toString();
    }
}
