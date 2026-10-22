//! Real-server check, skipped unless `FLAMEWM_RFS_TEST_URL` is set (for example
//! `sftp://ubuntu@play.arkflame.com/`). Performs read-only operations only: home resolution and
//! one directory listing. Never writes, deletes, or prints credentials.

use flamewm_remote_fs::{Cancel, Config, ListEvent, RemoteFs, Url};

#[test]
fn real_host_home_and_listing_read_only() {
    let Ok(raw) = std::env::var("FLAMEWM_RFS_TEST_URL") else {
        eprintln!("skip: FLAMEWM_RFS_TEST_URL not set");
        return;
    };
    let url = Url::parse(&raw).expect("FLAMEWM_RFS_TEST_URL must be a valid URL");
    let rfs = RemoteFs::new(Config::default());
    let home = rfs.home(&url).wait().expect("home resolution over ssh");
    eprintln!("REAL home resolved (path length {})", home.path.len());
    let rx = rfs.list_dir(&home, &Cancel::new());
    let mut count = 0usize;
    loop {
        match rx.recv().expect("channel") {
            ListEvent::Chunk(c) => count += c.len(),
            ListEvent::Done { total } => {
                eprintln!("REAL listing: {total} entries (chunks reported {count})");
                assert_eq!(total, count);
                break;
            }
            ListEvent::Failed(e) => panic!("real listing failed: {e}"),
        }
    }
}
