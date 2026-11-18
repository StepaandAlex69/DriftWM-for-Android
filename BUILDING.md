# Building Drift Launcher

Rust core (`rust/`, crate `drift-core`) + a thin Java layer (`android/`).
GPL-3.0-or-later; the canvas/snapping/placement logic is derived from DriftWM.

## One-time setup (Linux)

    rustup target add aarch64-linux-android          # add x86_64-linux-android for an emulator
    cargo install cargo-ndk
    # Android SDK command-line tools, then:
    sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0" "ndk;28.2.13676358"
    export ANDROID_HOME=$HOME/Android/Sdk
    export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358   # NDK r28+: 16 KB page alignment

JDK 17+ is required by Gradle/AGP.

## Build and install

    cd android
    ./gradlew assembleDebug                 # runs `cargo ndk` for libdrift_core.so first
    adb install -r app/build/outputs/apk/debug/app-debug.apk
    # make it the home app (or press Home and choose "Drift"):
    adb shell cmd package set-home-activity dev.drift.launcher/.LauncherActivity

Emulator: `./gradlew assembleDebug -PdriftAbis=arm64-v8a,x86_64`

## Tests (no Android needed)

    cd rust && cargo test

## Debugging on the device

    adb logcat | grep -iE "drift|AndroidRuntime|UnsatisfiedLink"

## Checking system navigation with Drift as home

In-app: open "Drift diagnostics" from Drift's app list (search "diagnostics"), tap Copy/Share.

Same facts over adb (nothing is changed):

    adb shell cmd role get-role-holders android.app.role.HOME
    adb shell cmd overlay lookup android android:string/config_recentsComponentName
    adb shell dumpsys activity service SystemUIService | grep -i -E "recents|quickstep|overview"
    adb shell cmd package query-services -a android.intent.action.QUICKSTEP_SERVICE
    adb shell settings get secure navigation_mode
    adb logcat -b all | grep -i -E "quickstep|recents|OverviewProxy|gesture"   # while swiping on the bad device

A/B on the affected device: (1) Drift as home, previews service off -> test recents and the bottom swipe;
(2) previews service on -> repeat; (3) stock launcher -> repeat. Send the diagnostics report plus the logcat.

## Strings and languages (V6)

All user-visible text lives in `tools/strings_table.py` as `(key, English, Russian)`.
After editing it run `python3 tools/gen_strings.py` (writes `res/values/strings.xml` and
`res/values-ru/strings.xml`), then `python3 tools/check_l10n.py`. The check also runs in CI and fails on:
a missing translation, different placeholders, Russian text without Cyrillic, a string literal passed to
`setText/setTitle/Toast.makeText/...` in Java, an undefined `R.string.X`, or a toast code in
`rust/src/messages.rs` without `msg_<code>` in both languages. The language follows the system and can be
forced in Drift settings > Interface > Language.

## State format

`workspace.json` is versioned. Version 2 adds dock folders, the dock size and the camera. Version 1 files are
read and migrated (a copy is kept once as `workspace.json.v1`). Older builds refuse a newer version instead of
misreading it.
