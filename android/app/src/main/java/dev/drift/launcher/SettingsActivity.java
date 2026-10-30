package dev.drift.launcher;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
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
 * "Drift settings": colours, background picture, About. Everything here is presentation; the
 * home screen picks the new look up when it comes back to the front (see LauncherActivity.onResume).
 * Reachable only from the "Drift settings" row of the app list.
 */
public final class SettingsActivity extends Activity {
    static final String PROJECT_URL = "https://github.com/StepaandAlex69/DriftWM-for-Android";
    private static final int REQ_IMAGE = 7001;

    private float d;
    private Appearance a;
    private Appearance.Palette pal;
    private ScrollView scroll;
    private LinearLayout root;
    private LinearLayout content;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        d = getResources().getDisplayMetrics().density;
        root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        scroll = new ScrollView(this);
        scroll.setVerticalScrollBarEnabled(false);
        scroll.setFillViewport(false);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        scroll.addView(content, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        root.addView(scroll, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.setOnApplyWindowInsetsListener((v, wi) -> {
            android.graphics.Insets b = wi.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(b.left, b.top, b.right, b.bottom);
            return WindowInsets.CONSUMED;
        });
        setContentView(root);
        rebuild();
    }

    // ---- page ------------------------------------------------------------------

    private void rebuild() {
        a = Appearance.read(this);
        pal = a.pal;
        root.setBackgroundColor(pal.base);
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
        content.setPadding(p, dp(16), p, dp(24));

        TextView title = text("Drift settings", 22, pal.text, true);
        content.addView(title, lp(0, 4));
        content.addView(caption("Changes apply when you go back to the home screen."), lp(0, 8));

        // --- Background ---
        content.addView(section("Background"), lp(12, 6));
        boolean image = a.source == Appearance.SOURCE_IMAGE;
        content.addView(segmented("Colors", !image, () -> {
            Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_COLORS);
            rebuild();
        }, "Image", image, () -> {
            if (Appearance.hasImage(this)) {
                Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_IMAGE);
                rebuild();
            } else {
                pickImage();
            }
        }), lp(0, 8));

        if (!image) {
            content.addView(colorRow("Background color", a.bg, Appearance.DEFAULT_BG, Appearance.K_BG), lp(0, 8));
            content.addView(colorRow("Dots color", a.dot, Appearance.DEFAULT_DOT, Appearance.K_DOT), lp(0, 8));
        } else {
            content.addView(row("Picture", Appearance.hasImage(this) ? "Tap to choose another" : "Not chosen", null,
                v -> pickImage()), lp(0, 8));
            boolean tiled = a.imageMode == Appearance.IMAGE_INFINITE;
            content.addView(segmented("Static", !tiled, () -> {
                Appearance.putInt(this, Appearance.K_IMAGE_MODE, Appearance.IMAGE_STATIC);
                rebuild();
            }, "Infinite", tiled, () -> {
                Appearance.putInt(this, Appearance.K_IMAGE_MODE, Appearance.IMAGE_INFINITE);
                rebuild();
            }), lp(0, 6));
            content.addView(caption(tiled
                ? "Infinite: the picture repeats forever and is attached to the canvas. It moves and zooms with the cards."
                : "Static: like ordinary wallpaper. It stays where it is while the canvas moves."), lp(0, 8));
            content.addView(row("Remove picture", "Go back to plain colors", null, v -> {
                //noinspection ResultOfMethodCallIgnored
                Appearance.imageFile(this).delete();
                Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_COLORS);
                rebuild();
            }), lp(0, 8));
        }

        // --- Interface ---
        content.addView(section("Interface"), lp(12, 6));
        content.addView(colorRow("Interface color", a.ui, Appearance.DEFAULT_UI, Appearance.K_UI), lp(0, 6));
        content.addView(caption("One color for cards, dock, buttons, text and the app list."), lp(0, 8));
        content.addView(row("Reset appearance", "Default colors, no picture", null, v ->
            new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
                .setMessage("Reset colors and remove the background picture?")
                .setPositiveButton("Reset", (dlg, w) -> {
                    Appearance.reset(this);
                    rebuild();
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show()), lp(0, 8));

        // --- About ---
        content.addView(section("About"), lp(12, 6));
        content.addView(row("About", PROJECT_URL.replace("https://", ""), null, v -> openProject()), lp(0, 8));

        scroll.post(() -> scroll.scrollTo(0, keep));
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
            Toast.makeText(this, "No browser found", Toast.LENGTH_LONG).show();
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
            Toast.makeText(this, "No app to pick a picture", Toast.LENGTH_LONG).show();
        }
    }

    @SuppressWarnings("deprecation")
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != REQ_IMAGE || result != RESULT_OK || data == null || data.getData() == null) return;
        final Uri uri = data.getData();
        Toast.makeText(this, "Loading picture...", Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            boolean ok = importImage(uri);
            runOnUiThread(() -> {
                if (isDestroyed()) return;
                if (ok) {
                    Appearance.putInt(this, Appearance.K_SOURCE, Appearance.SOURCE_IMAGE);
                    rebuild();
                } else {
                    Toast.makeText(this, "Couldn't read that picture", Toast.LENGTH_LONG).show();
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
