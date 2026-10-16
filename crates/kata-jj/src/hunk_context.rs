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
    /// JavaScript, TypeScript and the `<script>` block of a Svelte
    /// component. Svelte's markup never matches a signature rule, so
    /// hunks in the template part simply get no context.
    JavaScript,
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
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "svelte" => {
                Language::JavaScript
            }
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
/// also matches `return foo(` without it).
struct Rules {
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
        r"^\s*(pub(\([^)]*\))?\s+)?((async|const|unsafe|default|extern(\s+\x22[^\x22]*\x22)?)\s+)*(struct|enum|union|mod|trait|fn|impl|macro_rules!)\b[^;]*$",
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
        // function / generator / async function declarations
        r"^\s*(export\s+)?(default\s+)?(async\s+)?function\b.*$",
        // class, interface, enum, namespace, type alias declarations
        r"^\s*(export\s+)?(default\s+)?(declare\s+)?(abstract\s+)?(class|interface|enum|namespace|module)\s+[A-Za-z_$].*$",
        r"^\s*(export\s+)?(declare\s+)?type\s+[A-Za-z_$][\w$]*(<[^=]*>)?\s*=.*$",
        // arrow / function expression bound to a name
        r"^\s*(export\s+)?(const|let|var)\s+[A-Za-z_$][\w$]*(\s*:[^=]+)?\s*=\s*(async\s*)?(function\b|\([^)]*\)\s*(:[^=]+)?=>|[A-Za-z_$][\w$]*\s*=>).*$",
        // class / object methods: `  async handle(req): Promise<void> {`
        r"^\s*((public|private|protected|static|readonly|abstract|override|async|get|set)\s+)*\*?\s*[A-Za-z_$][\w$]*\s*(<[^>]*>)?\s*\([^;]*\)\s*(:\s*[^{;=]+)?\{\s*$",
        // test-suite blocks are the natural scope in a spec file
        r"^\s*(describe|context|suite|it|test)(\.(only|skip|each|concurrent))?\s*\(.*$",
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

regexes!(RUBY_ACCEPT, [r"^\s*((class|module|def)\s+[A-Za-z_].*)$"]);

regexes!(MARKDOWN_ACCEPT, [r"^ {0,3}#{1,6}\s+\S.*$"]);

regexes!(DEFAULT_ACCEPT, [r"^[A-Za-z_$].*$"]);

static NO_REJECT: &[Regex] = &[];

fn rules(lang: Language) -> Rules {
    match lang {
        Language::Rust => Rules {
            reject: NO_REJECT,
            accept: &RUST_ACCEPT,
        },
        Language::Java => Rules {
            reject: &JAVA_REJECT,
            accept: &JAVA_ACCEPT,
        },
        Language::Kotlin => Rules {
            reject: NO_REJECT,
            accept: &KOTLIN_ACCEPT,
        },
        Language::Go => Rules {
            reject: NO_REJECT,
            accept: &GO_ACCEPT,
        },
        Language::Python => Rules {
            reject: NO_REJECT,
            accept: &PYTHON_ACCEPT,
        },
        Language::JavaScript => Rules {
            reject: &JS_REJECT,
            accept: &JS_ACCEPT,
        },
        Language::C => Rules {
            reject: &C_REJECT,
            accept: &C_ACCEPT,
        },
        Language::Ruby => Rules {
            reject: NO_REJECT,
            accept: &RUBY_ACCEPT,
        },
        Language::Markdown => Rules {
            reject: NO_REJECT,
            accept: &MARKDOWN_ACCEPT,
        },
        Language::Default => Rules {
            reject: NO_REJECT,
            accept: &DEFAULT_ACCEPT,
        },
    }
}

/// Whether `line` reads as a signature under `lang`'s rules.
pub fn is_signature(line: &str, lang: Language) -> bool {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return false;
    }
    let r = rules(lang);
    !r.reject.iter().any(|re| re.is_match(line)) && r.accept.iter().any(|re| re.is_match(line))
}

/// The enclosing-scope label for a hunk starting at `first_line`
/// (1-based) of `lines`: the nearest line at or above it that
/// [`is_signature`] accepts, trimmed and capped. `None` when nothing
/// above the hunk qualifies (or `first_line` is out of range).
pub fn hunk_context(lines: &[&str], first_line: u32, lang: Language) -> Option<String> {
    let start = (first_line as usize).min(lines.len());
    lines[..start]
        .iter()
        .rev()
        .find(|l| is_signature(l, lang))
        .map(|l| truncate(l.trim()))
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
        assert_eq!(Language::for_path("Foo.svelte"), Language::JavaScript);
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
