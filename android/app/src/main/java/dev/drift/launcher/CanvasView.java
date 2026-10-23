package dev.drift.launcher;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Draws the display list produced by the Rust core. One View for the whole
 * canvas: cards are drawn, not instantiated, and cards outside the screen never
 * reach this class (the core culls them). Everything is drawn in dp.
 *
 * Display list layout (must match rust/src/render.rs):
 *   header[17], cards[n * 7], links[n * 4], ui[n * 7]
 */
final class CanvasView extends View {
    interface Host {
        /** Main thread; events emitted by the core (launch, drawer, ...). */
        void onCoreEvents(List<String> events);
    }

    private static final int HEADER = 17;
    private static final int CARD_STRIDE = 7;
    private static final int LINK_STRIDE = 4;
    private static final int UI_STRIDE = 7;

    private static final int CARD_SELECTED = 1;
    private static final int CARD_DRAGGING = 2;
    private static final int CARD_UNAVAILABLE = 4;
    private static final int CARD_SNAP = 8 | 16;

    private static final int UI_PRESSED = 1;
    private static final int UI_FILLED = 2;
    private static final int UI_DRAG_OVER = 4;
    private static final int UI_DRAG_SOURCE = 8;

    private static final int BG = 0xFF101216;
    private static final int DOT = 0xFF343944;
    private static final int CARD = 0xFF22262E;
    private static final int CARD_DRAG = 0xFF2B303A;
    private static final int STROKE = 0xFF363C48;
    private static final int ACCENT = 0xFF7AA2FF;
    private static final int SNAP = 0xFF5ED6A0;
    private static final int TEXT = 0xFFE6E8EC;
    private static final int TEXT_DIM = 0xFF8A91A0;
    private static final int PILL = 0xF01A1D23;
    private static final int[] GROUP = {
        0xFFE5766B, 0xFFE8B05C, 0xFF8FCB6A, 0xFF5DBFB4, 0xFF6C9BE8, 0xFFB58CE0
    };

    private static final String[] TOOL_LABEL = {"Center", "Group", "Pin", "Remove"};

    private final float density;
    private final IconCache icons;
    private Host host;

    private float[] list = new float[4096];
    private float[] dots = new float[2048];
    private int tableVersion = Integer.MIN_VALUE;
    private final ArrayList<String> keys = new ArrayList<>();
    private final ArrayList<String> labels = new ArrayList<>();
    private final ArrayList<Boolean> available = new ArrayList<>();
    private final String[] slotKeys = new String[5];

    private int insetL, insetT, insetR, insetB;
    private final ArrayList<String> pendingEvents = new ArrayList<>();
    private final Runnable deliverEvents = this::deliver;

    private final Paint dotPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint cardPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint cardDragPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint centered = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint left = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint bitmapPaint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.FILTER_BITMAP_FLAG);
    private final RectF r = new RectF();
    private final RectF r2 = new RectF();

    CanvasView(Context context, IconCache icons) {
        super(context);
        this.icons = icons;
        density = context.getResources().getDisplayMetrics().density;
        setFocusable(true);
        setFocusableInTouchMode(false);

        dotPaint.setStyle(Paint.Style.STROKE);
        dotPaint.setStrokeCap(Paint.Cap.ROUND);
        dotPaint.setColor(DOT);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        cardPaint.setColor(CARD);
        cardPaint.setShadowLayer(8f * density, 0f, 3f * density, 0x66000000);
        cardDragPaint.setColor(CARD_DRAG);
        cardDragPaint.setShadowLayer(18f * density, 0f, 8f * density, 0x99000000);
        centered.setTextAlign(Paint.Align.CENTER);
        centered.setColor(TEXT);
        left.setTextAlign(Paint.Align.LEFT);
        left.setColor(TEXT);
    }

    void setHost(Host h) {
        host = h;
    }

    void setInsets(int l, int t, int rr, int b) {
        insetL = l;
        insetT = t;
        insetR = rr;
        insetB = b;
        pushViewport();
    }

    @Override protected void onSizeChanged(int w, int h, int ow, int oh) {
        super.onSizeChanged(w, h, ow, oh);
        pushViewport();
    }

    private void pushViewport() {
        if (getWidth() <= 0 || getHeight() <= 0) return;
        Native.setViewport(getWidth(), getHeight(), density, insetL, insetT, insetR, insetB);
        invalidate();
    }

    // ---- input -----------------------------------------------------------------

    @Override public boolean onTouchEvent(MotionEvent e) {
        long t = e.getEventTime();
        switch (e.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                send(0, e, 0, t);
                break;
            case MotionEvent.ACTION_POINTER_DOWN:
                send(0, e, e.getActionIndex(), t);
                break;
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < e.getPointerCount(); i++) send(1, e, i, t);
                break;
            case MotionEvent.ACTION_POINTER_UP:
                send(2, e, e.getActionIndex(), t);
                break;
            case MotionEvent.ACTION_UP:
                send(2, e, e.getActionIndex(), t);
                break;
            case MotionEvent.ACTION_CANCEL:
                send(3, e, 0, t);
                break;
            default:
                return true;
        }
        invalidate();
        return true;
    }

    private static void send(int kind, MotionEvent e, int index, long t) {
        Native.touch(kind, e.getPointerId(index), e.getX(index), e.getY(index), t);
    }

    // ---- drawing ---------------------------------------------------------------

    @Override protected void onDraw(Canvas c) {
        boolean more = Native.tick(SystemClock.uptimeMillis());
        int n = Native.render(list);
        if (n > list.length) {
            list = new float[n + 2048];
            n = Native.render(list);
        }
        c.drawColor(BG);
        if (n < HEADER) {
            return;
        }
        int version = (int) list[0];
        if (version != tableVersion) {
            refreshTable();
            tableVersion = version;
        }

        float d = density;
        c.save();
        c.scale(d, d);
        drawDots(c);
        int nCards = (int) list[6];
        int nLinks = (int) list[7];
        int nUi = (int) list[8];
        int linkBase = HEADER + nCards * CARD_STRIDE;
        int uiBase = linkBase + nLinks * LINK_STRIDE;
        drawLinks(c, linkBase, nLinks);
        for (int i = 0; i < nCards; i++) {
            drawCard(c, HEADER + i * CARD_STRIDE);
        }
        drawUi(c, uiBase, nUi);
        c.restore();

        for (String ev; (ev = Native.pollEvent()) != null; ) {
            pendingEvents.add(ev);
        }
        if (!pendingEvents.isEmpty()) {
            post(deliverEvents);
        }
        if (more) {
            postInvalidateOnAnimation();
        }
    }

    private void deliver() {
        if (pendingEvents.isEmpty() || host == null) return;
        ArrayList<String> batch = new ArrayList<>(pendingEvents);
        pendingEvents.clear();
        host.onCoreEvents(batch);
        invalidate();
    }

    private void refreshTable() {
        keys.clear();
        labels.clear();
        available.clear();
        Arrays.fill(slotKeys, null);
        String table = Native.stringTable();
        if (table == null) return;
        for (String line : table.split("\n")) {
            String[] p = line.split("\t", -1);
            if (p.length >= 4 && p[0].equals("c")) {
                keys.add(p[1]);
                labels.add(p[2]);
                available.add(p[3].equals("1"));
            } else if (p.length >= 3 && p[0].equals("d")) {
                try {
                    int slot = Integer.parseInt(p[1]);
                    if (slot >= 0 && slot < slotKeys.length) slotKeys[slot] = p[2];
                } catch (NumberFormatException ignored) {
                    // malformed record: leave the slot empty
                }
            }
        }
    }

    private void drawDots(Canvas c) {
        float spacing = list[2];
        if (spacing < 1f) return;
        float ox = list[3];
        float oy = list[4];
        dotPaint.setStrokeWidth(list[5] * 2f);
        float vw = getWidth() / density;
        float vh = getHeight() / density;
        int cols = (int) ((vw - ox) / spacing) + 1;
        int rows = (int) ((vh - oy) / spacing) + 1;
        if (cols <= 0 || rows <= 0) return;
        int count = cols * rows;
        if (dots.length < count * 2) dots = new float[count * 2 + 512];
        int k = 0;
        for (int row = 0; row < rows; row++) {
            float y = oy + row * spacing;
            for (int col = 0; col < cols; col++) {
                dots[k++] = ox + col * spacing;
                dots[k++] = y;
            }
        }
        c.drawPoints(dots, 0, k, dotPaint);
    }

    private void drawLinks(Canvas c, int base, int n) {
        stroke.setColor((ACCENT & 0x00FFFFFF) | 0x66000000);
        stroke.setStrokeWidth(2.5f);
        for (int i = 0; i < n; i++) {
            int b = base + i * LINK_STRIDE;
            c.drawLine(list[b], list[b + 1], list[b + 2], list[b + 3], stroke);
        }
    }

    private void drawCard(Canvas c, int b) {
        int idx = (int) list[b];
        float x = list[b + 1];
        float y = list[b + 2];
        float w = list[b + 3];
        float h = list[b + 4];
        int flags = (int) list[b + 5];
        int group = (int) list[b + 6];
        boolean unavailable = (flags & CARD_UNAVAILABLE) != 0;
        boolean dragging = (flags & CARD_DRAGGING) != 0;
        boolean selected = (flags & CARD_SELECTED) != 0;

        float rad = Math.min(w, h) * 0.16f;
        r.set(x, y, x + w, y + h);
        c.drawRoundRect(r, rad, rad, dragging ? cardDragPaint : cardPaint);

        int border = STROKE;
        float bw = 1f;
        if (group >= 0) {
            border = GROUP[group % GROUP.length];
            bw = 1.5f;
        }
        if ((flags & CARD_SNAP) != 0) {
            border = SNAP;
            bw = 2.5f;
        }
        if (selected) {
            border = ACCENT;
            bw = 2.5f;
        }
        stroke.setColor(border);
        stroke.setStrokeWidth(bw);
        c.drawRoundRect(r, rad, rad, stroke);

        String key = idx >= 0 && idx < keys.size() ? keys.get(idx) : null;
        String label = idx >= 0 && idx < labels.size() ? labels.get(idx) : "";

        float iconSize = w * 0.56f;
        float ix = x + (w - iconSize) / 2f;
        float iy = y + h * 0.12f;
        int alpha = unavailable ? 90 : 255;
        Bitmap icon = key == null ? null : icons.get(key);
        if (icon != null) {
            bitmapPaint.setAlpha(alpha);
            r2.set(ix, iy, ix + iconSize, iy + iconSize);
            c.drawBitmap(icon, null, r2, bitmapPaint);
        } else {
            fill.setColor(0xFF3A404C);
            c.drawCircle(ix + iconSize / 2f, iy + iconSize / 2f, iconSize / 2f, fill);
        }

        float ts = h * 0.125f;
        if (ts >= 6.5f) {
            float ty = iy + iconSize + ts * 1.45f;
            drawLabel(c, label, x, w, ty, ts, unavailable ? TEXT_DIM : TEXT);
        }
        if (unavailable) {
            // Uninstalled/disabled app: keep the card, mark it, let the user remove it.
            float br = Math.max(5f, w * 0.1f);
            fill.setColor(0xFFE5766B);
            c.drawCircle(x + w - br - 3f, y + br + 3f, br, fill);
            fill.setColor(0xFF101216);
            float s = br * 0.45f;
            stroke.setColor(0xFF101216);
            stroke.setStrokeWidth(Math.max(1.2f, br * 0.22f));
            float cx = x + w - br - 3f;
            float cy = y + br + 3f;
            c.drawLine(cx - s, cy - s, cx + s, cy + s, stroke);
            c.drawLine(cx - s, cy + s, cx + s, cy - s, stroke);
        }
    }

    /** Single-line label; ellipsized without allocating. */
    private void drawLabel(Canvas c, String text, float x, float w, float baseline, float size, int color) {
        centered.setTextSize(size);
        left.setTextSize(size);
        centered.setColor(color);
        left.setColor(color);
        float avail = w * 0.88f;
        if (centered.measureText(text) <= avail) {
            c.drawText(text, x + w / 2f, baseline, centered);
            return;
        }
        float dots = left.measureText("…");
        int count = left.breakText(text, true, Math.max(0f, avail - dots), null);
        float x0 = x + w * 0.06f;
        c.drawText(text, 0, count, x0, baseline, left);
        c.drawText("…", x0 + left.measureText(text, 0, count), baseline, left);
    }

    private void drawUi(Canvas c, int base, int n) {
        float px = list[9], py = list[10], pw = list[11], ph = list[12];
        r.set(px, py, px + pw, py + ph);
        fill.setColor(PILL);
        c.drawRoundRect(r, ph * 0.4f, ph * 0.4f, fill);
        stroke.setColor(STROKE);
        stroke.setStrokeWidth(1f);
        c.drawRoundRect(r, ph * 0.4f, ph * 0.4f, stroke);

        float dockDragSlot = list[13];
        float dockDragX = list[15];
        float slotSize = 0f;

        for (int i = 0; i < n; i++) {
            int b = base + i * UI_STRIDE;
            int kind = (int) list[b];
            int slot = (int) list[b + 1];
            float x = list[b + 2], y = list[b + 3], w = list[b + 4], h = list[b + 5];
            int flags = (int) list[b + 6];
            boolean pressed = (flags & UI_PRESSED) != 0;
            r.set(x, y, x + w, y + h);
            switch (kind) {
                case 0:
                    slotSize = w;
                    drawSlot(c, slot, flags);
                    break;
                case 1:
                    drawDrawerButton(c, pressed);
                    break;
                case 2:
                case 3:
                    drawNavButton(c, kind, pressed);
                    break;
                default:
                    drawTool(c, kind - 4, pressed);
                    break;
            }
        }

        if (dockDragSlot >= 0f && slotSize > 0f) {
            int from = (int) dockDragSlot;
            String key = from < slotKeys.length ? slotKeys[from] : null;
            Bitmap icon = key == null ? null : icons.get(key);
            float s = slotSize * 1.1f;
            float cx = dockDragX;
            float cy = py + ph / 2f;
            if (icon != null) {
                bitmapPaint.setAlpha(255);
                r2.set(cx - s / 2f, cy - s / 2f, cx + s / 2f, cy + s / 2f);
                c.drawBitmap(icon, null, r2, bitmapPaint);
            }
        }
    }

    private void drawSlot(Canvas c, int slot, int flags) {
        // r holds the slot rect
        boolean pressed = (flags & UI_PRESSED) != 0;
        boolean filled = (flags & UI_FILLED) != 0;
        float cx = r.centerX(), cy = r.centerY();
        float s = r.width() * (pressed ? 0.8f : 0.9f);
        if ((flags & UI_DRAG_OVER) != 0) {
            stroke.setColor(ACCENT);
            stroke.setStrokeWidth(2f);
            c.drawRoundRect(r, r.width() * 0.28f, r.width() * 0.28f, stroke);
        }
        if (filled) {
            String key = slot < slotKeys.length ? slotKeys[slot] : null;
            Bitmap icon = key == null ? null : icons.get(key);
            bitmapPaint.setAlpha((flags & UI_DRAG_SOURCE) != 0 ? 70 : 255);
            if (icon != null) {
                r2.set(cx - s / 2f, cy - s / 2f, cx + s / 2f, cy + s / 2f);
                c.drawBitmap(icon, null, r2, bitmapPaint);
            } else {
                fill.setColor(0xFF3A404C);
                c.drawCircle(cx, cy, s / 2f, fill);
            }
        } else {
            // Empty slot: dashed outline with a plus; tapping it opens the app list.
            stroke.setColor(TEXT_DIM);
            stroke.setStrokeWidth(1.3f);
            stroke.setPathEffect(new DashPathEffect(new float[] {4f, 4f}, 0f));
            float inset = r.width() * 0.1f;
            r2.set(r.left + inset, r.top + inset, r.right - inset, r.bottom - inset);
            c.drawRoundRect(r2, r2.width() * 0.3f, r2.width() * 0.3f, stroke);
            stroke.setPathEffect(null);
            float a = r.width() * 0.16f;
            c.drawLine(cx - a, cy, cx + a, cy, stroke);
            c.drawLine(cx, cy - a, cx, cy + a, stroke);
        }
    }

    private void drawDrawerButton(Canvas c, boolean pressed) {
        fill.setColor(pressed ? 0xFF9BB8FF : ACCENT);
        float cx = r.centerX(), cy = r.centerY();
        c.drawCircle(cx, cy, r.width() / 2f, fill);
        fill.setColor(0xFF101216);
        float step = r.width() * 0.2f;
        for (int row = -1; row <= 1; row++) {
            for (int col = -1; col <= 1; col++) {
                c.drawCircle(cx + col * step, cy + row * step, r.width() * 0.05f, fill);
            }
        }
    }

    private void drawNavButton(Canvas c, int kind, boolean pressed) {
        fill.setColor(pressed ? 0xF0303541 : 0xE01A1D23);
        float cx = r.centerX(), cy = r.centerY();
        float rad = r.width() / 2f;
        c.drawCircle(cx, cy, rad, fill);
        stroke.setColor(TEXT);
        stroke.setStrokeWidth(1.8f);
        if (kind == 2) {
            // Overview: four corner brackets.
            float a = rad * 0.55f, k = rad * 0.28f;
            for (int sx = -1; sx <= 1; sx += 2) {
                for (int sy = -1; sy <= 1; sy += 2) {
                    float px = cx + sx * a, py = cy + sy * a;
                    c.drawLine(px, py, px - sx * k, py, stroke);
                    c.drawLine(px, py, px, py - sy * k, stroke);
                }
            }
        } else {
            // Recent: clock face.
            float a = rad * 0.5f;
            c.drawCircle(cx, cy, a, stroke);
            c.drawLine(cx, cy, cx, cy - a * 0.65f, stroke);
            c.drawLine(cx, cy, cx + a * 0.5f, cy + a * 0.2f, stroke);
        }
    }

    private void drawTool(Canvas c, int tool, boolean pressed) {
        fill.setColor(pressed ? 0xF0303541 : 0xF01A1D23);
        float rad = r.height() / 2f;
        c.drawRoundRect(r, rad, rad, fill);
        stroke.setColor(tool == 3 ? 0xFFE5766B : STROKE);
        stroke.setStrokeWidth(1f);
        c.drawRoundRect(r, rad, rad, stroke);
        centered.setTextSize(12f);
        centered.setColor(tool == 3 ? 0xFFE5766B : TEXT);
        c.drawText(TOOL_LABEL[tool], r.centerX(), r.centerY() + 4.2f, centered);
        centered.setColor(TEXT);
    }
}
