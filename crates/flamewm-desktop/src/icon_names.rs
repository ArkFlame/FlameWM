use flamewm_desktop_core::model::DesktopItemKind;
use flamewm_integrations_linux::icons::IconLookupPurpose;

/// Which icon lookup purpose a candidate name uses. Explicit `DesktopLauncher`
/// Icon= values are application icons; per-kind defaults are semantic.
#[must_use]
pub fn icon_purpose(kind: DesktopItemKind, has_override: bool) -> IconLookupPurpose {
    if kind == DesktopItemKind::DesktopLauncher && has_override {
        IconLookupPurpose::Application
    } else {
        IconLookupPurpose::Semantic
    }
}

/// Which icon name identifies the selected launcher for a given item kind.
#[must_use]
pub fn launcher_icon_name(kind: DesktopItemKind, icon_override: Option<&str>) -> String {
    icon_override
        .map(str::trim)
        .filter(|icon| !icon.is_empty())
        .map_or_else(|| icon_name(kind).to_owned(), str::to_owned)
}

/// Freedesktop MIME-like icon names for desktop files. The theme resolver owns
/// actual icon lookup; the filename extension only selects its semantic key.
#[must_use]
pub fn file_icon_name(path: &std::path::Path) -> &'static str {
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image-png",
        "jpg" | "jpeg" => "image-jpeg",
        "webp" => "image-webp",
        "gif" => "image-gif",
        "bmp" => "image-bmp",
        "svg" | "svgz" => "image-svg+xml",
        "tif" | "tiff" => "image-tiff",
        "avif" => "image-avif",
        "heic" | "heif" => "image-heic",
        "pdf" => "application-pdf",
        "md" | "markdown" => "text-markdown",
        "txt" | "log" | "ini" | "conf" => "text-plain",
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "java" | "sh" => "text-x-script",
        "json" | "xml" | "yaml" | "yml" | "toml" => "text-x-generic",
        "doc" | "docx" | "odt" => "x-office-document",
        "xls" | "xlsx" | "ods" | "csv" => "x-office-spreadsheet",
        "ppt" | "pptx" | "odp" => "x-office-presentation",
        "zip" => "application-zip",
        "tar" | "gz" | "xz" | "7z" | "rar" => "package-x-generic",
        "mp3" | "wav" | "ogg" | "flac" | "m4a" => "audio-x-generic",
        "mp4" | "mkv" | "avi" | "mov" | "webm" => "video-x-generic",
        "exe" | "appimage" => "application-x-executable",
        _ => "text-x-generic",
    }
}

pub(crate) fn icon_semantic(kind: DesktopItemKind) -> &'static str {
    match kind {
        DesktopItemKind::Directory => "directory",
        DesktopItemKind::DesktopLauncher => "desktop-launcher",
        DesktopItemKind::File => "file",
        DesktopItemKind::Symlink => "symlink",
        DesktopItemKind::TrashPseudo => "trash",
    }
}

pub(crate) fn icon_name(kind: DesktopItemKind) -> &'static str {
    match kind {
        DesktopItemKind::Directory => "folder",
        DesktopItemKind::DesktopLauncher => "desktop",
        DesktopItemKind::File => "document-edit",
        DesktopItemKind::Symlink => "link",
        DesktopItemKind::TrashPseudo => "trash",
    }
}
