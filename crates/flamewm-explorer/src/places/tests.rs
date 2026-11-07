use super::*;
use flamewm_integrations_linux::removable_mounts::{removable_devices, udisks_args};

#[test]
fn bookmark_round_trip_preserves_labels_and_rejects_non_file_uris() {
    let items = parse_bookmarks("file:///home/u/My%20Docs Work\nsftp://host/x\nfile:///tmp/a\n");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].path, PathBuf::from("/home/u/My Docs"));
    assert_eq!(items[0].display_name(), "Work");
    assert_eq!(items[1].display_name(), "a");
    let rendered = render_bookmarks(&items);
    assert_eq!(parse_bookmarks(&rendered), items);
}

#[test]
fn add_remove_rename_reorder_are_bounded_and_deduplicated() {
    let mut items = Vec::new();
    assert!(add(&mut items, Path::new("/a")));
    assert!(add(&mut items, Path::new("/b")));
    assert!(add(&mut items, Path::new("/c")));
    assert!(!add(&mut items, Path::new("/a")));
    assert!(reorder(&mut items, 2, 0));
    assert_eq!(
        items.iter().map(|b| b.path.clone()).collect::<Vec<_>>(),
        vec![
            PathBuf::from("/c"),
            PathBuf::from("/a"),
            PathBuf::from("/b")
        ]
    );
    assert!(rename(&mut items, 1, "  Alpha\u{7}  "));
    assert_eq!(items[1].label.as_deref(), Some("Alpha"));
    assert!(rename(&mut items, 1, "   "));
    assert_eq!(items[1].label, None);
    assert_eq!(remove(&mut items, 0).unwrap().path, PathBuf::from("/c"));
    assert!(remove(&mut items, 9).is_none());
    assert!(!reorder(&mut items, 9, 0));
}

#[test]
fn save_and_load_bookmarks_through_tempdir() {
    let dir = std::env::temp_dir().join(format!("flamewm-places-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let file = bookmarks_file(&dir);
    let items = vec![Bookmark {
        path: PathBuf::from("/srv/data"),
        label: Some("Data".into()),
    }];
    save_bookmarks(&file, &items).unwrap();
    assert_eq!(load_bookmarks(&file), items);
    assert!(!dir.join("gtk-3.0/.bookmarks.flamewm-tmp").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn network_filter_keeps_only_network_fstypes_on_sample_lines() {
    let text = "\
36 35 0:45 / /mnt/nas rw,relatime shared:12 - cifs //nas/media rw,vers=3.0
37 35 0:46 / /home/u/share\\040dir rw,relatime shared:13 - nfs4 host:/export rw,vers=4.2
38 35 0:47 / /home/u/sshfs rw,nosuid - fuse.sshfs user@host:/srv rw,user_id=1000
39 35 0:48 / /home/u/dav rw,nosuid shared:14 - fuse.davfs https://dav.example/ rw
40 35 0:49 / /run/user/1000/gvfs rw,nosuid,nodev,relatime shared:15 - fuse.gvfsd-fuse gvfsd-fuse rw,user_id=1000
41 35 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
42 35 0:50 / /run/media/u/USB rw shared:16 - vfat /dev/sdb1 rw
43 35 0:51 / /proc rw - proc proc rw
44 35 0:45 / /mnt/nas rw shared:12 - cifs //nas/media rw
garbage line with no separator
";
    let mounts = parse_network_mounts(text);
    let found: Vec<(&str, &str)> = mounts
        .iter()
        .map(|m| (m.mount_point.to_str().unwrap(), m.fstype.as_str()))
        .collect();
    assert_eq!(
        found,
        vec![
            ("/mnt/nas", "cifs"),
            ("/home/u/share dir", "nfs4"),
            ("/home/u/sshfs", "fuse.sshfs"),
            ("/home/u/dav", "fuse.davfs"),
            ("/run/user/1000/gvfs", "fuse.gvfsd-fuse"),
        ]
    );
}

#[test]
fn gvfs_folder_names_become_share_labels() {
    assert_eq!(
        gvfs_label("smb-share:server=nas,share=media"),
        "media on nas"
    );
    assert_eq!(gvfs_label("sftp:host=example.com,user=me"), "example.com");
    assert_eq!(gvfs_label("plain"), "plain");
}

#[test]
fn removable_devices_reports_mount_state_and_removable_flag() {
    let pairs = vec![
        ("AAAA-1111".to_owned(), "sdb1".to_owned()),
        ("BBBB".to_owned(), "nvme0n1p6".to_owned()),
    ];
    let mounted = vec![("sdb1".to_owned(), PathBuf::from("/run/media/u/Stick"))];
    let devices = removable_devices(&pairs, &mounted, |block| block == "sdb1");
    assert_eq!(
        devices[0],
        removable_mounts::Device {
            uuid: "AAAA-1111".into(),
            block: "sdb1".into(),
            removable: true,
            mounted: true
        }
    );
    assert_eq!(
        devices[1],
        removable_mounts::Device {
            uuid: "BBBB".into(),
            block: "nvme0n1p6".into(),
            removable: false,
            mounted: false
        }
    );
}

#[test]
fn uuid_blocks_is_empty_for_a_missing_directory() {
    assert!(uuid_blocks(Path::new("/nonexistent/flamewm/by-uuid")).is_empty());
}

#[test]
fn mountinfo_selects_media_mounts_and_unescapes_names() {
    let text = "36 25 8:17 / /run/media/u/My\\040Stick rw,nosuid shared:1 - vfat /dev/sdb1 rw\n\
                37 25 8:1 / / rw - ext4 /dev/sda1 rw\n\
                38 25 0:50 / /mnt/x rw - tmpfs tmpfs rw\n\
                39 25 8:33 / /media/u/Dup rw - vfat /dev/sdc1 rw\n\
                40 25 8:33 / /media/u/Dup rw - vfat /dev/sdc1 rw\n";
    let devices = parse_mountinfo(text);
    assert_eq!(devices.len(), 2);
    assert_eq!(
        devices[0].mount_point,
        PathBuf::from("/run/media/u/My Stick")
    );
    assert_eq!(devices[0].block_name, "sdb1");
    assert_eq!(devices[1].block_name, "sdc1");
}

#[test]
fn removable_flag_is_read_from_device_or_parent_disk() {
    let root = std::env::temp_dir().join(format!("flamewm-sysblock-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sdb/sdb1")).unwrap();
    // Mirror /sys/class/block: the entry is a symlink into the parent disk's directory.
    std::os::unix::fs::symlink(root.join("sdb/sdb1"), root.join("sdb1")).unwrap();
    fs::write(root.join("sdb/removable"), "1\n").unwrap();
    assert!(is_removable_in(&root, "sdb1"));
    fs::write(root.join("sdb/removable"), "0\n").unwrap();
    assert!(!is_removable_in(&root, "sdb1"));
    assert!(!is_removable_in(&root, "missing"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn udisks_commands_are_built_as_argument_vectors() {
    assert_eq!(
        udisks_args("mount", "/dev/sdb1").unwrap(),
        ["mount", "-b", "/dev/sdb1"]
    );
    assert_eq!(
        udisks_args("unmount", "/dev/sdb1").unwrap(),
        ["unmount", "-b", "/dev/sdb1"]
    );
    assert_eq!(
        udisks_args("eject", "/dev/sdb").unwrap(),
        ["power-off", "-b", "/dev/sdb"]
    );
    assert!(udisks_args("format", "/dev/sdb1").is_err());
    assert!(udisks_args("mount", "/dev/sdb1; rm -rf ~").is_err());
    assert!(udisks_args("mount", "sdb1").is_err());
}

#[test]
fn unlabelled_removable_filesystem_gets_dolphin_style_name() {
    let json = r#"{"blockdevices":[{"name":"sdb","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null,"children":[{"name":"sdb1","rm":true,"fstype":"vfat","mountpoint":null,"label":null}]}]}"#;
    let devices = parse_lsblk(json);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device, "/dev/sdb1");
    assert_eq!(devices[0].label, "Volume (sdb1)");
}

#[test]
fn lsblk_hides_loop_and_tiny_system_partitions() {
    let sample = r#"{"blockdevices":[
        {"name":"loop0","rm":true,"hotplug":true,"children":[{"name":"loop0p1","fstype":"squashfs","mountpoint":null,"label":null,"size":104857600}]},
        {"name":"ram0","fstype":"ext4","mountpoint":null,"label":"RAM","size":8000000000},
        {"name":"zram0","fstype":"ext4","mountpoint":null,"label":"Swap","size":8000000000,"rm":true},
        {"name":"sdq","rm":true,"hotplug":true,"children":[
            {"name":"sdq1","rm":true,"fstype":"vfat","mountpoint":null,"label":"EFI","size":104857600},
            {"name":"sdq2","rm":true,"fstype":"exfat","mountpoint":null,"label":"Big","size":8000000000}]}]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![BlockDevice {
            device: "/dev/sdq2".into(),
            label: "Big".into(),
            removable: true
        }]
    );
}

#[test]
fn lsblk_json_lists_unmounted_filesystems_on_internal_and_removable_disks() {
    let sample = r#"{"blockdevices": [
        {"name":"sda","rm":false,"hotplug":false,"fstype":null,"mountpoint":null,"label":null,
         "children":[
            {"name":"sda1","rm":false,"hotplug":false,"fstype":"ext4","mountpoint":"/","label":null},
            {"name":"sda2","fstype":"ntfs","mountpoint":null,"label":"Windows","size":500000000000},
            {"name":"sda3","fstype":"swap","mountpoint":null,"label":null,"size":8000000000},
            {"name":"sda4","fstype":"crypto_LUKS","mountpoint":null,"label":null,"size":8000000000},
            {"name":"sda5","fstype":"ext4","mountpoint":null,"label":"Tiny","size":1000}]},
        {"name":"sdb","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null,
         "children":[
            {"name":"sdb1","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":"/run/media/u/Backup","label":"Backup"},
            {"name":"sdb2","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":null,"label":"Stick \u00e9\n"},
            {"name":"sdb3","rm":true,"hotplug":true,"fstype":"swap","mountpoint":null,"label":null}]},
        {"name":"sdc","rm":"1","hotplug":"1","fstype":"exfat","mountpoint":null,"label":null},
        {"name":"sdd","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null},
        {"name":"sde","rm":true,"hotplug":true,"fstype":"vfat","mountpoints":[null],"label":"Camera","children":null}
    ]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![
            BlockDevice {
                device: "/dev/sda2".into(),
                label: "Windows".into(),
                removable: false
            },
            BlockDevice {
                device: "/dev/sdb2".into(),
                label: "Stick é".into(),
                removable: true
            },
            BlockDevice {
                device: "/dev/sdc".into(),
                label: "Volume (sdc)".into(),
                removable: true
            },
            BlockDevice {
                device: "/dev/sde".into(),
                label: "Camera".into(),
                removable: true
            },
        ]
    );
    assert!(parse_lsblk("not json").is_empty());
    assert!(
        parse_lsblk(r#"{"blockdevices": [{"name":"sdf","rm":true,"fstype":"vfat"}"#).is_empty()
    );
}

#[test]
fn drive_is_parent_disk_for_partitions_and_self_for_disks() {
    let root = std::env::temp_dir().join(format!("flamewm-drive-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sda/sda1")).unwrap();
    fs::write(root.join("sda/sda1/partition"), "1\n").unwrap();
    std::os::unix::fs::symlink(root.join("sda/sda1"), root.join("sda1")).unwrap();
    fs::create_dir_all(root.join("sdb")).unwrap();
    assert_eq!(drive_in(&root, "sda1"), "sda");
    assert_eq!(drive_in(&root, "sda"), "sda");
    assert_eq!(drive_in(&root, "sdb"), "sdb");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_unmounts_then_powers_off_the_drive() {
    let mut calls = Vec::new();
    let result = eject_with("/dev/sdb1", "/dev/sdb", |action, device| {
        calls.push(format!("{action} {device}"));
        Ok(String::new())
    });
    assert!(result.is_ok());
    assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
    assert_eq!(
        udisks_args("eject", "/dev/sdb").unwrap(),
        ["power-off", "-b", "/dev/sdb"]
    );
}

#[test]
fn eject_skips_unmount_when_already_unmounted_and_reports_busy_without_power_off() {
    let mut calls = Vec::new();
    let ok = eject_with("/dev/sdb1", "/dev/sdb", |action, _| {
        calls.push(action.to_owned());
        if action == "unmount" {
            Err("Error unmounting /dev/sdb1: Not mounted".into())
        } else {
            Ok(String::new())
        }
    });
    assert!(ok.is_ok());
    assert_eq!(calls, ["unmount", "eject"]);

    let mut calls = Vec::new();
    let busy = eject_with("/dev/sdb1", "/dev/sdb", |action, _| {
        calls.push(action.to_owned());
        Err("Error unmounting /dev/sdb1: Device or resource busy".into())
    });
    assert_eq!(
        busy.unwrap_err(),
        "the device is busy: close files or programs using it, then try again"
    );
    assert_eq!(calls, ["unmount"]);
}

#[test]
fn eject_device_refuses_invalid_paths_before_running_anything() {
    assert_eq!(
        eject_device("/dev/sdb1; rm -rf ~").unwrap_err(),
        "Refusing invalid device path"
    );
}

#[test]
fn lsblk_query_asks_for_partition_identity_columns() {
    for column in [
        "NAME",
        "FSTYPE",
        "MOUNTPOINT",
        "LABEL",
        "SIZE",
        "TYPE",
        "TRAN",
        "RM",
        "PARTTYPE",
        "PARTFLAGS",
    ] {
        assert!(
            LSBLK_COLUMNS.split(',').any(|name| name == column),
            "missing {column}"
        );
    }
    assert_eq!(lsblk_args(LSBLK_COLUMNS), ["-J", "-b", "-o", LSBLK_COLUMNS]);
    // The fallback stays queryable by lsblk versions without PARTTYPE.
    assert!(!LSBLK_COLUMNS_LEGACY.contains("PARTTYPE"));
}

#[test]
fn firmware_recovery_and_container_partitions_are_rejected_by_partition_type() {
    // Every one of these is a system partition whose *label* is empty or innocent: only the
    // GPT type GUID or the MBR type byte identifies it.
    let sample = r#"{"blockdevices":[
        {"name":"nvme0n1","type":"disk","children":[
            {"name":"nvme0n1p1","fstype":"vfat","label":"","size":536870912,"type":"part","parttype":"C12A7328-F81F-11D2-BA4B-00A0C93EC93B"},
            {"name":"nvme0n1p2","fstype":"vfat","label":"","size":134217728,"type":"part","parttype":"E3C9E316-0B5C-4DB8-817D-F92DF00215AE"},
            {"name":"nvme0n1p3","fstype":"ntfs","label":"","size":1073741824,"type":"part","parttype":"DE94BBA4-06D1-4D40-A16A-BFD50179D6AC"},
            {"name":"nvme0n1p4","fstype":"","label":"","size":2097152,"type":"part","parttype":"21686148-6449-6E6F-744E-656564454649"},
            {"name":"nvme0n1p5","fstype":"crypto_LUKS","label":"","size":8589934592,"type":"part","parttype":"CA7D7CCB-63ED-4C53-861C-1742536059CC"},
            {"name":"nvme0n1p6","fstype":"ext4","label":"Data","size":17179869184,"type":"part","parttype":"0FC63DAF-8483-4772-8E79-3D69D8477DE4"}]},
        {"name":"sdc","type":"disk","rm":true,"children":[
            {"name":"sdc1","fstype":"vfat","label":"","size":268435456,"type":"part","parttype":"0xef"},
            {"name":"sdc2","fstype":"ext4","label":"","size":268435456,"type":"part","parttype":"0xfd"},
            {"name":"sdc3","fstype":"exfat","label":"Camera","size":67108864000,"type":"part","parttype":"0x7"}]}]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![
            BlockDevice {
                device: "/dev/nvme0n1p6".into(),
                label: "Data".into(),
                removable: false
            },
            BlockDevice {
                device: "/dev/sdc3".into(),
                label: "Camera".into(),
                removable: true
            },
        ]
    );
}

#[test]
fn boot_attribute_flags_and_reserved_labels_are_rejected() {
    // PARTFLAGS bit 2 marks a GPT legacy-BIOS-bootable partition; 0x80 is the MBR active flag.
    let sample = r#"{"blockdevices":[
        {"name":"sda","type":"disk","children":[
            {"name":"sda1","fstype":"ext4","label":"","size":1073741824,"type":"part","partflags":"0x4"},
            {"name":"sda2","fstype":"ntfs","label":"","size":1073741824,"type":"part","partflags":"0x80"},
            {"name":"sda3","fstype":"vfat","label":"BIOS boot","size":1073741824,"type":"part"},
            {"name":"sda4","fstype":"ntfs","label":"System Reserved","size":1073741824,"type":"part"},
            {"name":"sda5","fstype":"ntfs","label":"Media","size":1073741824,"type":"part","partflags":"0x8000000000000000"},
            {"name":"sda6","fstype":"ext4","label":"Work","size":1073741824,"type":"part","parttype":"0x83"}]}]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![
            BlockDevice {
                device: "/dev/sda5".into(),
                label: "Media".into(),
                removable: false
            },
            BlockDevice {
                device: "/dev/sda6".into(),
                label: "Work".into(),
                removable: false
            },
        ]
    );
    assert_eq!(
        part_flags_bits("0x8000000000000000"),
        Some(0x8000_0000_0000_0000)
    );
    assert_eq!(part_flags_bits("garbage"), None);
}

#[test]
fn device_mapper_raid_and_container_nodes_are_never_listed() {
    let sample = r#"{"blockdevices":[
        {"name":"dm-0","type":"lvm","fstype":"ext4","label":"Vol","size":1073741824,"mountpoint":null},
        {"name":"md0","type":"raid1","fstype":"ext4","label":"Array","size":1073741824},
        {"name":"mapper/vg-root","type":"lvm","fstype":"ext4","label":"Root","size":1073741824},
        {"name":"sdb","type":"disk","children":[
            {"name":"sdb1","type":"part","fstype":"LVM2_member","label":"","size":1073741824},
            {"name":"sdb2","type":"part","fstype":"swap","label":"","size":1073741824},
            {"name":"sdb3","type":"part","fstype":"ext4","label":"Data","size":1073741824}]}]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![BlockDevice {
            device: "/dev/sdb3".into(),
            label: "Data".into(),
            removable: false
        }]
    );
}

#[test]
fn a_disk_holding_the_boot_chain_is_not_offered_as_media() {
    // /boot/efi is mounted on this disk, so its unmounted Windows partition is the same
    // installation rather than a removable volume to offer.
    let sample = r#"{"blockdevices":[
        {"name":"sda","type":"disk","children":[
            {"name":"sda1","type":"part","fstype":"vfat","label":"","size":536870912,"mountpoint":"/boot/efi","parttype":"C12A7328-F81F-11D2-BA4B-00A0C93EC93B"},
            {"name":"sda2","type":"part","fstype":"ntfs","label":"Windows","size":500000000000}]},
        {"name":"sdb","type":"disk","rm":true,"children":[
            {"name":"sdb1","type":"part","fstype":"exfat","label":"Stick","size":67108864000}]}]}"#;
    assert_eq!(
        parse_lsblk(sample),
        vec![BlockDevice {
            device: "/dev/sdb1".into(),
            label: "Stick".into(),
            removable: true
        }]
    );
    assert!(is_boot_mount("/boot"));
    assert!(is_boot_mount("/boot/efi"));
    assert!(is_boot_mount("/boot/grub2"));
    assert!(!is_boot_mount("/bootleg"));
    assert!(!is_boot_mount("/home"));
}

#[test]
fn mountinfo_lists_every_mounted_block_device() {
    let text = "36 25 8:17 / /run/media/u/My\\040Stick rw,nosuid shared:1 - vfat /dev/sdb1 rw\n\
                37 25 8:1 / / rw - ext4 /dev/sda1 rw\n\
                38 25 0:50 / /mnt/nas rw - cifs //nas/media rw\n\
                39 25 8:33 / /home/u/w rw - ext4 /dev/sdc2 rw\n\
                40 25 8:33 / /home/u/w rw - ext4 /dev/sdc2 rw\n";
    assert_eq!(
        mounted_block_devices(text),
        vec![
            ("sdb1".to_owned(), PathBuf::from("/run/media/u/My Stick")),
            ("sda1".to_owned(), PathBuf::from("/")),
            ("sdc2".to_owned(), PathBuf::from("/home/u/w")),
        ]
    );
}

/// Fake `/sys/class/block` tree: `sdb` with partitions `sdb1`/`sdb2`, entry symlinks like the real one.
fn fake_sysblock(tag: &str, removable: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("flamewm-eject-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sdb/sdb1")).unwrap();
    fs::create_dir_all(root.join("sdb/sdb2")).unwrap();
    fs::create_dir_all(root.join("nvme0n1/nvme0n1p1")).unwrap();
    fs::write(root.join("sdb/sdb1/partition"), "1\n").unwrap();
    fs::write(root.join("sdb/sdb2/partition"), "1\n").unwrap();
    fs::write(root.join("nvme0n1/nvme0n1p1/partition"), "1\n").unwrap();
    fs::write(root.join("sdb/removable"), format!("{removable}\n")).unwrap();
    fs::write(root.join("nvme0n1/removable"), "0\n").unwrap();
    std::os::unix::fs::symlink(root.join("sdb/sdb1"), root.join("sdb1")).unwrap();
    std::os::unix::fs::symlink(root.join("sdb/sdb2"), root.join("sdb2")).unwrap();
    std::os::unix::fs::symlink(root.join("nvme0n1/nvme0n1p1"), root.join("nvme0n1p1")).unwrap();
    root
}

#[test]
fn eject_runs_the_full_fake_udisks_sequence_for_removable_media() {
    let root = fake_sysblock("sequence", "1");
    let mut calls = Vec::new();
    // Only the target filesystem is mounted, so the drive can be powered off after unmounting.
    let mountinfo = "36 25 8:17 / /run/media/u/Stick rw - exfat /dev/sdb1 rw\n";
    let outcome = eject_device_in(&root, mountinfo, "/dev/sdb1", false, |action, device| {
        calls.push(format!("{action} {device}"));
        Ok(String::new())
    });
    assert_eq!(outcome, Ok(String::new()));
    assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_tolerates_an_already_unmounted_filesystem() {
    let root = fake_sysblock("unmounted", "1");
    let mut calls = Vec::new();
    let outcome = eject_device_in(&root, "", "/dev/sdb1", false, |action, device| {
        calls.push(format!("{action} {device}"));
        if action == "unmount" {
            Err("Error unmounting /dev/sdb1: Not mounted".into())
        } else {
            Ok(String::new())
        }
    });
    assert!(outcome.is_ok());
    assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_reports_a_busy_filesystem_and_never_powers_the_drive_off() {
    let root = fake_sysblock("busy", "1");
    let mut calls = Vec::new();
    let outcome = eject_device_in(&root, "", "/dev/sdb1", false, |action, device| {
        calls.push(format!("{action} {device}"));
        Err("Error unmounting /dev/sdb1: Device or resource busy".into())
    });
    assert_eq!(
        outcome.unwrap_err(),
        "the device is busy: close files or programs using it, then try again"
    );
    assert_eq!(calls, ["unmount /dev/sdb1"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_refuses_a_drive_that_still_holds_another_mounted_partition() {
    let root = fake_sysblock("sibling", "1");
    let mut calls: Vec<String> = Vec::new();
    let mountinfo = "37 25 8:18 / /run/media/u/Other rw - vfat /dev/sdb2 rw\n";
    let outcome = eject_device_in(&root, mountinfo, "/dev/sdb1", true, |action, device| {
        calls.push(format!("{action} {device}"));
        Ok(String::new())
    });
    let message = outcome.unwrap_err();
    assert!(message.contains("/dev/sdb2"), "{message}");
    assert!(message.contains("/run/media/u/Other"), "{message}");
    assert!(
        calls.is_empty(),
        "nothing must be unmounted or powered off: {calls:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_refuses_an_internal_drive_unless_the_user_confirmed_it() {
    let root = fake_sysblock("internal", "0");
    let mut calls: Vec<String> = Vec::new();
    let outcome = eject_device_in(&root, "", "/dev/nvme0n1p1", false, |action, device| {
        calls.push(format!("{action} {device}"));
        Ok(String::new())
    });
    assert!(outcome.unwrap_err().contains("confirm"));
    assert!(calls.is_empty());
    // The sidebar/menu click is the explicit confirmation, so the same device may be ejected then.
    let confirmed = eject_device_in(&root, "", "/dev/nvme0n1p1", true, |action, device| {
        calls.push(format!("{action} {device}"));
        Ok(String::new())
    });
    assert!(confirmed.is_ok());
    assert_eq!(calls, ["unmount /dev/nvme0n1p1", "eject /dev/nvme0n1"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn eject_refuses_device_mapper_volumes_and_bogus_paths() {
    let root = fake_sysblock("mapper", "1");
    let mut calls = 0;
    for device in [
        "/dev/dm-0",
        "/dev/md0",
        "/dev/mapper/vg-root",
        "/dev/../etc/passwd",
        "sdb1",
    ] {
        let outcome = eject_device_in(&root, "", device, true, |_, _| {
            calls += 1;
            Ok(String::new())
        });
        assert!(outcome.is_err(), "{device} must be refused");
    }
    assert_eq!(calls, 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn device_paths_are_validated_before_spawning() {
    assert!(valid_device_path("/dev/sdb1"));
    assert!(!valid_device_path("/dev/sdb1; rm -rf ~"));
    assert!(!valid_device_path("/dev/../etc"));
    assert!(!valid_device_path("sdb1"));
    assert!(udisks_action("format", "/dev/sdb1").is_err());
    assert!(udisks_action("mount", "/dev/../x").is_err());
}
