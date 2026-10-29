//! Icon kind for a file or folder, chosen from its extension. Each kind has one SVG glyph under
//! `ui/finder/icons`, and the window shows the matching image node.

use crate::index::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Document,
    Image,
    Code,
    Archive,
    Audio,
    Video,
    File,
}

/// Every kind, in the order the document declares its icon nodes.
pub const KINDS: [Kind; 8] = [
    Kind::Folder,
    Kind::Document,
    Kind::Image,
    Kind::Code,
    Kind::Archive,
    Kind::Audio,
    Kind::Video,
    Kind::File,
];

impl Kind {
    #[must_use]
    pub fn of(entry: &Entry) -> Self {
        if entry.is_dir {
            return Self::Folder;
        }
        let ext = entry
            .name()
            .rsplit_once('.')
            .filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
            .map(|(_, ext)| ext.to_ascii_lowercase());
        match ext.as_deref() {
            Some("pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "pages" | "tex") => {
                Self::Document
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "heic" | "tiff") => {
                Self::Image
            }
            Some(
                "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp" | "go"
                | "java" | "sh" | "html" | "css" | "json" | "toml" | "yaml" | "yml" | "xml" | "nix",
            ) => Self::Code,
            Some("zip" | "tar" | "gz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "deb" | "rpm") => {
                Self::Archive
            }
            Some("mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus") => Self::Audio,
            Some("mp4" | "mkv" | "webm" | "mov" | "avi") => Self::Video,
            _ => Self::File,
        }
    }

    /// Name of the glyph file, without the directory or extension.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Document => "document",
            Self::Image => "image",
            Self::Code => "code",
            Self::Archive => "archive",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::File => "file",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> Entry {
        Entry::new(path.to_owned(), false, 1, 0)
    }

    #[test]
    fn kinds_follow_extensions_case_insensitively() {
        assert_eq!(Kind::of(&file("/h/report.PDF")), Kind::Document);
        assert_eq!(Kind::of(&file("/h/photo.jpg")), Kind::Image);
        assert_eq!(Kind::of(&file("/h/main.rs")), Kind::Code);
        assert_eq!(Kind::of(&file("/h/backup.tar")), Kind::Archive);
        assert_eq!(Kind::of(&file("/h/song.flac")), Kind::Audio);
        assert_eq!(Kind::of(&file("/h/clip.mkv")), Kind::Video);
    }

    #[test]
    fn folders_and_unknown_or_dotfiles_get_their_own_glyphs() {
        assert_eq!(
            Kind::of(&Entry::new("/h/Docs".into(), true, 0, 0)),
            Kind::Folder
        );
        assert_eq!(Kind::of(&file("/h/blob.xyz")), Kind::File);
        assert_eq!(Kind::of(&file("/h/.bashrc")), Kind::File);
        assert_eq!(Kind::of(&file("/h/Makefile")), Kind::File);
    }

    #[test]
    fn every_kind_has_a_unique_slug() {
        let mut slugs: Vec<&str> = KINDS.iter().map(|kind| kind.slug()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), KINDS.len());
    }
}
