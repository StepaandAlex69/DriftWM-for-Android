package dev.drift.launcher;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.ImageDecoder;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

/**
 * "Drift settings": live preview, colour themes, background picture, dock size, language,
 * status of the preview service and of battery / background restrictions, help, diagnostics, About.
 * Appearance changes are visible in the preview immediately; the home screen picks them up when it
 * comes back to the front (see LauncherActivity.onResume). Nothing here changes a system setting by
 * itself: system screens are only opened when the user taps the matching row.
 */
public final class SettingsActivity extends Activity {
    static final String PROJECT_URL = "https://github.com/StepaandAlex69/DriftWM-for-Android";
    private static final int REQ_IMAGE = 7001;

    /** Ready-made looks: {background, dots, interface color}. Names: the theme_* strings. */
    private static final int[][] THEMES = {
        {0xFF101216, 0xFF343944, 0xFF7AA2FF},
        {0xFF0F1A14, 0xFF2C4A3A, 0xFF6FD39A},
        {0xFF1E1215, 0xFF4A2A33, 0xFFFF8A65},
        {0xFF0B1A24, 0xFF22435A, 0xFF4FC3F7},
        {0xFF17121F, 0xFF3E3358, 0xFFB388FF},
        {0xFFF3F1EA, 0xFFCFCBBE, 0xFF2F5BD9},
    };
    private static final int[] THEME_NAMES = {
        R.string.theme_midnight, R.string.theme_forest, R.string.theme_sunset,
        R.string.theme_ocean, R.string.theme_violet, R.string.theme_paper,
    };

    private float d;
    private Appearance a;
    private Appearance.Palette pal;
    private ScrollView scroll;
    private LinearLayout root;
    private LinearLayout content;
    private PreviewView preview;
    private int previewGen;
    private long previewRev = -1;
    private String statePath;

    @Override protected void attachBaseContext(android.content.Context base) {
        super.attachBaseContext(LocaleHelper.wrap(base));
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        d = getResources().getDisplayMetrics().density;
        statePath = new File(getFilesDir(), "workspace.json").getAbsolutePath();
        root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        preview = new PreviewView(this);
        LinearLayout.LayoutParams pp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(168));
        pp.setMargins(dp(16), dp(12), dp(16), dp(4));
        root.addView(preview, pp);
        scroll = new ScrollView(this);
        scroll.setVerticalScrollBarEnabled(false);
        scroll.setFillViewport(false);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        scroll.addView(content, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        root.addView(scroll, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        root.setOnApplyWindowInsetsListener((v, wi) -> {
            android.graphics.Insets b = wi.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(b.left, b.top, b.right, b.bottom);
            return WindowInsets.CONSUMED;
        });
        setContentView(root);
        Native.ensureInit(statePath);
        rebuild();
    }

    @Override protected void onResume() {
        super.onResume();
        if (content != null) rebuild(); // statuses may have changed in the system screens
    }

    // ---- page ------------------------------------------------------------------

    private void rebuild() {
        a = Appearance.read(this);
        pal = a.pal;
        root.setBackgroundColor(pal.base);
        updatePreview();
        try {
            WindowInsetsController ic = getWindow().getInsetsController();
            if (ic != null) {
                int bars = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                ic.setSystemBarsAppearance(pal.light ? bars : 0, bars);
            }
        } catch (RuntimeException ignored) {
            // cosmetic only
        }
        int keep = scroll.getScrollY();
        content.removeAllViews();
        int p = dp(16);
        content.setPadding(p, dp(8), p, dp(24));

        content.addView(text(getString(R.string.settings_title), 22, pal.text, true), lp(0, 4));
        content.addView(caption(getString(R.string.settings_live_hint)), lp(0, 8));

        // --- Themes ---
        content.addView(section(getString(R.string.section_themes)), lp(12, 6));
        content.addView(themeGrid(), lp(0, 8));

        // --- Background ---
        content.addView(section(getString(R.string.section_background)), lp(12, 6));
        boolean image = a.source == Appearance.SOURCE_IMAGE;
        content.addView(segmented(getString(R.string.bg_colors), !image, () -> {
            Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_COLORS);
            rebuild();
        }, getString(R.string.bg_image), image, () -> {
            if (Appearance.hasImage(this)) {
                Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_IMAGE);
                rebuild();
            } else {
                pickImage();
            }
        }), lp(0, 8));

        if (!image) {
            content.addView(colorRow(getString(R.string.bg_color), a.bg, Appearance.DEFAULT_BG, Appearance.K_BG), lp(0, 8));
            content.addView(colorRow(getString(R.string.dots_color), a.dot, Appearance.DEFAULT_DOT, Appearance.K_DOT), lp(0, 8));
        } else {
            content.addView(row(getString(R.string.bg_picture),
                getString(Appearance.hasImage(this) ? R.string.bg_picture_change : R.string.bg_picture_none), null,
                v -> pickImage()), lp(0, 8));
            boolean tiled = a.imageMode == Appearance.IMAGE_INFINITE;
            content.addView(segmented(getString(R.string.bg_static), !tiled, () -> {
                Appearance.putInt(this, Appearance.K_IMAGE_MODE, Appearance.IMAGE_STATIC);
                rebuild();
            }, getString(R.string.bg_infinite), tiled, () -> {
                Appearance.putInt(this, Appearance.K_IMAGE_MODE, Appearance.IMAGE_INFINITE);
                rebuild();
            }), lp(0, 6));
            content.addView(caption(getString(tiled ? R.string.bg_infinite_hint : R.string.bg_static_hint)), lp(0, 8));
            content.addView(row(getString(R.string.bg_remove), getString(R.string.bg_remove_sub), null, v -> {
                //noinspection ResultOfMethodCallIgnored
                Appearance.imageFile(this).delete();
                Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_COLORS);
                rebuild();
            }), lp(0, 8));
        }

        // --- Interface ---
        content.addView(section(getString(R.string.section_interface)), lp(12, 6));
        content.addView(colorRow(getString(R.string.ui_color), a.ui, Appearance.DEFAULT_UI, Appearance.K_UI), lp(0, 6));
        content.addView(caption(getString(R.string.ui_color_hint)), lp(0, 8));
        content.addView(dockRow(), lp(0, 4));
        content.addView(caption(getString(R.string.dock_count_hint)), lp(0, 8));
        content.addView(row(getString(R.string.language), languageName(), null, v -> chooseLanguage()), lp(0, 8));
        content.addView(row(getString(R.string.reset_appearance), getString(R.string.reset_appearance_sub), null, v ->
            new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
                .setMessage(R.string.reset_appearance_confirm)
                .setPositiveButton(R.string.action_reset, (dlg, w) -> {
                    Appearance.reset(this);
                    rebuild();
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show()), lp(0, 8));

        // --- Status ---
        content.addView(section(getString(R.string.section_status)), lp(12, 6));
        addStatusRows();

        // --- Help ---
        content.addView(section(getString(R.string.section_help)), lp(12, 6));
        content.addView(row(getString(R.string.help_gestures), getString(R.string.help_gestures_sub), null, v ->
            showHelp(R.string.help_gestures, new int[] {R.string.hg_t1, R.string.hg_t2, R.string.hg_t3, R.string.hg_t4,
                R.string.hg_t5, R.string.hg_t6}, new int[] {R.string.hg_b1, R.string.hg_b2, R.string.hg_b3,
                R.string.hg_b4, R.string.hg_b5, R.string.hg_b6})), lp(0, 8));
        content.addView(row(getString(R.string.help_folders), getString(R.string.help_folders_sub), null, v ->
            showHelp(R.string.help_folders, new int[] {R.string.hf_t1, R.string.hf_t2, R.string.hf_t3, R.string.hf_t4,
                R.string.hf_t5, R.string.hf_t6, R.string.hf_t7}, new int[] {R.string.hf_b1, R.string.hf_b2,
                R.string.hf_b3, R.string.hf_b4, R.string.hf_b5, R.string.hf_b6, R.string.hf_b7})), lp(0, 8));

        // --- Diagnostics ---
        content.addView(section(getString(R.string.section_diagnostics)), lp(12, 6));
        content.addView(row(getString(R.string.diagnostics), getString(R.string.diagnostics_sub), null, v ->
            startActivity(new Intent(this, DiagnosticsActivity.class))), lp(0, 8));

        // --- About ---
        content.addView(section(getString(R.string.section_about)), lp(12, 6));
        content.addView(row(getString(R.string.about), PROJECT_URL.replace("https://", ""), null, v -> openProject()), lp(0, 8));

        scroll.post(() -> scroll.scrollTo(0, keep));
    }

    // ---- preview ---------------------------------------------------------------

    /** Pushes the current settings into the preview; a picture is decoded off the main thread. */
    private void updatePreview() {
        final int slots = Native.dockCount();
        if (a.source != Appearance.SOURCE_IMAGE || !Appearance.hasImage(this)) {
            previewGen++;
            preview.set(a, slots);
            return;
        }
        final long rev = Appearance.rev(this);
        if (rev == previewRev && preview.hasBitmap()) {
            preview.setSlots(slots);
            preview.setColors(a);
            return;
        }
        preview.set(a, slots); // colours at once, the picture follows
        final int gen = ++previewGen;
        final android.content.Context app = getApplicationContext();
        new Thread(() -> {
            Appearance full = Appearance.readWithBitmap(app);
            runOnUiThread(() -> {
                if (isDestroyed() || gen != previewGen) return;
                previewRev = rev;
                preview.set(full, Native.dockCount());
            });
        }, "drift-preview").start();
    }

    /**
     * Miniature of the home screen. Every Paint, Path and Rect is created once; set() rebuilds only
     * the shader and the layout numbers, and onDraw allocates nothing.
     */
    private final class PreviewView extends View {
        private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint line = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint pic = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.FILTER_BITMAP_FLAG);
        private final RectF box = new RectF();
        private final RectF tmp = new RectF();
        private final android.graphics.Matrix m = new android.graphics.Matrix();
        private final android.graphics.DashPathEffect dash = new android.graphics.DashPathEffect(new float[] {dp(3), dp(3)}, 0f);
        private Appearance ap = Appearance.defaults();
        private android.graphics.BitmapShader shader;
        private int slots = 5;
        private final float[] cards = new float[12]; // 3 cards: x, y, w, h
        private final float[] dock = new float[4]; // pill x, y, w, h
        private float slot;

        PreviewView(android.content.Context c) {
            super(c);
            line.setStyle(Paint.Style.STROKE);
            line.setStrokeWidth(dp(1));
        }

        boolean hasBitmap() {
            return shader != null;
        }

        void setSlots(int n) {
            slots = n;
            layoutDock();
            invalidate();
        }

        void setColors(Appearance n) {
            ap = n;
            invalidate();
        }

        void set(Appearance n, int slotCount) {
            ap = n;
            slots = slotCount;
            shader = null;
            if (n.source == Appearance.SOURCE_IMAGE && n.bitmap != null) {
                shader = new android.graphics.BitmapShader(n.bitmap,
                    n.imageMode == Appearance.IMAGE_INFINITE ? Shader.TileMode.REPEAT : Shader.TileMode.CLAMP,
                    n.imageMode == Appearance.IMAGE_INFINITE ? Shader.TileMode.REPEAT : Shader.TileMode.CLAMP);
                fitShader();
            }
            layoutDock();
            invalidate();
        }

        private void fitShader() {
            if (shader == null || getWidth() == 0) return;
            Bitmap b = ap.bitmap;
            if (ap.imageMode == Appearance.IMAGE_INFINITE) {
                float k = Math.min(1f, dp(70) / (float) Math.max(b.getWidth(), b.getHeight()));
                m.setScale(k, k);
            } else {
                float k = Math.max(getWidth() / (float) b.getWidth(), getHeight() / (float) b.getHeight());
                m.setScale(k, k);
                m.postTranslate((getWidth() - b.getWidth() * k) / 2f, (getHeight() - b.getHeight() * k) / 2f);
            }
            shader.setLocalMatrix(m);
            pic.setShader(shader); // set again after every matrix change
        }

        @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
            float cw = w * 0.2f;
            float ch = h * 0.34f;
            cards[0] = w * 0.08f; cards[1] = h * 0.14f; cards[2] = cw; cards[3] = ch;
            cards[4] = w * 0.36f; cards[5] = h * 0.08f; cards[6] = cw; cards[7] = ch;
            cards[8] = w * 0.62f; cards[9] = h * 0.2f; cards[10] = cw; cards[11] = ch;
            layoutDock();
            fitShader();
        }

        private void layoutDock() {
            float w = getWidth();
            float h = getHeight();
            if (w == 0) return;
            float margin = w * 0.04f;
            float btn = h * 0.2f;
            float avail = w - margin * 2 - btn - margin - dp(6);
            slot = Math.min(h * 0.17f, (avail - dp(2) * (slots - 1)) / slots);
            float pw = slot * slots + dp(2) * (slots - 1) + dp(6);
            dock[0] = margin; dock[2] = pw; dock[3] = slot + dp(6); dock[1] = h - margin - dock[3];
        }

        @Override protected void onDraw(Canvas c) {
            Appearance.Palette pl = ap.pal;
            float w = getWidth();
            float h = getHeight();
            float r = dp(16);
            box.set(0, 0, w, h);
            if (shader != null) {
                c.drawRoundRect(box, r, r, pic);
            } else {
                fill.setColor(ap.source == Appearance.SOURCE_IMAGE ? ap.avg : ap.bg);
                c.drawRoundRect(box, r, r, fill);
                if (ap.source != Appearance.SOURCE_IMAGE) {
                    fill.setColor(ap.dot);
                    float step = dp(14);
                    for (float y = step / 2; y < h; y += step) {
                        for (float x = step / 2; x < w; x += step) c.drawCircle(x, y, dp(1), fill);
                    }
                }
            }
            for (int i = 0; i < 3; i++) {
                float x = cards[i * 4], y = cards[i * 4 + 1], cw = cards[i * 4 + 2], ch = cards[i * 4 + 3];
                tmp.set(x, y, x + cw, y + ch);
                fill.setColor(0x40000000);
                box.set(x - 1, y + 1, x + cw + 1, y + ch + 3);
                c.drawRoundRect(box, dp(7), dp(7), fill);
                fill.setColor(pl.card);
                c.drawRoundRect(tmp, dp(6), dp(6), fill);
                line.setColor(i == 1 ? pl.accent : pl.stroke);
                line.setStrokeWidth(i == 1 ? dp(2) : dp(1));
                c.drawRoundRect(tmp, dp(6), dp(6), line);
                fill.setColor(pl.placeholder);
                c.drawCircle(x + cw / 2f, y + ch * 0.4f, cw * 0.2f, fill);
                fill.setColor(pl.textDim);
                box.set(x + cw * 0.2f, y + ch * 0.78f, x + cw * 0.8f, y + ch * 0.78f + dp(3));
                c.drawRoundRect(box, dp(2), dp(2), fill);
            }
            // dock
            tmp.set(dock[0], dock[1], dock[0] + dock[2], dock[1] + dock[3]);
            fill.setColor(pl.pill);
            c.drawRoundRect(tmp, dock[3] * 0.4f, dock[3] * 0.4f, fill);
            line.setStrokeWidth(dp(1));
            line.setColor(pl.stroke);
            c.drawRoundRect(tmp, dock[3] * 0.4f, dock[3] * 0.4f, line);
            for (int i = 0; i < slots; i++) {
                float sx = dock[0] + dp(3) + i * (slot + dp(2));
                float sy = dock[1] + dp(3);
                if (i < 3) {
                    fill.setColor(pl.placeholder);
                    c.drawCircle(sx + slot / 2f, sy + slot / 2f, slot * 0.42f, fill);
                } else {
                    line.setColor(pl.textDim);
                    line.setPathEffect(dash);
                    box.set(sx + slot * 0.1f, sy + slot * 0.1f, sx + slot * 0.9f, sy + slot * 0.9f);
                    c.drawRoundRect(box, slot * 0.25f, slot * 0.25f, line);
                    line.setPathEffect(null);
                }
            }
            float bx = w - w * 0.04f - h * 0.1f;
            fill.setColor(pl.accent);
            c.drawCircle(bx, dock[1] + dock[3] / 2f, h * 0.1f, fill);
        }
    }

    // ---- themes ----------------------------------------------------------------

    private LinearLayout themeGrid() {
        LinearLayout grid = new LinearLayout(this);
        grid.setOrientation(LinearLayout.VERTICAL);
        for (int rowIdx = 0; rowIdx < 2; rowIdx++) {
            LinearLayout r = new LinearLayout(this);
            r.setOrientation(LinearLayout.HORIZONTAL);
            for (int col = 0; col < 3; col++) {
                final int[] t = THEMES[rowIdx * 3 + col];
                boolean on = a.source == Appearance.SOURCE_COLORS && a.bg == t[0] && a.dot == t[1] && a.ui == t[2];
                TextView chip = text(getString(THEME_NAMES[rowIdx * 3 + col]), 13, t[2], true);
                chip.setGravity(Gravity.CENTER);
                chip.setSingleLine(true);
                chip.setPadding(dp(4), dp(14), dp(4), dp(14));
                GradientDrawable g = rounded(t[0], 12);
                g.setStroke(on ? dp(2.5f) : dp(1), on ? t[2] : t[1]);
                chip.setBackground(g);
                chip.setOnClickListener(v -> {
                    Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_COLORS);
                    Appearance.putInt(this, Appearance.K_BG, t[0]);
                    Appearance.putInt(this, Appearance.K_DOT, t[1]);
                    Appearance.putInt(this, Appearance.K_UI, t[2]);
                    rebuild();
                });
                LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
                cp.setMargins(dp(3), dp(3), dp(3), dp(3));
                r.addView(chip, cp);
            }
            grid.addView(r);
        }
        return grid;
    }

    // ---- dock size -------------------------------------------------------------

    private LinearLayout dockRow() {
        final int n = Native.dockCount();
        LinearLayout controls = new LinearLayout(this);
        controls.setOrientation(LinearLayout.HORIZONTAL);
        controls.setGravity(Gravity.CENTER_VERTICAL);
        TextView minus = stepButton("\u2212", n > 3, () -> changeDock(n - 1));
        TextView value = text(String.valueOf(n), 18, pal.text, true);
        value.setGravity(Gravity.CENTER);
        TextView plus = stepButton("+", n < 8, () -> changeDock(n + 1));
        controls.addView(minus, new LinearLayout.LayoutParams(dp(40), dp(40)));
        controls.addView(value, new LinearLayout.LayoutParams(dp(36), ViewGroup.LayoutParams.WRAP_CONTENT));
        controls.addView(plus, new LinearLayout.LayoutParams(dp(40), dp(40)));
        return row(getString(R.string.dock_count), getString(R.string.dock_count_sub), controls, null);
    }

    private TextView stepButton(String label, boolean enabled, Runnable click) {
        TextView t = text(label, 20, enabled ? pal.ink : pal.textDim, true);
        t.setGravity(Gravity.CENTER);
        t.setBackground(rounded(enabled ? pal.accent : pal.pill, 20));
        if (enabled) t.setOnClickListener(v -> click.run());
        return t;
    }

    /** Growing is always safe. Shrinking asks first when the slots being removed are occupied. */
    private void changeDock(int count) {
        int current = Native.dockCount();
        if (count == current || count < 3 || count > 8) return;
        if (count > current) {
            applyDock(count);
            return;
        }
        int[] plan = Native.dockPlanResize(count);
        if (plan == null || plan[0] == 0) {
            applyDock(count);
            return;
        }
        // plan: {occupied slots being removed, items moved into free slots, items that would be lost, apps lost}
        String msg = plan[2] > 0
            ? getString(R.string.dock_shrink_loses, plan[0], plan[1], plan[2], plan[3])
            : getString(R.string.dock_shrink_moves, plan[0], plan[1]);
        new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle(R.string.dock_shrink_title)
            .setMessage(msg)
            .setPositiveButton(R.string.action_shrink, (dlg, w) -> applyDock(count))
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    private void applyDock(int count) {
        Native.dockSetCount(count);
        // Saved with the layout; the file write happens off the main thread.
        final String json = Native.snapshot();
        if (json != null) new Thread(() -> Native.writeSnapshot(json), "drift-save-dock").start();
        rebuild();
    }

    // ---- language --------------------------------------------------------------

    private String languageName() {
        String f = LocaleHelper.forced(this);
        if (LocaleHelper.EN.equals(f)) return getString(R.string.lang_english);
        if (LocaleHelper.RU.equals(f)) return getString(R.string.lang_russian);
        return getString(R.string.lang_system);
    }

    private void chooseLanguage() {
        final String[] values = {LocaleHelper.SYSTEM, LocaleHelper.EN, LocaleHelper.RU};
        String[] names = {getString(R.string.lang_system), getString(R.string.lang_english), getString(R.string.lang_russian)};
        int checked = 0;
        for (int i = 0; i < values.length; i++) if (values[i].equals(LocaleHelper.forced(this))) checked = i;
        new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle(R.string.language)
            .setSingleChoiceItems(names, checked, (dlg, which) -> {
                dlg.dismiss();
                if (!values[which].equals(LocaleHelper.forced(this))) {
                    LocaleHelper.setForced(this, values[which]);
                    recreate(); // the launcher notices the change in onResume
                }
            })
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    // ---- status ----------------------------------------------------------------

    private void addStatusRows() {
        boolean svc = ThumbService.isEnabled(this);
        content.addView(row(getString(R.string.status_preview_service),
            getString(svc ? R.string.status_on : R.string.status_off_tap), null,
            v -> openSettings(new Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS))), lp(0, 8));

        android.os.PowerManager pm = (android.os.PowerManager) getSystemService(POWER_SERVICE);
        boolean unrestricted = pm != null && pm.isIgnoringBatteryOptimizations(getPackageName());
        content.addView(row(getString(R.string.status_battery),
            getString(unrestricted ? R.string.status_battery_unrestricted : R.string.status_battery_optimized), null,
            v -> openSettings(new Intent(android.provider.Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))), lp(0, 8));

        android.app.ActivityManager am = (android.app.ActivityManager) getSystemService(ACTIVITY_SERVICE);
        boolean bgRestricted = am != null && am.isBackgroundRestricted();
        content.addView(row(getString(R.string.status_background),
            getString(bgRestricted ? R.string.status_bg_restricted : R.string.status_bg_free), null,
            v -> openSettings(appInfoIntent())), lp(0, 8));

        content.addView(row(getString(R.string.status_standby), standbyName(), null,
            v -> openSettings(appInfoIntent())), lp(0, 8));

        boolean home = false;
        try {
            android.app.role.RoleManager rm = getSystemService(android.app.role.RoleManager.class);
            home = rm != null && rm.isRoleHeld(android.app.role.RoleManager.ROLE_HOME);
        } catch (RuntimeException ignored) {
            // unknown: shown as "not selected"
        }
        content.addView(row(getString(R.string.status_home_app),
            getString(home ? R.string.status_home_yes : R.string.status_home_no), null,
            v -> openSettings(new Intent(android.provider.Settings.ACTION_MANAGE_DEFAULT_APPS_SETTINGS))), lp(0, 8));

        content.addView(row(getString(R.string.status_app_info), getString(R.string.status_app_info_sub), null,
            v -> openSettings(appInfoIntent())), lp(0, 8));
        content.addView(caption(getString(R.string.status_hint)), lp(0, 8));
    }

    private Intent appInfoIntent() {
        return new Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + getPackageName()));
    }

    /** Only ever opens a system screen the user tapped; never changes anything itself. */
    private void openSettings(Intent i) {
        try {
            startActivity(i);
        } catch (ActivityNotFoundException | SecurityException e) {
            try {
                startActivity(appInfoIntent());
            } catch (ActivityNotFoundException | SecurityException e2) {
                Toast.makeText(this, R.string.toast_cant_open_settings, Toast.LENGTH_LONG).show();
            }
        }
    }

    private String standbyName() {
        try {
            android.app.usage.UsageStatsManager um = (android.app.usage.UsageStatsManager) getSystemService(USAGE_STATS_SERVICE);
            int b = um == null ? -1 : um.getAppStandbyBucket();
            if (b <= 10) return getString(R.string.bucket_active);
            if (b <= 20) return getString(R.string.bucket_working);
            if (b <= 30) return getString(R.string.bucket_frequent);
            if (b <= 40) return getString(R.string.bucket_rare);
            return getString(R.string.bucket_restricted);
        } catch (RuntimeException e) {
            return getString(R.string.status_unknown);
        }
    }

    // ---- help ------------------------------------------------------------------

    private void showHelp(int titleRes, int[] heads, int[] bodies) {
        android.text.SpannableStringBuilder sb = new android.text.SpannableStringBuilder();
        for (int i = 0; i < heads.length; i++) {
            int start = sb.length();
            sb.append(getString(heads[i]));
            sb.setSpan(new android.text.style.StyleSpan(Typeface.BOLD), start, sb.length(),
                android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            sb.append("\n").append(getString(bodies[i])).append("\n\n");
        }
        TextView tv = new TextView(this);
        tv.setText(sb);
        tv.setTextSize(15);
        tv.setPadding(dp(20), dp(8), dp(20), dp(8));
        ScrollView sv = new ScrollView(this);
        sv.addView(tv);
        new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle(titleRes)
            .setView(sv)
            .setPositiveButton(android.R.string.ok, null)
            .show();
    }

    private void openProject() {
        Uri uri = Uri.parse(PROJECT_URL);
        // Ask for the default browser itself, so an installed GitHub app does not take the link.
        Intent browser = Intent.makeMainSelectorActivity(Intent.ACTION_MAIN, Intent.CATEGORY_APP_BROWSER);
        browser.setData(uri);
        browser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            startActivity(browser);
            return;
        } catch (ActivityNotFoundException | SecurityException ignored) {
            // no app registered as the browser: fall through to a plain VIEW
        }
        try {
            startActivity(new Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.toast_no_browser, Toast.LENGTH_LONG).show();
        }
    }

    // ---- picture ---------------------------------------------------------------

    private void pickImage() {
        Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("image/*");
        try {
            startActivityForResult(i, REQ_IMAGE);
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.toast_no_picker, Toast.LENGTH_LONG).show();
        }
    }

    @SuppressWarnings("deprecation")
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != REQ_IMAGE || result != RESULT_OK || data == null || data.getData() == null) return;
        final Uri uri = data.getData();
        Toast.makeText(this, R.string.toast_loading_picture, Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            boolean ok = importImage(uri);
            runOnUiThread(() -> {
                if (isDestroyed()) return;
                if (ok) {
                    Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_IMAGE);
                    rebuild();
                } else {
                    Toast.makeText(this, R.string.toast_bad_picture, Toast.LENGTH_LONG).show();
                }
            });
        }, "drift-image-import").start();
    }

    /**
     * Copies the picked image into the app's own storage, shrunk to a size that is enough for any
     * screen. After this Drift never needs the original Uri or any storage permission again.
     */
    private boolean importImage(Uri uri) {
        File tmp = new File(getFilesDir(), Appearance.IMAGE_FILE + ".tmp");
        try {
            Bitmap b = ImageDecoder.decodeBitmap(ImageDecoder.createSource(getContentResolver(), uri), (dec, info, src) -> {
                dec.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
                int w = info.getSize().getWidth();
                int h = info.getSize().getHeight();
                int m = Math.max(w, h);
                if (m > Appearance.STATIC_MAX_SIDE) {
                    float k = (float) Appearance.STATIC_MAX_SIDE / m;
                    dec.setTargetSize(Math.max(1, Math.round(w * k)), Math.max(1, Math.round(h * k)));
                }
            });
            boolean saved;
            try (FileOutputStream out = new FileOutputStream(tmp)) {
                saved = b.hasAlpha()
                    ? b.compress(Bitmap.CompressFormat.PNG, 100, out)
                    : b.compress(Bitmap.CompressFormat.JPEG, 92, out);
            }
            b.recycle();
            if (!saved) return false;
            File dest = Appearance.imageFile(this);
            //noinspection ResultOfMethodCallIgnored
            dest.delete();
            return tmp.renameTo(dest);
        } catch (IOException | RuntimeException | OutOfMemoryError e) {
            return false;
        } finally {
            if (tmp.exists()) {
                //noinspection ResultOfMethodCallIgnored
                tmp.delete();
            }
        }
    }

    // ---- widgets ---------------------------------------------------------------

    private int dp(float v) {
        return Math.round(v * d);
    }

    private static LinearLayout.LayoutParams lp(int topDp, int bottomDp) {
        LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        float d = android.content.res.Resources.getSystem().getDisplayMetrics().density;
        p.topMargin = Math.round(topDp * d);
        p.bottomMargin = Math.round(bottomDp * d);
        return p;
    }

    private TextView text(String s, float size, int color, boolean bold) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(size);
        t.setTextColor(color);
        if (bold) t.setTypeface(Typeface.DEFAULT_BOLD);
        return t;
    }

    private TextView section(String s) {
        return text(s, 13, pal.accent, true);
    }

    private TextView caption(String s) {
        return text(s, 13, pal.textDim, false);
    }

    private GradientDrawable rounded(int color, float radiusDp) {
        GradientDrawable g = new GradientDrawable();
        g.setColor(color);
        g.setCornerRadius(dp(radiusDp));
        return g;
    }

    private LinearLayout row(String title, String subtitle, View trailing, View.OnClickListener click) {
        LinearLayout r = new LinearLayout(this);
        r.setOrientation(LinearLayout.HORIZONTAL);
        r.setGravity(Gravity.CENTER_VERTICAL);
        r.setPadding(dp(16), dp(13), dp(16), dp(13));
        r.setBackground(new RippleDrawable(ColorStateList.valueOf((pal.text & 0x00FFFFFF) | 0x30000000),
            rounded(pal.card, 14), null));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(text(title, 16, pal.text, false));
        if (subtitle != null) col.addView(text(subtitle, 13, pal.textDim, false));
        r.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        if (trailing != null) r.addView(trailing);
        r.setOnClickListener(click);
        return r;
    }

    private LinearLayout colorRow(String title, int color, int defaultColor, String key) {
        View swatch = new View(this);
        GradientDrawable g = new GradientDrawable();
        g.setShape(GradientDrawable.OVAL);
        g.setColor(color);
        g.setStroke(dp(1.5f), pal.stroke);
        swatch.setBackground(g);
        swatch.setLayoutParams(new LinearLayout.LayoutParams(dp(30), dp(30)));
        String hex = String.format(java.util.Locale.ROOT, "#%06X", color & 0xFFFFFF);
        return row(title, hex, swatch, v ->
            ColorPickerDialog.show(this, title, color, defaultColor, c -> {
                Appearance.putInt(this, key, c);
                rebuild();
            }));
    }

    private LinearLayout segmented(String left, boolean leftOn, Runnable onLeft, String right, boolean rightOn, Runnable onRight) {
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.HORIZONTAL);
        box.setPadding(dp(4), dp(4), dp(4), dp(4));
        box.setBackground(rounded(pal.card, 14));
        box.addView(segment(left, leftOn, onLeft), new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        box.addView(segment(right, rightOn, onRight), new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        return box;
    }

    private TextView segment(String label, boolean on, Runnable click) {
        TextView t = text(label, 15, on ? pal.ink : pal.text, on);
        t.setGravity(Gravity.CENTER);
        t.setPadding(dp(8), dp(10), dp(8), dp(10));
        t.setBackground(on ? rounded(pal.accent, 11) : null);
        t.setOnClickListener(v -> {
            if (!on) click.run();
        });
        return t;
    }
}
