package dev.drift.launcher;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import java.io.File;

/**
 * How the home screen looks: background (two colours, or a picture that is either fixed to the
 * screen or tiled across the canvas), dot colour, and one "interface" colour from which every
 * other colour of the UI is derived. Pure presentation: nothing here touches the workspace logic.
 *
 * Stored in SharedPreferences "drift_appearance". Every change bumps {@code rev}, which is how the
 * home screen notices (on resume) that it has to reload. The picture is a private copy in
 * filesDir (see SettingsActivity), so no storage permission and no persistent Uri grant is needed.
 */
final class Appearance {
    static final int DEFAULT_BG = 0xFF101216;
    static final int DEFAULT_DOT = 0xFF343944;
    static final int DEFAULT_UI = 0xFF7AA2FF;

    static final int SOURCE_COLORS = 0;
    static final int SOURCE_IMAGE = 1;
    static final int IMAGE_STATIC = 0;
    static final int IMAGE_INFINITE = 1;

    static final String IMAGE_FILE = "background.img";
    /** A tile can be any size; huge ones are shrunk so tiling stays cheap. */
    private static final int TILE_MAX_SIDE = 1024;
    /** Enough to cover any phone screen when the picture is fixed to the screen. */
    static final int STATIC_MAX_SIDE = 2400;

    private static final String PREFS = "drift_appearance";
    static final String K_BG = "bg";
    static final String K_DOT = "dot";
    static final String K_UI = "ui";
    static final String K_SOURCE = "source";
    static final String K_IMAGE_MODE = "image_mode";
    private static final String K_REV = "rev";

    final int bg;
    final int dot;
    final int ui;
    final int source;
    final int imageMode;
    final Palette pal;
    /** Decoded picture, or null (colours mode, still loading, or unreadable). */
    final Bitmap bitmap;
    /** Mean colour of the picture; used when a tile gets too small to draw. */
    final int avg;
    /** 0..1 brightness behind the status bar, to choose light or dark system-bar icons. */
    final float barLum;

    private Appearance(int bg, int dot, int ui, int source, int imageMode, Bitmap bitmap, int avg, float barLum) {
        this.bg = bg;
        this.dot = dot;
        this.ui = ui;
        this.source = source;
        this.imageMode = imageMode;
        this.pal = Palette.of(ui);
        this.bitmap = bitmap;
        this.avg = avg;
        this.barLum = barLum;
    }

    static Appearance defaults() {
        return new Appearance(DEFAULT_BG, DEFAULT_DOT, DEFAULT_UI, SOURCE_COLORS, IMAGE_STATIC, null, DEFAULT_BG, lum(DEFAULT_BG));
    }

    Appearance withBitmap(Bitmap b, int avg, float barLum) {
        return new Appearance(bg, dot, ui, source, imageMode, b, avg, barLum);
    }

    // ---- storage ---------------------------------------------------------------

    private static SharedPreferences prefs(Context c) {
        return c.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static File imageFile(Context c) {
        return new File(c.getFilesDir(), IMAGE_FILE);
    }

    static boolean hasImage(Context c) {
        return imageFile(c).isFile();
    }

    static long rev(Context c) {
        return prefs(c).getLong(K_REV, 0);
    }

    /** Settings only, no picture yet (cheap; safe on the main thread). */
    static Appearance read(Context c) {
        SharedPreferences p = prefs(c);
        int source = p.getInt(K_SOURCE, SOURCE_COLORS);
        if (source == SOURCE_IMAGE && !hasImage(c)) source = SOURCE_COLORS;
        int mode = p.getInt(K_IMAGE_MODE, IMAGE_STATIC) == IMAGE_INFINITE ? IMAGE_INFINITE : IMAGE_STATIC;
        int bg = opaque(p.getInt(K_BG, DEFAULT_BG));
        return new Appearance(bg, opaque(p.getInt(K_DOT, DEFAULT_DOT)), opaque(p.getInt(K_UI, DEFAULT_UI)),
            source, mode, null, bg, lum(bg));
    }

    /** Settings plus the decoded picture (slow: call off the main thread). */
    static Appearance readWithBitmap(Context c) {
        Appearance a = read(c);
        if (a.source != SOURCE_IMAGE) return a;
        Bitmap b = decode(imageFile(c), a.imageMode);
        if (b == null) return a;
        int avg = 0xFF000000;
        float top = 0.2f;
        try {
            Bitmap one = Bitmap.createScaledBitmap(b, 1, 1, true);
            avg = opaque(one.getPixel(0, 0));
            if (one != b) one.recycle();
            // Brightness of the top strip is what sits behind the status bar.
            int th = Math.max(1, b.getHeight() / 8);
            Bitmap strip = Bitmap.createBitmap(b, 0, 0, b.getWidth(), th);
            Bitmap tiny = Bitmap.createScaledBitmap(strip, 1, 1, true);
            top = lum(tiny.getPixel(0, 0));
            if (tiny != strip) tiny.recycle();
            if (strip != b) strip.recycle();
        } catch (RuntimeException | OutOfMemoryError ignored) {
            // only the status-bar icon colour depends on this
        }
        return a.withBitmap(b, avg, top);
    }

    static void putInt(Context c, String key, int value) {
        SharedPreferences p = prefs(c);
        p.edit().putInt(key, value).putLong(K_REV, p.getLong(K_REV, 0) + 1).apply();
    }

    /** Call after the picture file was replaced or removed. */
    static void touch(Context c) {
        SharedPreferences p = prefs(c);
        p.edit().putLong(K_REV, p.getLong(K_REV, 0) + 1).apply();
    }

    static void reset(Context c) {
        SharedPreferences p = prefs(c);
        long rev = p.getLong(K_REV, 0) + 1;
        p.edit().clear().putLong(K_REV, rev).apply();
        //noinspection ResultOfMethodCallIgnored
        imageFile(c).delete();
    }

    private static Bitmap decode(File f, int mode) {
        try {
            int max = mode == IMAGE_INFINITE ? TILE_MAX_SIDE : STATIC_MAX_SIDE;
            BitmapFactory.Options o = new BitmapFactory.Options();
            o.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(f.getAbsolutePath(), o);
            if (o.outWidth <= 0 || o.outHeight <= 0) return null;
            int side = Math.max(o.outWidth, o.outHeight);
            int sample = 1;
            while (side / (sample * 2) >= max) sample *= 2;
            o.inJustDecodeBounds = false;
            o.inSampleSize = sample;
            o.inPreferredConfig = Bitmap.Config.ARGB_8888;
            Bitmap b = BitmapFactory.decodeFile(f.getAbsolutePath(), o);
            if (b == null) return null;
            int s = Math.max(b.getWidth(), b.getHeight());
            if (s > max) {
                float k = (float) max / s;
                Bitmap scaled = Bitmap.createScaledBitmap(b, Math.max(1, Math.round(b.getWidth() * k)),
                    Math.max(1, Math.round(b.getHeight() * k)), true);
                if (scaled != b) b.recycle();
                b = scaled;
            }
            if (mode == IMAGE_INFINITE) b.setHasMipMap(true); // smooth when zoomed far out
            return b;
        } catch (RuntimeException | OutOfMemoryError e) {
            return null;
        }
    }

    // ---- colour helpers --------------------------------------------------------

    static int opaque(int c) {
        return 0xFF000000 | c;
    }

    static float lum(int c) {
        return (0.299f * ((c >> 16) & 255) + 0.587f * ((c >> 8) & 255) + 0.114f * (c & 255)) / 255f;
    }

    static int mix(int a, int b, float t) {
        int r = Math.round(((a >> 16) & 255) * (1 - t) + ((b >> 16) & 255) * t);
        int g = Math.round(((a >> 8) & 255) * (1 - t) + ((b >> 8) & 255) * t);
        int bl = Math.round((a & 255) * (1 - t) + (b & 255) * t);
        return 0xFF000000 | (r << 16) | (g << 8) | bl;
    }

    private static int alpha(int rgb, int a) {
        return (a << 24) | (rgb & 0x00FFFFFF);
    }

    /**
     * Every colour of the interface (cards, dock, buttons, text, lines, the app list), derived
     * from the single "interface colour". That colour is the accent; surfaces and text keep enough
     * contrast against each other: a light accent gets dark surfaces, a dark accent gets light ones.
     * The default accent reproduces the original look exactly.
     */
    static final class Palette {
        final boolean light;
        final int base;
        final int card;
        final int cardDrag;
        final int stroke;
        final int accent;
        final int accentPressed;
        final int text;
        final int textDim;
        final int pill;
        final int nav;
        final int pressedFill;
        final int scrim;
        final int strip;
        final int placeholder;
        final int cardButton;
        final int drawerBg;
        final int searchBg;
        final int hint;
        /** Colour for marks drawn on top of the accent colour. */
        final int ink;

        private Palette(boolean light, int base, int card, int cardDrag, int stroke, int text, int textDim,
                        int pillRgb, int pressedRgb, int placeholder, int searchBg, int hint,
                        int accent, int accentPressed) {
            this.light = light;
            this.base = base;
            this.card = card;
            this.cardDrag = cardDrag;
            this.stroke = stroke;
            this.text = text;
            this.textDim = textDim;
            this.placeholder = placeholder;
            this.searchBg = searchBg;
            this.hint = hint;
            this.accent = accent;
            this.accentPressed = accentPressed;
            this.ink = base;
            this.pill = alpha(pillRgb, 0xF0);
            this.nav = alpha(pillRgb, 0xE0);
            this.pressedFill = alpha(pressedRgb, 0xF0);
            this.scrim = alpha(base, 0xCC);
            this.strip = alpha(base, 0xB0);
            this.cardButton = alpha(base, 0xE6);
            this.drawerBg = alpha(base, 0xF2);
        }

        static Palette of(int ui) {
            if (ui == DEFAULT_UI) {
                return new Palette(false, 0xFF101216, 0xFF22262E, 0xFF2B303A, 0xFF363C48, 0xFFE6E8EC, 0xFF8A91A0,
                    0xFF1A1D23, 0xFF303541, 0xFF3A404C, 0xFF1B1E24, 0xFF6B7280, DEFAULT_UI, 0xFF9BB8FF);
            }
            if (lum(ui) >= 0.4f) { // light accent on dark surfaces, tinted towards the accent
                return new Palette(false,
                    mix(0xFF101216, ui, .05f), mix(0xFF22262E, ui, .10f), mix(0xFF2B303A, ui, .12f),
                    mix(0xFF363C48, ui, .30f), mix(0xFFE6E8EC, ui, .15f), mix(0xFF8A91A0, ui, .35f),
                    mix(0xFF1A1D23, ui, .08f), mix(0xFF303541, ui, .15f), mix(0xFF3A404C, ui, .15f),
                    mix(0xFF1B1E24, ui, .08f), mix(0xFF6B7280, ui, .25f),
                    ui, mix(ui, 0xFFFFFFFF, .25f));
            }
            return new Palette(true, // dark accent on light surfaces
                mix(0xFFF4F5F7, ui, .05f), mix(0xFFE7E9EE, ui, .10f), mix(0xFFF1F2F6, ui, .08f),
                mix(0xFFC0C5D0, ui, .30f), mix(0xFF14161B, ui, .25f), mix(0xFF5B6270, ui, .30f),
                mix(0xFFE9EBF0, ui, .08f), mix(0xFFD3D7DF, ui, .15f), mix(0xFFC9CDD6, ui, .20f),
                mix(0xFFE3E5EA, ui, .08f), mix(0xFF7C8494, ui, .20f),
                ui, mix(ui, 0xFF000000, .20f));
        }
    }
}
