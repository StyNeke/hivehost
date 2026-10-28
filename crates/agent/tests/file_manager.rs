#![cfg(target_os = "linux")]

use manager_agent::file_manager::{EntryKind, Volume, validate};
use std::{fs, os::unix::fs::symlink};
use uuid::Uuid;

fn volume() -> (tempfile::TempDir, std::path::PathBuf, Volume) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("volume");
    fs::create_dir_all(root.join("world")).unwrap();
    fs::create_dir(root.join("lost+found")).unwrap();
    fs::write(root.join("server.properties"), "motd=hi\n").unwrap();
    fs::write(dir.path().join("outside.txt"), "secret").unwrap();
    symlink(dir.path(), root.join("escape")).unwrap();
    let volume = Volume::open(&root).unwrap();
    (dir, root, volume)
}

#[test]
fn paths_are_validated_and_internal_names_hidden() {
    assert_eq!(validate("/world/region/", true).unwrap(), "world/region");
    assert_eq!(validate("", true).unwrap(), "");
    for bad in [
        "",
        "../x",
        "a/../../x",
        "a//b",
        "./a",
        "lost+found/x",
        ".gm-upload-1",
        "a/.gm-restore-staging",
        "a\\b",
    ] {
        assert!(validate(bad, false).is_err(), "{bad:?}");
    }
    let (_dir, _root, volume) = volume();
    let names: Vec<_> = volume
        .list("")
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("world".into(), EntryKind::Directory),
            ("escape".into(), EntryKind::Link),
            ("server.properties".into(), EntryKind::File)
        ]
    );
}

#[test]
fn links_are_never_followed_even_when_swapped_in_later() {
    let (dir, root, volume) = volume();
    assert!(volume.read_text("escape/outside.txt").is_err());
    assert!(volume.list("escape").is_err());
    assert!(volume.write_text("escape/new.txt", "x", None).is_err());
    assert!(volume.create_directory("escape/new").is_err());
    assert!(volume.rename("server.properties", "escape/stolen").is_err());
    assert!(volume.read_chunk("escape/outside.txt", 0, 10).is_err());
    // A directory replaced by a link after validation (as a running game could do) is refused.
    fs::remove_dir(root.join("world")).unwrap();
    symlink(dir.path(), root.join("world")).unwrap();
    assert!(volume.read_text("world/outside.txt").is_err());
    assert!(!dir.path().join("new.txt").exists() && !dir.path().join("stolen").exists());
    // Deleting a folder that contains a link removes the link, not what it points to.
    fs::create_dir(root.join("mods")).unwrap();
    symlink(dir.path().join("outside.txt"), root.join("mods/link.jar")).unwrap();
    volume.delete("mods").unwrap();
    assert!(!root.join("mods").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("outside.txt")).unwrap(),
        "secret"
    );
    volume.delete("escape").unwrap();
    assert!(dir.path().join("outside.txt").exists());
}

#[test]
fn text_edits_detect_conflicts_and_refuse_binary_files() {
    let (_dir, root, volume) = volume();
    let (text, version) = volume.read_text("server.properties").unwrap();
    assert_eq!(text, "motd=hi\n");
    fs::write(root.join("server.properties"), "motd=changed elsewhere\n").unwrap();
    let conflict = volume
        .write_text("server.properties", "motd=mine\n", Some(&version))
        .unwrap_err();
    assert_eq!(conflict.kind(), std::io::ErrorKind::AlreadyExists);
    let (_, current) = volume.read_text("server.properties").unwrap();
    let saved = volume
        .write_text("server.properties", "motd=mine\n", Some(&current))
        .unwrap();
    assert_eq!(
        volume.read_text("server.properties").unwrap(),
        ("motd=mine\n".into(), saved)
    );
    assert!(
        volume.write_text("server.properties", "x", None).is_err(),
        "creating must not overwrite"
    );
    volume.write_text("world/notes.txt", "new", None).unwrap();
    fs::write(root.join("level.dat"), [0x1f, 0x8b, 0, 1, 2]).unwrap();
    assert!(
        volume
            .read_text("level.dat")
            .unwrap_err()
            .to_string()
            .contains("binary")
    );
    fs::write(root.join("huge.log"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    assert!(volume.read_text("huge.log").is_err());
    assert!(volume.read_text("world").is_err());
}

#[test]
fn uploads_appear_only_when_complete_and_chunks_must_be_in_order() {
    let (_dir, root, volume) = volume();
    let upload = Uuid::new_v4();
    volume
        .write_chunk(upload, "world/data.bin", 0, b"hello ", false)
        .unwrap();
    assert!(
        !root.join("world/data.bin").exists(),
        "not visible before the last chunk"
    );
    let skipped = volume
        .write_chunk(upload, "world/data.bin", 99, b"x", false)
        .unwrap_err();
    assert!(skipped.to_string().contains("out of order"), "{skipped}");
    let upload = Uuid::new_v4();
    volume
        .write_chunk(upload, "world/data.bin", 0, b"hello ", false)
        .unwrap();
    volume
        .write_chunk(upload, "world/data.bin", 6, b"world", true)
        .unwrap();
    assert_eq!(
        fs::read(root.join("world/data.bin")).unwrap(),
        b"hello world"
    );
    let (chunk, size) = volume.read_chunk("world/data.bin", 6, 100).unwrap();
    assert_eq!((chunk.as_slice(), size), (&b"world"[..], 11));
    assert!(
        volume
            .write_chunk(Uuid::new_v4(), "world", 0, b"x", true)
            .is_err(),
        "cannot replace a folder"
    );
    let leftover = Uuid::new_v4();
    volume
        .write_chunk(leftover, "world/partial.bin", 0, b"half", false)
        .unwrap();
    volume.clean_uploads().unwrap();
    assert!(
        fs::read_dir(root.join("world")).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".gm-"))
    );
}

#[test]
fn folders_and_renames_never_overwrite() {
    let (_dir, root, volume) = volume();
    volume.create_directory("config").unwrap();
    assert!(volume.create_directory("config").is_err());
    volume.write_text("config/a.txt", "a", None).unwrap();
    volume.write_text("config/b.txt", "b", None).unwrap();
    assert!(volume.rename("config/a.txt", "config/b.txt").is_err());
    volume.rename("config/a.txt", "world/a.txt").unwrap();
    assert_eq!(fs::read_to_string(root.join("world/a.txt")).unwrap(), "a");
    volume.delete("config").unwrap();
    assert!(!root.join("config").exists());
    assert!(volume.delete("missing").is_err());
}
