package dev.drift.launcher;

import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.view.animation.DecelerateInterpolator;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Pop-up of one dock folder: dimmed background, a card with the folder name (tap = rename) and
 * a grid of apps inside a ScrollView limited to ~74% of the screen height. The last tile is "+".
 *
 * All state lives in the Rust core (Native.dock*); this view only shows it. Tiles are kept per
 * app key and re-synced in place, so late icons only set a bitmap and nothing is rebuilt.
 * Dragging a tile shows an elevated copy under the finger; dropping on another tile calls
 * Native.dockMoveApp and the grid reflows with a FLIP animation (~260 ms); dropping on the dim
 * background asks to remove the app from the folder.
 */
final class FolderView extends FrameLayout {
    interface Callback {
        void onLaunch(String key);

        void onAddApp(int folderId);

        void onRename(int folderId);

        /** The core's state changed (move / remove): redraw the dock. */
        void onChanged();

        void onClosed();
    }

    private static final int FLIP_MS = 260;
    private static final float MAX_HEIGHT_FRACTION = 0.74f;

    private final float density;
    private final AppRepo repo;
    private final IconCache icons;
    private Callback callback;
    private Appearance.Palette pal = Appearance.defaults().pal;

    private final LinearLayout card;
    private final TextView title;
    private final ClampScroll scroll;
    private final Grid grid;
    private final AddTile addTile;
    private final LinkedHashMap<String, Tile> tiles = new LinkedHashMap<>();
    private final ArrayList<String> order = new ArrayList<>();

    private int folderId = -1;
    private int insetL, insetT, insetR, insetB;

    // drag state
    private Tile dragTile;
    private ImageView ghost;
    private View menuCatcher;
    private View menu;
    private float downX, downY;
    private final int[] loc = new int[2];
    private final Runnable autoScroll = this::autoScrollStep;
    private float dragRawX, dragRawY;

    FolderView(Context context, AppRepo repo, IconCache icons) {
        super(context);
        this.repo = repo;
        this.icons = icons;
        density = context.getResources().getDisplayMetrics().density;
        setVisibility(GONE);
        setClickable(true);

        card = new LinearLayout(context);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setClickable(true);
        title = new TextView(context);
        title.setTextSize(18);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        title.setGravity(Gravity.CENTER);
        title.setSingleLine(true);
        title.setEllipsize(android.text.TextUtils.TruncateAt.END);
        title.setPadding(dp(16), dp(14), dp(16), dp(10));
        title.setOnClickListener(v -> {
            if (callback != null && folderId >= 0) callback.onRename(folderId);
        });
        card.addView(title, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        scroll = new ClampScroll(context);
        scroll.setVerticalScrollBarEnabled(false);
        scroll.setOverScrollMode(OVER_SCROLL_NEVER);
        grid = new Grid(context);
        scroll.addView(grid, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        card.addView(scroll, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        addTile = new AddTile(context);
        addView(card, new LayoutParams(dp(320), ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        applyPalette();
    }

    void setCallback(Callback c) {
        callback = c;
    }

    void setAppearance(Appearance a) {
        pal = a.pal;
        applyPalette();
        for (Tile t : tiles.values()) t.applyPalette();
        addTile.applyPalette();
    }

    private void applyPalette() {
        setBackgroundColor(0xAA000000);
        GradientDrawable bg = new GradientDrawable();
        bg.setColor(pal.drawerBg);
        bg.setCornerRadius(dp(22));
        bg.setStroke(dp(1), pal.stroke);
        card.setBackground(bg);
        title.setTextColor(pal.text);
    }

    void setInsets(int l, int t, int r, int b) {
        insetL = l;
        insetT = t;
        insetR = r;
        insetB = b;
        setPadding(l, t, r, b);
    }

    boolean isShowing() {
        return getVisibility() == VISIBLE;
    }

    int folderId() {
        return folderId;
    }

    void open(int id) {
        folderId = id;
        order.clear();
        for (Tile t : tiles.values()) grid.removeView(t);
        tiles.clear();
        scroll.scrollTo(0, 0);
        setAlpha(0f);
        setVisibility(VISIBLE);
        if (!sync(false)) return;
        animate().alpha(1f).setDuration(120).start();
    }

    void close() {
        if (!isShowing()) return;
        endDrag(false);
        dismissMenu();
        animate().cancel();
        setVisibility(GONE);
        setAlpha(1f);
        folderId = -1;
        if (callback != null) callback.onClosed();
    }

    /** Re-reads the folder from the core. Returns false (and closes) if the folder no longer exists. */
    boolean refresh() {
        return isShowing() && sync(true);
    }

    /** Late icons: only set bitmaps, never rebuild the grid. */
    void iconsLoaded() {
        if (!isShowing()) return;
        for (Map.Entry<String, Tile> e : tiles.entrySet()) e.getValue().setIcon(icons.get(e.getKey()));
    }

    private boolean sync(boolean animate) {
        String[] apps = Native.dockFolderApps(folderId);
        if (apps == null) {
            close();
            return false;
        }
        String name = Native.dockFolderName(folderId);
        if (name == null || name.isEmpty()) name = getContext().getString(R.string.folder_default_name);
        title.setText(name + "  ✎");

        HashMap<View, float[]> before = animate ? snapshotPositions() : null;
        ArrayList<String> next = new ArrayList<>(apps.length);
        for (String k : apps) next.add(k);
        boolean same = next.equals(order);
        if (!same || grid.getChildCount() != next.size() + 1) {
            // drop tiles of removed apps, create tiles of new ones, then put all in the stored order
            for (String k : new ArrayList<>(tiles.keySet())) {
                if (!next.contains(k)) grid.removeView(tiles.remove(k));
            }
            grid.removeAllViews();
            for (String k : next) {
                Tile t = tiles.get(k);
                if (t == null) {
                    t = new Tile(getContext(), k);
                    tiles.put(k, t);
                }
                t.setLabel(repo.label(k));
                t.setIcon(icons.get(k));
                grid.addView(t);
            }
            grid.addView(addTile);
            order.clear();
            order.addAll(next);
            if (before != null) playFlip(before);
        }
        return true;
    }

    // ---- sizing: card width, scroll height ~74% of the view, recomputed on every measure ----

    @Override protected void onMeasure(int wSpec, int hSpec) {
        int w = MeasureSpec.getSize(wSpec);
        int h = MeasureSpec.getSize(hSpec);
        int availW = w - insetL - insetR - dp(32);
        int cardW = Math.max(dp(200), Math.min(availW, dp(360)));
        LayoutParams lp = (LayoutParams) card.getLayoutParams();
        if (lp.width != cardW) {
            lp.width = cardW;
            card.setLayoutParams(lp);
        }
        int availH = h - insetT - insetB - dp(32);
        int maxScroll = Math.round(h * MAX_HEIGHT_FRACTION);
        // the title row must fit as well
        scroll.maxHeight = Math.max(dp(96), Math.min(maxScroll, availH - dp(56)));
        super.onMeasure(wSpec, hSpec);
    }

    private static final class ClampScroll extends ScrollView {
        int maxHeight = Integer.MAX_VALUE;

        ClampScroll(Context c) {
            super(c);
        }

        @Override protected void onMeasure(int wSpec, int hSpec) {
            int limit = maxHeight;
            if (MeasureSpec.getMode(hSpec) != MeasureSpec.UNSPECIFIED) limit = Math.min(limit, MeasureSpec.getSize(hSpec));
            super.onMeasure(wSpec, MeasureSpec.makeMeasureSpec(limit, MeasureSpec.AT_MOST));
        }
    }

    /** Fixed-size cells, 3..5 columns depending on width. */
    private final class Grid extends ViewGroup {
        int cols = 3;
        int cellW;
        final int cellH = dp(92);

        Grid(Context c) {
            super(c);
            setPadding(dp(8), dp(4), dp(8), dp(12));
        }

        @Override protected void onMeasure(int wSpec, int hSpec) {
            int w = MeasureSpec.getSize(wSpec);
            int inner = w - getPaddingLeft() - getPaddingRight();
            cols = Math.max(3, Math.min(5, inner / dp(78)));
            cellW = inner / cols;
            int n = getChildCount();
            int rows = Math.max(1, (n + cols - 1) / cols);
            for (int i = 0; i < n; i++) {
                getChildAt(i).measure(MeasureSpec.makeMeasureSpec(cellW, MeasureSpec.EXACTLY),
                    MeasureSpec.makeMeasureSpec(cellH, MeasureSpec.EXACTLY));
            }
            setMeasuredDimension(w, rows * cellH + getPaddingTop() + getPaddingBottom());
        }

        @Override protected void onLayout(boolean changed, int l, int t, int r, int b) {
            for (int i = 0; i < getChildCount(); i++) {
                int x = getPaddingLeft() + (i % cols) * cellW;
                int y = getPaddingTop() + (i / cols) * cellH;
                getChildAt(i).layout(x, y, x + cellW, y + cellH);
            }
        }

        /** Index of the cell under (x,y) in grid coordinates, or -1. */
        int cellAt(float x, float y) {
            float gx = x - getPaddingLeft();
            float gy = y - getPaddingTop();
            if (gx < 0 || gy < 0 || cellW <= 0) return -1;
            int c = (int) (gx / cellW);
            int r = (int) (gy / cellH);
            if (c >= cols) return -1;
            int i = r * cols + c;
            return i < getChildCount() ? i : -1;
        }
    }

    // ---- FLIP ----

    private HashMap<View, float[]> snapshotPositions() {
        HashMap<View, float[]> m = new HashMap<>();
        for (int i = 0; i < grid.getChildCount(); i++) {
            View v = grid.getChildAt(i);
            m.put(v, new float[] {v.getLeft() + v.getTranslationX(), v.getTop() + v.getTranslationY()});
        }
        return m;
    }

    private void playFlip(HashMap<View, float[]> before) {
        final ViewTreeObserver vto = grid.getViewTreeObserver();
        vto.addOnPreDrawListener(new ViewTreeObserver.OnPreDrawListener() {
            @Override public boolean onPreDraw() {
                if (grid.getViewTreeObserver().isAlive()) grid.getViewTreeObserver().removeOnPreDrawListener(this);
                for (int i = 0; i < grid.getChildCount(); i++) {
                    View v = grid.getChildAt(i);
                    float[] old = before.get(v);
                    if (old == null) continue;
                    float dx = old[0] - v.getLeft();
                    float dy = old[1] - v.getTop();
                    if (Math.abs(dx) < 0.5f && Math.abs(dy) < 0.5f) continue;
                    v.setTranslationX(dx);
                    v.setTranslationY(dy);
                    v.animate().translationX(0f).translationY(0f).setDuration(FLIP_MS)
                        .setInterpolator(new DecelerateInterpolator(1.6f)).start();
                }
                return true;
            }
        });
        grid.requestLayout();
    }

    // ---- scrim ----

    @Override public boolean onTouchEvent(MotionEvent e) {
        if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
            downX = e.getX();
            downY = e.getY();
        } else if (e.getActionMasked() == MotionEvent.ACTION_UP) {
            if (Math.abs(e.getX() - downX) < dp(10) && Math.abs(e.getY() - downY) < dp(10)) close();
        }
        return true;
    }

    // ---- menu ----

    private void showMenu(Tile t) {
        dismissMenu();
        menuCatcher = new View(getContext());
        menuCatcher.setOnTouchListener((v, ev) -> {
            if (ev.getActionMasked() == MotionEvent.ACTION_DOWN) dismissMenu();
            return true;
        });
        addView(menuCatcher, new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        LinearLayout m = new LinearLayout(getContext());
        m.setOrientation(LinearLayout.VERTICAL);
        GradientDrawable bg = new GradientDrawable();
        bg.setColor(pal.cardDrag);
        bg.setCornerRadius(dp(14));
        bg.setStroke(dp(1), pal.stroke);
        m.setBackground(bg);
        m.setElevation(dp(8));
        m.addView(menuItem(R.string.folder_menu_open, false, () -> {
            dismissMenu();
            if (callback != null) callback.onLaunch(t.key);
        }));
        m.addView(menuItem(R.string.folder_menu_remove, true, () -> {
            dismissMenu();
            confirmRemove(t.key);
        }));
        m.measure(MeasureSpec.makeMeasureSpec(dp(220), MeasureSpec.AT_MOST), MeasureSpec.UNSPECIFIED);
        getLocationOnScreen(loc);
        int ox = loc[0];
        int oy = loc[1];
        t.getLocationOnScreen(loc);
        float x = loc[0] - ox + t.getWidth() / 2f - m.getMeasuredWidth() / 2f;
        float y = loc[1] - oy + t.getHeight() + dp(4);
        if (y + m.getMeasuredHeight() > getHeight() - insetB) y = loc[1] - oy - m.getMeasuredHeight() - dp(4);
        x = Math.max(insetL + dp(8), Math.min(x, getWidth() - insetR - m.getMeasuredWidth() - dp(8)));
        LayoutParams lp = new LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.leftMargin = Math.round(x);
        lp.topMargin = Math.round(y);
        addView(m, lp);
        menu = m;
        t.setHighlight(true);
    }

    private TextView menuItem(int textRes, boolean danger, Runnable action) {
        TextView tv = new TextView(getContext());
        tv.setText(textRes);
        tv.setTextSize(16);
        tv.setTextColor(danger ? 0xFFE5766B : pal.text);
        tv.setPadding(dp(18), dp(12), dp(18), dp(12));
        tv.setOnClickListener(v -> action.run());
        return tv;
    }

    private void dismissMenu() {
        if (menu != null) {
            removeView(menu);
            menu = null;
        }
        if (menuCatcher != null) {
            removeView(menuCatcher);
            menuCatcher = null;
        }
        for (Tile t : tiles.values()) t.setHighlight(false);
    }

    private void confirmRemove(String key) {
        new AlertDialog.Builder(getContext(), android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setMessage(getContext().getString(R.string.confirm_remove_from_folder, repo.label(key)))
            .setPositiveButton(R.string.action_remove, (d, w) -> {
                Native.dockRemoveApp(folderId, key);
                sync(true);
                if (callback != null) callback.onChanged();
            })
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    // ---- drag ----

    private void beginDrag(Tile t, float rawX, float rawY) {
        dismissMenu();
        dragTile = t;
        float size = dp(56) * 1.25f;
        ghost = new ImageView(getContext());
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(16));
        bg.setColor(pal.cardDrag);
        bg.setStroke(dp(2), pal.accent);
        ghost.setBackground(bg);
        ghost.setPadding(dp(6), dp(6), dp(6), dp(6));
        ghost.setImageBitmap(icons.get(t.key));
        ghost.setElevation(dp(14)); // soft shadow of the lifted copy
        LayoutParams lp = new LayoutParams(Math.round(size), Math.round(size));
        addView(ghost, lp);
        ghost.setScaleX(0.8f);
        ghost.setScaleY(0.8f);
        ghost.animate().scaleX(1f).scaleY(1f).setDuration(120).start();
        moveGhost(rawX, rawY);
        t.setAlpha(0.25f);
        for (Tile o : tiles.values()) {
            if (o != t) o.animate().alpha(0.5f).setDuration(140).start();
        }
        addTile.animate().alpha(0.5f).setDuration(140).start();
        t.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS);
        removeCallbacks(autoScroll);
        post(autoScroll);
    }

    private void moveGhost(float rawX, float rawY) {
        dragRawX = rawX;
        dragRawY = rawY;
        if (ghost == null) return;
        getLocationOnScreen(loc);
        ghost.setX(rawX - loc[0] - ghost.getLayoutParams().width / 2f);
        ghost.setY(rawY - loc[1] - ghost.getLayoutParams().height / 2f - dp(24));
    }

    private void autoScrollStep() {
        if (dragTile == null) return;
        scroll.getLocationOnScreen(loc);
        float top = loc[1];
        float bottom = top + scroll.getHeight();
        int edge = dp(44);
        int step = 0;
        if (dragRawY < top + edge) step = -dp(7);
        else if (dragRawY > bottom - edge) step = dp(7);
        if (step != 0) scroll.scrollBy(0, step);
        postOnAnimation(autoScroll);
    }

    /** Cell index under a screen point, or -2 for "outside the card", -1 for "inside, no cell". */
    private int hitCell(float rawX, float rawY) {
        card.getLocationOnScreen(loc);
        if (rawX < loc[0] || rawX > loc[0] + card.getWidth() || rawY < loc[1] || rawY > loc[1] + card.getHeight()) return -2;
        grid.getLocationOnScreen(loc);
        int i = grid.cellAt(rawX - loc[0], rawY - loc[1]);
        return i;
    }

    private void endDrag(boolean drop) {
        Tile t = dragTile;
        if (t == null) return;
        dragTile = null;
        removeCallbacks(autoScroll);
        if (ghost != null) {
            removeView(ghost);
            ghost = null;
        }
        int cell = drop ? hitCell(dragRawX, dragRawY) : -3;
        for (Tile o : tiles.values()) o.animate().alpha(1f).setDuration(140).start();
        addTile.animate().alpha(1f).setDuration(140).start();
        t.setAlpha(1f);
        if (!drop) return;
        int from = order.indexOf(t.key);
        if (cell == -2) {
            confirmRemove(t.key);
        } else if (from >= 0 && cell >= 0) {
            int to = Math.min(cell, order.size() - 1); // dropping on "+" moves to the end
            if (to != from && Native.dockMoveApp(folderId, from, to)) {
                sync(true);
                if (callback != null) callback.onChanged();
            }
        }
    }

    // ---- tiles ----

    private class Tile extends LinearLayout {
        final String key;
        private final ImageView icon;
        private final TextView label;
        private boolean longPressed;
        private boolean dragging;
        private float sx, sy;
        private final Runnable longPress = () -> {
            longPressed = true;
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS);
            getParent().requestDisallowInterceptTouchEvent(true);
            showMenu(this);
        };

        Tile(Context c, String key) {
            super(c);
            this.key = key;
            setOrientation(VERTICAL);
            setGravity(Gravity.CENTER_HORIZONTAL);
            setPadding(dp(2), dp(8), dp(2), dp(4));
            icon = new ImageView(c);
            addView(icon, new LayoutParams(dp(52), dp(52)));
            label = new TextView(c);
            label.setTextSize(11);
            label.setGravity(Gravity.CENTER);
            label.setMaxLines(2);
            label.setEllipsize(android.text.TextUtils.TruncateAt.END);
            LayoutParams lp = new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            lp.topMargin = dp(4);
            addView(label, lp);
            applyPalette();
        }

        void applyPalette() {
            label.setTextColor(pal.text);
        }

        void setLabel(String s) {
            label.setText(s);
        }

        void setIcon(Bitmap b) {
            if (b != null) icon.setImageBitmap(b);
            else icon.setImageDrawable(null);
        }

        void setHighlight(boolean on) {
            if (on) {
                GradientDrawable bg = new GradientDrawable();
                bg.setCornerRadius(dp(14));
                bg.setColor((pal.accent & 0x00FFFFFF) | 0x33000000);
                bg.setStroke(dp(1), pal.accent);
                setBackground(bg);
            } else {
                setBackground(null);
            }
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    sx = e.getRawX();
                    sy = e.getRawY();
                    longPressed = false;
                    dragging = false;
                    postDelayed(longPress, ViewConfiguration.getLongPressTimeout());
                    return true;
                case MotionEvent.ACTION_MOVE: {
                    float dx = e.getRawX() - sx;
                    float dy = e.getRawY() - sy;
                    boolean moved = Math.hypot(dx, dy) > ViewConfiguration.get(getContext()).getScaledTouchSlop();
                    if (dragging) {
                        moveGhost(e.getRawX(), e.getRawY());
                    } else if (longPressed && moved) {
                        dragging = true; // finger moved after the long press: close the menu, lift the copy
                        beginDrag(this, e.getRawX(), e.getRawY());
                    } else if (!longPressed && moved) {
                        removeCallbacks(longPress); // plain scroll
                    }
                    return true;
                }
                case MotionEvent.ACTION_UP:
                    removeCallbacks(longPress);
                    if (dragging) {
                        endDrag(true);
                    } else if (!longPressed) {
                        if (Math.hypot(e.getRawX() - sx, e.getRawY() - sy) < dp(12) && callback != null) callback.onLaunch(key);
                    } // long press without movement: the menu stays open
                    dragging = false;
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    removeCallbacks(longPress);
                    if (dragging) endDrag(false);
                    dragging = false;
                    return true;
                default:
                    return true;
            }
        }
    }

    private final class AddTile extends LinearLayout {
        private final TextView plus;

        AddTile(Context c) {
            super(c);
            setGravity(Gravity.CENTER);
            plus = new TextView(c);
            plus.setText("+");
            plus.setTextSize(28);
            plus.setGravity(Gravity.CENTER);
            addView(plus, new LayoutParams(dp(52), dp(52)));
            setContentDescription(c.getString(R.string.folder_add));
            setOnClickListener(v -> {
                if (callback != null && folderId >= 0) callback.onAddApp(folderId);
            });
            applyPalette();
        }

        void applyPalette() {
            GradientDrawable d = new GradientDrawable();
            d.setCornerRadius(dp(16));
            d.setStroke(dp(1), pal.textDim, dp(4), dp(4));
            d.setColor(0);
            plus.setBackground(d);
            plus.setTextColor(pal.textDim);
        }
    }

    private int dp(int v) {
        return Math.round(v * density);
    }
}
