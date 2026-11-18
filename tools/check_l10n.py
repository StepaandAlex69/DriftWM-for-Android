#!/usr/bin/env python3
"""Localization gate (runs in CI on every change).

Fails when:
  * strings.xml (EN) and values-ru/strings.xml have different keys or different placeholders,
  * a Russian string is not Russian (no Cyrillic) and is not on the short "same in both" list,
  * the generated XML is out of date compared to tools/strings_table.py,
  * Java code uses R.string.X that is not defined,
  * a user-visible literal is passed to a UI call (setText, setTitle, Toast.makeText, ...) in Java,
  * a toast code in rust/src/messages.rs has no msg_<code> string in both languages.
Add `// l10n-ignore` at the end of a line to exempt a deliberate literal.
"""
import os
import re
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
RES = os.path.join(ROOT, "android", "app", "src", "main", "res")
JAVA = os.path.join(ROOT, "android", "app", "src", "main", "java")
MESSAGES = os.path.join(ROOT, "rust", "src", "messages.rs")

SAME_IN_BOTH = {"app_name", "thumb_service_label", "perf_resume", "lang_english", "lang_russian"}
SINKS = {
    "setText", "setTitle", "setMessage", "setHint", "setContentDescription", "setPositiveButton",
    "setNegativeButton", "setNeutralButton", "makeText", "text", "caption", "section", "row",
    "segment", "colorRow", "segmented", "setLabel", "setSubtitle",
}
errors = []


def err(msg):
    errors.append(msg)


def load(folder):
    path = os.path.join(RES, folder, "strings.xml")
    if not os.path.exists(path):
        err("missing " + path)
        return {}
    out = {}
    for el in ET.parse(path).getroot().iter("string"):
        out[el.get("name")] = el.text or ""
    return out


def placeholders(s):
    return sorted(re.findall(r"%\d+\$[sd]|%[sd]", s))


en = load("values")
ru = load("values-ru")

for k in sorted(set(en) - set(ru)):
    err("missing Russian string: " + k)
for k in sorted(set(ru) - set(en)):
    err("Russian string without English original: " + k)
for k in sorted(set(en) & set(ru)):
    if placeholders(en[k]) != placeholders(ru[k]):
        err("placeholders differ in '%s': %s vs %s" % (k, placeholders(en[k]), placeholders(ru[k])))
    if k not in SAME_IN_BOTH and not re.search("[Ѐ-ӿ]", ru[k]):
        err("Russian string has no Cyrillic (untranslated?): " + k)
    if not en[k].strip() or not ru[k].strip():
        err("empty string: " + k)

# generated files must match the table
sys.path.insert(0, os.path.join(ROOT, "tools"))
try:
    from strings_table import STRINGS
    keys = [r[0] for r in STRINGS]
    if len(keys) != len(set(keys)):
        err("duplicate keys in strings_table.py")
    if set(keys) != set(en):
        err("strings.xml is out of date: run python3 tools/gen_strings.py")
except ImportError:
    err("tools/strings_table.py missing")

# Java sources
string_ref = re.compile(r"(?<!android\.)\bR\.string\.(\w+)")
used = set()


def literals_in(args):
    return re.findall(r'"((?:[^"\\]|\\.)*)"', args)


def balanced(src, i):
    """src[i] == '('; return the index of the matching ')' skipping string literals."""
    depth = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c == '"':
            i += 1
            while i < n and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c == "'":
            i += 1
            while i < n and src[i] != "'":
                i += 2 if src[i] == "\\" else 1
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return n


for dp, _, files in os.walk(JAVA):
    for f in files:
        if not f.endswith(".java"):
            continue
        path = os.path.join(dp, f)
        src = open(path, encoding="utf-8").read()
        for m in string_ref.finditer(src):
            used.add(m.group(1))
            if m.group(1) not in en:
                err("%s: R.string.%s is not defined" % (f, m.group(1)))
        for m in re.finditer(r"\b(\w+)\s*\(", src):
            if m.group(1) not in SINKS:
                continue
            # skip declarations such as "private TextView text(String s, ..."
            before = src[max(0, m.start() - 40):m.start()]
            if re.search(r"\b(private|static|void|TextView|LinearLayout|String)\s+$", before):
                continue
            end = balanced(src, m.end() - 1)
            args = src[m.end():end]
            for lit in literals_in(args):
                if re.match(r"^[\w.+-]+:", lit) or "://" in lit:  # URIs and URL fragments, not UI text
                    continue
                if re.search(r"[A-Za-zЀ-ӿ]{2,}", lit):
                    line = src.count("\n", 0, m.start()) + 1
                    text_line = src.splitlines()[line - 1]
                    if "l10n-ignore" in text_line:
                        continue
                    err("%s:%d: hard-coded UI text %r in %s(...); use a string resource" % (f, line, lit[:40], m.group(1)))

# toast codes from the core
if os.path.exists(MESSAGES):
    codes = re.findall(r'pub const [A-Z_]+: &str = "([a-z_]+)";', open(MESSAGES, encoding="utf-8").read())
    if not codes:
        err("no toast codes found in rust/src/messages.rs")
    for c in codes:
        for lang, table in (("en", en), ("ru", ru)):
            if "msg_" + c not in table:
                err("toast code '%s' has no msg_%s in %s" % (c, c, lang))
        used.add("msg_" + c)
    # the core must not produce user-facing text of its own
    for f in os.listdir(os.path.join(ROOT, "rust", "src")):
        if not f.endswith(".rs") or f == "messages.rs":
            continue
        for i, line in enumerate(open(os.path.join(ROOT, "rust", "src", f), encoding="utf-8"), 1):
            if re.search(r'toast(_with)?\(\s*"', line) and "#[cfg(test)]" not in line:
                err("rust/src/%s:%d: toast with a literal instead of a messages:: code" % (f, i))
else:
    err("rust/src/messages.rs missing")

unused = sorted(k for k in en if k not in used and not k.startswith("msg_"))
if unused:
    print("note: strings not referenced from Java (manifest/resources may use them): " + ", ".join(unused))

if errors:
    print("l10n check FAILED (%d problems):" % len(errors))
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print("l10n check OK: %d strings, EN/RU in sync" % len(en))
