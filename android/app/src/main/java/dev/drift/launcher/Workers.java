package dev.drift.launcher;

import android.os.Process;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Background executors that never compete with the UI thread: lowered thread priority. */
final class Workers {
    private Workers() {}

    /** Single worker thread at the given android.os.Process priority (e.g. THREAD_PRIORITY_BACKGROUND). */
    static ExecutorService single(String name, int priority) {
        return Executors.newSingleThreadExecutor(r -> new Thread(() -> {
            Process.setThreadPriority(priority);
            r.run();
        }, name));
    }
}
