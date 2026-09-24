use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use flamewm_integrations_linux::PortalFileChooser;

pub struct WallpaperChooserWorker {
    result: Receiver<Result<Option<PathBuf>, String>>,
    thread: Option<JoinHandle<()>>,
}

impl WallpaperChooserWorker {
    pub fn start() -> Result<Self, String> {
        let (sender, result) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("flamewm-settings-wallpaper-portal".to_owned())
            .spawn(move || {
                let outcome = PortalFileChooser::connect()
                    .and_then(|chooser| chooser.choose_wallpaper(""))
                    .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
            })
            .map_err(|error| format!("failed to start wallpaper chooser worker: {error}"))?;
        Ok(Self {
            result,
            thread: Some(thread),
        })
    }

    pub fn poll(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        match self.result.try_recv() {
            Ok(result) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(
                "wallpaper chooser worker disconnected without a result".to_owned(),
            )),
        }
    }
}
