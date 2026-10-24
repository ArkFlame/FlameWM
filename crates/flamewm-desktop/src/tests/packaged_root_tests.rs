    use super::{installed_data_root, resolve_packaged_root};
    use std::path::{Path, PathBuf};

    #[test]
    fn installed_binary_maps_to_sibling_share_data_root() {
        let executable = Path::new("/opt/flamewm/usr/bin/flamewm-desktop");

        assert_eq!(
            installed_data_root(executable),
            Some(PathBuf::from("/opt/flamewm/usr/share/flamewm"))
        );
        assert_eq!(
            resolve_packaged_root(Some(executable), Path::new("/workspace/flamewm"), |_| true),
            PathBuf::from("/opt/flamewm/usr/share/flamewm")
        );
    }

    #[test]
    fn unavailable_install_assets_fall_back_to_manifest_root() {
        let executable = Path::new("/opt/flamewm/usr/bin/flamewm-desktop");
        let manifest_root = Path::new("/workspace/flamewm");

        assert_eq!(
            resolve_packaged_root(Some(executable), manifest_root, |_| false),
            manifest_root.to_path_buf()
        );
    }

    #[test]
    fn development_executable_uses_manifest_root_without_cwd_lookup() {
        let executable = Path::new("/workspace/flamewm/target/debug/flamewm-desktop");
        let manifest_root = Path::new("/workspace/flamewm");

        assert_eq!(
            resolve_packaged_root(Some(executable), manifest_root, |_| true),
            manifest_root.to_path_buf()
        );
    }
