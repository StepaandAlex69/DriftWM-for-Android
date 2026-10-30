package dev.drift.launcher;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.Typeface;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.BaseAdapter;
import android.widget.EditText;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.TextView;

/**
 * The all-apps list: plain ListView, no animations beyond a short fade, search
 * ranked by the Rust core. Picking an app is forwarded to the core, which decides
 * between "jump to existing card", "create a card and launch", and "assign to a
 * dock slot".
 */
final class DrawerView extends LinearLayout {
    interface Callback {
        void onPick(String key);

        void onClose();

        /** The "Drift settings" row was tapped. */
        void onSettings();
    }

    private final float density;
    private final AppRepo repo;
    private final IconCache icons;
    private final TextView title;
    private final EditText search;
    private final TextView settingsRow;
    private Appearance.Palette pal = Appearance.defaults().pal;
    private int slotMode = -1;
    private final ListView listView;
    private final Adapter adapter = new Adapter();
    private Callback callback;
    private String[] keys = new String[0];

    DrawerView(Context context, AppRepo repo, IconCache icons) {
        super(context);
        this.repo = repo;
        this.icons = icons;
        density = context.getResources().getDisplayMetrics().density;
        setOrientation(VERTICAL);
        setVisibility(GONE);
        setClickable(true); // swallow touches so the canvas below never sees them

        int p = dp(16);
        title = new TextView(context);
        title.setTextSize(14);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        title.setPadding(p, dp(12), p, 0);
        title.setVisibility(GONE);
        addView(title, new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        search = new EditText(context);
        search.setHint("Search apps");
        search.setTextSize(16);
        search.setSingleLine(true);
        search.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        search.setImeOptions(EditorInfo.IME_ACTION_SEARCH);
        search.setPadding(p, dp(12), p, dp(12));
        search.setOnEditorActionListener((v, action, ev) -> {
            hideKeyboard();
            return true;
        });
        search.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void onTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void afterTextChanged(Editable s) {
                reload();
            }
        });
        LayoutParams sp = new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        sp.setMargins(p, dp(8), p, dp(8));
        addView(search, sp);

        // Entry to Drift's own settings; shown in the normal list (and when the search matches it).
        settingsRow = new TextView(context);
        settingsRow.setText("Drift settings  \u203A");
        settingsRow.setTextSize(16);
        settingsRow.setTypeface(Typeface.DEFAULT_BOLD);
        settingsRow.setGravity(Gravity.CENTER_VERTICAL);
        settingsRow.setSingleLine(true);
        settingsRow.setPadding(p, dp(10), p, dp(10));
        settingsRow.setOnClickListener(v -> {
            if (callback != null) {
                hideKeyboard();
                callback.onSettings();
            }
        });
        addView(settingsRow, new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        applyPalette();

        listView = new ListView(context);
        listView.setDivider(null);
        listView.setAdapter(adapter);
        // No scroll indicators at all: the list must look identical on every screen width.
        listView.setFastScrollEnabled(false);
        listView.setVerticalScrollBarEnabled(false);
        listView.setHorizontalScrollBarEnabled(false);
        listView.setOverScrollMode(OVER_SCROLL_NEVER);
        listView.setOnItemClickListener((parent, v, pos, id) -> {
            if (callback != null && pos >= 0 && pos < keys.length) {
                hideKeyboard();
                callback.onPick(keys[pos]);
            }
        });
        addView(listView, new LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
    }

    void setAppearance(Appearance a) {
        pal = a.pal;
        applyPalette();
        adapter.notifyDataSetChanged();
    }

    private void applyPalette() {
        setBackgroundColor(pal.drawerBg);
        title.setTextColor(pal.accent);
        search.setHintTextColor(pal.hint);
        search.setTextColor(pal.text);
        search.setBackgroundColor(pal.searchBg);
        settingsRow.setTextColor(pal.accent);
    }

    void setCallback(Callback c) {
        callback = c;
    }

    void setInsets(int l, int t, int r, int b) {
        setPadding(l, t, r, b);
    }

    boolean isShowing() {
        return getVisibility() == VISIBLE;
    }

    /** @param slot dock slot being chosen (0-based), or -1 for the normal list */
    void show(int slot) {
        slotMode = slot;
        if (slot >= 0) {
            title.setText("Choose an app for dock slot " + (slot + 1));
            title.setVisibility(VISIBLE);
        } else {
            title.setVisibility(GONE);
        }
        search.setText("");
        reload();
        listView.setSelection(0);
        setAlpha(0f);
        setVisibility(VISIBLE);
        animate().alpha(1f).setDuration(110).start();
    }

    void hide() {
        if (!isShowing()) return;
        hideKeyboard();
        animate().cancel();
        setVisibility(GONE);
        setAlpha(1f);
        if (callback != null) callback.onClose();
    }

    /** The app list or icons changed underneath an open drawer. */
    void refresh() {
        if (isShowing()) reload();
    }

    void iconsLoaded() {
        if (isShowing()) adapter.notifyDataSetChanged();
    }

    private void reload() {
        String[] found = Native.searchApps(search.getText().toString());
        keys = found == null ? new String[0] : found;
        String q = search.getText().toString().trim().toLowerCase(java.util.Locale.ROOT);
        boolean matches = q.isEmpty() || "drift settings".contains(q);
        settingsRow.setVisibility(slotMode < 0 && matches ? VISIBLE : GONE);
        adapter.notifyDataSetChanged();
    }

    private void hideKeyboard() {
        InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm != null) imm.hideSoftInputFromWindow(search.getWindowToken(), 0);
        search.clearFocus();
    }

    private int dp(int v) {
        return Math.round(v * density);
    }

    private final class Adapter extends BaseAdapter {
        @Override public int getCount() {
            return keys.length;
        }

        @Override public Object getItem(int i) {
            return keys[i];
        }

        @Override public long getItemId(int i) {
            return i;
        }

        @Override public View getView(int i, View convert, ViewGroup parent) {
            Row row;
            if (convert instanceof Row) {
                row = (Row) convert;
            } else {
                row = new Row(parent.getContext());
            }
            String key = keys[i];
            row.name.setText(repo.label(key));
            row.name.setTextColor(pal.text);
            Bitmap icon = icons.get(key);
            if (icon != null) {
                row.icon.setImageBitmap(icon);
            } else {
                row.icon.setImageDrawable(null);
            }
            return row;
        }
    }

    private final class Row extends LinearLayout {
        final ImageView icon;
        final TextView name;

        Row(Context context) {
            super(context);
            setOrientation(HORIZONTAL);
            setGravity(Gravity.CENTER_VERTICAL);
            setPadding(dp(16), dp(8), dp(16), dp(8));
            icon = new ImageView(context);
            addView(icon, new LayoutParams(dp(44), dp(44)));
            name = new TextView(context);
            name.setTextSize(16);
            name.setSingleLine(true);
            name.setEllipsize(android.text.TextUtils.TruncateAt.END);
            LayoutParams lp = new LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            lp.leftMargin = dp(16);
            addView(name, lp);
            setBackgroundColor(Color.TRANSPARENT);
        }
    }
}
