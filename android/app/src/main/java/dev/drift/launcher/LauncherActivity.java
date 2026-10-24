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
public final class LauncherActivity extends Activity implements CanvasView.Host, DrawerView.Callback {
    private static final long SAVE_DEBOUNCE_MS = 400;

    private AppRepo repo;
    private IconCache icons;
    private CanvasView canvas;
    private DrawerView drawer;
    private FrameLayout root;
    private Toast toast;

    private HandlerThread saveThread;
    private Handler saveHandler;
    private final Runnable saveTask = Native::save;
    private OnBackInvokedCallback backCallback;

    private boolean ready;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        installCrashRecorder();
        String previous = readCrash();
        if (previous != null) {
            showCrash(previous);
            return;
        }
        try {
            setUp();
            ready = true;
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
        WindowInsetsController ic = w.getInsetsController();
        if (ic != null) {
            ic.setSystemBarsAppearance(0,
                WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
        }

        File stateFile = new File(getFilesDir(), "workspace.json");
        int loaded = Native.init(stateFile.getAbsolutePath());
        if (loaded == 2) {
            toast("Workspace restored from backup");
        } else if (loaded == 3) {
            toast("Saved workspace was unreadable; started empty (old file kept)");
        }

        saveThread = new HandlerThread("drift-save");
        saveThread.start();
        saveHandler = new Handler(saveThread.getLooper());

        float density = getResources().getDisplayMetrics().density;
        repo = new AppRepo(this);
        icons = new IconCache(repo, density);
        canvas = new CanvasView(this, icons);
        drawer = new DrawerView(this, repo, icons);
        canvas.setHost(this);
        drawer.setCallback(this);
        icons.setListener(() -> {
            canvas.invalidate();
            drawer.iconsLoaded();
        });
        repo.setListener(() -> {
            drawer.refresh();
            canvas.invalidate();
        });

        root = new FrameLayout(this);
        root.addView(canvas, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(drawer, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.setOnApplyWindowInsetsListener((v, wi) -> {
            int bars = WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout();
            Insets sb = wi.getInsets(bars);
            Insets sg = wi.getInsets(WindowInsets.Type.systemGestures());
            Insets ime = wi.getInsets(WindowInsets.Type.ime());
            // Keep the dock out of the back-gesture zones at the sides.
            canvas.setInsets(Math.max(sb.left, sg.left), sb.top, Math.max(sb.right, sg.right), sb.bottom);
            drawer.setInsets(sb.left, sb.top, sb.right, Math.max(sb.bottom, ime.bottom));
            return WindowInsets.CONSUMED;
        });
        setContentView(root);

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
        Native.resetClock();
        repo.scheduleRefresh();
        canvas.invalidate();
    }

    @Override protected void onPause() {
        // The process can be killed while another app is in front: persist now.
        if (ready) {
            saveHandler.removeCallbacks(saveTask);
            Native.save();
        }
        super.onPause();
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (!ready) return;
        // Home pressed while the launcher is already showing.
        if (drawer.isShowing()) {
            drawer.hide();
        }
        Native.onHome();
        canvas.invalidate();
    }

    @Override protected void onDestroy() {
        if (repo != null) repo.stop();
        if (Build.VERSION.SDK_INT >= 33 && backCallback != null) {
            getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(backCallback);
        }
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
                case "dockmenu":
                    showDockMenu(parseInt(arg, 0));
                    break;
                case "toast":
                    toast(arg);
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
            toast("Can't open " + repo.label(key));
            repo.scheduleRefresh();
        }
    }

    private void showDrawer(int slot) {
        drawer.show(slot);
    }

    private void showDockMenu(int slot) {
        String[] items = {"Replace app", "Remove from dock"};
        new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle("Dock slot " + (slot + 1))
            .setItems(items, (d, which) -> {
                if (which == 0) {
                    Native.dockReplace(slot);
                } else {
                    Native.dockRemove(slot);
                }
                canvas.invalidate();
            })
            .show();
    }

    // ---- drawer callbacks ------------------------------------------------------

    @Override public void onPick(String key) {
        // The core decides: jump to an existing card, create + launch, or fill a dock slot.
        Native.drawerPick(key);
        drawer.hide();
        canvas.invalidate();
    }

    @Override public void onClose() {
        Native.drawerClosed();
        canvas.invalidate();
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
        head.setText("Drift crashed. Details (select and copy, or send to the developer):");
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
        retry.setText("Clear report and retry");
        retry.setOnClickListener(v -> {
            crashFile().delete();
            recreate();
        });
        col.addView(retry);
        setContentView(col);
    }

    // ---- helpers ---------------------------------------------------------------

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
