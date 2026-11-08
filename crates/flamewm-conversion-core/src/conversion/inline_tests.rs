use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_desktop_core::model::is_temporary_convert_name;
    /// Tests that take conversion slots share one global budget; serialise them.
    pub(super) static SLOT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[test]
    fn same_format_is_not_offered_and_source_detection_is_case_insensitive() {
        let formats = offered_formats(Path::new("A.PNG"));
        assert!(!formats.contains(&"png"));
        assert!(formats.contains(&"webp"));
        assert!(offered_formats(Path::new("folder")).is_empty());
        assert!(!offered_formats(Path::new("readme.txt")).contains(&"webp"));
        assert!(!offered_formats(Path::new("photo.JPEG")).contains(&"jpg"));
        assert!(is_temporary_convert_name(std::ffi::OsStr::new(
            ".flamewm-convert-123-42.webp"
        )));
        assert!(!is_temporary_convert_name(std::ffi::OsStr::new(
            ".hidden-photo.webp"
        )));
    }
    #[test]
    fn output_names_remain_beside_source_without_overwriting_original() {
        let source = Path::new("/home/user/Desktop/photo.png");
        assert_eq!(
            destination(source, "webp", 0).unwrap(),
            Path::new("/home/user/Desktop/photo.webp")
        );
        assert_eq!(
            destination(source, "webp", 1).unwrap(),
            Path::new("/home/user/Desktop/photo (converted 2).webp")
        );
    }
    #[test]
    fn multiframe_sources_select_first_frame_for_single_image_destinations() {
        let source = Path::new("/home/user/Desktop/animated picture.gif");
        assert_eq!(
            source_argument(source, "jpg"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "png"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "avif"),
            OsString::from("/home/user/Desktop/animated picture.gif[0]")
        );
        assert_eq!(
            source_argument(source, "webp"),
            source.as_os_str().to_os_string()
        );
        assert_eq!(
            source_argument(source, "gif"),
            source.as_os_str().to_os_string()
        );
    }
    #[test]
    fn concurrent_conversion_budget_is_released_even_on_early_failure() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = ConversionSlot::acquire().expect("first slot");
        let second = ConversionSlot::acquire().expect("second slot");
        assert!(
            ConversionSlot::acquire().is_err(),
            "third conversion must not start"
        );
        drop(first);
        let replacement = ConversionSlot::acquire().expect("released slot reused");
        drop(second);
        drop(replacement);
    }
    #[test]
    fn converter_timeout_stops_a_blocked_child_without_waiting_for_completion() {
        let mut sleeper = Command::new("sleep");
        sleeper.arg("2");
        let start = Instant::now();
        let error = wait_for_converter_with_timeout(sleeper, Duration::from_millis(10))
            .expect_err("hung conversion must time out");
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn rejects_symlink_and_unknown_formats_before_launching_converter() {
        assert!(convert_image(Path::new("file.png"), "ps").is_err());
        assert!(convert_image(Path::new("file.txt"), "png").is_err());
    }
}

#[cfg(test)]
mod document_format_contracts {
    use super::*;
    #[test]
    fn text_file_has_markdown_target_without_optional_provider() {
        assert!(offered_formats(Path::new("notes.txt")).contains(&"md"));
    }
    #[test]
    fn unknown_binary_never_offers_document_conversion() {
        assert!(offered_formats(Path::new("program.bin")).is_empty());
    }
}

#[cfg(test)]
mod media_format_contracts {
    use super::tests::SLOT_TESTS;
    use super::*;
    #[test]
    fn audio_and_video_targets_exclude_the_source_and_fit_eight_slots() {
        for source in AUDIO_SOURCES.iter().chain(VIDEO_SOURCES.iter()) {
            let targets = media_targets(source);
            assert!(!targets.contains(source), "{source} offers itself");
            assert!(!targets.is_empty() && targets.len() <= 8, "{source}");
        }
        assert!(media_targets("png").is_empty());
        assert_eq!(media_targets("srt"), vec!["vtt"]);
        assert_eq!(media_targets("vtt"), vec!["srt"]);
        assert!(media_source(Path::new("cues.SRT")));
        assert!(media_targets("mp3").contains(&"wav"));
        assert!(!media_targets("mp3").contains(&"mp3"));
        assert!(media_targets("mp4").contains(&"gif"));
        assert!(media_targets("mp4").contains(&"mp3"));
    }
    #[test]
    fn every_media_target_has_codec_arguments_and_audio_drops_video() {
        for target in AUDIO_TARGETS.iter().chain(VIDEO_TARGETS.iter()) {
            assert!(!media_codec_args(target).is_empty(), "{target}");
        }
        for target in AUDIO_TARGETS {
            assert!(media_codec_args(target).contains(&"-vn"), "{target}");
        }
        assert!(media_codec_args("gif").contains(&"-an"));
        assert!(media_codec_args("unknown").is_empty());
    }
    #[test]
    fn ffmpeg_converts_audio_beside_the_source_without_overwriting() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if available_program("ffmpeg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-media-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let source = dir.join("tone.wav");
        let made = Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .arg(&source)
            .status()
            .expect("ffmpeg runs");
        assert!(made.success());
        let first = convert_file(&source, "flac").expect("first conversion");
        assert_eq!(first, dir.join("tone.flac"));
        let second = convert_file(&source, "flac").expect("second conversion");
        assert_eq!(second, dir.join("tone (converted 2).flac"));
        assert!(source.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn tool_names_and_slots_fit_the_wheel() {
        assert_eq!(
            tool_destination(Path::new("/d/photo.png"), " (1280)", "png", 0).unwrap(),
            Path::new("/d/photo (1280).png")
        );
        assert_eq!(
            tool_destination(Path::new("/d/photo.png"), " (1280)", "png", 1).unwrap(),
            Path::new("/d/photo (1280) 2.png")
        );
        for tool in [
            Tool::ImageResize,
            Tool::ImageJpeg,
            Tool::AudioCompress,
            Tool::VideoFrame,
            Tool::Zip,
            Tool::Sha256,
        ] {
            assert!(!tool.label().is_empty());
        }
        assert!(
            offered_tools(Path::new("folder")).is_empty() || available_program("zip").is_some()
        );
        assert!(!offered_tools(Path::new("notes.txt")).contains(&Tool::AudioMono));
    }
    #[test]
    fn tools_run_beside_the_source_and_keep_the_original() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("flamewm-tools-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        if available_program("sha256sum").is_some() {
            let hash = run_tool(&text, Tool::Sha256).expect("sha256 tool");
            assert_eq!(hash, dir.join("notes (sha256).txt"));
            assert!(fs::read_to_string(&hash).unwrap().starts_with("2cf24dba"));
        }
        if available_program("zip").is_some() {
            let zipped = run_tool(&text, Tool::Zip).expect("zip tool");
            assert_eq!(zipped, dir.join("notes.zip"));
            assert!(text.is_file(), "original must survive");
        }
        if available_program("ffmpeg").is_some() {
            let wav = dir.join("tone.wav");
            let made = Command::new("ffmpeg")
                .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.5")
                .arg(&wav)
                .status()
                .expect("ffmpeg runs");
            assert!(made.success());
            let mono = run_tool(&wav, Tool::AudioMono).expect("mono tool");
            assert_eq!(mono, dir.join("tone (mono).wav"));
        }
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn zip_extract_writes_a_fresh_folder_and_keeps_the_archive() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if available_program("unzip").is_none() || available_program("zip").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-extract-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        let zipped = run_tool(&text, Tool::Zip).expect("zip tool");
        let folder = run_tool(&zipped, Tool::Extract).expect("extract tool");
        assert_eq!(folder, dir.join("notes (extracted)"));
        assert_eq!(
            fs::read_to_string(folder.join("notes.txt")).expect("extracted"),
            "hello"
        );
        assert!(zipped.is_file(), "archive must survive");
        let again = run_tool(&zipped, Tool::Extract).expect("second extract");
        assert_eq!(again, dir.join("notes (extracted) 2"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_a_running_child_and_leaves_later_jobs_alone() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sleeper = Command::new("sleep");
        sleeper.arg("5");
        let canceller = thread::spawn(|| {
            thread::sleep(Duration::from_millis(200));
            cancel_conversions();
        });
        let started = Instant::now();
        let result = wait_for_converter_with_timeout(sleeper, Duration::from_secs(30));
        canceller.join().expect("canceller thread");
        assert_eq!(result.unwrap_err(), "conversion cancelled");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "cancel must not wait for the child"
        );
        let quick = Command::new("true");
        let later = wait_for_converter_with_timeout(quick, Duration::from_secs(5));
        assert!(later.expect("job after cancel runs").success());
    }

    #[test]
    fn ffmpeg_converts_srt_to_vtt_beside_the_source() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if available_program("ffmpeg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-subs-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let source = dir.join("cues.srt");
        fs::write(
            &source,
            "1\n00:00:00,000 --> 00:00:01,000\nHello\n\n2\n00:00:01,000 --> 00:00:02,500\nWorld\n",
        )
        .expect("write srt");
        let vtt = convert_file(&source, "vtt").expect("srt to vtt");
        assert_eq!(vtt, dir.join("cues.vtt"));
        let text = fs::read_to_string(&vtt).expect("vtt text");
        assert!(text.starts_with("WEBVTT"), "{text}");
        assert!(text.contains("00:00.000 --> 00:01.000"), "{text}");
        assert!(text.contains("World"), "{text}");
        assert!(source.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_gz_archive_tool_writes_beside_the_source() {
        let _serial = SLOT_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if available_program("tar").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("flamewm-tar-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory");
        let text = dir.join("notes.txt");
        fs::write(&text, "hello").expect("write text");
        let archive = run_tool(&text, Tool::TarGz).expect("tar tool");
        assert_eq!(archive, dir.join("notes (archive).tar.gz"));
        let listing = Command::new("tar")
            .arg("-tzf")
            .arg(&archive)
            .output()
            .expect("tar lists");
        assert!(listing.status.success());
        assert!(String::from_utf8_lossy(&listing.stdout).contains("notes.txt"));
        assert!(text.is_file(), "original must survive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_routing_is_case_insensitive_and_separate_from_images() {
        assert!(media_source(Path::new("Clip.MP4")));
        assert!(media_source(Path::new("song.Opus")));
        assert!(!media_source(Path::new("photo.gif")));
        assert!(!media_source(Path::new("notes.txt")));
    }
}
