    use std::time::UNIX_EPOCH;
    use super::*;
    #[test] fn row_slot_index_matches_paint_mapping() {
        // Slot 1 is the first window row; slot 0 is the glide buffer above it.
        assert_eq!(row_slot_index(0, 1), 0);
        assert_eq!(row_slot_index(0, 0), -1);
        assert_eq!(row_slot_index(5, 3), 7);
    }
    #[test] fn lsblk_unmounted_partition_maps_to_unmounted_device_place() {
        let sample = r#"{"blockdevices":[{"name":"sdb","rm":true,"hotplug":true,"children":[
            {"name":"sdb1","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":null,"label":"Backup"}]}]}"#;
        let devices = places::parse_lsblk(sample);
        let rows: Vec<Place> = devices.iter().map(unmounted_place).collect();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].mounted());
        assert_eq!(rows[0].name, "Backup");
        assert_eq!(rows[0].device.as_deref(), Some("/dev/sdb1"));
        assert_eq!(rows[0].group, PlaceGroup::Devices);
        // Removable media keeps the drive's removable flag and the USB glyph.
        assert!(rows[0].removable);
        assert_eq!(rows[0].icon, "usb");
        // An unmounted internal partition is still offered, but not as removable media.
        let internal = places::parse_lsblk(
            r#"{"blockdevices":[{"name":"nvme0n1","rm":false,"children":[
                {"name":"nvme0n1p6","fstype":"ext4","mountpoint":null,"label":"Data","size":17179869184}]}]}"#,
        );
        let internal: Vec<Place> = internal.iter().map(unmounted_place).collect();
        assert_eq!(internal.len(), 1);
        assert_eq!(internal[0].name, "Data");
        assert!(!internal[0].removable);
        assert_eq!(internal[0].icon, "drive");
    }
    #[test] fn flamewm_explorer_desktop_entry_is_listed_in_system_menu_bucket() {
        let dir = std::env::temp_dir().join(format!("flamewm-explorer-xdg-{}", std::process::id()));
        let apps = dir.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let exe = std::env::current_exe().unwrap();
        let text = include_str!("../../../../packaging/flamewm-explorer.desktop")
            .replace("Exec=flamewm-explorer", &format!("Exec={}", exe.display()))
            .replace("TryExec=flamewm-explorer", &format!("TryExec={}", exe.display()));
        std::fs::write(apps.join("flamewm-explorer.desktop"), text).unwrap();
        let catalog = flamewm_applications::ApplicationCatalog::from_dirs(&[apps]).unwrap();
        let app = catalog.all().into_iter().find(|a| a.name == "FlameWM Explorer");
        std::fs::remove_dir_all(&dir).unwrap();
        let app = app.expect("explorer entry must survive NoDisplay/TryExec filtering");
        assert_eq!(app.categories.first().map(String::as_str), Some("System"));
    }
    #[test] fn home_alias_and_remote_protocols_do_not_confuse_native_paths() {
        let home=Path::new("/home/demo");
        assert_eq!(display_location(home,home),"~");
        assert_eq!(display_location(&home.join("Documents"),home),"~/Documents");
        assert_eq!(display_location(Path::new("/etc"),home),"/etc");
        let remote_home=remote::to_path(&flamewm_remote_fs::Url::parse("sftp://ubuntu@play.arkflame.com/home/ubuntu").unwrap());
        assert_eq!(display_location(&remote_home,home),"sftp://ubuntu@play.arkflame.com/home/ubuntu");
        let custom=remote::to_path(&flamewm_remote_fs::Url::parse("ssh://u@h:2222/srv").unwrap());
        assert_eq!(display_location(&custom,home),"ssh://u@h:2222/srv");
        assert!(matches!(parse_address("~",home),Ok(AddressTarget::Local(p)) if p==home));
        assert!(matches!(parse_address("sftp://host/home/demo",home),Ok(AddressTarget::Remote(_))));
        assert!(matches!(parse_address("fish://host/~/x",home),Ok(AddressTarget::Remote(_))));
        assert!(parse_address("sftp://user:secret@host/",home).is_err());
        assert!(matches!(parse_address("smb://server/share",home),Ok(AddressTarget::ExternalUri(_))));
        assert!(parse_address("javascript://evil",home).is_err());
        assert!(parse_address("sftp://",home).is_err());
    }
    #[test] fn archive_names_and_marquee_selection_respect_filtered_rows() {
        assert!(is_archive(Path::new("work.TAR.GZ")));
        assert!(is_archive(Path::new("photo.zip")));
        assert!(!is_archive(Path::new("photo.png")));
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..6).map(|i|Entry::new(PathBuf::from(format!("/tmp/file{i}")),format!("file{i}"),false,false,0,None)).collect();
        tab.select_vertical_band(1,3,ROW_SLOTS);
        assert_eq!(tab.selected_paths.len(),3);
        assert!(tab.selected_paths.contains(Path::new("/tmp/file2")));
        tab.select_one(PathBuf::from("/tmp/file4"));
        assert_eq!(tab.selected_paths.len(),1);
        tab.visit(PathBuf::from("/tmp/other"));
        assert!(tab.selected_paths.is_empty());
    }
    #[test] fn history_and_tabs_do_not_lose_locations() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));model.tab_mut().visit(PathBuf::from("/"));
        assert!(model.tab_mut().back());assert_eq!(model.tab().path(),Path::new("/tmp"));
        assert!(model.tab_mut().forward());model.new_tab();model.tab_mut().visit(PathBuf::from("/tmp"));model.switch_tab(0);assert_eq!(model.tab().path(),Path::new("/"));
    }
    #[test]
    fn mouse_selection_ctrl_toggle_shift_anchor_and_ctrl_shift_union() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..6).map(|i| Entry::new(PathBuf::from(format!("/tmp/item{i}")),format!("item{i}"),false,false,0,None)).collect();
        let item=|index| PathBuf::from(format!("/tmp/item{index}"));
        tab.select_one(item(1));
        tab.toggle_selection(item(5));
        assert_eq!(tab.selected_paths.len(),2);
        tab.toggle_selection(item(5));
        assert_eq!(tab.selected_paths.len(),1);
        tab.select_one(item(1));
        tab.select_range_to(item(3));
        assert_eq!(tab.selected_paths.len(),3);
        assert!(tab.selected_paths.contains(&item(1)));
        assert!(tab.selected_paths.contains(&item(3)));
        tab.select_one(item(0));
        tab.toggle_selection(item(5));
        tab.select_range_to_with_mode(item(3),true);
        assert_eq!(tab.selected_paths.len(),4); // original 0 plus range 3..5
        assert_eq!(tab.ordered_selection(),vec![item(0),item(3),item(4),item(5)]);
    }

    #[test] fn scroll_window_follows_viewport_and_keeps_indices() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.loading=false;
        tab.entries=(0..100).map(|n| Entry::new(PathBuf::from(format!("/tmp/item-{n:03}")),format!("item-{n:03}"),false,false,n,None)).collect();
        let rows=12;
        tab.scroll_by(20,rows);
        assert_eq!(tab.scroll,20);
        let window=tab.visible_window(rows);
        assert_eq!(window.len(),rows);
        assert_eq!(window[0].name,"item-020");
        assert_eq!(tab.visible_index_of(&window[0].path),Some(20));
        // Keyboard reveal scrolls only as far as needed: index 31 is one past the window.
        tab.reveal(31,rows);
        assert_eq!(tab.scroll,20);
        tab.reveal(32,rows);
        assert_eq!(tab.scroll,21);
        tab.reveal(0,rows);
        assert_eq!(tab.scroll,0);
        // Scrolling clamps at the last full window.
        tab.scroll_by(1000,rows);
        assert_eq!(tab.scroll,88);
        assert_eq!(tab.visible_window(rows)[rows-1].name,"item-099");
        tab.scroll_by(-1000,rows);
        assert_eq!(tab.scroll,0);
        // A filter that shrinks the list pulls a stale offset back into range.
        tab.scroll=50;
        tab.search="item-09".into();
        assert_eq!(tab.filtered_count(),10);
        assert_eq!(tab.visible_window(rows).len(),10);
        assert_eq!(tab.visible_window(rows)[0].name,"item-090");
    }
    #[test] fn list_starts_at_top_and_resize_reclamps_without_blank_space() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.loading=false;
        let item=|n:usize| Entry::new(PathBuf::from(format!("/tmp/f{n:02}")),format!("f{n:02}"),false,false,0,None);
        tab.entries=(0..19).map(item).collect();
        // A fresh tab starts at the top, and so does a window that fits every row.
        assert_eq!(tab.scroll,0);
        assert_eq!(tab.window_start(13),0);
        assert_eq!(tab.visible_window(30).len(),19);
        assert_eq!(tab.visible_window(30)[0].name,"f00");
        // Scroll to the bottom of a 13-row window, then grow and shrink the window.
        tab.scroll_by(1000,13);
        assert_eq!(tab.window_start(13),6);
        // Growing past the list re-clamps to the top: no blank rows below the list.
        assert_eq!(tab.window_start(30),0);
        assert_eq!(tab.visible_window(30)[0].name,"f00");
        // Shrinking keeps the bottom row visible without leaving gaps.
        tab.scroll_by(1000,5);
        assert_eq!(tab.scroll,14);
        assert_eq!(tab.visible_window(5)[0].name,"f14");
        assert_eq!(tab.visible_window(5).len(),5);
        // A stale large offset that is larger than the list re-clamps when a window is shown again.
        tab.scroll=500;
        assert_eq!(tab.window_start(13),6);
        assert_eq!(tab.visible_window(13).len(),13);
        assert_eq!(tab.visible_window(13)[0].name,"f06");
    }
    #[test] fn stale_worker_reply_is_discarded() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));let old=model.generation;model.next_generation();
        assert!(!model.apply_listing(old,Path::new("/tmp"),Ok(Vec::new())));
    }
    #[test] fn move_refuses_existing_destination_and_preserves_both_files() {
        let base = std::env::temp_dir().join(format!("flamewm-explorer-noreplace-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("source"), b"source").unwrap();
        fs::write(base.join("existing"), b"existing").unwrap();
        assert!(move_entry(&base.join("source"), &base.join("existing"), &mut |_| {}).is_err());
        assert_eq!(fs::read(base.join("source")).unwrap(), b"source");
        assert_eq!(fs::read(base.join("existing")).unwrap(), b"existing");
        fs::remove_dir_all(base).unwrap();
    }
    #[test] fn keyboard_range_toggle_and_select_all_follow_filtered_order() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..8).map(|i|Entry::new(PathBuf::from(format!("/tmp/file{i}")),format!("file{i}"),false,false,0,None)).collect();
        tab.select_one(PathBuf::from("/tmp/file1"));
        tab.select_range_to(PathBuf::from("/tmp/file4"));
        assert_eq!(tab.selected_paths.len(),4);
        assert!(tab.selected_paths.contains(Path::new("/tmp/file2")));
        tab.toggle_selection(PathBuf::from("/tmp/file2"));
        assert_eq!(tab.selected_paths.len(),3);
        tab.search="file7".into();tab.select_all_filtered();
        assert_eq!(tab.ordered_selection(),vec![PathBuf::from("/tmp/file7")]);
        tab.clear_selection();assert!(tab.ordered_selection().is_empty());
    }
    #[test] fn batch_copy_continues_after_conflict_without_overwriting() {
        let base=std::env::temp_dir().join(format!("flamewm-explorer-batch-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source=base.join("source");let dest=base.join("dest");
        fs::create_dir_all(&source).unwrap();fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a"),"alpha").unwrap();fs::write(source.join("b"),"beta").unwrap();
        fs::write(dest.join("a"),"existing").unwrap();
        let report=transfer_batch(&[source.join("a"),source.join("b")],&dest,false,&mut |_|{});
        assert_eq!((report.completed,report.failed),(2,0));
        assert_eq!(fs::read(dest.join("a")).unwrap(),b"existing");
        assert_eq!(fs::read(dest.join("a (2)")).unwrap(),b"alpha");
        assert_eq!(fs::read(dest.join("b")).unwrap(),b"beta");
        let failing=transfer_batch(&[source.join("missing"),source.join("a")],&dest,true,&mut |_|{});
        assert_eq!((failing.completed,failing.failed),(1,1));
        assert!(failing.first_error.unwrap().contains("missing"));
        assert_eq!(failing.failed_sources,vec![source.join("missing")]);
        assert_eq!(fs::read(dest.join("a (3)")).unwrap(),b"alpha");
        assert!(!source.join("a").exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test] fn rescan_reconciles_selection_of_deleted_files() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));
        model.tab_mut().select_one(PathBuf::from("/tmp/gone"));
        let generation=model.generation;
        assert!(model.apply_listing(generation,Path::new("/tmp"),Ok(vec![])));
        assert!(model.tab().selected_paths.is_empty());
        assert!(model.tab().selected.is_none());
    }
    #[test] fn drop_noop_rules_cover_self_descendant_and_same_folder() {
        let folder=PathBuf::from("/home/u/projects/app");
        let file=PathBuf::from("/home/u/docs/a.txt");
        // Dropping onto a dragged item, or a folder into itself or its own descendant, is a no-op.
        assert!(drop_is_noop(std::slice::from_ref(&folder),&folder));
        assert!(drop_is_noop(std::slice::from_ref(&folder),&folder.join("src")));
        assert!(drop_is_noop(&[file.clone(),folder.clone()],&folder));
        // Dropping into the folder the items already live in is a no-op (no copy, no "name (1)").
        assert!(drop_is_noop(std::slice::from_ref(&file),Path::new("/home/u/docs")));
        assert!(drop_is_noop(&[file.clone(),PathBuf::from("/home/u/docs/b.txt")],Path::new("/home/u/docs")));
        // A sibling folder, a different folder, or a prefix-named sibling is a real drop.
        assert!(!drop_is_noop(std::slice::from_ref(&file),&folder));
        assert!(!drop_is_noop(std::slice::from_ref(&folder),Path::new("/home/u/projects/app-old")));
        assert!(!drop_is_noop(std::slice::from_ref(&file),Path::new("/home/u/docs/sub")));
        assert!(!drop_is_noop(&[],Path::new("/home/u/docs")));
    }
    #[test] fn drop_policy_moves_on_same_device_copies_cross_device_and_respects_ctrl() {
        assert!(drop_moves(false,100,100));
        assert!(!drop_moves(false,100,200));
        assert!(!drop_moves(true,100,100));
        let base=std::env::temp_dir().join(format!("flamewm-drag-policy-{}-{}",
            std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source_dir=base.join("source");let dest_dir=base.join("dest");
        fs::create_dir_all(&source_dir).unwrap();fs::create_dir_all(&dest_dir).unwrap();
        let original=source_dir.join("text.txt");fs::write(&original,b"preserve").unwrap();
        let copied=transfer_drop_batch(std::slice::from_ref(&original),&dest_dir,true,&mut |_| {});
        assert_eq!((copied.completed,copied.failed),(1,0));
        assert!(original.exists());assert_eq!(fs::read(dest_dir.join("text.txt")).unwrap(),b"preserve");
        let moved=transfer_drop_batch(std::slice::from_ref(&original),&dest_dir,false,&mut |_| {});
        assert_eq!((moved.completed,moved.failed),(1,0));
        assert!(!original.exists());assert_eq!(fs::read(dest_dir.join("text.txt (2)")).unwrap(),b"preserve");
        fs::remove_dir_all(base).unwrap();
    }

    #[test] fn transfer_rejects_symlink_alias_to_child_before_writing() {
        let base=std::env::temp_dir().join(format!("flamewm-drag-alias-{}-{}",
            std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source=base.join("source");
        fs::create_dir_all(source.join("child")).unwrap();
        fs::write(source.join("item"), b"original").unwrap();
        symlink(&source.join("child"),base.join("alias")).unwrap();
        let outcome=transfer_batch(&[source.clone()],&base.join("alias"),true,&mut |_| {});
        assert_eq!((outcome.completed,outcome.failed),(0,1));
        assert!(source.join("item").exists());
        assert!(fs::read_dir(source.join("child")).unwrap().next().is_none());
        fs::remove_dir_all(base).unwrap();
    }

    #[test] fn safe_copy_prevents_overwriting_and_recursing_into_itself() {
        let base=std::env::temp_dir().join(format!("flamewm-explorer-test-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(base.join("a")).unwrap();fs::write(base.join("a/item"),"content").unwrap();
        assert!(copy_entry(&base.join("a"),&base.join("a/inside"),&mut |_| {}).is_err());
        copy_entry(&base.join("a/item"),&base.join("copy"),&mut |_|{}).unwrap();
        assert!(copy_entry(&base.join("a/item"),&base.join("copy"),&mut |_|{}).is_err());
        assert_eq!(fs::read(base.join("copy")).unwrap(),b"content");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sidebar_rows_group_places_and_hide_empty_headings() {
        let place = |name: &str, group: PlaceGroup| Place { name: name.into(), path: PathBuf::from("/x"), removable: false, group, icon: "folder", device: None };
        let places = vec![
            place("Home", PlaceGroup::Places),
            place("Docs", PlaceGroup::Places),
            place("Local Disk", PlaceGroup::Devices),
            place("Work", PlaceGroup::Pinned),
        ];
        assert_eq!(
            sidebar_rows(&places),
            vec![
                SidebarRow::Heading("Places"),
                SidebarRow::Place(0),
                SidebarRow::Place(1),
                SidebarRow::Heading("Devices"),
                SidebarRow::Place(2),
                SidebarRow::Heading("Pinned Folders"),
                SidebarRow::Place(3),
            ]
        );
        let without_pins: Vec<Place> = places.iter().filter(|p| p.group != PlaceGroup::Pinned).cloned().collect();
        assert!(!sidebar_rows(&without_pins).contains(&SidebarRow::Heading("Pinned Folders")));
        assert!(sidebar_rows(&[]).is_empty());
    }
