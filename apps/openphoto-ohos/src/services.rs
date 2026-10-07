//! Platform services for the HarmonyOS host: the same codecs and document I/O as desktop,
//! backed by the app sandbox, plus the system file dialogs.
//!
//! The Ability bridge is asynchronous while the UI's dialog hooks are synchronous, so both dialogs
//! are split in two, exactly like the web build splits its pickers:
//!
//! - **Open**: `pick_open` fires the system picker on a worker thread and returns `None`. The
//!   chosen files are read and pushed into the `inbox`, which the UI drains every frame and opens.
//! - **Save**: `pick_save` fires the system save dialog and immediately returns a real path in the
//!   app's Documents directory, so export and every later Save work with one click. When the user
//!   confirms a location, the worker copies the file there and remembers the redirect, so later
//!   Saves (and `write`) go straight to the chosen URI. Cancelling keeps the Documents copy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

use openharmony_ability::OpenHarmonyApp;
use openharmony_ability_plugin_files::{FileDialogFilter, FileDialogOptions, FilesExt, dialog_type};
use openphoto_ui_egui::{Inbox, Services};

use crate::bridge::block_on;

/// Formats File › Open accepts (mirrors the desktop dialog filter).
const OPEN_PATTERNS: &[(&str, &str)] = &[
    ("Images and documents", "pcraft;psd;psb;png;jpg;jpeg;tif;tiff;webp;gif;bmp;tga;ico;qoi;exr;hdr;pbm;pgm;ppm;pam;pfm"),
    ("Camera raw", "dng;cr2;cr3;nef;nrw;arw;pef;orf;rw2;raf"),
    ("Photoshop brushes and gradients", "abr;grd"),
];

/// Shared dialog state: the redirect table (local path → user-chosen URI path) and the
/// in-flight guard so rapid menu clicks don't stack pickers.
struct Dialogs {
    app: OpenHarmonyApp,
    inbox: Inbox,
    documents: String,
    redirect: Mutex<HashMap<String, String>>,
    busy: AtomicBool,
}

impl Dialogs {
    /// Runs `request` on a worker thread (the ability main thread must never block), unless a
    /// dialog is already open. Returns `false` when busy.
    fn spawn(&self, request: impl FnOnce(OpenHarmonyApp) + Send + 'static) -> bool {
        if self.busy.swap(true, Ordering::AcqRel) {
            return false;
        }
        let app = self.app.clone();
        std::thread::spawn(move || {
            request(app);
        });
        true
    }

    fn done(&self) {
        self.busy.store(false, Ordering::Release);
    }

    /// Reads a picker URI into `(display name, bytes)`.
    fn read_uri(uri: &str) -> Option<(String, Vec<u8>)> {
        let path = ohos_fileuri_binding::get_path_from_uri(uri).ok()?;
        let bytes = std::fs::read(&path).ok()?;
        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| uri.to_string());
        Some((name, bytes))
    }

    /// Unique path in Documents for `suggested` (never overwrites an existing file).
    fn staging_path(&self, suggested: &str) -> String {
        let file = std::path::Path::new(suggested)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled.psd");
        let base = std::path::Path::new(&self.documents).join(file);
        if !base.exists() {
            return base.to_string_lossy().into_owned();
        }
        let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled");
        let ext = base.extension().and_then(|e| e.to_str()).unwrap_or("psd");
        // Dozens of duplicates is already absurd; cap the scan so a hostile name can't spin.
        for i in 2..=9999 {
            let candidate = std::path::Path::new(&self.documents).join(format!("{stem}-{i}.{ext}"));
            if !candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
        base.to_string_lossy().into_owned()
    }
}

/// Builds the services the editor needs. Import, export and PNG encoding are the desktop
/// implementations (same crates, same options); open/save go through the system dialogs above.
pub fn build(app: &OpenHarmonyApp) -> Services {
    let inbox: Inbox = Arc::default();
    let documents = app
        .base_path()
        .map(|base| format!("{base}/files/Documents"))
        .unwrap_or_else(|| "/data/storage/el2/base/files/Documents".into());
    if let Err(error) = std::fs::create_dir_all(&documents) {
        log::warn!("cannot create {documents}: {error}");
    }
    let dialogs = Arc::new(Dialogs {
        app: app.clone(),
        inbox: inbox.clone(),
        documents,
        redirect: Mutex::new(HashMap::new()),
        busy: AtomicBool::new(false),
    });
    let open_dialogs = dialogs.clone();
    let save_dialogs = dialogs.clone();
    let write_dialogs = dialogs.clone();
    Services {
        import: Some(Box::new(|name: &str, bytes: &[u8]| {
            openphoto_io::import(name, bytes)
                .map(|result| (result.document, result.warnings))
                .map_err(|error| error.to_string())
        })),
        export: Some(Box::new(|doc: &openphoto_doc::Document, path: &str, settings: &openphoto_ui_egui::ExportSettings| {
            let mut options = openphoto_io::ExportOptions::default();
            if let Some(quality) = settings.jpeg_quality {
                options.encode.jpeg_quality = quality;
            }
            openphoto_io::export(doc, path, &options)
                .map(|result| (result.bytes, result.warnings))
                .map_err(|error| error.to_string())
        })),
        pick_open: Some(Box::new(move || {
            // Fire the system picker; the picks arrive through the inbox (see below).
            let dialogs = open_dialogs.clone();
            let worker = dialogs.clone();
            if !dialogs.spawn(move |app| {
                log::info!("open dialog started");
                let result = block_on(app.show_file_dialog(
                    FileDialogOptions::new(dialog_type::OPEN_FILE).allow_many(true).filters(
                        OPEN_PATTERNS
                            .iter()
                            .map(|(name, pattern)| {
                                FileDialogFilter::new().name(*name).pattern(*pattern)
                            })
                            .collect(),
                    ),
                ));
                match result {
                    Ok(response) if !response.files.is_empty() => {
                        log::info!("picked {} file(s)", response.files.len());
                        let mut inbox = worker.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        for uri in &response.files {
                            match Dialogs::read_uri(uri) {
                                Some((name, bytes)) => inbox.push((name, bytes)),
                                None => log::warn!("cannot read picked file {uri}"),
                            }
                        }
                    }
                    Ok(_) => log::info!("open dialog cancelled"),
                    Err(error) => log::warn!("open dialog failed: {error}"),
                }
                worker.done();
                log::info!("open dialog done");
            }) {
                log::info!("a file dialog is already open");
            }
            None
        })),
        pick_save: Some(Box::new(move |suggested: &str| {
            // Save locally right away (like the web build's download); the system dialog picks
            // the user-visible copy, and later writes redirect there (see `write`).
            let path = save_dialogs.staging_path(suggested);
            let dialogs = save_dialogs.clone();
            let worker = dialogs.clone();
            let staged = path.clone();
            if !dialogs.spawn(move |app| {
                log::info!("save dialog started for {staged}");
                let ext = std::path::Path::new(&staged)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("psd");
                // Point the picker at the staged file so the name is prefilled; best-effort.
                let mut options = FileDialogOptions::new(dialog_type::SAVE_FILE).filters(vec![
                    FileDialogFilter::new().name("Same as document").pattern(ext),
                ]);
                if let Ok(uri) = ohos_fileuri_binding::get_uri_from_path(&staged) {
                    options = options.default_location(uri);
                }
                let result = block_on(app.show_file_dialog(options));
                match result {
                    Ok(response) => match response.files.first() {
                        Some(uri) => match ohos_fileuri_binding::get_path_from_uri(uri) {
                            Ok(dest) => {
                                let copy = std::fs::copy(&staged, &dest);
                                match copy {
                                    Ok(_) => {
                                        log::info!("saved user copy to {dest}");
                                        worker
                                            .redirect
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .insert(staged, dest);
                                    }
                                    Err(error) => log::warn!("cannot write user copy to {dest}: {error}"),
                                }
                            }
                            Err(error) => log::warn!("cannot resolve save URI {uri}: {error}"),
                        },
                        None => log::info!("save dialog cancelled; kept {staged}"),
                    },
                    Err(error) => log::warn!("save dialog failed: {error}"),
                }
                worker.done();
                log::info!("save dialog done");
            }) {
                log::info!("a file dialog is already open");
            }
            Some(path)
        })),
        write: Some(Box::new(move |path: &str, bytes: &[u8]| {
            // A Save-As whose system dialog resolved redirects straight to the chosen URI.
            if let Some(dest) = write_dialogs
                .redirect
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(path)
                .cloned()
            {
                return std::fs::write(&dest, bytes)
                    .map_err(|error| format!("cannot write {dest}: {error}"));
            }
            openphoto_format::atomic_write(std::path::Path::new(path), bytes)
                .map_err(|error| error.to_string())
        })),
        encode_png: Some(Box::new(|width: u32, height: u32, rgba: &[u8]| {
            let image = openphoto_codecs::Image::from_u8(
                width,
                height,
                openphoto_codecs::ChannelLayout::Rgba,
                rgba.to_vec(),
            )
            .map_err(|error| error.to_string())?;
            openphoto_codecs::encode(
                &image,
                openphoto_codecs::Format::Png,
                &openphoto_codecs::EncodeOptions::default(),
            )
            .map_err(|error| error.to_string())
        })),
        inbox: Some(inbox),
        ..Services::default()
    }
}
