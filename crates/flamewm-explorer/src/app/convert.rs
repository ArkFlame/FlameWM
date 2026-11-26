use super::super::*;
use flamewm_conversion_core::conversion::{convert_file, offered_formats};

/// Picker rows shown at once (one column); formats beyond this are not offered from Explorer.
pub(crate) const CONVERT_SLOTS: usize = 8;

/// `offered_formats` scans `PATH` (a stat per directory) and probes the local codecs, and the picker is
/// re-projected while it is open: the offer and its uppercased labels are built once per source file.
struct PickerCache { path: PathBuf, labels: Rc<Vec<String>> }
thread_local! {
    static PICKER_CACHE: RefCell<Option<PickerCache>> = const { RefCell::new(None) };
    static CONVERT_IDS: RefCell<Option<Rc<Vec<String>>>> = const { RefCell::new(None) };
}
fn picker_labels(path: &Path) -> Rc<Vec<String>> {
    PICKER_CACHE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if let Some(cached) = cell.as_ref()
            && cached.path.as_path() == path
        {
            return Rc::clone(&cached.labels);
        }
        let labels = Rc::new(
            offered_formats(path)
                .iter()
                .map(|format| format.to_ascii_uppercase())
                .collect::<Vec<_>>(),
        );
        *cell = Some(PickerCache { path: path.to_path_buf(), labels: Rc::clone(&labels) });
        labels
    })
}
fn convert_ids() -> Rc<Vec<String>> {
    CONVERT_IDS.with(|cell| {
        Rc::clone(cell.borrow_mut().get_or_insert_with(|| {
            Rc::new((0..CONVERT_SLOTS).map(|slot| format!("convert-choice-{slot}")).collect::<Vec<String>>())
        }))
    })
}

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
        let labels = self.convert_picker.as_deref().map(picker_labels);
        doc.visible("convert-dialog", labels.is_some())?;
        let Some(labels) = labels else {
            return Ok(());
        };
        let rows = labels.len().min(CONVERT_SLOTS) as f32;
        let (width, height) = (300.0, 77.0 + rows * 35.0 + 85.0 + 16.0);
        doc.size("convert-dialog", width, height)?;
        doc.position(
            "convert-dialog",
            ((self.window.0 - width) / 2.0).max(0.0),
            ((self.window.1 - height) / 2.0).max(0.0),
        )?;
        let ids = convert_ids();
        for (slot, id) in ids.iter().enumerate() {
            match labels.get(slot) {
                Some(label) => {
                    doc.visible(id, true)?;
                    doc.text_ref(id, label)?;
                }
                None => doc.visible(id, false)?,
            }
        }
        Ok(())
    }
}
