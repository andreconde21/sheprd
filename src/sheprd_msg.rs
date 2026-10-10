//! andreconde fork (sheprd): agents messaging agents across machines.
//!
//! The logic is one Python script (`scripts/sheprd-msg`) so the exact same tool runs on machines
//! that only have stock herdr: `sheprd msg setup` copies it there. sheprd carries it inside the
//! binary, runs it for `sheprd msg …`, and keeps its relay running while the client is open.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const SCRIPT: &str = include_str!("../scripts/sheprd-msg");
/// The Claude Code integration; `setup` installs it from next to the script.
const CLAUDE_HOOK: &str = include_str!("../scripts/sheprd-claude-hook");
/// The example `[[status]]` command for the work panel; `setup` installs it from next to the script.
const STATUS_GITHUB: &str = include_str!("../scripts/sheprd-status-github");
/// Opens a document in the user's editor in a split (Ctrl+click on a Markdown path, and agents).
const DOC: &str = include_str!("../scripts/sheprd-doc");

/// Writes the bundled scripts next to the sheprd binary (only when they changed) and returns
/// the messaging script's path.
fn script_path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    for (name, content) in [
        ("sheprd-claude-hook", CLAUDE_HOOK),
        ("sheprd-status-github", STATUS_GITHUB),
        ("sheprd-doc", DOC),
        ("sheprd-msg", SCRIPT),
    ] {
        let path = exe.with_file_name(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(content) {
            std::fs::write(&path, content)?;
        }
    }
    Ok(exe.with_file_name("sheprd-msg"))
}

fn command(args: &[String]) -> std::io::Result<Command> {
    let mut cmd = Command::new("python3");
    cmd.arg(script_path()?).args(args);
    // The script drives herdr through this binary, so it reaches the servers sheprd talks to.
    if let Ok(exe) = std::env::current_exe() {
        cmd.env("HERDR_BIN_PATH", exe);
    }
    Ok(cmd)
}

/// One of the bundled scripts, written next to the sheprd binary (`sheprd-doc`, …).
pub(crate) fn bundled_script(name: &str) -> Option<PathBuf> {
    script_path().ok()?;
    let path = std::env::current_exe().ok()?.with_file_name(name);
    path.exists().then_some(path)
}

/// `sheprd msg <args>`: runs the script and exits with its status.
pub fn run_cli(args: &[String]) -> ! {
    match command(args).and_then(|mut cmd| cmd.status()) {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(err) => {
            eprintln!("sheprd msg: could not run python3: {err}");
            std::process::exit(1);
        }
    }
}

/// Starts the relay once per client process. It delivers messages queued on machines that cannot
/// reach this one, holds its own lock (a second window's relay exits at once) and stops when this
/// process exits.
pub fn ensure_relay() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    if cfg!(test) {
        return;
    }
    STARTED.call_once(|| {
        let spawned = command(&["relay".to_owned()]).and_then(|mut cmd| {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        });
        if let Err(err) = spawned {
            tracing::warn!("sheprd msg relay not started: {err}");
        }
    });
}
