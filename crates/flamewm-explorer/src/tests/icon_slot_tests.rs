use super::*;

fn entry(name:&str,directory:bool)->Entry {
    Entry::new(PathBuf::from(name),name.into(),directory,false,0,None)
}

#[test]
fn every_row_family_is_a_compiled_row_icon() {
    // `view::file_icon` names are the template's `ico-<slot>-<kind>` ids: a name the template does
    // not compile would hide every image in the slot and leave it blank.
    for (name,directory) in [
        ("Photos",true),("roadmap.pdf",false),("main.rs",false),("photo.jpg",false),
        ("clip.svg",false),("app.ico",false),("app.desktop",false),("notes.txt",false),("LICENSE",false),
    ] {
        let family=view::file_icon(&entry(name,directory));
        assert!(ROW_ICONS.contains(&family),"{name} maps to unknown row icon {family}");
    }
}

#[test]
fn a_missing_or_failed_thumbnail_falls_back_to_the_type_glyph() {
    let photo=entry("photo.jpg",false);
    assert_eq!(row_icon_choice(&photo,None),RowIcon::Type("file"));
    let ready=(7,Arc::new(vec![0u8;THUMB_SIZE as usize*THUMB_SIZE as usize*4]));
    assert_eq!(row_icon_choice(&photo,Some(&ready)),RowIcon::Thumbnail(7));
    assert_eq!(row_icon_choice(&entry("app.desktop",false),None),RowIcon::Type("file"));
}

#[test]
fn every_sidebar_place_icon_is_a_compiled_place_icon() {
    let home=PathBuf::from("/tmp/flamewm-icon-slot-test-home");
    let devices=[bookmarks::BlockDevice{device:"/dev/sdb1".into(),label:"Stick".into(),removable:true}];
    for place in places(&home,&devices) {
        assert!(PLACE_ICONS.contains(&place.icon),"place {} maps to unknown icon {}",place.name,place.icon);
    }
    // The raster and the layout box are the same square, so no icon is scaled out of the cell.
    assert_eq!(ROW_ICON_BOX,THUMB_SIZE as f32);
}

#[test]
fn the_template_compiles_every_icon_slot_the_code_can_show() {
    // The document API can only toggle elements the template declares: a family or slot that is
    // missing here would be a silently blank icon on a packaged install.
    let html=include_str!("../../../../ui/explorer/explorer.html");
    for slot in 0..ROW_SLOTS {
        for kind in ROW_ICONS {
            let id=format!("id=\"ico-{slot}-{kind}\"");
            assert!(html.contains(&id),"row slot {slot} has no {kind} glyph ({id})");
        }
        let thumb=format!("id=\"ico-{slot}-thumb\"");
        assert!(html.contains(&thumb),"row slot {slot} has no thumbnail element ({thumb})");
    }
    for slot in 0..SIDEBAR_SLOTS {
        for kind in PLACE_ICONS {
            let id=format!("id=\"pico-{slot}-{kind}\"");
            assert!(html.contains(&id),"sidebar slot {slot} has no {kind} icon ({id})");
        }
    }
}

#[test]
fn every_referenced_icon_ships_with_the_explorer() {
    // The template loads `<icon>.svg` and `row-<kind>.svg` from the packaged icon set: a missing
    // file is an empty slot, even on a fully offline install with no icon theme present.
    let icons=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/explorer/icons");
    for kind in PLACE_ICONS {
        let path=icons.join(format!("{kind}.svg"));
        assert!(path.is_file(),"sidebar icon {} is not shipped",path.display());
    }
    for kind in ROW_ICONS {
        let path=icons.join(format!("row-{kind}.svg"));
        assert!(path.is_file(),"row icon {} is not shipped",path.display());
    }
}
