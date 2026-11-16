from __future__ import annotations

import re
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parent.parent
# This fork has no private server: the hot path is the client shell and its UI.
HOT_PATH_SOURCES = (
    *sorted(p for p in (PROJECT_ROOT / "src" / "ui.rs",) if p.exists()),
    *sorted((PROJECT_ROOT / "src" / "ui").rglob("*.rs")),
)
# Presentation helpers now live with their consumers, not in src/app. Keep
# checking them (and the actual composition paths), rather than an empty legacy root.
PRESENTATION_SOURCES = (
    *HOT_PATH_SOURCES,
    PROJECT_ROOT / "src/client/shell.rs",
    *sorted(p for p in (PROJECT_ROOT / "src/client/shell").rglob("*.rs")
            if "tests" not in p.relative_to(PROJECT_ROOT).parts),
    PROJECT_ROOT / "src/client/core/view.rs",
    *sorted((PROJECT_ROOT / "src/client/core/view").rglob("*.rs")),
    PROJECT_ROOT / "src/selection.rs",
    *sorted((PROJECT_ROOT / "src/selection").rglob("*.rs")),
)
TEST_MODULE = re.compile(r"(?m)^#\[cfg\(test\)\]\s*\nmod\s+\w+\s*\{")
INPUT_STATE_CALL = re.compile(r"(?:\.|::)input_state\b")
KEYBOARD_STATE_ANSI_CALL = re.compile(
    r"(?:\.|::)(?:keyboard_state_ansi|kitty_keyboard_state_ansi)\b"
)
AGGREGATE_STATE_CALLS = (
    (INPUT_STATE_CALL, "aggregate terminal input state; add a narrow accessor"),
    (KEYBOARD_STATE_ANSI_CALL, "formatted keyboard state"),
)
FORBIDDEN_CALLS = (
    *AGGREGATE_STATE_CALLS,
    (
        re.compile(r"(?:\.|::)screen_text_snapshot\b"),
        "formatted terminal screen snapshot",
    ),
    (
        re.compile(r"\bforeground_job\s*\("),
        "process-tree inspection",
    ),
)


def blank_non_newlines(chars: list[str], start: int, end: int) -> None:
    for index in range(start, end):
        if chars[index] != "\n":
            chars[index] = " "


def mask_comments_and_literals(source: str) -> str:
    chars = list(source)
    index = 0
    while index < len(source):
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = len(source) if end == -1 else end
            blank_non_newlines(chars, index, end)
            index = end
            continue

        if source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(source) and depth > 0:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank_non_newlines(chars, index, end)
            index = end
            continue

        if source[index] == "r":
            quote = index + 1
            while quote < len(source) and source[quote] == "#":
                quote += 1
            if quote < len(source) and source[quote] == '"':
                suffix = '"' + "#" * (quote - index - 1)
                end = source.find(suffix, quote + 1)
                end = len(source) if end == -1 else end + len(suffix)
                blank_non_newlines(chars, index, end)
                index = end
                continue

        if source[index] == '"':
            end = index + 1
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            blank_non_newlines(chars, index, min(end, len(source)))
            index = end
            continue

        if source[index] == "'":
            end = index + 2
            if index + 1 < len(source) and source[index + 1] == "\\":
                end += 1
            if end < len(source) and source[end] == "'":
                end += 1
                blank_non_newlines(chars, index, end)
                index = end
                continue

        index += 1

    return "".join(chars)


def production_code(source: str) -> str:
    code = mask_comments_and_literals(source)
    chars = list(code)
    search_from = 0

    while test_module := TEST_MODULE.search(code, search_from):
        depth = 0
        end = test_module.end() - 1
        while end < len(code):
            if code[end] == "{":
                depth += 1
            elif code[end] == "}":
                depth -= 1
                if depth == 0:
                    end += 1
                    break
            end += 1
        blank_non_newlines(chars, test_module.start(), end)
        code = "".join(chars)
        search_from = end

    return code


def find_violations(paths, rules) -> list[str]:
    violations: list[str] = []
    for path in paths:
        code = production_code(path.read_text(encoding="utf-8"))
        for pattern, description in rules:
            for match in pattern.finditer(code):
                line = code.count("\n", 0, match.start()) + 1
                relative_path = path.relative_to(PROJECT_ROOT)
                violations.append(f"{relative_path}:{line}: {description}")
    return violations


class UiHotPathArchitectureTests(unittest.TestCase):
    def test_render_hot_paths_avoid_known_expensive_runtime_queries(self) -> None:
        violations = find_violations(HOT_PATH_SOURCES, FORBIDDEN_CALLS)

        self.assertEqual(
            violations,
            [],
            "Render/layout code must not perform pane-scaled expensive reads:\n"
            + "\n".join(violations),
        )

    def test_client_presentation_avoids_aggregate_terminal_state(self) -> None:
        relative_paths = {str(p.relative_to(PROJECT_ROOT)) for p in PRESENTATION_SOURCES}
        self.assertTrue({
            "src/client/core/view.rs", "src/client/shell/composition.rs",
            "src/selection/word.rs", "src/ui/colors.rs",
        }.issubset(relative_paths), "Missing client presentation sources")
        violations = find_violations(PRESENTATION_SOURCES, AGGREGATE_STATE_CALLS)

        self.assertEqual(
            violations,
            [],
            "Client presentation must use narrow terminal-state accessors:\n"
            + "\n".join(violations),
        )

    def test_shared_terminal_module_is_pure_and_runtime_independent(self) -> None:
        paths = [PROJECT_ROOT / "src/terminal.rs"]
        self.assertTrue(paths[0].is_file())
        rules = (
            (
                re.compile(r"\b(?:crate|super)::(?:client|pane|platform|ghostty)\b"),
                "shared terminal data/codecs must not depend on runtime owners",
            ),
            (
                re.compile(r"\b(?:std|tokio)::(?:io|fs|env|process|thread|sync|net|time)\b"),
                "shared terminal data/codecs must not perform I/O or own runtime state",
            ),
        )
        violations = find_violations(paths, rules)
        self.assertEqual(violations, [], "\n".join(violations))

    def test_shared_input_is_independent_of_host_runtime_and_shell_presentation(self) -> None:
        paths = sorted((PROJECT_ROOT / "src/input").glob("*.rs"))
        self.assertTrue(paths)
        rules = (
            (
                re.compile(r"\b(?:crate|super)::(?:client|pane|platform|ui)\b"),
                "shared input must not depend on runtime or presentation owners",
            ),
            (
                re.compile(r"\b(?:std|tokio)::(?:io|fs|env|process|thread|sync|net|time)\b"),
                "shared input models and parsers must not access the host runtime",
            ),
            (
                re.compile(r"\bcrossterm::(?:terminal|execute|queue)\b"),
                "query and mutate the host terminal in client/host_terminal",
            ),
            (
                re.compile(
                    r"\b(?:KeybindAction|KeybindDispatch|KeybindMatch|KeybindHelpGroup|"
                    r"HostGeometry|HostPixels|MouseProtocolMode|MouseProtocolEncoding)\b"
                ),
                "keep owner-specific presentation and test types with their consumers",
            ),
        )
        violations = find_violations(paths, rules)
        self.assertEqual(violations, [], "\n".join(violations))

    def test_osc_scanners_do_not_own_pane_terminal_mutation(self) -> None:
        violations = find_violations(
            [PROJECT_ROOT / "src/pane/osc.rs"],
            ((re.compile(r"\bcrate::ghostty::Terminal\b"),
              "apply terminal state in pane/terminal, not the OSC scanners"),),
        )
        self.assertEqual(violations, [], "\n".join(violations))

    def test_pane_terminal_facade_keeps_runtime_implementation_in_children(self) -> None:
        facade = PROJECT_ROOT / "src/pane/terminal.rs"
        code = production_code(facade.read_text(encoding="utf-8"))
        for module in ("stream", "input", "viewport", "read"):
            self.assertRegex(code, rf"\bmod\s+{module}\s*;")
        violations = find_violations(
            [facade],
            (
                (
                    re.compile(r"\.(?:core|key_encoder)\s*\.\s*lock\s*\("),
                    "keep terminal locking in the focused implementation modules",
                ),
                (
                    re.compile(
                        r"\.(?:observe|scroll_viewport_\w+|screen_text_rows(?:_range)?|"
                        r"read_(?:ansi|text)_(?:screen|viewport))\s*\("
                    ),
                    "keep stream parsing and screen reads out of the facade",
                ),
            ),
        )
        self.assertEqual(violations, [], "\n".join(violations))

    def test_retained_text_algorithms_do_not_access_terminal_runtime(self) -> None:
        violations = find_violations(
            [PROJECT_ROOT / "src/pane/terminal/text.rs"],
            (
                (
                    re.compile(r"\b(?:GhosttyPaneCore|GhosttyPaneTerminal|PaneTerminal)\b"),
                    "retained-text algorithms must operate on captured rows, not runtime state",
                ),
                (
                    re.compile(r"\.lock\s*\(|\bcrate::ghostty::Terminal\b"),
                    "capture rows in read.rs before invoking pure text algorithms",
                ),
            ),
        )
        self.assertEqual(violations, [], "\n".join(violations))

    def test_scanner_ignores_non_production_references(self) -> None:
        source = '''
// runtime.input_state()
const EXAMPLE: &str = "runtime.input_state()";
#[cfg(test)]
mod tests {
    fn aggregate_state_test() { runtime.input_state(); }
}
fn production_after_tests() {}
'''
        code = production_code(source)
        self.assertNotRegex(code, FORBIDDEN_CALLS[0][0])
        self.assertIn("fn production_after_tests()", code)
        self.assertEqual(code.count("\n"), source.count("\n"))

    def test_scanner_checks_production_after_test_modules(self) -> None:
        source = '''
#[cfg(test)]
mod tests {
    const BRACES: &str = "}}";
}
fn render() { TerminalRuntime::input_state; }
'''
        self.assertRegex(production_code(source), FORBIDDEN_CALLS[0][0])

    def test_scanner_catches_each_aggregate_state_call(self) -> None:
        cases = (
            ("fn render() { runtime.input_state(); }", INPUT_STATE_CALL),
            ("fn render() { runtime.keyboard_state_ansi(); }", KEYBOARD_STATE_ANSI_CALL),
            ("fn render() { runtime.kitty_keyboard_state_ansi(); }", KEYBOARD_STATE_ANSI_CALL),
        )
        for source, pattern in cases:
            with self.subTest(source=source):
                self.assertRegex(production_code(source), pattern)

    def test_scanner_catches_imported_process_query(self) -> None:
        source = "fn render() { foreground_job(pid); }"
        self.assertRegex(production_code(source), FORBIDDEN_CALLS[3][0])


if __name__ == "__main__":
    unittest.main()
