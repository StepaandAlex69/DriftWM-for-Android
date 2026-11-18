package dev.drift.launcher;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.graphics.Insets;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.HapticFeedbackConstants;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.graphics.Typeface;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.List;

/**
 * Home screen. Owns no workspace logic: it forwards touches to the Rust core,
 * draws its display list, and carries out the events the core emits (start an
 * app, open the app list, show the dock menu, save).
 */
public final class LauncherActivity extends Activity implements CanvasView.Host, DrawerView.Callback, FolderView.Callback {
    private static final long SAVE_DEBOUNCE_MS = 400;

    private AppRepo repo;
    private IconCache icons;
    private ThumbCache thumbs;
    private CanvasView canvas;
    private DrawerView drawer;
    private FolderView folder;
    private boolean resumed;
    private String langAtCreate = "";
    private FrameLayout root;
    private Toast toast;

    private HandlerThread saveThread;
    private Handler saveHandler;
    private final Runnable saveTask = Native::save; // runs on the save thread only
    private OnBackInvokedCallback backCallback;

    private boolean ready;

    private Appearance appearance = Appearance.defaults();
    private long appearanceRev = -1;
    private int appearanceGen;

    @Override protected void attachBaseContext(android.content.Context base) {
        super.attachBaseContext(LocaleHelper.wrap(base));
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        PerfStats.activityCreated();
        langAtCreate = LocaleHelper.forced(this);
        installCrashRecorder();
        String previous = readCrash();
        if (previous != null) {
            showCrash(previous);
            return;
        }
        try {
            setUp();
            ready = true;
            offerPreviews();
        } catch (Throwable t) {
            String text = describe(Thread.currentThread(), t);
            writeCrash(text);
            showCrash(text);
        }
    }

    private void setUp() {
        Window w = getWindow();
        w.setDecorFitsSystemWindows(false);
        w.setNavigationBarContrastEnforced(false);

        File stateFile = new File(getFilesDir(), "workspace.json");
        int loaded = Native.init(stateFile.getAbsolutePath());
        if (loaded == 2) {
            toast(getString(R.string.toast_restored_backup));
        } else if (loaded == 3) {
            toast(getString(R.string.toast_state_unreadable));
        }

        saveThread = new HandlerThread("drift-save");
        saveThread.start();
        saveHandler = new Handler(saveThread.getLooper());

        float density = getResources().getDisplayMetrics().density;
        repo = new AppRepo(this);
        icons = new IconCache(this, repo, density);
        repo.attachIcons(icons);
        thumbs = new ThumbCache(this);
        ThumbStore.setListener(thumbs);
        thumbs.setListener(() -> canvas.invalidate());
        canvas = new CanvasView(this, icons, thumbs);
        drawer = new DrawerView(this, repo, icons);
        folder = new FolderView(this, repo, icons);
        canvas.setHost(this);
        drawer.setCallback(this);
        folder.setCallback(this);
        icons.setListener(() -> {
            canvas.invalidate();
            drawer.iconsLoaded();
            folder.iconsLoaded();
        });
        repo.setListener(() -> {
            drawer.refresh();
            folder.refresh();
            canvas.invalidate();
        });

        root = new FrameLayout(this);
        root.addView(canvas, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(folder, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(drawer, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.setOnApplyWindowInsetsListener((v, wi) -> {
            int bars = WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout();
            Insets sb = wi.getInsets(bars);
            Insets sg = wi.getInsets(WindowInsets.Type.systemGestures());
            Insets ime = wi.getInsets(WindowInsets.Type.ime());
            // Keep the dock out of the back-gesture zones at the sides.
            ThumbStore.cropTop = sb.top;
            ThumbStore.cropBottom = sb.bottom;
            canvas.setInsets(Math.max(sb.left, sg.left), sb.top, Math.max(sb.right, sg.right), sb.bottom);
            drawer.setInsets(sb.left, sb.top, sb.right, Math.max(sb.bottom, ime.bottom));
            folder.setInsets(sb.left, sb.top, sb.right, sb.bottom);
            return WindowInsets.CONSUMED;
        });
        setContentView(root);
        refreshAppearance();

        if (Build.VERSION.SDK_INT >= 33) {
            backCallback = this::handleBack;
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT, backCallback);
        }
        repo.start();
    }

    @Override protected void onResume() {
        super.onResume();
        if (!ready) return;
        long t0 = System.nanoTime();
        PerfStats.resumeStarted();
        resumed = true;
        if (!LocaleHelper.forced(this).equals(langAtCreate)) { // language was changed in settings
            recreate();
            return;
        }
        Native.resetClock();
        refreshAppearance(); // may have changed in Drift settings
        repo.refreshIfStale(); // LauncherApps callbacks keep the list current; full read at most once a minute
        icons.retryFailed();
        canvas.invalidate();
        PerfStats.RESUME.add(System.nanoTime() - t0, 0);
    }

    @Override protected void onPause() {
        resumed = false;
        // The process can be killed while another app is in front: persist now, but without file I/O
        // on the main thread. The snapshot is taken here (memory only); the save thread writes it.
        if (ready) {
            saveHandler.removeCallbacks(saveTask);
            final String json = Native.snapshot();
            if (json != null) saveHandler.post(() -> Native.writeSnapshot(json));
        }
        super.onPause();
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (!ready) return;
        boolean alreadyHome = resumed; // Home pressed while the launcher is already in front
        if (drawer.isShowing()) drawer.hide();
        if (folder.isShowing()) folder.close();
        if (alreadyHome) {
            Native.homePressed(); // deselect / back to the home view
        } else {
            Native.onHome(); // returning from an app: never move the camera
        }
        canvas.invalidate();
    }

    @Override protected void onDestroy() {
        if (repo != null) repo.stop();
        if (Build.VERSION.SDK_INT >= 33 && backCallback != null) {
            getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(backCallback);
        }
        ThumbStore.setListener(null);
        if (saveThread != null) saveThread.quitSafely();
        super.onDestroy();
    }

    @SuppressWarnings("deprecation")
    @Override public void onBackPressed() {
        // Only reached below API 33 (or without the predictive-back callback).
        handleBack();
    }

    private void handleBack() {
        if (!ready) return;
        if (drawer.isShowing()) {
            drawer.hide();
        } else if (folder.isShowing()) {
            folder.close();
        } else if (Native.onBack()) {
            canvas.invalidate();
        }
        // A home screen has nothing to go back to: swallow the gesture.
    }

    // ---- events from the core --------------------------------------------------

    @Override public void onCoreEvents(List<String> events) {
        for (String ev : events) {
            String[] p = ev.split("\t", 2);
            String arg = p.length > 1 ? p[1] : "";
            switch (p[0]) {
                case "launch":
                    launch(arg);
                    break;
                case "drawer":
                    showDrawer(parseInt(arg, -1));
                    break;
                case "dockmenu": {
                    int slot = parseInt(arg, 0);
                    if (canvas.folderIdAtSlot(slot) >= 0) showFolderMenu(slot, canvas.folderIdAtSlot(slot));
                    else showDockMenu(slot);
                    break;
                }
                case "dockempty":
                    showEmptySlotMenu(parseInt(arg, 0));
                    break;
                case "folder":
                    openFolder(parseInt(arg, -1));
                    break;
                case "toast":
                    coreToast(arg);
                    break;
                case "haptic":
                    canvas.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS);
                    break;
                case "save":
                    saveHandler.removeCallbacks(saveTask);
                    saveHandler.postDelayed(saveTask, SAVE_DEBOUNCE_MS);
                    break;
                default:
                    break;
            }
        }
    }

    private void launch(String key) {
        if (drawer.isShowing()) {
            drawer.hide();
        }
        if (!repo.launch(key)) {
            toast(getString(R.string.toast_cant_open, repo.label(key)));
            repo.scheduleRefresh();
        }
    }

    /** Reloads colours and the background picture when Drift settings changed them. */
    private void refreshAppearance() {
        final long rev = Appearance.rev(this);
        if (rev == appearanceRev) return;
        appearanceRev = rev;
        final int gen = ++appearanceGen; // any load still running for an older state is dropped
        Appearance quick = Appearance.read(this);
        // With a picture on screen, keep it until the new one is decoded instead of flashing plain colours.
        if (quick.source != Appearance.SOURCE_IMAGE || appearance.bitmap == null) applyAppearance(quick);
        if (quick.source != Appearance.SOURCE_IMAGE) return;
        final android.content.Context app = getApplicationContext();
        new Thread(() -> {
            Appearance full = Appearance.readWithBitmap(app);
            runOnUiThread(() -> {
                if (!isDestroyed() && gen == appearanceGen) applyAppearance(full);
            });
        }, "drift-appearance").start();
    }

    private void applyAppearance(Appearance a) {
        appearance = a;
        canvas.setAppearance(a);
        drawer.setAppearance(a);
        updateBars();
    }

    /**
     * System-bar icons must stay readable: dark icons over a light background, light icons over a
     * dark one. While the app list is open it is the list's colour that sits behind the bars.
     * Must run after setContentView: before that the window has no decor view (NPE on some Samsung builds).
     */
    private void updateBars() {
        try {
            WindowInsetsController ic = getWindow().getInsetsController();
            if (ic == null) return;
            boolean lightStatus;
            boolean lightNav;
            if (drawer != null && drawer.isShowing()) {
                lightStatus = appearance.pal.light;
                lightNav = lightStatus;
            } else {
                lightStatus = appearance.barLum > 0.6f;
                lightNav = (appearance.bitmap != null ? Appearance.lum(appearance.avg) : appearance.barLum) > 0.6f;
            }
            ic.setSystemBarsAppearance(lightStatus ? WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS : 0,
                WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS);
            ic.setSystemBarsAppearance(lightNav ? WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS : 0,
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
        } catch (RuntimeException ignored) {
            // cosmetic only
        }
    }

    /** One-time (up to three times) explanation of the optional preview service. */
    private void offerPreviews() {
        if (ThumbService.isEnabled(this)) return;
        android.content.SharedPreferences prefs = getSharedPreferences("drift", MODE_PRIVATE);
        int asked = prefs.getInt("preview_prompts", 0);
        if (asked >= 3) return;
        prefs.edit().putInt("preview_prompts", asked + 1).apply();
        dialog()
            .setTitle(R.string.previews_title)
            .setMessage(R.string.previews_message)
            .setPositiveButton(R.string.action_open_settings, (d, i) ->
                startActivity(new Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS)))
            .setNeutralButton(R.string.action_app_info, (d, i) ->
                startActivity(new Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                    android.net.Uri.parse("package:" + getPackageName()))))
            .setNegativeButton(R.string.action_not_now, null)
            .show();
    }

    private AlertDialog.Builder dialog() {
        return new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert);
    }

    private void showDrawer(int slot) {
        drawer.show(slot);
        updateBars();
    }

    private void showDockMenu(int slot) {
        String[] items = {getString(R.string.dock_replace), getString(R.string.dock_create_folder),
            getString(R.string.dock_remove)};
        dialog().setTitle(getString(R.string.dock_slot_title, slot + 1))
            .setItems(items, (d, which) -> {
                if (which == 0) {
                    Native.dockReplace(slot);
                } else if (which == 1) {
                    createFolder(slot);
                } else {
                    Native.dockRemove(slot);
                }
                canvas.invalidate();
            })
            .show();
    }

    private void showEmptySlotMenu(int slot) {
        String[] items = {getString(R.string.dock_add_app), getString(R.string.dock_create_folder)};
        dialog().setTitle(getString(R.string.dock_slot_title, slot + 1))
            .setItems(items, (d, which) -> {
                if (which == 0) Native.dockReplace(slot);
                else createFolder(slot);
                canvas.invalidate();
            })
            .show();
    }

    private void showFolderMenu(int slot, int id) {
        String[] items = {getString(R.string.folder_menu_open_folder), getString(R.string.folder_rename),
            getString(R.string.dock_remove)};
        dialog().setTitle(folderName(id))
            .setItems(items, (d, which) -> {
                if (which == 0) openFolder(id);
                else if (which == 1) showRename(id);
                else confirmRemoveFolder(slot, id);
            })
            .show();
    }

    private String folderName(int id) {
        String n = Native.dockFolderName(id);
        return n == null || n.isEmpty() ? getString(R.string.folder_default_name) : n;
    }

    private void createFolder(int slot) {
        int id = Native.dockCreateFolder(slot);
        if (id < 0) {
            toast(getString(R.string.toast_folder_failed));
            return;
        }
        canvas.invalidate();
        openFolder(id); // the slot's app is its first app
    }

    private void openFolder(int id) {
        if (id < 0) return;
        folder.open(id);
    }

    /** Removing a folder from the dock keeps its apps in the app list; ask first when it is not empty. */
    private void confirmRemoveFolder(int slot, int id) {
        String[] apps = Native.dockFolderApps(id);
        int n = apps == null ? 0 : apps.length;
        Runnable remove = () -> {
            Native.dockRemove(slot);
            canvas.invalidate();
        };
        if (n == 0) {
            remove.run();
            return;
        }
        dialog().setMessage(getString(R.string.confirm_remove_folder, folderName(id), n))
            .setPositiveButton(R.string.action_remove, (d, w) -> remove.run())
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    private void showRename(int id) {
        final android.widget.EditText input = new android.widget.EditText(this);
        input.setSingleLine(true);
        input.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(32)});
        String cur = Native.dockFolderName(id);
        input.setText(cur == null ? "" : cur);
        input.setSelectAllOnFocus(true);
        input.setHint(R.string.folder_default_name);
        int pad = Math.round(20 * getResources().getDisplayMetrics().density);
        FrameLayout box = new FrameLayout(this);
        box.setPadding(pad, pad / 2, pad, 0);
        box.addView(input);
        dialog().setTitle(R.string.folder_rename)
            .setView(box)
            .setPositiveButton(android.R.string.ok, (d, w) -> {
                Native.dockRenameFolder(id, input.getText().toString().trim());
                folder.refresh();
                canvas.invalidate();
            })
            .setNegativeButton(android.R.string.cancel, null)
            .show();
    }

    // ---- folder callbacks ------------------------------------------------------

    @Override public void onLaunch(String key) {
        folder.close();
        launch(key);
    }

    @Override public void onAddApp(int folderId) {
        drawer.showForFolder(folderId, folderName(folderId));
        updateBars();
    }

    @Override public void onRename(int folderId) {
        showRename(folderId);
    }

    @Override public void onChanged() {
        canvas.invalidate();
    }

    @Override public void onClosed() {
        canvas.invalidate();
    }

    // ---- drawer callbacks ------------------------------------------------------

    @Override public void onPick(String key) {
        int fid = drawer.folderMode();
        if (fid >= 0) { // adding to a dock folder: the core owns the dock, Java only reports
            int res = Native.dockAddApp(fid, key);
            drawer.hide();
            if (res == 1) toast(getString(R.string.msg_folder_already_has_app));
            else if (res == 2) toast(getString(R.string.msg_folder_gone));
            else if (res == 3) toast(getString(R.string.msg_dock_app_not_installed));
            if (res == 2) folder.close();
            else folder.refresh();
            canvas.invalidate();
            return;
        }
        // The core decides: jump to an existing card, create + launch, or fill a dock slot.
        Native.drawerPick(key);
        drawer.hide();
        canvas.invalidate();
    }

    @Override public void onClose() {
        Native.drawerClosed();
        updateBars();
        canvas.invalidate();
    }

    @Override public void onSettings() {
        drawer.hide();
        startActivity(new Intent(this, SettingsActivity.class));
    }

    // ---- crash report: a home app that dies silently leaves the user with no launcher ----

    private File crashFile() {
        return new File(getFilesDir(), "crash.txt");
    }

    private static String describe(Thread t, Throwable e) {
        StringWriter sw = new StringWriter();
        PrintWriter pw = new PrintWriter(sw);
        pw.println("Thread: " + t.getName());
        pw.println("Android " + Build.VERSION.RELEASE + " (API " + Build.VERSION.SDK_INT + "), "
            + Build.MANUFACTURER + " " + Build.MODEL + ", ABIs " + java.util.Arrays.toString(Build.SUPPORTED_ABIS));
        e.printStackTrace(pw);
        pw.flush();
        return sw.toString();
    }

    private void writeCrash(String text) {
        try (FileWriter w = new FileWriter(crashFile())) {
            w.write(text);
        } catch (IOException ignored) {
            // nothing more we can do
        }
    }

    private void installCrashRecorder() {
        Thread.UncaughtExceptionHandler prev = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((t, e) -> {
            writeCrash(describe(t, e));
            if (prev != null) prev.uncaughtException(t, e);
        });
    }

    private String readCrash() {
        File f = crashFile();
        if (!f.exists()) return null;
        try {
            return new String(java.nio.file.Files.readAllBytes(f.toPath()), java.nio.charset.StandardCharsets.UTF_8);
        } catch (IOException e) {
            return null;
        }
    }

    private void showCrash(String text) {
        int p = Math.round(16 * getResources().getDisplayMetrics().density);
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(p, p * 3, p, p * 3);
        col.setBackgroundColor(0xFF101216);
        TextView head = new TextView(this);
        head.setText(R.string.crash_title);
        head.setTextColor(0xFFE5766B);
        head.setTextSize(15);
        col.addView(head);
        TextView body = new TextView(this);
        body.setText(text);
        body.setTextColor(0xFFE6E8EC);
        body.setTextSize(11);
        body.setTypeface(Typeface.MONOSPACE);
        body.setTextIsSelectable(true);
        ScrollView sv = new ScrollView(this);
        sv.addView(body);
        col.addView(sv, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        Button retry = new Button(this);
        retry.setText(R.string.crash_retry);
        retry.setOnClickListener(v -> {
            crashFile().delete();
            recreate();
        });
        col.addView(retry);
        setContentView(col);
    }

    // ---- helpers ---------------------------------------------------------------

    /** Toast event from the core: "<code>\t<arg>" is shown from the string resource msg_<code>. */
    private void coreToast(String payload) {
        String[] p = payload.split("\t", 2);
        int id = getResources().getIdentifier("msg_" + p[0], "string", getPackageName());
        String arg = p.length > 1 ? p[1] : "";
        if (id == 0) return; // unknown code: checked by tools/check_l10n.py, never show raw core text
        toast(getString(id, arg));
    }

    private void toast(String msg) {
        if (toast != null) toast.cancel();
        toast = Toast.makeText(this, msg, Toast.LENGTH_SHORT);
        toast.show();
    }

    private static int parseInt(String s, int fallback) {
        try {
            return Integer.parseInt(s.trim());
        } catch (NumberFormatException e) {
            return fallback;
        }
    }
}
