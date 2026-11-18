package dev.drift.launcher;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.os.LocaleList;

import java.util.Locale;

/**
 * UI language. By default Drift follows the system language (re-read on every activity
 * creation, and a system language change recreates the activities, so it is dynamic).
 * Settings can force English or Russian instead. Every activity wraps its base context
 * with {@link #wrap}; views and dialogs then resolve strings from the right resources.
 */
final class LocaleHelper {
    static final String SYSTEM = "";
    static final String EN = "en";
    static final String RU = "ru";

    private static final String PREFS = "drift_language";
    private static final String KEY = "forced";

    private LocaleHelper() {}

    private static SharedPreferences prefs(Context c) {
        return c.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /** "" (follow the system), "en" or "ru". */
    static String forced(Context c) {
        String v = prefs(c).getString(KEY, SYSTEM);
        return EN.equals(v) || RU.equals(v) ? v : SYSTEM;
    }

    static void setForced(Context c, String lang) {
        prefs(c).edit().putString(KEY, lang == null ? SYSTEM : lang).apply();
    }

    static Context wrap(Context base) {
        String lang = forced(base);
        if (lang.isEmpty()) return base;
        Locale locale = Locale.forLanguageTag(lang);
        Configuration cfg = new Configuration(base.getResources().getConfiguration());
        cfg.setLocale(locale);
        cfg.setLocales(new LocaleList(locale));
        return base.createConfigurationContext(cfg);
    }
}
