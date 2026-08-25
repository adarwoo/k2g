//! k2g's entry point.
//!
//! # No console window in a shipped build
//!
//! Windows decides whether a process gets a console from the subsystem recorded in the
//! executable header, at link time — there is no run-time way to refuse one, and nothing
//! the application does after `main` starts can close a window that was opened before it.
//! The attribute below is therefore the only place this can be said, and a console-
//! subsystem GUI application says it by leaving a black window sitting behind its own for
//! the whole session.
//!
//! `not(debug_assertions)` rather than unconditional: a development run is driven *from* a
//! terminal, and the log on stdout is most of what a developer has to go on — Windows
//! attaches a console-subsystem process to the shell's existing console, so debug builds
//! keep printing into the terminal they were started from and open no window of their own.
//! A shipped build is started from a shortcut, the Start menu, or KiCad's toolbar, where
//! there is no console to inherit and Windows makes one.
//!
//! The cost is that the two command-line flags lose their default output channel, which
//! [`attach_parent_console`] hands back. The attribute is inert on Linux and macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod build_info;
mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;
mod version;

use std::io::Write;

use ui::UiLaunchData;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    // First, before anything at all. Two reasons, and the second is the one that
    // matters: the log registry below would interleave its output with the block being
    // printed, and `claim_single_instance` further down holds a lock for the life of
    // the process — so a `--version` placed after it would print nothing while a k2g
    // window is open, which is precisely when the question gets asked.
    if let Some(text) = respond_to_arguments(std::env::args().skip(1)) {
        // Borrow the launching shell's console first: a shipped Windows build has none of
        // its own, and an answer nobody can read is not an answer. No-op everywhere else.
        attach_parent_console();
        // Not `println!`, which panics if the write fails. There may genuinely be no
        // console to write to — a shortcut carrying `--version`, a launch from Explorer —
        // and the right answer to "nobody is listening" is silence, not a crash with no
        // window to report it in.
        let _ = writeln!(std::io::stdout(), "{text}");
        return;
    }

    // Two parallel sinks under one shared filter: the usual stdout formatter, plus
    // an in-memory capture that backs the in-app Logs viewer (see
    // `runtime::log_capture`). The `EnvFilter` on the registry gates both, so the
    // viewer honours `RUST_LOG` exactly like the console does.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(runtime::CaptureLayer)
        .init();

    dioxus_logger::initialize_default();

    // The full build stamp, not just the version: this line is what a session log has
    // to answer "which build was that" with, months later, when the binary is gone.
    log::info!("Starting {}", build_info::one_line());

    // WebKitGTK renders through a DMABUF path that assumes the GPU stack can share
    // buffers between the web process and the compositor. Where it can't, the web
    // process dies mid-frame and the only thing k2g sees is
    // `Error sending edits to webview: Broken pipe` — the window is simply blank,
    // with the actual fault (a rejected GPU command submission) buried in pages of
    // driver noise on stderr, under no heading that mentions the webview.
    //
    // Set unconditionally rather than probing the driver: it was found on nouveau,
    // but the same path breaks on the proprietary NVIDIA driver and inside VMs, and
    // a blank window is a far worse failure than losing hardware compositing on a
    // UI this static. WebKit reads `0` as "keep the renderer", so a stack that
    // works can have it back with `WEBKIT_DISABLE_DMABUF_RENDERER=0`; only an
    // absent variable is overridden, never a value the user chose.
    //
    // Safe here, and only here: `set_var` requires that no other thread is reading
    // the environment, and main() is still single-threaded at this point — the
    // KiCad connection below and the webview itself both come later.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        log::info!(
            "Disabled the WebKit DMABUF renderer; \
             set WEBKIT_DISABLE_DMABUF_RENDERER=0 to keep hardware compositing"
        );
    }

    // One k2g per data directory. Taken before anything else touches that directory,
    // and held — in `_instance` — for the whole run: dropping the file releases the
    // lock, so binding it to a name that lives until `main` returns is what keeps the
    // claim. See `runtime::single_instance` for why two instances are not merely
    // redundant but destructive.
    let _instance = match claim_single_instance() {
        Some(claim) => claim,
        None => return,
    };

    // Repair any KiCad plugin registration that still points at a previous build,
    // before connecting — an update replaces the executable, and the registration
    // has to follow it or the toolbar button starts launching nothing. Only ever
    // touches a registration the user explicitly created; it never makes one.
    runtime::kicad_integration::refresh_registrations();

    // Collect the reachable KiCad's open board (at most one). Stitching happens
    // once when the board is cached in the ctx (see `AppCtx`).
    //
    // When KiCad launched us as a plugin it put `KICAD_API_SOCKET` and
    // `KICAD_API_TOKEN` in our environment, and `kicad-ipc-rs` prefers both over its
    // temp-directory guess — so this connects straight back to the instance that
    // started us, with no discovery involved.
    let acquired = runtime::acquire_board();

    ui::launch(UiLaunchData {
        kicad_status: acquired.status,
        board_snapshot: acquired.board,
        copper: acquired.copper,
    });
}

/// Takes the single-instance lock, or hands this launch over to the k2g already running.
///
/// `None` means stop — quietly, because the operator's request has been answered by the
/// window coming forward. Where it could not be raised (no portable way off Windows, or
/// the window was not found) the refusal is said out loud instead: a launch that appears
/// to do nothing at all is the one outcome worth avoiding, since the natural response to
/// it is to press the button again.
fn claim_single_instance() -> Option<runtime::single_instance::Claim> {
    use runtime::single_instance::{claim, raise_running_window, Claim};

    let Some(data_dir) = paths::k2g_data_dir() else {
        // No data directory resolved: the launch has larger problems than coordination,
        // and `ensure_app_dirs` reports them properly a moment from now.
        return Some(Claim::Unknown("no platform data directory".to_string()));
    };
    // The lock lives in the directory it protects, so the directory has to exist first.
    // Ignored on failure for the same reason the claim itself is: an unusable path is
    // reported by the real check, not by this one.
    let _ = std::fs::create_dir_all(&data_dir);

    match claim(&data_dir) {
        held @ Claim::Held(_) => Some(held),
        Claim::Unknown(reason) => {
            log::warn!("could not take the single-instance lock ({reason}); starting anyway");
            Some(Claim::Unknown(reason))
        }
        Claim::Taken => {
            log::info!("k2g is already running for this user; handing over to it");
            if !raise_running_window() {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Info)
                    .set_title("k2g is already running")
                    .set_description(
                        "k2g is already open for this user. Switch to its window — a second \
                         copy would share the same settings and profiles, and each would \
                         overwrite the other's changes.",
                    )
                    .show();
            }
            None
        }
    }
}

/// What to print and exit for, or `None` to launch the application.
///
/// # Deliberately permissive
///
/// Only `--version`/`-V` and `--help`/`-h` are recognised; **everything else falls
/// through and launches the GUI exactly as before**. Six things start this binary — the
/// KiCad toolbar shim, a desktop shortcut, the installed build, the portable zip,
/// `cargo run` and `dx serve` — and a strict parser that rejected an unrecognised
/// argument would be a way to break one of those launch paths for no gain. There is
/// nothing here that needs a parser library; it is a match on two strings.
fn respond_to_arguments(args: impl Iterator<Item = String>) -> Option<String> {
    for arg in args {
        match arg.as_str() {
            "--version" | "-V" => return Some(build_info::describe()),
            "--help" | "-h" => return Some(usage()),
            _ => {}
        }
    }
    None
}

/// Borrows the launching shell's console, so `--version` and `--help` have somewhere to
/// print in a build that has no console of its own.
///
/// # Why this is needed
///
/// A shipped Windows build is linked as a GUI application (see the crate attribute), and a
/// GUI application starts with no console at all: `println!` writes to a handle that is not
/// there, silently, and the two flags whose whole purpose is to be asked from a shell
/// answer nothing. `AttachConsole(ATTACH_PARENT_PROCESS)` joins the console the shell
/// already owns. It never makes one — that is `AllocConsole`, and calling it here would put
/// back precisely the window the subsystem attribute exists to remove — so a launch from a
/// shortcut, the Start menu or KiCad's toolbar stays windowless and silent.
///
/// # Why attaching alone is not enough
///
/// Attaching gives the process a console but leaves its standard handles as they were, and
/// for a GUI process those are usually null. Opening `CONOUT$` — the active screen buffer
/// of whichever console was just joined — and installing it as stdout is what makes the
/// write land somewhere a person can read.
///
/// Handles the shell *did* pass are left alone. `k2g --version > build.txt` and
/// `k2g --version | findstr commit` both arrive with a real stdout already inherited, and
/// overwriting it would put the output on the terminal the user had explicitly redirected
/// it away from.
///
/// # What it cannot fix
///
/// A GUI application does not hold the shell's prompt, so an interactive `k2g --version`
/// prints after the next prompt has been drawn. Redirecting or piping — how a script would
/// read it — makes both `cmd` and PowerShell wait, so the scripted case stays exact and
/// only the interactive one looks untidy. Buying that back means shipping a second,
/// console-subsystem executable whose only job is to print two strings.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use std::ptr;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    // SAFETY: a documented constant is the only argument, and there is nothing to keep
    // alive across the call. Failure is the ordinary case rather than an error: a launch
    // from Explorer has no parent console to join, and the flags then print into the void
    // a GUI launch asked for.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }

    // `CONOUT$` names the attached console's screen buffer whatever stdout is pointing at,
    // which is the point: it cannot be redirected out from under us. NUL-terminated UTF-16,
    // as `CreateFileW` requires.
    let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();

    for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: `id` is one of the documented standard-handle identifiers.
        let existing = unsafe { GetStdHandle(id) };
        if !existing.is_null() && existing != INVALID_HANDLE_VALUE {
            continue; // the shell passed a real pipe or file; that is the user's choice
        }

        // SAFETY: `name` is a live NUL-terminated wide string that outlives the call. Both
        // share modes are required — the console is already open in the shell we joined —
        // and the security-attributes and template arguments are null, which the API
        // documents as "defaults" and "no template".
        let console = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING, // a console buffer is never created
                0,
                ptr::null_mut(),
            )
        };
        if console == INVALID_HANDLE_VALUE {
            continue;
        }

        // Deliberately never closed: it has to outlive every write, and the only caller
        // returns straight into a process exit that closes it anyway.
        //
        // SAFETY: installing a handle this call has just opened, against the identifier it
        // was opened for.
        unsafe { SetStdHandle(id, console) };
    }
}

/// Nothing to attach to, or nothing that needs attaching.
///
/// A debug build is a console-subsystem binary already writing into the terminal that
/// started it, and Linux and macOS never tie stdout to a window in the first place.
#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console() {}

/// `--help` output. Short on purpose: k2g is a desktop application with no command-line
/// interface to document, and these two flags are the whole of it.
fn usage() -> String {
    format!(
        "k2g — KiCad → GCode, CAM for machining PCBs\n\
         \n\
         Usage: k2g [OPTIONS]\n\
         \n\
         Run with no arguments to open the application.\n\
         \n\
         Options:\n\
         \x20 -V, --version  Report the running build — commit, build time and path\n\
         \x20 -h, --help     Show this message\n\
         \n\
         {}",
        build_info::describe()
    )
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    fn respond(args: &[&str]) -> Option<String> {
        respond_to_arguments(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn both_spellings_of_version_report_the_build() {
        for flag in ["--version", "-V"] {
            let text = respond(&[flag]).unwrap_or_else(|| panic!("{flag} is not recognised"));
            assert_eq!(text, build_info::describe());
        }
    }

    #[test]
    fn both_spellings_of_help_explain_the_two_flags() {
        for flag in ["--help", "-h"] {
            let text = respond(&[flag]).unwrap_or_else(|| panic!("{flag} is not recognised"));
            assert!(text.contains("--version"));
            assert!(text.contains("--help"));
            // Help ends with the build stamp, so `k2g -h` answers the same question.
            assert!(text.ends_with(&build_info::describe()));
        }
    }

    /// **Nothing else stops the launch.** This is the invariant that keeps six launch
    /// paths working: an argument this does not recognise means "open the application",
    /// never "refuse and print usage".
    #[test]
    fn anything_else_launches_the_application() {
        let unknown = [
            vec![],
            vec![""],
            vec!["--serve"],
            vec!["--hot-reload"],
            vec![r"E:\boards\panel.kicad_pcb"],
            vec!["-v"],           // lower case is not the version flag
            vec!["version"],      // no leading dashes
            vec!["--versions"],   // near miss
        ];

        for args in unknown {
            assert!(
                respond(&args).is_none(),
                "{args:?} must launch the application, not print and exit"
            );
        }
    }

    /// `--` is not an end-of-flags marker here, and that is a decision rather than an
    /// omission: k2g takes no positional arguments, so there is nothing for `--` to
    /// protect from being read as a flag. Implementing the convention would add a rule
    /// to a two-flag scanner in order to change the behaviour of an invocation nobody
    /// has a reason to type.
    #[test]
    fn a_double_dash_separator_is_not_treated_as_one() {
        assert!(respond(&["--", "--version"]).is_some());
    }

    /// A recognised flag anywhere in the list still answers, so `dx serve`-style
    /// wrappers that prepend their own arguments do not hide it.
    #[test]
    fn a_flag_is_found_wherever_it_sits() {
        assert!(respond(&["--profile", "release", "--version"]).is_some());
    }

    /// **The arguments are answered before the single-instance lock is taken.**
    ///
    /// A source scan, because the failure is silent rather than a compile error: put
    /// `respond_to_arguments` after `claim_single_instance` and `k2g --version` prints
    /// nothing whenever a k2g window is open — which is the one situation in which
    /// somebody runs it. Nothing in the type system prevents that reordering.
    #[test]
    fn the_arguments_are_answered_before_the_instance_lock() {
        let source = include_str!("main.rs");
        let body = source
            .split_once("fn main() {")
            .expect("main.rs declares fn main")
            .1;

        let args = body
            .find("respond_to_arguments")
            .expect("main() calls respond_to_arguments");
        let lock = body
            .find("claim_single_instance()")
            .expect("main() calls claim_single_instance");

        assert!(
            args < lock,
            "main() takes the single-instance lock before answering --version, so the \
             flag prints nothing while a k2g window is open. Move the \
             `respond_to_arguments` block back to the top of main()."
        );
    }

    /// And before the log registry, so the block is not interleaved with log lines.
    #[test]
    fn the_arguments_are_answered_before_logging_starts() {
        let source = include_str!("main.rs");
        let body = source.split_once("fn main() {").expect("fn main").1;

        let args = body.find("respond_to_arguments").expect("respond_to_arguments");
        let logging = body
            .find("tracing_subscriber::registry()")
            .expect("main() initialises the log registry");

        assert!(args < logging, "--version output would be interleaved with log lines");
    }
}
