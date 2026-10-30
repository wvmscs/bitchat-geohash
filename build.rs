// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The version shown by the page (`VERSION`): the `v*` tag of this commit,
//! else the first 7 characters of the commit. Packaging from a copy without
//! `.git` gives it in `BITCHAT_BUILD_VERSION`; failing that, the version of
//! `Cargo.toml`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// Rebuild when the commit or the tags move. A missing path would run this
/// script at every build; `--git-path` also finds them from a worktree.
fn watch_git() {
    let head = git(&["symbolic-ref", "-q", "HEAD"]);
    let names = ["HEAD", "packed-refs", "refs/tags"];
    for name in names.into_iter().chain(head.as_deref()) {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// From this crate's own repository, not one that happens to hold it.
fn from_git() -> Option<String> {
    if !Path::new(".git").exists() {
        return None;
    }
    watch_git();
    git(&[
        "describe",
        "--tags",
        "--exact-match",
        "--match",
        "v*",
        "HEAD",
    ])
    .or_else(|| git(&["rev-parse", "HEAD"]).map(|c| c.chars().take(7).collect()))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BITCHAT_BUILD_VERSION");
    let given = std::env::var("BITCHAT_BUILD_VERSION").ok();
    let version = given
        .filter(|v| !v.is_empty())
        .or_else(from_git)
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));
    println!("cargo:rustc-env=BITCHAT_VERSION={version}");
}
