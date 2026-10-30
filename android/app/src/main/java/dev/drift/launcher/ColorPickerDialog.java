package dev.drift.launcher;

import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.ComposeShader;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.PorterDuff;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.drawable.GradientDrawable;
import android.text.Editable;
import android.text.InputFilter;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.LinearLayout;

import java.util.Locale;
import java.util.function.IntConsumer;

/** Small HSV colour picker (no libraries): saturation/value square, hue bar, hex field. Opaque colours only. */
final class ColorPickerDialog {
    private ColorPickerDialog() {}

    static void show(Context ctx, String title, int initial, int defaultColor, IntConsumer onPick) {
        final float d = ctx.getResources().getDisplayMetrics().density;
        final float[] hsv = new float[3];
        Color.colorToHSV(initial, hsv);

        LinearLayout col = new LinearLayout(ctx);
        col.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(20 * d);
        col.setPadding(pad, Math.round(8 * d), pad, 0);

        SvView sv = new SvView(ctx, hsv);
        col.addView(sv, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Math.round(190 * d)));

        HueView hue = new HueView(ctx, hsv);
        LinearLayout.LayoutParams hp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Math.round(32 * d));
        hp.topMargin = Math.round(14 * d);
        col.addView(hue, hp);

        LinearLayout row = new LinearLayout(ctx);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(android.view.Gravity.CENTER_VERTICAL);
        LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        rp.topMargin = Math.round(14 * d);
        col.addView(row, rp);

        View swatch = new View(ctx);
        GradientDrawable sd = new GradientDrawable();
        sd.setShape(GradientDrawable.OVAL);
        sd.setStroke(Math.round(1.5f * d), 0x66FFFFFF);
        swatch.setBackground(sd);
        row.addView(swatch, new LinearLayout.LayoutParams(Math.round(36 * d), Math.round(36 * d)));

        EditText hex = new EditText(ctx);
        hex.setSingleLine(true);
        hex.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS | InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS);
        hex.setFilters(new InputFilter[] {new InputFilter.LengthFilter(7)});
        hex.setTextSize(16);
        LinearLayout.LayoutParams ep = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        ep.leftMargin = Math.round(14 * d);
        row.addView(hex, ep);

        final boolean[] syncing = {false};
        Runnable refresh = () -> {
            int c = Color.HSVToColor(hsv);
            sd.setColor(c);
            syncing[0] = true;
            String t = String.format(Locale.ROOT, "#%06X", c & 0xFFFFFF);
            if (!hex.getText().toString().equalsIgnoreCase(t)) hex.setText(t);
            syncing[0] = false;
        };
        Runnable[] changed = {null};
        changed[0] = () -> {
            sv.hueChanged();
            sv.invalidate();
            hue.invalidate();
            refresh.run();
        };
        sv.listener = refresh;
        hue.listener = changed[0];
        hex.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void onTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void afterTextChanged(Editable e) {
                if (syncing[0]) return;
                String t = e.toString().trim();
                if (t.startsWith("#")) t = t.substring(1);
                if (t.length() != 6) return;
                try {
                    int c = 0xFF000000 | Integer.parseInt(t, 16);
                    Color.colorToHSV(c, hsv);
                    sv.hueChanged();
                    sv.invalidate();
                    hue.invalidate();
                    sd.setColor(c);
                } catch (NumberFormatException ignored) {
                    // incomplete or invalid: keep the last good colour
                }
            }
        });
        refresh.run();

        new AlertDialog.Builder(ctx, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle(title)
            .setView(col)
            .setPositiveButton(android.R.string.ok, (dlg, w) -> onPick.accept(0xFF000000 | Color.HSVToColor(hsv)))
            .setNeutralButton("Default", (dlg, w) -> onPick.accept(defaultColor))
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    /** Saturation (x) and value (y) for the current hue. */
    private static final class SvView extends View {
        private final float[] hsv;
        private final Paint paint = new Paint();
        private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF box = new RectF();
        private float shaderHue = -1f;
        Runnable listener;

        SvView(Context c, float[] hsv) {
            super(c);
            this.hsv = hsv;
            ring.setStyle(Paint.Style.STROKE);
        }

        void hueChanged() {
            shaderHue = -1f;
        }

        @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
            shaderHue = -1f;
        }

        @Override protected void onDraw(Canvas c) {
            float w = getWidth();
            float h = getHeight();
            if (shaderHue != hsv[0]) {
                Shader value = new LinearGradient(0, 0, 0, h, Color.WHITE, Color.BLACK, Shader.TileMode.CLAMP);
                Shader sat = new LinearGradient(0, 0, w, 0, Color.WHITE,
                    Color.HSVToColor(new float[] {hsv[0], 1f, 1f}), Shader.TileMode.CLAMP);
                paint.setShader(new ComposeShader(value, sat, PorterDuff.Mode.MULTIPLY));
                shaderHue = hsv[0];
            }
            float d = getResources().getDisplayMetrics().density;
            box.set(0, 0, w, h);
            c.drawRoundRect(box, 8 * d, 8 * d, paint);
            float x = hsv[1] * w;
            float y = (1f - hsv[2]) * h;
            ring.setStrokeWidth(3 * d);
            ring.setColor(0xFF000000);
            c.drawCircle(x, y, 9 * d, ring);
            ring.setStrokeWidth(1.8f * d);
            ring.setColor(0xFFFFFFFF);
            c.drawCircle(x, y, 9 * d, ring);
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) getParent().requestDisallowInterceptTouchEvent(true);
            hsv[1] = Math.max(0f, Math.min(1f, e.getX() / getWidth()));
            hsv[2] = 1f - Math.max(0f, Math.min(1f, e.getY() / getHeight()));
            invalidate();
            if (listener != null) listener.run();
            return true;
        }
    }

    private static final class HueView extends View {
        private final float[] hsv;
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint thumb = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF box = new RectF();
        Runnable listener;

        HueView(Context c, float[] hsv) {
            super(c);
            this.hsv = hsv;
            thumb.setStyle(Paint.Style.STROKE);
        }

        @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
            int[] colors = new int[7];
            for (int i = 0; i < 7; i++) colors[i] = Color.HSVToColor(new float[] {i * 60f, 1f, 1f});
            paint.setShader(new LinearGradient(0, 0, w, 0, colors, null, Shader.TileMode.CLAMP));
        }

        @Override protected void onDraw(Canvas c) {
            float d = getResources().getDisplayMetrics().density;
            float w = getWidth();
            float h = getHeight();
            box.set(0, h * 0.2f, w, h * 0.8f);
            c.drawRoundRect(box, 8 * d, 8 * d, paint);
            float x = hsv[0] / 360f * w;
            thumb.setStrokeWidth(3 * d);
            thumb.setColor(0xFF000000);
            c.drawCircle(x, h / 2f, h * 0.42f, thumb);
            thumb.setStrokeWidth(1.8f * d);
            thumb.setColor(0xFFFFFFFF);
            c.drawCircle(x, h / 2f, h * 0.42f, thumb);
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) getParent().requestDisallowInterceptTouchEvent(true);
            hsv[0] = Math.max(0f, Math.min(359.99f, e.getX() / getWidth() * 360f));
            invalidate();
            if (listener != null) listener.run();
            return true;
        }
    }
}
