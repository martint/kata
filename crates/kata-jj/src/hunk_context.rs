//! Function context for diff hunks — the `fn foo(...)` tail on a
//! GitHub-style `@@ … @@` hunk header.
//!
//! Given the full text of one side of a file and the 1-based line
//! a hunk starts on, [`hunk_context`] walks upwards from that line
//! looking for the nearest "signature" line: a function, method,
//! class, type or (for prose) heading declaration. Which lines count
//! is decided per language by a small regex table in the spirit of
//! git's built-in `xfuncname` drivers (`userdiff.c`). Files whose
//! language has no table fall back to git's default rule: a line
//! that starts in column 0 with a letter, underscore or `$`.
//!
//! The scan starts *at* the hunk's first line rather than the line
//! before it (git's choice): a hunk whose leading context line is
//! itself the signature is about that function, and labelling it
//! with the previous function would mislead.
//!
//! The result is trimmed and capped at [`MAX_CONTEXT_CHARS`]
//! characters so a monster signature can't blow up the header row.

use std::sync::LazyLock;

use regex::Regex;

/// Longest context string returned, in characters. Longer matches
/// are cut and end in `…`.
pub const MAX_CONTEXT_CHARS: usize = 120;

/// Languages with a dedicated signature table. Anything else uses
/// [`Language::Default`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Rust,
    Java,
    Kotlin,
    Go,
    Python,
    /// JavaScript and TypeScript.
    JavaScript,
    /// A Svelte component: the JavaScript rules inside `<script>`,
    /// plus `<script>` / `</script>` / `<style>` / `</style>` tags
    /// acting as block boundaries. The upward scan stops at a
    /// boundary, so a template or style hunk gets no context instead
    /// of the last function in the script block above it.
    Svelte,
    /// C, C++ and Objective-C headers / sources.
    C,
    Ruby,
    /// Markdown: ATX headings (`## Title`) act as the context.
    Markdown,
    /// git's default rule: any non-indented line starting with a
    /// letter, underscore or `$`.
    Default,
}

impl Language {
    /// Pick the table from the file's extension (case-insensitive).
    pub fn for_path(path: &str) -> Language {
        let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let ext = match base.rfind('.') {
            Some(i) => &base[i + 1..],
            None => "",
        };
        match ext {
            "rs" => Language::Rust,
            "java" => Language::Java,
            "kt" | "kts" => Language::Kotlin,
            "go" => Language::Go,
            "py" | "pyi" => Language::Python,
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => Language::JavaScript,
            "svelte" => Language::Svelte,
            "c" | "h" | "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "m"
            | "mm" => Language::C,
            "rb" | "rake" => Language::Ruby,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Default,
        }
    }
}

/// One language's rules. A line is a signature when it matches none
/// of `reject` and at least one of `accept`. `reject` exists for
/// languages whose accept rule is loose (Java's "words then `(`"
/// also matches `return foo(` without it). A line matching
/// `boundary` ends the upward scan with no context: it separates
/// blocks whose signatures don't enclose what follows (Svelte's
/// script / template / style sections).
struct Rules {
    boundary: &'static [Regex],
    reject: &'static [Regex],
    accept: &'static [Regex],
}

macro_rules! regexes {
    ($name:ident, [$($pat:expr),* $(,)?]) => {
        static $name: LazyLock<Vec<Regex>> = LazyLock::new(|| {
            vec![$(Regex::new($pat).expect(concat!("valid regex: ", $pat))),*]
        });
    };
}

// `[^;]*$` on the declaration rules keeps `pub mod foo;`, `void run();`
// and friends — declarations without a body — from posing as the
// enclosing scope.
regexes!(
    RUST_ACCEPT,
    [
        r"^\s*(pub(\([^)]*\))?\s+)?((async|const|unsafe|default|extern(\s+\x22[^\x22]*\x22)?)\s+)*((struct|enum|union|mod|trait|fn|impl)\b|macro_rules!)[^;]*$",
    ]
);

regexes!(
    JAVA_REJECT,
    [r"^\s*(catch|do|else|for|if|instanceof|new|return|switch|synchronized|throw|try|while)\b"]
);
regexes!(
    JAVA_ACCEPT,
    [
        // modifiers / type words, then a name, then `(`; no `;` so
        // abstract declarations and call statements don't count.
        r"^\s*(([A-Za-z_][A-Za-z_0-9<>\[\],.?]*\s+)+[A-Za-z_][A-Za-z_0-9]*\s*\([^;]*)$",
        r"^\s*(([a-z]+\s+)*(class|enum|interface|record|@interface)\s+.*)$",
    ]
);

regexes!(
    KOTLIN_ACCEPT,
    [r"^\s*(([a-z]+\s+)*(fun|class|interface|object|typealias)\b[^;]*)$",]
);

regexes!(GO_ACCEPT, [r"^\s*func\b.*$", r"^\s*type\s+[A-Za-z_].*$"]);

regexes!(
    PYTHON_ACCEPT,
    [r"^\s*(async\s+)?(def|class)\s+[A-Za-z_].*$"]
);

regexes!(
    JS_REJECT,
    [
        r"^\s*(if|else|for|while|do|switch|case|catch|try|finally|return|throw|new|await|yield|with|typeof|delete|void)\b"
    ]
);
regexes!(
    JS_ACCEPT,
    [
        // function / generator / async function declarations (not a
        // bodiless TS overload, nor a one-liner that encloses nothing)
        r"^\s*(export\s+)?(default\s+)?(async\s+)?function\b[^;]*$",
        // class, interface, enum, namespace, type alias declarations
        r"^\s*(export\s+)?(default\s+)?(declare\s+)?(abstract\s+)?(class|interface|enum|namespace|module)\s+[A-Za-z_$].*$",
        r"^\s*(export\s+)?(declare\s+)?type\s+[A-Za-z_$][\w$]*(<[^=]*>)?\s*=.*$",
        // arrow / function expression bound to a name, only when it
        // opens a body: the arrow ends the line, optionally followed by
        // `{` or `(`. `const inc = (x) => x + 1;` is an expression.
        r"^\s*(export\s+)?(const|let|var)\s+[A-Za-z_$][\w$]*(\s*:[^=]+)?\s*=\s*(async\s*)?(function\b[^;]*|(\([^)]*\)\s*(:[^=]+)?|[A-Za-z_$][\w$]*\s*)=>\s*[{(]?)\s*$",
        // class / object methods: `  async handle(req): Promise<void> {`
        r"^\s*((public|private|protected|static|readonly|abstract|override|async|get|set)\s+)*\*?\s*[A-Za-z_$][\w$]*\s*(<[^>]*>)?\s*\([^;]*\)\s*(:\s*[^{;=]+)?\{\s*$",
        // test-suite blocks are the natural scope in a spec file
        r"^\s*(describe|context|suite|it|test)(\.(only|skip|each|concurrent))?\s*\([^;]*$",
    ]
);

regexes!(
    C_REJECT,
    [
        // `label:` lines (goto targets, `public:` etc.)
        r"^\s*[A-Za-z_][A-Za-z_0-9]*:\s*($|/[/*])",
    ]
);
regexes!(C_ACCEPT, [r"^((::\s*)?[A-Za-z_][^;]*)$"]);

regexes!(SVELTE_BOUNDARY, [r"^\s*</?(script|style)\b"]);

regexes!(RUBY_ACCEPT, [r"^\s*((class|module|def)\s+[A-Za-z_].*)$"]);

regexes!(MARKDOWN_ACCEPT, [r"^ {0,3}#{1,6}\s+\S.*$"]);

regexes!(DEFAULT_ACCEPT, [r"^[A-Za-z_$].*$"]);

static NONE: &[Regex] = &[];

fn rules(lang: Language) -> Rules {
    let (boundary, reject, accept): (&'static [Regex], &'static [Regex], &'static [Regex]) =
        match lang {
            Language::Rust => (NONE, NONE, &RUST_ACCEPT),
            Language::Java => (NONE, &JAVA_REJECT, &JAVA_ACCEPT),
            Language::Kotlin => (NONE, NONE, &KOTLIN_ACCEPT),
            Language::Go => (NONE, NONE, &GO_ACCEPT),
            Language::Python => (NONE, NONE, &PYTHON_ACCEPT),
            Language::JavaScript => (NONE, &JS_REJECT, &JS_ACCEPT),
            Language::Svelte => (&SVELTE_BOUNDARY, &JS_REJECT, &JS_ACCEPT),
            Language::C => (NONE, &C_REJECT, &C_ACCEPT),
            Language::Ruby => (NONE, NONE, &RUBY_ACCEPT),
            Language::Markdown => (NONE, NONE, &MARKDOWN_ACCEPT),
            Language::Default => (NONE, NONE, &DEFAULT_ACCEPT),
        };
    Rules {
        boundary,
        reject,
        accept,
    }
}

/// What a line means to the upward scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineKind {
    /// A scope declaration: the scan stops and labels the hunk with it.
    Signature,
    /// A block boundary: the scan stops with no context.
    Boundary,
    /// Anything else: the scan keeps going.
    Other,
}

fn classify(line: &str, lang: Language) -> LineKind {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return LineKind::Other;
    }
    let r = rules(lang);
    if r.boundary.iter().any(|re| re.is_match(line)) {
        LineKind::Boundary
    } else if !r.reject.iter().any(|re| re.is_match(line))
        && r.accept.iter().any(|re| re.is_match(line))
    {
        LineKind::Signature
    } else {
        LineKind::Other
    }
}

/// Whether `line` reads as a signature under `lang`'s rules.
pub fn is_signature(line: &str, lang: Language) -> bool {
    classify(line, lang) == LineKind::Signature
}

/// The enclosing-scope label for a hunk starting at `first_line`
/// (1-based) of `lines`: the nearest line at or above it that
/// [`is_signature`] accepts, trimmed and capped. `None` when nothing
/// above the hunk qualifies, when a block boundary comes first, or
/// when `first_line` is 0.
///
/// Labelling several hunks of one file? [`ContextScanner`] gives the
/// same answers in a single pass.
pub fn hunk_context(lines: &[&str], first_line: u32, lang: Language) -> Option<String> {
    let start = (first_line as usize).min(lines.len());
    lines[..start]
        .iter()
        .rev()
        .map(|l| (l, classify(l, lang)))
        .find(|(_, k)| *k != LineKind::Other)
        .and_then(|(l, k)| (k == LineKind::Signature).then(|| truncate(l.trim())))
}

/// [`hunk_context`] for many hunks of one file. Hunks arrive in
/// ascending order, so instead of rescanning upward from each one
/// the scanner walks the file top-down once, remembering the latest
/// signature (forgotten again at a block boundary), and answers each
/// hunk from where the previous one left off. That makes a whole
/// file O(lines) rather than O(hunks × lines). A start line above
/// the previous one falls back to a fresh [`hunk_context`] scan, so
/// the answers never depend on call order.
pub struct ContextScanner<'a> {
    lines: &'a [&'a str],
    lang: Language,
    /// `lines[..scanned]` have been classified.
    scanned: usize,
    /// Index of the latest signature in `lines[..scanned]` with no
    /// boundary after it.
    last: Option<usize>,
}

impl<'a> ContextScanner<'a> {
    pub fn new(lines: &'a [&'a str], lang: Language) -> Self {
        ContextScanner {
            lines,
            lang,
            scanned: 0,
            last: None,
        }
    }

    /// Same result as `hunk_context(lines, first_line, lang)`.
    pub fn context_at(&mut self, first_line: u32) -> Option<String> {
        let start = (first_line as usize).min(self.lines.len());
        if start < self.scanned {
            return hunk_context(self.lines, first_line, self.lang);
        }
        for (i, line) in self.lines[self.scanned..start].iter().enumerate() {
            match classify(line, self.lang) {
                LineKind::Signature => self.last = Some(self.scanned + i),
                LineKind::Boundary => self.last = None,
                LineKind::Other => {}
            }
        }
        self.scanned = start;
        self.last.map(|i| truncate(self.lines[i].trim()))
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_CONTEXT_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_CONTEXT_CHARS - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(src: &str, first_line: u32, lang: Language) -> Option<String> {
        let lines: Vec<&str> = src.lines().collect();
        hunk_context(&lines, first_line, lang)
    }

    #[test]
    fn picks_language_from_extension() {
        assert_eq!(Language::for_path("crates/x/src/lib.rs"), Language::Rust);
        assert_eq!(Language::for_path("Foo.JAVA"), Language::Java);
        assert_eq!(Language::for_path("a/b.kt"), Language::Kotlin);
        assert_eq!(Language::for_path("main.go"), Language::Go);
        assert_eq!(Language::for_path("x.py"), Language::Python);
        assert_eq!(Language::for_path("x.ts"), Language::JavaScript);
        assert_eq!(Language::for_path("Foo.svelte"), Language::Svelte);
        assert_eq!(Language::for_path("x.hpp"), Language::C);
        assert_eq!(Language::for_path("x.rb"), Language::Ruby);
        assert_eq!(Language::for_path("docs/SPEC.md"), Language::Markdown);
        assert_eq!(Language::for_path("Cargo.lock"), Language::Default);
        assert_eq!(Language::for_path("Makefile"), Language::Default);
    }

    #[test]
    fn rust_nearest_fn_wins_over_enclosing_impl() {
        let src = "\
use std::fmt;

pub struct Foo;

impl Foo {
    pub fn new() -> Self {
        Foo
    }

    pub(crate) async fn run(&self, n: u32) -> Result<(), E> {
        let x = 1;
        let y = 2;
        Ok(())
    }
}
";
        // Inside `run`: the method, not the impl block.
        assert_eq!(
            ctx(src, 12, Language::Rust).as_deref(),
            Some("pub(crate) async fn run(&self, n: u32) -> Result<(), E> {")
        );
        // Inside `new`.
        assert_eq!(
            ctx(src, 7, Language::Rust).as_deref(),
            Some("pub fn new() -> Self {")
        );
        // Between items: `pub struct Foo;` is a declaration, not a
        // scope — `[^;]*$` rejects it, so nothing qualifies.
        assert_eq!(ctx(src, 4, Language::Rust), None);
        // Hunk starting exactly on the signature line labels itself.
        assert_eq!(ctx(src, 5, Language::Rust).as_deref(), Some("impl Foo {"));
    }

    #[test]
    fn rust_ignores_use_and_let() {
        let src = "\
use foo::bar;
let x = 5;
";
        assert_eq!(ctx(src, 2, Language::Rust), None);
    }

    #[test]
    fn java_method_not_call_or_control_flow() {
        let src = "\
package com.example;

public class Greeter {
    private final String name;

    public static List<String> greet(String name, int times) {
        if (times == 0) {
            return List.of();
        }
        helper.run(name);
        String s = format(name);
        for (int i = 0; i < times; i++) {
            out.add(s);
        }
        return out;
    }
}
";
        assert_eq!(
            ctx(src, 14, Language::Java).as_deref(),
            Some("public static List<String> greet(String name, int times) {")
        );
        // Inside the class but before any method.
        assert_eq!(
            ctx(src, 4, Language::Java).as_deref(),
            Some("public class Greeter {")
        );
    }

    #[test]
    fn java_abstract_declaration_is_not_a_scope() {
        let src = "\
interface Runner {
    void run();
    int count(int x);
}
";
        assert_eq!(
            ctx(src, 3, Language::Java).as_deref(),
            Some("interface Runner {")
        );
    }

    #[test]
    fn python_def_and_class() {
        let src = "\
import os


class Greeter:
    def __init__(self, name):
        self.name = name

    async def greet(self):
        print(self.name)
        return 1
";
        assert_eq!(
            ctx(src, 10, Language::Python).as_deref(),
            Some("async def greet(self):")
        );
        assert_eq!(
            ctx(src, 6, Language::Python).as_deref(),
            Some("def __init__(self, name):")
        );
        assert_eq!(ctx(src, 2, Language::Python), None);
    }

    #[test]
    fn javascript_functions_methods_and_describe_blocks() {
        let src = "\
import { x } from './x';

export const add = (a: number, b: number): number => {
  return a + b;
};

export default class Thing {
  async handle(req: Request): Promise<void> {
    if (req.ok) {
      await go();
    }
  }
}

describe('add', () => {
  test('adds', () => {
    expect(add(1, 2)).toBe(3);
  });
});
";
        assert_eq!(
            ctx(src, 4, Language::JavaScript).as_deref(),
            Some("export const add = (a: number, b: number): number => {")
        );
        // `if (req.ok) {` must not pose as a method.
        assert_eq!(
            ctx(src, 10, Language::JavaScript).as_deref(),
            Some("async handle(req: Request): Promise<void> {")
        );
        assert_eq!(
            ctx(src, 17, Language::JavaScript).as_deref(),
            Some("test('adds', () => {")
        );
        assert_eq!(ctx(src, 2, Language::JavaScript), None);
    }

    #[test]
    fn javascript_one_line_arrow_is_not_a_scope() {
        let src = "\
export function run(items: Item[]) {
  const isCancelled = () => cancelled;
  const inc = (x) => x + 1
  const pick = async (id) => { return items[id]; };
  const fmt = function (x) { return String(x); };
  for (const item of items) {
    go(item);
  }
}

const handler = async (ev: Event) => {
  ev.preventDefault();
};

const render = () => (
  <div />
);

const onClick = function (ev) {
  go(ev);
};

it('is a one-liner', () => expect(1).toBe(1));
export function overload(x: string): void;
";
        // One-line arrows / function expressions inside `run` must not
        // shadow the enclosing function.
        assert_eq!(
            ctx(src, 7, Language::JavaScript).as_deref(),
            Some("export function run(items: Item[]) {")
        );
        // Arrows that open a body (`{` or a parenthesised expression)
        // still count.
        assert_eq!(
            ctx(src, 12, Language::JavaScript).as_deref(),
            Some("const handler = async (ev: Event) => {")
        );
        assert_eq!(
            ctx(src, 16, Language::JavaScript).as_deref(),
            Some("const render = () => (")
        );
        assert_eq!(
            ctx(src, 20, Language::JavaScript).as_deref(),
            Some("const onClick = function (ev) {")
        );
        // A one-line test and a bodiless overload are statements, not
        // scopes: the nearest real scope above them is `onClick`.
        assert_eq!(
            ctx(src, 24, Language::JavaScript).as_deref(),
            Some("const onClick = function (ev) {")
        );
    }

    #[test]
    fn svelte_markup_and_style_get_no_script_context() {
        let src = "\
<script lang=\"ts\">
  import { onMount } from 'svelte';

  function load(id: string) {
    const isCancelled = () => cancelled;
    return fetch(id);
  }
</script>

<div class=\"wrap\">
  {#if ready}
    <span>{name}</span>
  {/if}
</div>

<style>
  .wrap {
    display: flex;
  }
</style>
";
        // Inside the script block: the function.
        assert_eq!(
            ctx(src, 6, Language::Svelte).as_deref(),
            Some("function load(id: string) {")
        );
        // Script block before any function: nothing.
        assert_eq!(ctx(src, 2, Language::Svelte), None);
        // Template and style hunks must not inherit `load`.
        assert_eq!(ctx(src, 12, Language::Svelte), None);
        assert_eq!(ctx(src, 8, Language::Svelte), None);
        assert_eq!(ctx(src, 18, Language::Svelte), None);
        assert_eq!(ctx(src, 20, Language::Svelte), None);
        // Plain TS has no block boundaries: the same lines fall
        // through to the function.
        assert_eq!(
            ctx(src, 12, Language::JavaScript).as_deref(),
            Some("function load(id: string) {")
        );
    }

    #[test]
    fn rust_macro_rules_with_space() {
        let src = "\
macro_rules! regexes {
    ($name:ident) => {};
}
#[macro_export]
macro_rules!tight {
    () => {};
}
";
        assert_eq!(
            ctx(src, 2, Language::Rust).as_deref(),
            Some("macro_rules! regexes {")
        );
        assert_eq!(
            ctx(src, 6, Language::Rust).as_deref(),
            Some("macro_rules!tight {")
        );
    }

    /// [`ContextScanner`] must agree with a fresh [`hunk_context`]
    /// scan for every start line, whether visited in ascending order
    /// (the fast path) or out of order (the fallback).
    #[test]
    fn scanner_matches_backward_scan() {
        let svelte = "\
<script>
  function a() {
    x();
  }
</script>
<p>hi</p>
<script context=\"module\">
  export const b = () => {
    y();
  };
</script>
<style>
  p { color: red; }
</style>
";
        let rust = "\
use x;

impl A {
    fn b() {
        c();
    }
}

macro_rules! m {
    () => {};
}
";
        for (src, lang) in [(svelte, Language::Svelte), (rust, Language::Rust)] {
            let lines: Vec<&str> = src.lines().collect();
            let n = lines.len() as u32 + 2;
            let mut asc = ContextScanner::new(&lines, lang);
            for start in 0..n {
                assert_eq!(
                    asc.context_at(start),
                    hunk_context(&lines, start, lang),
                    "ascending, {lang:?} line {start}"
                );
            }
            let mut mixed = ContextScanner::new(&lines, lang);
            for start in [5, 2, 9, 9, 0, 12, 3, 99, 1] {
                assert_eq!(
                    mixed.context_at(start),
                    hunk_context(&lines, start, lang),
                    "mixed, {lang:?} line {start}"
                );
            }
        }
    }

    #[test]
    fn go_func_and_type() {
        let src = "\
package main

type Server struct {
    addr string
}

func (s *Server) Run() error {
    return nil
}
";
        assert_eq!(
            ctx(src, 8, Language::Go).as_deref(),
            Some("func (s *Server) Run() error {")
        );
        assert_eq!(
            ctx(src, 4, Language::Go).as_deref(),
            Some("type Server struct {")
        );
    }

    #[test]
    fn kotlin_fun_with_modifiers() {
        let src = "\
class Greeter(private val name: String) {
    override suspend fun greet(): String {
        return name
    }
}
";
        assert_eq!(
            ctx(src, 3, Language::Kotlin).as_deref(),
            Some("override suspend fun greet(): String {")
        );
    }

    #[test]
    fn c_column_zero_definitions_skip_labels_and_declarations() {
        let src = "\
#include <stdio.h>

static int counter;

static int
add(int a, int b)
{
out:
    return a + b;
}
";
        assert_eq!(
            ctx(src, 9, Language::C).as_deref(),
            Some("add(int a, int b)")
        );
        assert_eq!(ctx(src, 4, Language::C), None);
    }

    #[test]
    fn markdown_headings() {
        let src = "\
# Title

Some prose here.

### 5.2 Whole-file vs. hunks

Diffs render as hunks.
";
        assert_eq!(
            ctx(src, 7, Language::Markdown).as_deref(),
            Some("### 5.2 Whole-file vs. hunks")
        );
        assert_eq!(ctx(src, 3, Language::Markdown).as_deref(), Some("# Title"));
    }

    #[test]
    fn default_rule_is_any_unindented_identifier_line() {
        let src = "\
[package]
name = \"kata\"
  indented = 1
# comment
";
        assert_eq!(
            ctx(src, 4, Language::Default).as_deref(),
            Some("name = \"kata\"")
        );
        assert_eq!(ctx(src, 1, Language::Default), None);
    }

    #[test]
    fn out_of_range_start_clamps_to_file_end() {
        let src = "fn a() {}\n";
        assert_eq!(ctx(src, 99, Language::Rust).as_deref(), Some("fn a() {}"));
        assert_eq!(ctx(src, 0, Language::Rust), None);
        assert_eq!(hunk_context(&[], 1, Language::Rust), None);
    }

    #[test]
    fn long_signatures_are_capped_with_ellipsis() {
        let long = format!("fn {}() {{", "x".repeat(200));
        let got = ctx(&long, 1, Language::Rust).unwrap();
        assert_eq!(got.chars().count(), MAX_CONTEXT_CHARS);
        assert!(got.ends_with('…'));
        assert!(got.starts_with("fn xxx"));
    }

    #[test]
    fn result_is_trimmed() {
        let src = "    def f():   \n        pass\n";
        assert_eq!(ctx(src, 2, Language::Python).as_deref(), Some("def f():"));
    }
}
