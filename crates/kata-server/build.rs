//! Build the Svelte bundle so the resulting binary can embed it.
//!
//! - Skips if `KATA_SKIP_WEB_BUILD=1` (use `--web-dir` at runtime instead).
//! - Otherwise requires `pnpm` on `PATH`. Failing loudly here avoids the
//!   previous silent-skip footgun where a missing pnpm shipped a stale
//!   `web/dist` without telling you.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let web_dir = PathBuf::from(&manifest_dir).join("../../web");
    let dist_dir = web_dir.join("dist");

    // Re-run when sources or config change. (Adding a path that doesn't
    // exist is fine — cargo treats it as a sentinel.)
    for sub in ["src", "package.json", "index.html", "vite.config.ts", "tsconfig.json"] {
        println!("cargo:rerun-if-changed={}", web_dir.join(sub).display());
    }
    println!("cargo:rerun-if-env-changed=KATA_SKIP_WEB_BUILD");

    if std::env::var_os("KATA_SKIP_WEB_BUILD").is_some() {
        ensure_placeholder(&dist_dir);
        return;
    }

    if which("pnpm").is_none() {
        panic!(
            "\n\n\
             pnpm not found on PATH; refusing to ship a stale web bundle.\n\
             Either:\n\
               1. Install pnpm (https://pnpm.io/installation) and make sure\n\
                  `pnpm --version` works in the same shell that runs cargo.\n\
               2. Set KATA_SKIP_WEB_BUILD=1 to skip the web build and pass\n\
                  `--web-dir web/dist` (or a prebuilt dir) at runtime.\n\n",
        );
    }

    if !web_dir.join("node_modules").exists() {
        run(&web_dir, "pnpm", &["install"]);
    }
    run(&web_dir, "pnpm", &["run", "build"]);
    ensure_placeholder(&dist_dir); // belt-and-braces: dist must always exist for rust-embed
}

fn run(cwd: &Path, prog: &str, args: &[&str]) {
    let status = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("running {prog} {:?}: {e}", args));
    if !status.success() {
        panic!("{prog} {:?} exited with {status}", args);
    }
}

fn which(prog: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(prog))
        .find(|p| p.is_file())
}

fn ensure_placeholder(dist_dir: &Path) {
    std::fs::create_dir_all(dist_dir).expect("create web/dist");
    let index = dist_dir.join("index.html");
    if !index.exists() {
        std::fs::write(
            &index,
            "<!doctype html><meta charset=utf-8><title>review</title>\
             <p>Web bundle was not built. Build it with <code>pnpm --dir web run build</code> \
             or pass <code>--web-dir</code>.",
        )
        .expect("write placeholder");
    }
}
