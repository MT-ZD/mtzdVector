//! Single instance: opening files while mtzdVector is running hands them to
//! the running window (new tabs) instead of starting a second process.
//!
//! The first instance listens on a Unix socket in the runtime dir (inside
//! Flatpak: the per-app dir shared by all instances). A later launch sends
//! its absolute paths as JSON and exits. `--new-window` opts out.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Requests received from later launches (each a list of files; an empty
/// list just raises the window).
static PENDING: Mutex<Vec<Vec<PathBuf>>> = Mutex::new(Vec::new());

fn socket_path() -> Option<PathBuf> {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let dir = match std::env::var("FLATPAK_ID") {
        Ok(id) => run.join("app").join(id),
        Err(_) => run,
    };
    Some(dir.join(format!("{}.instance", super::desktop::APP_ID)))
}

/// Outcome of trying to become the primary instance.
pub enum Role {
    /// Files were handed to the running instance: exit.
    Forwarded,
    /// This process is the primary instance (listening for later launches).
    Primary,
}

/// Forward `files` to a running instance, or start listening. `wake` is
/// called (from the listener thread) when a request arrives.
pub fn claim(files: &[PathBuf], wake: impl Fn() + Send + 'static) -> Role {
    let Some(path) = socket_path() else { return Role::Primary };
    claim_at(&path, files, wake)
}

pub fn claim_at(path: &Path, files: &[PathBuf], wake: impl Fn() + Send + 'static) -> Role {
    let abs: Vec<PathBuf> = files.iter().map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone())).collect();
    if forward(path, &abs) {
        return Role::Forwarded;
    }
    // Nobody answered: a stale socket from a crash, or none yet.
    let _ = std::fs::remove_file(path);
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        // Another instance won a race to bind: hand over to it.
        Err(_) if forward(path, &abs) => return Role::Forwarded,
        Err(_) => return Role::Primary,
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(paths) = receive(stream) {
                PENDING.lock().unwrap().push(paths);
                wake();
            }
        }
    });
    Role::Primary
}

fn forward(path: &Path, files: &[PathBuf]) -> bool {
    let Ok(mut s) = UnixStream::connect(path) else { return false };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok(mut msg) = serde_json::to_vec(files) else { return false };
    msg.push(b'\n');
    if s.write_all(&msg).is_err() {
        return false;
    }
    // Wait for the acknowledgement so a dying instance doesn't eat the files.
    let mut ack = String::new();
    BufReader::new(&s).read_line(&mut ack).is_ok() && ack.trim() == "ok"
}

fn receive(mut s: UnixStream) -> Option<Vec<PathBuf>> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    BufReader::new((&s).take(1 << 20)).read_line(&mut line).ok()?;
    let paths: Vec<PathBuf> = serde_json::from_str(line.trim()).ok()?;
    let _ = s.write_all(b"ok\n");
    Some(paths)
}

/// Requests handed over since the last call.
pub fn take_requests() -> Vec<Vec<PathBuf>> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}
