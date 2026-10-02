//! Unit tests for the path jail.

use std::path::PathBuf;

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "netvideo-jail-{name}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn resolves_a_file_under_the_root() {
    let root = temp_dir("resolve");
    std::fs::create_dir_all(root.join("album")).unwrap();
    std::fs::write(root.join("album/song.flac"), b"data").unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));

    let resolved = roots.resolve(0, "album/song.flac").expect("resolve");
    assert!(resolved.is_file());
    assert_eq!(resolved.file_name().unwrap(), "song.flac");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_parent_directory_navigation() {
    let root = temp_dir("parent");
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    let error = roots.resolve(0, "../secret.flac").expect_err("reject");
    assert!(matches!(error, ServerError::PathRejected(_)));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_nested_parent_navigation() {
    let root = temp_dir("nested-parent");
    std::fs::create_dir_all(root.join("a/b")).unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(0, "a/b/../../escape.flac").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_absolute_paths() {
    let root = temp_dir("absolute");
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(0, "/etc/passwd").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_nul_bytes() {
    let root = temp_dir("nul");
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(0, "bad\0name.flac").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_unknown_and_negative_root_index() {
    let root = temp_dir("index");
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(5, "a.flac").is_err());
    assert!(roots.resolve(-1, "a.flac").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_missing_files() {
    let root = temp_dir("missing");
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(0, "nope.flac").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rejects_directories() {
    let root = temp_dir("dir");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    assert!(roots.resolve(0, "sub").is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escaping_the_root() {
    let root = temp_dir("symlink-root");
    let outside = temp_dir("symlink-outside");
    std::fs::write(outside.join("secret.flac"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.flac"), root.join("link.flac")).unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));

    let error = roots.resolve(0, "link.flac").expect_err("must reject");
    assert!(matches!(error, ServerError::PathEscape { .. }));
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn open_returns_a_handle_to_the_file_under_the_root() {
    use std::io::Read;
    let root = temp_dir("open");
    std::fs::write(root.join("movie.mkv"), b"bytes").unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    let mut contents = String::new();
    roots
        .open(0, "movie.mkv")
        .expect("open")
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "bytes");
}

#[cfg(unix)]
#[test]
fn open_refuses_a_symlink_out_of_the_root() {
    let root = temp_dir("open-escape");
    let outside = temp_dir("open-outside");
    std::fs::write(outside.join("secret.mkv"), b"secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("dir")).unwrap();
    let roots = LibraryRoots::new(std::slice::from_ref(&root));
    let error = roots.open(0, "dir/secret.mkv").unwrap_err();
    assert!(matches!(error, ServerError::PathEscape { .. }));
}

#[cfg(target_os = "linux")]
#[test]
fn opened_path_reports_the_real_location() {
    let root = temp_dir("opened-path");
    std::fs::write(root.join("a.mkv"), b"a").unwrap();
    let canonical = std::fs::canonicalize(root.join("a.mkv")).unwrap();
    let file = std::fs::File::open(&canonical).unwrap();
    // A stale `resolved` argument is ignored: the kernel's view wins.
    let opened = opened_path(&file, Path::new("/elsewhere")).unwrap();
    assert_eq!(opened, canonical);
}
