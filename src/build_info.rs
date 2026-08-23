//! Which *build* is running, as opposed to which release.
//!
//! [`version`](crate::version) answers "which release is this" — the number in
//! `Cargo.toml`, compared against release tags by the updater. This module answers the
//! other question, and it is the one that actually gets asked during development: k2g
//! starts from the KiCad toolbar button, a desktop shortcut, the installed MSI, the
//! portable zip, `cargo run` and `dx serve`, and every one of those runs a *different
//! file*. All six report `0.13.0`.
//!
//! # Two halves, answering two different things
//!
//! **`built` and `exe` are read at runtime**, from [`std::env::current_exe`] and its
//! modification time. They are exact by construction: no build script is involved, so
//! there is nothing that can go stale, and they stay correct for a binary copied to
//! another machine. This is the half that answers *is the last build running* — the
//! path says which file, the timestamp says how old it is.
//!
//! **`commit` and `ci` are compiled in** by `build.rs`. They are provenance rather than
//! freshness: they say what source a build came from, and they are only as current as
//! the last time the build script ran. `build.rs` watches the whole `src` tree precisely
//! so that is every time the binary changes — but a stamp is still the weaker of the two
//! signals, and when they disagree the mtime is right.
//!
//! # Why the build number is not in the version
//!
//! The obvious shape — patch digit as a build counter, `0.13.124` — breaks
//! [`version::is_newer`](crate::version::is_newer), which orders releases on
//! `(major, minor, patch)`. A build stamped `0.13.124` computes
//! `is_newer("v0.13.1", "0.13.124") == false`, so the updater would refuse a genuine
//! hotfix. It would also leave every local build reading `0.13.0`, which is the case
//! that most needs telling apart.

use std::sync::OnceLock;

/// Everything known about the running build.
///
/// Fields are pre-formatted strings rather than the types they came from: the only
/// consumers render them, and holding a `SystemTime` here would push the same
/// `chrono` formatting into each one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    /// `CARGO_PKG_VERSION` — the release this build claims to be.
    pub version: String,
    /// Short commit hash, or empty when the build had no git to ask.
    pub commit: String,
    /// Whether the tree carried uncommitted changes when this was built.
    pub dirty: bool,
    /// `"Rust run 124"`, or empty for a build made outside CI.
    pub ci: String,
    /// Local-time modification stamp of the running executable, or empty when the
    /// filesystem cannot say.
    pub built: String,
    /// Absolute path of the running executable, or empty when it cannot be resolved.
    pub exe: String,
}

/// The running build, resolved once.
///
/// Cached deliberately rather than read per call. k2g's updater replaces the executable
/// **while the application is running**, so a later read of `current_exe()`'s mtime
/// would report the replacement's build rather than the one this process is executing —
/// which is the exact question this module exists to answer correctly.
pub fn current() -> &'static BuildInfo {
    static CURRENT: OnceLock<BuildInfo> = OnceLock::new();
    CURRENT.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: env!("K2G_COMMIT").to_string(),
            dirty: !env!("K2G_COMMIT_DIRTY").is_empty(),
            ci: env!("K2G_CI").to_string(),
            built: exe.as_deref().and_then(built_at).unwrap_or_default(),
            exe: exe.map(|path| path.display().to_string()).unwrap_or_default(),
        }
    })
}

/// A file's modification time as local wall-clock, `2026-08-23 14:32:10`.
///
/// **Local**, not UTC: the comparison being made is against "I ran `cargo build` a
/// minute ago", and a UTC stamp turns a one-glance check into arithmetic.
fn built_at(exe: &std::path::Path) -> Option<String> {
    let modified = std::fs::metadata(exe).and_then(|meta| meta.modified()).ok()?;
    let stamp: chrono::DateTime<chrono::Local> = modified.into();
    Some(stamp.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// The block `--version` prints.
///
/// Lines whose value is unknown are **omitted**, never printed empty: a build from a
/// source tarball has no commit, and `commit  ` followed by nothing reads as a bug in
/// the reporting rather than an absence of data.
pub fn describe() -> String {
    render(current())
}

/// The startup log line — the same facts on one line, for stdout, the in-app Logs
/// viewer and any captured session log.
///
/// Emitted whether or not anyone asked for it, because the log of a session that went
/// wrong is exactly where "which build was that" gets asked and cannot be recovered
/// afterwards.
pub fn one_line() -> String {
    let info = current();
    let mut parts = Vec::new();
    if !info.commit.is_empty() {
        parts.push(match info.dirty {
            true => format!("{} dirty", info.commit),
            false => info.commit.clone(),
        });
    }
    if !info.ci.is_empty() {
        parts.push(info.ci.clone());
    }
    if !info.built.is_empty() {
        parts.push(format!("built {}", info.built));
    }
    if !info.exe.is_empty() {
        parts.push(info.exe.clone());
    }

    match parts.is_empty() {
        true => format!("k2g {}", info.version),
        false => format!("k2g {} ({})", info.version, parts.join(", ")),
    }
}

/// [`describe`] against an explicit [`BuildInfo`], so the formatting is testable without
/// a build script or a filesystem behind it.
fn render(info: &BuildInfo) -> String {
    let mut lines = vec![format!("k2g {}", info.version)];
    if !info.commit.is_empty() {
        lines.push(match info.dirty {
            true => format!("commit  {} (dirty)", info.commit),
            false => format!("commit  {}", info.commit),
        });
    }
    if !info.ci.is_empty() {
        lines.push(format!("ci      {}", info.ci));
    }
    if !info.built.is_empty() {
        lines.push(format!("built   {}", info.built));
    }
    if !info.exe.is_empty() {
        lines.push(format!("exe     {}", info.exe));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> BuildInfo {
        BuildInfo {
            version: "0.13.0".into(),
            commit: "cd9b7b9".into(),
            dirty: true,
            ci: String::new(),
            built: "2026-08-23 14:32:10".into(),
            exe: r"E:\gax\dev\k2g\target\debug\k2g.exe".into(),
        }
    }

    /// The local-build block, exactly as it reaches the terminal. Column alignment is
    /// part of it: four facts scanned down the left edge is the whole point of a block
    /// rather than a sentence.
    #[test]
    fn a_local_build_reports_its_commit_time_and_path() {
        assert_eq!(
            render(&full()),
            "k2g 0.13.0\n\
             commit  cd9b7b9 (dirty)\n\
             built   2026-08-23 14:32:10\n\
             exe     E:\\gax\\dev\\k2g\\target\\debug\\k2g.exe"
        );
    }

    /// A CI build gains one line, between the commit and the time.
    #[test]
    fn a_ci_build_names_its_run() {
        let info = BuildInfo {
            commit: "343f5d2".into(),
            dirty: false,
            ci: "Rust run 124".into(),
            built: "2026-08-20 09:14:02".into(),
            exe: r"C:\Program Files\k2g\k2g.exe".into(),
            ..full()
        };

        assert_eq!(
            render(&info),
            "k2g 0.13.0\n\
             commit  343f5d2\n\
             ci      Rust run 124\n\
             built   2026-08-20 09:14:02\n\
             exe     C:\\Program Files\\k2g\\k2g.exe"
        );
    }

    /// A clean tree prints no parenthetical at all — `(clean)` on every line of every
    /// release build is noise, and its absence is the signal.
    #[test]
    fn a_clean_tree_says_nothing_about_being_clean() {
        let clean = BuildInfo { dirty: false, ..full() };

        assert!(render(&clean).contains("commit  cd9b7b9\n"));
        assert!(!render(&clean).contains("dirty"));
        assert!(!render(&clean).contains("clean"));
    }

    /// **Unknown is omitted, not blank.** A build from a source tarball has no git and
    /// no CI, and `commit  ` followed by nothing reads as a broken reporter rather than
    /// as an absence of data. What the filesystem can still answer is still printed.
    #[test]
    fn a_build_with_no_git_behind_it_omits_the_lines_it_cannot_fill() {
        let bare = BuildInfo {
            commit: String::new(),
            dirty: false,
            ci: String::new(),
            ..full()
        };

        let text = render(&bare);
        assert_eq!(
            text,
            "k2g 0.13.0\n\
             built   2026-08-23 14:32:10\n\
             exe     E:\\gax\\dev\\k2g\\target\\debug\\k2g.exe"
        );
        for line in text.lines() {
            assert!(
                !line.trim_end().ends_with(char::is_whitespace) && line.split_whitespace().count() >= 2,
                "{line:?} is a label with no value"
            );
        }
    }

    /// **The dirty flag never appears without a commit to attach it to.** `build.rs`
    /// reads "no git" as clean rather than dirty, but a stamp that somehow carried one
    /// without the other must not print a bare `(dirty)`.
    #[test]
    fn a_dirty_flag_with_no_commit_prints_nothing() {
        let odd = BuildInfo {
            commit: String::new(),
            dirty: true,
            ..full()
        };

        assert!(!render(&odd).contains("dirty"));
    }

    /// Version alone, when nothing else can be established. Still a valid answer rather
    /// than an empty string — the caller is printing it to a terminal.
    #[test]
    fn a_build_that_knows_only_its_version_still_says_that() {
        let nothing = BuildInfo {
            version: "0.13.0".into(),
            commit: String::new(),
            dirty: false,
            ci: String::new(),
            built: String::new(),
            exe: String::new(),
        };

        assert_eq!(render(&nothing), "k2g 0.13.0");
    }

    /// The stamp `build.rs` actually compiled in, checked for shape rather than value.
    ///
    /// The values differ every build, so there is nothing to assert them against — but
    /// a `K2G_COMMIT` carrying a newline, a `fatal:` or a full 40-character hash would
    /// mean the build script captured something other than what it meant to, and that
    /// is worth catching here rather than in a screenshot.
    #[test]
    fn the_compiled_in_stamp_has_the_shape_the_build_script_promises() {
        let commit = env!("K2G_COMMIT");
        assert!(
            commit.is_empty()
                || (commit.len() <= 12 && commit.chars().all(|c| c.is_ascii_hexdigit())),
            "K2G_COMMIT is {commit:?}, which is not a short hash"
        );

        let dirty = env!("K2G_COMMIT_DIRTY");
        assert!(dirty.is_empty() || dirty == "1", "K2G_COMMIT_DIRTY is {dirty:?}");

        let ci = env!("K2G_CI");
        assert!(
            ci.is_empty() || ci.contains(" run "),
            "K2G_CI is {ci:?}, which is not `<workflow> run <n>`"
        );
    }

    /// The log line stays one line — it goes through `log::info!`, and a stamp that
    /// wrapped would break every line-oriented reader of the session log.
    ///
    /// Asserted against the real build rather than a fixture, because that is the value
    /// that will actually be logged and the only way its width is ever tested.
    #[test]
    fn the_log_line_is_one_line() {
        let line = one_line();

        assert!(!line.contains('\n'), "{line:?} spans lines");
        assert!(line.starts_with(&format!("k2g {}", env!("CARGO_PKG_VERSION"))));
        assert!(
            !line.contains("()") && !line.contains(", ,"),
            "{line:?} has a gap where an unknown field was dropped"
        );
    }
}
