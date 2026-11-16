//! Android has no file dialogs of this kind: its pickers belong to the
//! Android shell, so a pick asked from a page comes back empty there.

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct FileDialog;

impl FileDialog {
    pub fn new() -> Self {
        Self
    }

    pub fn set_title(self, _title: impl Into<String>) -> Self {
        self
    }

    pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
        self
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        None
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        None
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        None
    }
}
