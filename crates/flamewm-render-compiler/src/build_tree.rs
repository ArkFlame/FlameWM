use std::fs;
use std::path::Path;

pub fn stage_tree(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create staged asset dir");
    for entry in fs::read_dir(source).expect("read staged asset dir") {
        let entry = entry.expect("read staged asset entry");
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, dest.join(entry.file_name())).expect("stage asset file");
        }
    }
}

pub fn rerun_tree(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files = fs::read_dir(dir)
        .expect("read tracked asset dir")
        .map(|entry| entry.expect("read tracked asset entry").path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
}
