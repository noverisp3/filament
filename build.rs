//! Records what this binary is, so the startup banner can say it.
//!
//! A server log that cannot say which build produced it is a log you have to
//! guess about. The three things worth having are the commit, whether the tree
//! was clean when it was built, and when: a log line and a client's disconnect
//! report can then be lined up against each other.
//!
//! Everything here degrades to "unknown" rather than failing. A build from a
//! tarball with no git, or on a machine with no git, should still produce a
//! server.

use std::process::Command;

/// The compile-time profile, which is the difference between a server running at
/// full speed and one running at `opt-level = 1`.
fn profile() -> String {
    std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string())
}

fn target() -> String {
    std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string())
}

/// Runs a git command, returning `None` if git is missing or the directory is
/// not a repository.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

/// The short commit hash, and whether the tree had uncommitted changes.
///
/// A dirty tree is worth saying out loud: the binary and the repository are not
/// the same thing, and a bug found in one of them may not be in the other.
fn commit() -> (String, bool) {
    let Some(sha) = git(&["rev-parse", "--short", "HEAD"]) else {
        return ("unknown".to_string(), false);
    };
    let sha = if sha.is_empty() { "unknown" } else { &sha }.to_string();

    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    (sha, dirty)
}

/// When the build ran, as a unix timestamp, printed in UTC.
///
/// `SOURCE_DATE_EPOCH` wins when it is set, because a reproducible build that
/// stamps the wall clock is not reproducible, and the convention exists to let
/// a build stamp a fixed time instead.
///
/// UTC rather than local time on purpose. A build stamp is an instant, and
/// instants do not have a timezone: it is the reading of it that does. Printed
/// local, the same build would carry a different string on a machine in
/// Bangkok and one in Berlin, which makes two builds of the same commit look
/// different. It is labelled, because a bare `Z` was read as local by the person
/// who wrote it and cost a confusing minute.
fn built_at() -> String {
    let seconds: u64 = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });

    // Civil time from days since the epoch, by hand. The standard library has
    // no calendar and a build script that pulled one in to print a date would be
    // a heavier thing to accept than the arithmetic is.
    let days = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );

    // Howard Hinnant's civil_from_days, which is the standard formulation and is
    // exact for the whole range a unix timestamp can hold.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z (UTC)")
}

fn main() {
    // Which files this banner depends on, listed rather than left to the default.
    //
    // Two mistakes are worth recording. Listing only `build.rs` looks like
    // tidiness and freezes the banner: any `rerun-if-changed` switches Cargo to
    // "only these paths", so the commit and build time stopped moving while the
    // binary kept rebuilding with the fixes in it. And leaving it to the default
    // alongside a `rerun-if-env-changed` did not work either: the script stopped
    // running at all once the sources changed.
    //
    // Saying "the banner depends on src, Cargo.toml and build.rs" is dull and
    // correct, which is the whole requirement -- with one omission that was not
    // dull, it was wrong.
    //
    // Git was missing. A commit changes no source file, so cargo had no reason
    // to run this script, and the banner kept printing the commit from before
    // the commit: it said `40cf2cb+dirty` over a binary that had `6ac72d1` in
    // it. A build stamp that names the wrong build is worse than none, because
    // it is believed. The evidence that settled it was the build *time* staying
    // put too -- the same instant, across two commits.
    //
    // Tracking `.git/HEAD` is enough for the commit. It cannot see the tree
    // becoming dirty, because a dirty tree is the absence of a change and cargo
    // only re-runs on changes; `+dirty` can therefore lag by one build, which
    // is the smaller half of the lie.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    watch_the_branch();

    let (sha, dirty) = commit();

    println!("cargo:rustc-env=FILAMENT_GIT_SHA={sha}");
    println!("cargo:rustc-env=FILAMENT_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=FILAMENT_PROFILE={}", profile());
    println!("cargo:rustc-env=FILAMENT_TARGET={}", target());
    println!("cargo:rustc-env=FILAMENT_BUILT_AT={}", built_at());
    println!("cargo:rustc-env=FILAMENT_RUSTC={}", rustc_version());
}

/// Watches the ref the current branch points at, so a commit rebuilds.
///
/// Adding `.git/HEAD` was the wrong fix and looked right for a while. A commit
/// does not touch `HEAD` -- that file holds the text `ref: refs/heads/master`,
/// and a commit moves the ref that line points at, leaving the line alone. So
/// `HEAD`'s timestamp sat still through several commits while the banner
/// cheerfully named a build that had never existed, which is the one thing a
/// build stamp must never do.
///
/// `packed-refs` goes alongside because a packed branch has no loose file: the
/// ref path does not exist and the packing is what changes instead. Neither is
/// watched inside `.git/worktrees`, where a linked checkout puts them
/// elsewhere, which is a gap left open rather than papered over -- the banner
/// degrades to the commit it was built at, which is what it does anyway.
fn watch_the_branch() {
    if let Some(packed) = git(&["rev-parse", "--git-path", "packed-refs"])
        && !packed.is_empty()
    {
        println!("cargo:rerun-if-changed={packed}");
    }
    let Some(symbolic) = git(&["symbolic-ref", "HEAD"]) else {
        return;
    };
    if let Some(path) = git(&["rev-parse", "--git-path", &symbolic])
        && !path.is_empty()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}

/// The compiler that built this, and its own commit and release date.
///
/// Which matters when a bug turns out to be one in the compiler rather than in
/// the code. Note that the hash here is the Rust project's, not this one's: a
/// banner that prints two hashes with nothing to say which is which is a banner
/// that gets misread.
fn rustc_version() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());
    out.trim().to_string()
}
