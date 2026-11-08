//! image/png clipboard hand-off. Ownership lives in `flamewm-clipboard`; this module only
//! reads the saved file and runs the short-lived helper process that holds the selection.

use std::path::Path;
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, ConnectionExt as _};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn atom<C: Connection>(conn: &C, name: &[u8]) -> Result<Atom, String> {
    conn.intern_atom(false, name)
        .map_err(err)?
        .reply()
        .map(|a| a.atom)
        .map_err(err)
}

/// `path` is read once and then no longer needed, so clipboard ownership is
/// independent of subsequent screenshot file deletion or replacement. Blocks until
/// another client takes CLIPBOARD.
pub fn serve(path: &Path, temporary: bool) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(err)?;
    if temporary {
        let _ = std::fs::remove_file(path);
    }
    flamewm_clipboard::Clipboard::connect()?.serve_png_until_replaced(&bytes)
}

/// Makes clipboard persistence optional: no resident process unless --copy is requested.
pub fn spawn_owner(path: &Path, temporary: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(err)?;
    let (conn, _) = x11rb::connect(None).map_err(err)?;
    let selection = atom(&conn, b"CLIPBOARD")?;
    let previous = conn
        .get_selection_owner(selection)
        .map_err(err)?
        .reply()
        .map_err(err)?
        .owner;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--serve-clipboard").arg(path);
    if temporary {
        cmd.arg("--temporary-clipboard");
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(err)?;
    // Avoid returning before the new owner actually takes CLIPBOARD.
    for _ in 0..100 {
        if child.try_wait().map_err(err)?.is_some() {
            return Err("clipboard owner exited before claiming selection".into());
        }
        let owner = conn
            .get_selection_owner(selection)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .owner;
        if owner != NONE && owner != previous {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("clipboard owner did not become ready".into())
}
