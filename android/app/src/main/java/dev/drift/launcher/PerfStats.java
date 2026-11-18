package dev.drift.launcher;

/**
 * Cheap timing counters for Diagnostics > Performance. Nothing runs in the background and
 * nothing is logged: each measured step adds one number (count, sum, last, max), so leaving it
 * on costs a few nanoseconds per frame. Read by DiagnosticsActivity in the same process.
 */
final class PerfStats {
    private PerfStats() {}

    static final class Stat {
        final int labelRes;
        private long count;
        private long sum;
        private long last;
        private long max;
        private long over; // samples above the budget

        Stat(int labelRes) {
            this.labelRes = labelRes;
        }

        synchronized void add(long nanos, long budgetNanos) {
            count++;
            sum += nanos;
            last = nanos;
            if (nanos > max) max = nanos;
            if (budgetNanos > 0 && nanos > budgetNanos) over++;
        }

        synchronized void reset() {
            count = sum = last = max = over = 0;
        }

        /** {count, avg, last, max, over} in nanoseconds (count and over are plain numbers). */
        synchronized long[] snapshot() {
            return new long[] {count, count == 0 ? 0 : sum / count, last, max, over};
        }
    }

    /** One 60 Hz frame; frame work above this is a dropped frame at 60 fps. */
    static final long FRAME_BUDGET_NS = 16_600_000L;

    static final Stat COLD_START = new Stat(R.string.perf_cold_start);
    static final Stat RESUME = new Stat(R.string.perf_resume);
    static final Stat APPS_FETCH = new Stat(R.string.perf_apps_fetch);
    static final Stat ICON_LOAD = new Stat(R.string.perf_icon_load);
    static final Stat THUMB_LOAD = new Stat(R.string.perf_thumb_load);
    static final Stat NATIVE_RENDER = new Stat(R.string.perf_native_render);
    static final Stat JAVA_DRAW = new Stat(R.string.perf_java_draw);
    static final Stat DESKTOP_REFRESH = new Stat(R.string.perf_desktop_refresh);

    static final Stat[] ALL = {
        COLD_START, RESUME, APPS_FETCH, ICON_LOAD, THUMB_LOAD, NATIVE_RENDER, JAVA_DRAW, DESKTOP_REFRESH
    };

    /** Set once per process by the first LauncherActivity.onCreate; the first drawn frame then closes the cold-start measurement. */
    private static boolean coldPending;
    private static boolean coldSeen;
    /** Set by onResume (System.nanoTime); the next drawn frame closes the "desktop refresh" measurement. */
    private static long refreshStartNs;

    static void activityCreated() {
        if (!coldSeen) {
            coldSeen = true;
            coldPending = true;
        }
    }

    static void resumeStarted() {
        refreshStartNs = System.nanoTime();
    }

    /** Called after every frame (main thread): a couple of comparisons when there is nothing to record. */
    static void frameDrawn() {
        if (coldPending) {
            coldPending = false;
            long ms = android.os.SystemClock.elapsedRealtime() - android.os.Process.getStartElapsedRealtime();
            COLD_START.add(ms * 1_000_000L, 0);
        }
        if (refreshStartNs != 0) {
            DESKTOP_REFRESH.add(System.nanoTime() - refreshStartNs, 0);
            refreshStartNs = 0;
        }
    }

    static void resetAll() {
        for (Stat s : ALL) s.reset();
    }
}
