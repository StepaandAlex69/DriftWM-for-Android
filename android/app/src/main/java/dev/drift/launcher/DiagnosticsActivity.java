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
    private boolean perfMode = true;
    private Button reset;
    private Button refresh;
    private TextView tabPerf;
    private TextView tabNav;
    private Appearance.Palette pal;
    private float d;

    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(LocaleHelper.wrap(base));
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        d = getResources().getDisplayMetrics().density;
        pal = Appearance.read(this).pal;
        int p = Math.round(16 * d);

        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setBackgroundColor(pal.base);
        col.setOnApplyWindowInsetsListener((v, wi) -> {
            android.graphics.Insets b = wi.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(p + b.left, p + b.top, p + b.right, p + b.bottom);
            return WindowInsets.CONSUMED;
        });

        TextView head = new TextView(this);
        head.setText(R.string.diag_title);
        head.setTextColor(pal.accent);
        head.setTextSize(18);
        head.setTypeface(Typeface.DEFAULT_BOLD);
        col.addView(head);

        LinearLayout tabs = new LinearLayout(this);
        tabs.setOrientation(LinearLayout.HORIZONTAL);
        tabPerf = tab(R.string.diag_tab_performance, true);
        tabNav = tab(R.string.diag_tab_navigation, false);
        tabs.addView(tabPerf, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        tabs.addView(tabNav, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        tp.topMargin = Math.round(10 * d);
        tp.bottomMargin = Math.round(8 * d);
        col.addView(tabs, tp);

        body = new TextView(this);
        body.setTextColor(pal.text);
        body.setTextSize(11);
        body.setTypeface(Typeface.MONOSPACE);
        body.setTextIsSelectable(true);
        ScrollView sv = new ScrollView(this);
        sv.addView(body);
        col.addView(sv, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        refresh = new Button(this);
        refresh.setText(R.string.action_refresh);
        refresh.setOnClickListener(v -> render());
        reset = new Button(this);
        reset.setText(R.string.action_reset);
        reset.setOnClickListener(v -> {
            PerfStats.resetAll();
            render();
        });
        Button copy = new Button(this);
        copy.setText(R.string.action_copy);
        copy.setOnClickListener(v -> {
            ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) cm.setPrimaryClip(ClipData.newPlainText(getString(R.string.diag_clip_label), currentText()));
            Toast.makeText(this, R.string.toast_copied, Toast.LENGTH_SHORT).show();
        });
        Button share = new Button(this);
        share.setText(R.string.action_share);
        share.setOnClickListener(v -> {
            Intent i = new Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, currentText());
            startActivity(Intent.createChooser(i, getString(R.string.diag_send_report)));
        });
        for (Button bt : new Button[] {refresh, reset, copy, share}) {
            row.addView(bt, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        }
        col.addView(row);
        setContentView(col);
        render();

        // The navigation report reads settings and runs getprop: off the main thread, and only once.
        new Thread(() -> {
            String r;
            try {
                r = collect();
            } catch (RuntimeException e) {
                r = getString(R.string.diag_report_failed, e.toString());
            }
            final String text = r;
            runOnUiThread(() -> {
                report = text;
                if (!perfMode) render();
            });
        }, "drift-diagnostics").start();
    }

    @Override protected void onResume() {
        super.onResume();
        if (body != null) render();
    }

    private TextView tab(int textRes, boolean perf) {
        TextView t = new TextView(this);
        t.setText(textRes);
        t.setTextSize(15);
        t.setGravity(android.view.Gravity.CENTER);
        t.setPadding(0, Math.round(10 * d), 0, Math.round(10 * d));
        t.setOnClickListener(v -> {
            perfMode = perf;
            render();
        });
        return t;
    }

    private void styleTab(TextView t, boolean on) {
        android.graphics.drawable.GradientDrawable g = new android.graphics.drawable.GradientDrawable();
        g.setCornerRadius(Math.round(10 * d));
        g.setColor(on ? pal.accent : pal.card);
        t.setBackground(g);
        t.setTextColor(on ? pal.ink : pal.text);
    }

    private String currentText() {
        return perfMode ? perfText() : report;
    }

    private void render() {
        styleTab(tabPerf, perfMode);
        styleTab(tabNav, !perfMode);
        reset.setVisibility(perfMode ? android.view.View.VISIBLE : android.view.View.GONE);
        refresh.setVisibility(perfMode ? android.view.View.VISIBLE : android.view.View.GONE);
        body.setText(perfMode ? perfText() : (report.isEmpty() ? getString(R.string.diag_collecting) : report));
    }

    private static String ms(long nanos) {
        return String.format(Locale.ROOT, "%.1f", nanos / 1_000_000.0);
    }

    /** Snapshot of PerfStats: counters only, nothing is measured while this screen is closed. */
    private String perfText() {
        StringBuilder sb = new StringBuilder();
        sb.append(getString(R.string.perf_intro, ms(PerfStats.FRAME_BUDGET_NS))).append("\n\n");
        for (PerfStats.Stat st : PerfStats.ALL) {
            long[] v = st.snapshot();
            sb.append(getString(st.labelRes)).append('\n');
            if (v[0] == 0) {
                sb.append("  ").append(getString(R.string.perf_no_samples)).append("\n\n");
                continue;
            }
            sb.append("  ").append(getString(R.string.perf_row, v[0], ms(v[1]), ms(v[2]), ms(v[3]), v[4])).append("\n\n");
        }
        return sb.toString();
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

        section(sb, getString(R.string.diag_sec_device));
        kv(sb, "manufacturer / brand", Build.MANUFACTURER + " / " + Build.BRAND);
        kv(sb, "model / device", Build.MODEL + " / " + Build.DEVICE);
        kv(sb, "android", Build.VERSION.RELEASE + " (API " + Build.VERSION.SDK_INT + "), patch " + Build.VERSION.SECURITY_PATCH);
        kv(sb, "display id", Build.DISPLAY);
        kv(sb, "fingerprint", Build.FINGERPRINT);
        sb.append("vendor properties (emui/harmony/hw/honor/magic):\n").append(vendorProps());

        section(sb, getString(R.string.diag_sec_nav));
        kv(sb, "Settings.Secure navigation_mode", Settings.Secure.getInt(getContentResolver(), "navigation_mode", -1)
            + "  (0 = 3 buttons, 1 = 2 buttons, 2 = gestures, -1 = not set here)");
        Integer cfgMode = systemInt("config_navBarInteractionMode");
        kv(sb, "config_navBarInteractionMode", cfgMode == null ? "not exposed" : cfgMode);
        sb.append("settings with nav/gesture/swipe/recent/launcher in the name:\n").append(settingsDump());

        section(sb, getString(R.string.diag_sec_home));
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

        section(sb, getString(R.string.diag_sec_recents));
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

        section(sb, getString(R.string.diag_sec_access));
        kv(sb, "Drift previews service enabled", ThumbService.isEnabled(this));
        kv(sb, "enabled services", Settings.Secure.getString(getContentResolver(), Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES));

        section(sb, getString(R.string.diag_sec_reading));
        sb.append(verdict(ours, defPkg, recentsPkg, quickstepInRecentsPkg, !qs.isEmpty()));
        sb.append('\n').append(getString(R.string.diag_footer)).append('\n');
        return sb.toString();
    }

    private String verdict(boolean ours, String defPkg, String recentsPkg, boolean quickstepInRecentsPkg, boolean anyQuickstep) {
        if (!ours) return getString(R.string.diag_verdict_not_home) + "\n";
        if (recentsPkg == null) return getString(R.string.diag_verdict_no_recents) + "\n";
        if (quickstepInRecentsPkg) return getString(R.string.diag_verdict_stock, recentsPkg) + "\n";
        String where = getString(anyQuickstep ? R.string.diag_where_other : R.string.diag_where_none);
        String def = defPkg != null ? getString(R.string.diag_default_home_now, defPkg) : "";
        return getString(R.string.diag_verdict_vendor, recentsPkg, where, recentsPkg, def) + "\n";
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
