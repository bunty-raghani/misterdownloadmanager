"""Collect every translatable English text of the app into i18n/en.tsv (M16, D-019).

    python tools/i18n/extract.py [--check]

Rows are craft-i18n catalog rows (context, source, translation); in en.tsv the translation is
the source itself. Found:
  * explicit `t("…")`, `tc("ctx", "…")`, `tn(n, "one", "other")` (an `@plural` row);
  * labels given to widgets that translate them: `button`, `button_light`, `checkbox`,
    `checkbox_light`, `radio_light`, dialog titles (`window(ctx, id, "Title", …)`, `question(…)`), light tabs
    (`label: "…"`), tree rows (context ""), Options' page titles, group boxes and labels
    (`header`, `group`, `disabled_label`);
  * labels given to a helper that hands them on to one of those (a closure in the same file,
    `let btn = |ui, x, label| winui::button(…, label, …)`; a function or method in any file),
    found by themselves, wrappers of wrappers too (#80);
  * a literal alone on its line under a `// i18n:` note (or with one at its end): a text chosen
    first (an `if` arm, a tuple, `Some(…)`) and translated where it is shown;
  * menu entries `Item`/`I`/`Radio`/`Sub`/`off`/`by` (context "menu"), toolbar buttons
    `(Icon::…, "Label", …)` ("toolbar"), list column titles (`TITLES`, "column"), the menu bar
    (`MENUS`, "menubar") and unit templates ("unit": `{n} MB` …).

Each row is preceded by a translator note (`#. …` lines, comments to the catalog parser): what
the text is (button, menu command, column title…), where it appears, what its placeholders
hold, and any note written in the code as a `// i18n: …` comment on the line above the text or
at the end of its line (part of speech, what it refers to). The translation tool sends the
notes with the text.
`--check` fails when en.tsv is out of date (CI), or when the helper detection stops finding the
labels of its self-test.
"""

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from wrap import split_args  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "crates" / "app" / "src"
OUT = ROOT / "i18n" / "en.tsv"
# The browser extension's texts (its name, description, right-click command): rows of context
# "extension", which tools/extension/locales.py turns into its _locales.
EXTENSION = ROOT / "extension" / "_locales" / "en" / "messages.json"

LIT = r'"((?:[^"\\]|\\.)*)"'
LITERAL = re.compile(r"^\s*" + LIT + r"\s*$")

# Calls whose n-th argument (0-based) is a label: (index, context; None = the first argument).
SINKS = {
    "button": (2, ""), "button_light": (2, ""), "checkbox": (3, ""), "checkbox_until": (3, ""),
    "checkbox_light": (3, ""), "checkbox_light_until": (3, ""), "radio_light_until": (3, ""),
    "radio_light": (3, ""), "window": (2, ""), "question": (2, ""), "notice": (2, ""), "row": (1, ""), "t": (0, ""), "tc": (1, None),
    "tn": (1, "@plural"), "mark": (0, ""), "branch": (0, ""),
    "Item": (0, "menu"), "I": (0, "menu"), "Radio": (0, "menu"), "Sub": (0, "menu"),
    "off": (0, "menu"), "by": (0, "menu"), "link": (4, ""),
    # Options' own page helpers (options.rs), which translate their labels.
    "header": (3, ""), "group": (6, ""), "disabled_label": (3, ""), "page_title": (4, ""),
    # A greyed control's reason on hover (winui::why_disabled).
    "why_disabled": (2, ""),
    # A button's width measured for its label (winui::button_fit_width): the label of a button.
    "button_fit_width": (1, ""),
    # Where a checkbox's caption ends (winui::checkbox_caption_end); a page title's lines before a
    # control (options.rs).
    "checkbox_caption_end": (2, ""), "page_title_lines": (2, ""),
}
SINK_CALL = re.compile(
    r"\b(?:winui::|MenuEntry::|crate::i18n::|i18n::)?(" + "|".join(SINKS) + r")\s*\(")
OTHER = [
    (re.compile(r"\blabel:\s*" + LIT), "", "tab"),
    (re.compile(r"\(\s*Icon::\w+\s*,\s*" + LIT), "toolbar", "toolbar button"),
]
CONST_ARRAYS = [
    (re.compile(r"pub const TITLES: \[&str; COLS\] = \[(.*?)\];", re.S), "column",
     "column title in the download list"),
    (re.compile(r"const MENUS: \[\(&str, f32, f32, f32\); 5\] = \[(.*?)\];", re.S), "menubar",
     "menu bar title"),
    # Column filters' value lists (list.rs), shown through translating checkboxes.
    (re.compile(r"pub fn filter_values\((.*?)\n\}", re.S), "",
     "value in a column's filter list"),
]
# Tables of (key, what it does): Help → Keyboard shortcuts.
PAIRS = re.compile(r"const SHORTCUTS: \[\(&str, &str\); \d+\] = \[(.*?)\];", re.S)
# rustfmt puts a long pair on its own lines with a trailing comma.
PAIR = re.compile(r"\(\s*" + LIT + r"\s*,\s*" + LIT + r"\s*,?\s*\)")
# Units craft-i18n looks up for sizes and speeds (model.rs).
UNITS = ["{n} KB", "{n} MB", "{n} GB", "{n} KB/sec", "{n} MB/sec"]

# What a text given to each call is.
ROLES = {
    "button": "button", "button_light": "button", "checkbox": "checkbox",
    "checkbox_light": "checkbox", "checkbox_light_until": "checkbox", "checkbox_until": "checkbox",
    "radio_light": "option (radio button)", "radio_light_until": "option (radio button)",
    "window": "window title", "question": "window title", "notice": "window title", "row": "item in the Categories tree",
    "branch": "item in the Categories tree", "Item": "menu command", "I": "menu command",
    "Radio": "menu choice", "Sub": "submenu", "off": "menu command", "by": "menu choice",
    "tn": "count", "link": "link (opens a web page)", "header": "page title",
    "group": "group box title", "disabled_label": "label", "page_title": "page title",
    "why_disabled": "tooltip (why a control is greyed)", "button_fit_width": "button",
    "checkbox_caption_end": "checkbox", "page_title_lines": "page title",
}
CONTEXT_ROLES = {
    "menu": "menu command", "toolbar": "toolbar button", "column": "column title",
    "menubar": "menu bar title", "unit": "unit after a number", "key": "key name",
}
# Where the texts of each file appear.
PLACES = {
    "app.rs": "the main window",
    "batch.rs": "the Batch download window",
    "checksum.rs": "the Checksum window",
    "deleter.rs": "the main window's messages (deleting downloads' files)",
    "chrome.rs": "the main window (menus, toolbar, Categories pane)",
    "dialogs.rs": "a dialog box",
    "extractq.rs": "extracting archives (a download's log, the Archive password window)",
    "find.rs": "the Find window",
    "grabber.rs": "the Link grabber window",
    "help.rs": "the Help menu's windows (About, updates, shortcuts, Tell a friend)",
    "list.rs": "the download list",
    "model.rs": "the download list (sizes, speeds, status)",
    "options.rs": "the Options window",
    "progress.rs": "a download's progress window",
    "properties.rs": "a download's Properties window",
    "queue.rs": "the queues",
    "quota.rs": "the Scheduler's download limits",
    "scheduler.rs": "the Scheduler window",
    "settings.rs": "the settings",
    "torrentdlg.rs": "the Add torrent window",
    "torrents.rs": "torrent downloads",
    "tray.rs": "the tray icon's menu",
    "videodlg.rs": "the Download video window",
    "winui.rs": "standard windows",
    "extension": "the browser extension",
}
# What the placeholders hold; any other is named in a `// i18n:` note.
PLACEHOLDERS = {
    "n": "a number", "size": "a file size such as \"4.2 MB\"", "bytes": "a number of bytes",
    "site": "a web site's name such as \"example.com\"",
    "category": "a category name such as \"Music\"", "host": "a server name such as \"example.com\"",
    "queue": "a queue name such as \"Main queue\"", "error": "an error message",
    "time": "a time of day", "algorithm": "a checksum algorithm such as \"SHA-256\"",
    "column": "a column title", "version": "a version number", "format": "a video format code",
    "source": "how the download was added, e.g. \"from the browser\"",
    "text": "text the user searched for", "mb": "a number of megabytes",
    "hours": "a number of hours", "h": "hours", "m": "minutes", "s": "seconds",
    "name": "a file name", "count": "a number", "rate": "a speed such as \"1.2 MB/sec\"",
}
NOTE = re.compile(r"//\s*i18n:\s*(.*)$")
# A text chosen before it is translated (an `if`/`match` arm, a tuple, `Some(…)`, a `let`): a
# literal alone on its line, with a `// i18n:` note above it or at its end.
NOTED = re.compile(r"^\s*(?:Some\(\s*)?" + LIT + r"\s*\)?\s*[,;]?\s*(?://.*)?$")


def esc(s):
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


def unesc_rust(s):
    return (s.replace('\\"', '"').replace("\\n", "\n").replace("\\t", "\t")
            .replace("\\\\", "\\"))


def literal(s, span):
    m = LITERAL.match(re.sub(r"//[^\n]*", "", s[span[0]:span[1]]))
    return unesc_rust(m.group(1)) if m else None


def code_note(lines, *line_nos):
    """`// i18n: …` at the end of one of these lines, or in the comment lines right above."""
    notes = []
    for n in sorted(set(line_nos)):
        m = NOTE.search(lines[n])
        if m:
            notes.append(m.group(1).strip())
        k = n - 1
        above = []
        while k >= 0 and lines[k].lstrip().startswith("//"):
            m = NOTE.search(lines[k])
            if m and lines[k].lstrip().startswith("// i18n:"):
                above.insert(0, m.group(1).strip())
            k -= 1
        notes.extend(above)
    return notes


class Rows:
    def __init__(self):
        self.rows, self.plurals = {}, {}

    def add(self, key, role, file, notes=(), table=None):
        table = self.rows if table is None else table
        info = table.setdefault(key, {"uses": [], "notes": []})
        use = (role, file)
        if use not in info["uses"]:
            info["uses"].append(use)
        for n in notes:
            if n not in info["notes"]:
                info["notes"].append(n)


def in_comment(s, pos):
    """Whether `pos` is in a `//` comment (doc comments too): a call quoted in a comment is not
    a text of the interface. String literals before it on the line don't count (`"https://"`)."""
    start = s.rfind("\n", 0, pos) + 1
    before = re.sub(r'"(?:[^"\\]|\\.)*"', '""', s[start:pos])
    return "//" in before


def balanced_end(s, i):
    """Where the expression starting at `i` ends: its block's `}` when it starts with `{`,
    else the first `,` `;` or closing bracket outside brackets and strings."""
    depth, quote = 0, False
    start_block = s[i:i + 1] == "{"
    while i < len(s):
        c = s[i]
        if quote:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                quote = False
        elif c == '"':
            quote = True
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0 and start_block:
                return i + 1
            if depth < 0:
                return i
        elif c in ",;" and depth == 0:
            return i
        i += 1
    return i


def param_names(text):
    """A parameter list's names, in order (`ui: &mut Ui, label: &str` → ["ui", "label"])."""
    names, depth, cur = [], 0, ""
    for c in text + ",":
        if c in "<([":
            depth += 1
        elif c in ">)]":
            depth -= 1
        if c == "," and depth == 0:
            cur = cur.strip()
            if cur:
                name = cur.split(":")[0].strip()
                name = re.sub(r"^(?:&\s*)?(?:'\w+\s+)?(?:mut\s+)?", "", name)
                names.append(name)
            cur = ""
        else:
            cur += c
    return names


CLOSURE_DEF = re.compile(r"\blet\s+(\w+)\s*=\s*(?:move\s*)?\|([^|]*)\|")
FN_DEF = re.compile(r"\bfn\s+(\w+)\s*(?:<[^>(]*>)?\s*\(")
SELF = {"self", "&self", "&mut self", "mut self"}
# What a wrapper's argument may be around the parameter itself: `&label`, `t(label)`.
AROUND = re.compile(r"^&?\s*(?:(?:crate::)?(?:i18n::)?t\(\s*(\w+)\s*\)|(\w+))$")


def call_pattern(names):
    return re.compile(r"\b(?:winui::|MenuEntry::|crate::i18n::|i18n::|Self::|self\.|\w+::)?("
                      + "|".join(sorted(names, key=len, reverse=True)) + r")\s*\(")


def wrappers_in(s, sinks):
    """The closures and functions in `s` that hand one of their parameters to a sink: name →
    (parameter index, context, role, is a method). A method's index leaves `self` out."""
    found = {}
    defs = []
    for m in CLOSURE_DEF.finditer(s):
        body = m.end()
        while s[body:body + 1].isspace():
            body += 1
        if s.startswith("->", body):  # `|x| -> T { … }`
            body = s.find("{", body)
        defs.append((m.group(1), param_names(m.group(2)), body, balanced_end(s, body), False))
    for m in FN_DEF.finditer(s):
        spans, close = split_args(s, m.end() - 1)
        params = param_names(s[m.end():close])
        method = bool(params) and params[0] in SELF | {"self"}
        if method:
            params = params[1:]
        body = s.find("{", close)
        semi = s.find(";", close)
        if body < 0 or (0 <= semi < body):  # a declaration without a body
            continue
        defs.append((m.group(1), params, body, balanced_end(s, body), method))
    rx = call_pattern(sinks)
    for name, params, a, b, method in defs:
        if name in found or not params:
            continue
        for c in rx.finditer(s, a, b):
            if in_comment(s, c.start()):
                continue
            sink = c.group(1)
            if sink == name:
                continue
            n, ctx, role = sinks[sink]
            if ctx is None or ctx == "@plural":  # its context or plural is another argument
                continue
            spans, _ = split_args(s, c.end() - 1)
            if len(spans) <= n:
                continue
            am = AROUND.match(s[spans[n][0]:spans[n][1]].strip())
            p = am and (am.group(1) or am.group(2))
            if p in params:
                found[name] = (params.index(p), ctx, role, method)
                break
    return found


def all_sinks(sources):
    """SINKS, plus every function that hands a parameter to one (in any file), and per file the
    closures that do (`let btn = |ui, x, label| winui::button(ui, …, label, …)`): a label given
    to such a helper is translated where it is shown, so it belongs in the catalog (#80)."""
    base = {k: (n, ctx, ROLES.get(k, "text")) for k, (n, ctx) in SINKS.items()}
    global_ = dict(base)
    local = {f: {} for f in sources}
    for _ in range(4):  # a wrapper of a wrapper: until nothing new turns up
        grew = False
        fns = {}
        for f, s in sources.items():
            sinks = {**global_, **local[f]}
            for name, w in wrappers_in(s, sinks).items():
                defined_fn = re.search(r"\bfn\s+" + name + r"\b", s)
                if defined_fn and name not in base:
                    fns.setdefault(name, set()).add(w)
                elif not defined_fn and local[f].get(name) != w[:3]:
                    local[f][name] = w[:3]
                    grew = True
        for name, ws in fns.items():
            # A function name defined twice with different labels: each file keeps its own.
            if len(ws) == 1:
                w = next(iter(ws))
                if global_.get(name) != w[:3]:
                    global_[name] = w[:3]
                    grew = True
            else:
                for f, s in sources.items():
                    own = wrappers_in(s, {**global_, **local[f]}).get(name)
                    if own and local[f].get(name) != own[:3]:
                        local[f][name] = own[:3]
                        grew = True
        if not grew:
            break
    return {f: {**global_, **local[f]} for f in sources}


def collect():
    out = Rows()
    files = [f for f in sorted(SRC.glob("*.rs")) if f.name != "i18n.rs"]
    sources = {f.name: f.read_text(encoding="utf-8") for f in files}
    sinks_of = all_sinks(sources)
    for f in files:
        s = sources[f.name]
        lines = s.split("\n")
        sinks = sinks_of[f.name]
        sink_call = call_pattern(sinks)

        def line_of(pos):
            return s.count("\n", 0, pos)

        taken = set()  # lines whose literal a call above already took
        for m in sink_call.finditer(s):
            if s[max(0, m.start() - 3):m.start()] == "fn " or in_comment(s, m.start()):
                continue
            name = m.group(1)
            n, ctx, sink_role = sinks[name]
            spans, _ = split_args(s, m.end() - 1)
            if len(spans) <= n:
                continue
            text = literal(s, spans[n])
            if text is None or not any(ch.isalpha() for ch in text):
                continue
            taken.update(range(line_of(spans[n][0]), line_of(spans[n][1] - 1) + 1))
            # The call's line, and the text's own (a note may sit between the arguments).
            notes = code_note(lines, line_of(m.start()), line_of(spans[n][0]),
                              line_of(spans[n][1] - 1))
            if ctx == "@plural":
                other = literal(s, spans[2]) if len(spans) > 2 else None
                if other:
                    out.add((text, other), "count", f.name, notes, out.plurals)
                continue
            if ctx is None:
                ctx = literal(s, spans[0])
                if ctx is None:
                    continue
            role = CONTEXT_ROLES.get(ctx) or sink_role
            out.add((ctx, text), role, f.name, notes)
        for rx, ctx, role in OTHER:
            for m in rx.finditer(s):
                text = unesc_rust(m.group(1))
                if any(ch.isalpha() for ch in text) and not in_comment(s, m.start()):
                    out.add((ctx, text), role, f.name, code_note(lines, line_of(m.start())))
        for rx, ctx, role in CONST_ARRAYS:
            for m in rx.finditer(s):
                base = m.start(1)
                for lm in re.finditer(LIT, m.group(1)):
                    taken.add(line_of(base + lm.start()))
                    out.add((ctx, unesc_rust(lm.group(1))), role, f.name,
                            code_note(lines, line_of(base + lm.start())))
        for m in PAIRS.finditer(s):
            for pm in PAIR.finditer(m.group(1)):
                out.add(("key", unesc_rust(pm.group(1))), "key name or combination", f.name,
                        ["translate key names as the keyboard shows them (e.g. Strg for Ctrl)"])
                out.add(("", unesc_rust(pm.group(2))), "what a keyboard shortcut does", f.name)
        for k, line in enumerate(lines):
            nm = NOTED.match(line)
            if not nm or k in taken:
                continue
            notes = code_note(lines, k)
            text = unesc_rust(nm.group(1))
            if notes and any(ch.isalpha() for ch in text):
                out.add(("", text), "text", f.name, notes)
    for u in UNITS:
        out.add(("unit", u), "unit after a number", "model.rs")
    for m in json.loads(EXTENSION.read_text(encoding="utf-8")).values():
        # Chrome's $name$ placeholders are {name} in the catalogs, as everywhere else.
        text = re.sub(r"\$(\w+)\$", r"{\1}", m["message"])
        out.add(("extension", text), "text", "extension", [m["description"]])
    out.add(("{n} byte", "{n} bytes"), "count", "model.rs", (), out.plurals)
    return out.rows, out.plurals


def describe(texts, info):
    """The translator note for a row: its uses, its code notes, its placeholders, conventions."""
    uses = []
    for role, file in info["uses"]:
        place = PLACES.get(file, file)
        if role == "text" and file == "app.rs":
            uses.append("message or event log entry in the main window")
        else:
            uses.append(f"{role} in {place}")
    parts = ["; ".join(dict.fromkeys(uses))]
    parts += info["notes"]
    joined = " ".join(texts)
    names = list(dict.fromkeys(re.findall(r"\{(\w+)\}", joined)))
    held = [f"{{{p}}} = {PLACEHOLDERS[p]}" for p in names if p in PLACEHOLDERS]
    if held:
        parts.append("; ".join(held) + " (keep the braces as they are)")
    if re.search(r"&[^&\s]", joined):
        parts.append("& marks the keyboard shortcut letter: keep one & before a letter of the"
                     " translation")
    opens = {"button", "menu command", "toolbar button"}
    if any(t.rstrip().endswith("...") for t in texts):
        if any(role in opens for role, _ in info["uses"]):
            parts.append("the trailing ... means it opens a window: keep it")
        else:
            parts.append("the trailing ... means it is still going on: keep it")
    # A code note about the line break (an input-box sentence's) replaces the general one.
    if "\n" in joined and not any("line break" in n for n in info["notes"]):
        parts.append("\\n starts a new line: break lines where they fit the translation")
    return [p for p in parts if p]


def render(rows, plurals):
    lines = [
        "# Mister Download Manager: every English text of the interface (tools/i18n/extract.py).",
        "# context\tsource\ttranslation; a catalog for another language has the same rows.",
        "# A '#.' line is the translator note for the row under it.",
        "@id\tlanguage.name\tEnglish",
    ]
    for one, other in sorted(plurals):
        for note in describe([one, other], plurals[(one, other)]):
            lines.append(f"#. {esc(note)}")
        lines.append(f"@plural\t{esc(one)}|{esc(other)}\t{esc(one)}|{esc(other)}")
    for ctx, text in sorted(rows, key=lambda k: (k[0], k[1].lower(), k[1])):
        for note in describe([text], rows[(ctx, text)]):
            lines.append(f"#. {esc(note)}")
        lines.append(f"{esc(ctx)}\t{esc(text)}\t{esc(text)}")
    return "\n".join(lines) + "\n"


def plural_problems(plurals):
    """craft-i18n rejects a plural whose forms are empty or hold different placeholders (every
    translated form must hold `other`'s), so such a row could never be translated. Write the
    singular sentence for exactly one as its own `t()` and give `tn()` `{n}` in both forms:
    Russian and others use the "one" form for 21, 31…"""
    out = []
    for one, other in sorted(plurals):
        a, b = (set(re.findall(r"\{(\w+)\}", s)) for s in (one, other))
        if not one or not other or a != b:
            files = ", ".join(sorted({f for _, f in plurals[(one, other)]["uses"]}))
            out.append(f"{files}: plural forms differ in placeholders or are empty:"
                       f" {one!r} / {other!r}")
    return out


SELF_TEST = {
    "a.rs": '''
fn dialog(ui: &mut Ui) {
    let btn = |ui: &mut Ui, x: f32, label: &str, kind| {
        winui::button(ui, r(x), label, kind, false, None)
    };
    let with = btn(ui, 115.0, "Open with...", ButtonKind::Normal);
    let caption = |n: u32, label: &str| crate::i18n::t(label);
    caption(3, "Saved as");
    shared_head(ui, "Speed");
    self.note(ui, "A method's label");
}
''',
    "b.rs": '''
pub fn shared_head(ui: &Ui, label: &str) {
    let lift = |text: &str| winui::checkbox(ui, r(), &mut on, text);
    lift(label);
}
impl W {
    fn note(&self, ui: &Ui, text: &str) { winui::button_light(ui, r(), text, K, false); }
}
''',
}


def self_test():
    """The extractor must keep finding labels given through local helpers (#80): a closure, a
    closure through `t()`, a function in another file (a wrapper of a wrapper) and a method."""
    sinks = all_sinks(SELF_TEST)
    s = SELF_TEST["a.rs"]
    found = set()
    for m in call_pattern(sinks["a.rs"]).finditer(s):
        if s[max(0, m.start() - 3):m.start()] == "fn ":
            continue
        n = sinks["a.rs"][m.group(1)][0]
        spans, _ = split_args(s, m.end() - 1)
        text = literal(s, spans[n]) if len(spans) > n else None
        if text:
            found.add(text)
    want = {"Open with...", "Saved as", "Speed", "A method's label"}
    missing = want - found
    if missing:
        print(f"extract.py self-test: labels given through helpers not found: {sorted(missing)}")
    return not missing


def main():
    if not self_test():
        return 1
    rows, plurals = collect()
    problems = plural_problems(plurals)
    for p in problems:
        print(p)
    if problems:
        return 1
    text = render(rows, plurals)
    if "--check" in sys.argv:
        old = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if old != text:
            print("i18n/en.tsv is out of date: run python tools/i18n/extract.py")
            return 1
        return 0
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(text, encoding="utf-8", newline="")
    print(f"{len(rows)} texts, {len(plurals)} plural(s) -> {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
