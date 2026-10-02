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
