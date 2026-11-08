use super::super::*;
use flamewm_conversion_core::conversion::{convert_file, offered_formats};

/// Picker rows shown at once (one column); formats beyond this are not offered from Explorer.
pub(crate) const CONVERT_SLOTS: usize = 8;

impl ExplorerApp {
    /// Opens the "Convert to" picker for `path`, or reports why the file cannot be converted.
    pub(crate) fn open_convert_picker(&mut self, path: PathBuf) {
        if offered_formats(&path).is_empty() {
            self.model.status = "No conversions for this file".into();
            return;
        }
        self.convert_picker = Some(path);
    }

    /// Handles a picker button: `cancel` closes it, a slot number starts that conversion.
    pub(crate) fn convert_picker_action(&mut self, action: &str) {
        let Some(path) = self.convert_picker.take() else {
            return;
        };
        if action == "cancel" {
            return;
        }
        let format = action
            .parse::<usize>()
            .ok()
            .filter(|slot| *slot < CONVERT_SLOTS)
            .and_then(|slot| offered_formats(&path).get(slot).copied());
        match format {
            Some(format) => self.start_convert(path, format),
            None => self.convert_picker = Some(path),
        }
    }

    /// Runs the conversion on its own thread; the UI thread only hears the outcome through `WorkerResult::Converted`.
    pub(crate) fn start_convert(&mut self, path: PathBuf, format: &'static str) {
        let Some((tx, mut wake)) = self
            .events
            .as_ref()
            .and_then(|(tx, wake)| Some((tx.clone(), wake.try_clone().ok()?)))
        else {
            self.model.status = "Conversion unavailable".into();
            return;
        };
        self.model.status = format!("Converting to {}…", format.to_ascii_uppercase());
        std::thread::spawn(move || {
            let outcome = convert_file(&path, format);
            if tx.send(WorkerResult::Converted(outcome)).is_ok() {
                let _ = wake.write_all(&[1]);
            }
        });
    }

    /// Reports the finished conversion and refreshes the folder so the new file appears.
    pub(crate) fn conversion_finished(&mut self, outcome: Result<PathBuf, String>) {
        match outcome {
            Ok(output) => {
                let name = output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.model.status = format!("Converted to {name}");
                self.scan();
            }
            Err(error) => self.model.status = format!("Conversion failed: {error}"),
        }
    }

    /// Projects the picker: one labelled button per offered format, centred in the window.
    pub(crate) fn project_convert_picker(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let formats = self
            .convert_picker
            .as_deref()
            .map(offered_formats)
            .unwrap_or_default();
        doc.visible("convert-dialog", self.convert_picker.is_some())?;
        if self.convert_picker.is_none() {
            return Ok(());
        }
        let rows = formats.len().min(CONVERT_SLOTS) as f32;
        let (width, height) = (300.0, 77.0 + rows * 35.0 + 85.0 + 16.0);
        doc.size("convert-dialog", width, height)?;
        doc.position(
            "convert-dialog",
            ((self.window.0 - width) / 2.0).max(0.0),
            ((self.window.1 - height) / 2.0).max(0.0),
        )?;
        for slot in 0..CONVERT_SLOTS {
            let id = format!("convert-choice-{slot}");
            match formats.get(slot) {
                Some(format) => {
                    doc.visible(&id, true)?;
                    doc.text(&id, format.to_ascii_uppercase())?;
                }
                None => doc.visible(&id, false)?,
            }
        }
        Ok(())
    }
}
