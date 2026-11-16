//! File and folder pickers. On Linux the dialog runs on a thread of its own
//! and its result comes back as a command in a later frame (see [`drain`]),
//! so the UI keeps drawing while the dialog is open; elsewhere the dialog
//! is asked right away, as it always was.

use std::path::PathBuf;

use nikgob_view::Command;
#[cfg(target_os = "android")]
use crate::no_rfd as rfd;

#[derive(Clone, Copy)]
enum Kind {
    Files,
    File,
    Folder,
}

/// Several files; `then` makes the command of what was picked.
pub fn files(
    build: impl FnOnce(rfd::FileDialog) -> rfd::FileDialog + Send + 'static,
    then: impl FnOnce(Vec<PathBuf>) -> Command + Send + 'static,
    out: &mut Vec<Command>,
) {
    start(Kind::Files, build, Box::new(then), out);
}

/// One file.
pub fn file(
    build: impl FnOnce(rfd::FileDialog) -> rfd::FileDialog + Send + 'static,
    then: impl FnOnce(PathBuf) -> Command + Send + 'static,
    out: &mut Vec<Command>,
) {
    start(Kind::File, build, Box::new(move |v| then(v.into_iter().next().unwrap_or_default())), out);
}

/// One folder.
pub fn folder(
    build: impl FnOnce(rfd::FileDialog) -> rfd::FileDialog + Send + 'static,
    then: impl FnOnce(PathBuf) -> Command + Send + 'static,
    out: &mut Vec<Command>,
) {
    start(Kind::Folder, build, Box::new(move |v| then(v.into_iter().next().unwrap_or_default())), out);
}

type Then = Box<dyn FnOnce(Vec<PathBuf>) -> Command + Send>;

fn ask(kind: Kind, dialog: rfd::FileDialog) -> Vec<PathBuf> {
    match kind {
        Kind::Files => dialog.pick_files().unwrap_or_default(),
        Kind::File => dialog.pick_file().into_iter().collect(),
        Kind::Folder => dialog.pick_folder().into_iter().collect(),
    }
}

#[cfg(not(target_os = "linux"))]
fn start(
    kind: Kind,
    build: impl FnOnce(rfd::FileDialog) -> rfd::FileDialog,
    then: Then,
    out: &mut Vec<Command>,
) {
    let paths = ask(kind, build(rfd::FileDialog::new()));
    if !paths.is_empty() {
        out.push(then(paths));
    }
}

/// Called every frame: hands the commands of the dialogs that were closed
/// since the last one to `out`, and remembers the context to repaint.
#[cfg(not(target_os = "linux"))]
pub fn drain(_ctx: &egui::Context, _out: &mut Vec<Command>) {}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Mutex, OnceLock};

    pub static BUSY: AtomicBool = AtomicBool::new(false);
    pub static CTX: OnceLock<egui::Context> = OnceLock::new();
    pub static DONE: Mutex<Vec<Box<dyn FnOnce() -> nikgob_view::Command + Send>>> = Mutex::new(Vec::new());
}

#[cfg(target_os = "linux")]
fn start(
    kind: Kind,
    build: impl FnOnce(rfd::FileDialog) -> rfd::FileDialog + Send + 'static,
    then: Then,
    out: &mut Vec<Command>,
) {
    use std::sync::PoisonError;
    use std::sync::atomic::Ordering;

    let _ = out;
    // One dialog at a time: a second click while it is open does nothing.
    if linux::BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let ctx = linux::CTX.get().cloned();
    let spawned = std::thread::Builder::new().name("file-dialog".into()).spawn(move || {
        let paths = ask(kind, build(rfd::FileDialog::new()));
        if !paths.is_empty() {
            linux::DONE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Box::new(move || then(paths)));
        }
        linux::BUSY.store(false, Ordering::Release);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    if let Err(e) = spawned {
        log::warn!("cannot open the file dialog: {e}");
        linux::BUSY.store(false, Ordering::Release);
    }
}

#[cfg(target_os = "linux")]
pub fn drain(ctx: &egui::Context, out: &mut Vec<Command>) {
    use std::sync::PoisonError;

    linux::CTX.get_or_init(|| ctx.clone());
    let done = std::mem::take(&mut *linux::DONE.lock().unwrap_or_else(PoisonError::into_inner));
    out.extend(done.into_iter().map(|f| f()));
}
