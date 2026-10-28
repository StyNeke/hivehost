#![cfg(target_os = "linux")]

use manager_agent::backups::{
    Guarded, PHASE_TWO, PREVIOUS, STAGING, clean_restore_leftovers, extract_archive, swap_in,
    verify_archive, write_archive,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn tree(root: &Path) {
    fs::create_dir_all(root.join("world/region")).unwrap();
    fs::write(root.join("world/region/r.0.0.mca"), vec![7u8; 100_000]).unwrap();
    fs::write(root.join("server.properties"), "server-port=25600\n").unwrap();
    fs::create_dir(root.join("lost+found")).unwrap();
    symlink("/etc/passwd", root.join("passwd-link")).unwrap();
    let fifo = std::ffi::CString::new(root.join("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
}

#[test]
fn archives_round_trip_without_following_links_or_special_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    tree(&source);
    let mut archive = vec![];
    write_archive(&source, &mut archive).unwrap();
    assert!(
        !String::from_utf8_lossy(&archive).contains("root:x:0:0"),
        "link target content is not archived"
    );
    assert_eq!(
        verify_archive(&archive[..]).unwrap(),
        5,
        "world, region, mca, properties, link"
    );
    let staging = dir.path().join("restore");
    extract_archive(&archive[..], &staging).unwrap();
    assert_eq!(
        fs::read(staging.join("world/region/r.0.0.mca")).unwrap(),
        vec![7u8; 100_000]
    );
    assert_eq!(
        fs::read_link(staging.join("passwd-link")).unwrap(),
        Path::new("/etc/passwd")
    );
    assert!(!staging.join("pipe").exists() && !staging.join("lost+found").exists());
    assert!(
        extract_archive(&archive[..], &staging).is_err(),
        "staging must be new"
    );
}

#[test]
fn truncated_archives_fail_verification() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let mut archive = vec![];
    write_archive(dir.path(), &mut archive).unwrap();
    assert!(verify_archive(&archive[..archive.len() / 2]).is_err());
    assert!(verify_archive(&b"not an archive"[..]).is_err());
}

// (path, kind, data, mode, link target)
type Entry<'a> = (&'a str, tar::EntryType, &'a [u8], u32, Option<&'a str>);

fn hostile(entries: &[Entry]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        vec![],
        flate2::Compression::fast(),
    ));
    for (path, kind, data, mode, link) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        // Write names raw so the tar crate cannot sanitise them before extraction sees them.
        let name = header.as_old_mut().name.as_mut();
        name[..path.len()].copy_from_slice(path.as_bytes());
        if let Some(link) = link {
            header.set_link_name(link).unwrap();
        }
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn hostile_archives_cannot_write_outside_or_through_links() {
    let dir = tempfile::tempdir().unwrap();
    let file = tar::EntryType::Regular;
    for (case, entries) in [
        ("parent", vec![("../escape", file, &b"x"[..], 0o644, None)]),
        (
            "absolute",
            vec![("/tmp/gm-absolute-escape", file, &b"x"[..], 0o644, None)],
        ),
        (
            "nested parent",
            vec![("a/../../escape", file, &b"x"[..], 0o644, None)],
        ),
        (
            "through symlink",
            vec![
                ("link", tar::EntryType::Symlink, &b""[..], 0o777, Some("..")),
                ("link/escape", file, &b"x"[..], 0o644, None),
            ],
        ),
    ] {
        let staging = dir
            .path()
            .join(format!("staging-{}", case.replace(' ', "-")));
        assert!(
            extract_archive(&hostile(&entries)[..], &staging).is_err(),
            "{case} was accepted"
        );
        assert!(!dir.path().join("escape").exists(), "{case} wrote outside");
    }
    assert!(!Path::new("/tmp/gm-absolute-escape").exists());

    let staging = dir.path().join("staging-modes");
    let entries = vec![
        ("setuid", file, &b"#!/bin/sh"[..], 0o4755, None),
        (
            "hard",
            tar::EntryType::Link,
            &b""[..],
            0o644,
            Some("setuid"),
        ),
        ("device", tar::EntryType::Char, &b""[..], 0o644, None),
        ("lost+found/x", file, &b"x"[..], 0o644, None),
    ];
    assert_eq!(
        extract_archive(&hostile(&entries)[..], &staging).unwrap(),
        1
    );
    assert_eq!(
        fs::metadata(staging.join("setuid"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    assert!(
        !staging.join("hard").exists()
            && !staging.join("device").exists()
            && !staging.join("lost+found").exists()
    );
}

#[test]
fn the_reserve_guard_stops_a_growing_archive() {
    let mut calls = 0;
    let mut writer = Guarded::new(vec![], |written| {
        calls += 1;
        if written > 0 {
            Err(std::io::Error::other("reserve reached"))
        } else {
            Ok(())
        }
    });
    writer.write_all(&vec![0u8; 70 * 1024 * 1024]).unwrap();
    assert!(writer.write_all(b"more").is_err());
    drop(writer);
    assert_eq!(calls, 2);
}

fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn swaps_replace_content_and_interrupted_swaps_recover_the_original_data() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("lost+found")).unwrap();
    fs::write(root.join("old.txt"), "old").unwrap();
    fs::create_dir(root.join(STAGING)).unwrap();
    fs::write(root.join(STAGING).join("new.txt"), "new").unwrap();
    swap_in(root).unwrap();
    assert_eq!(names(root), ["lost+found", "new.txt"]);

    // Interrupted in phase one: some originals still in place, some moved aside.
    fs::create_dir(root.join(PREVIOUS)).unwrap();
    fs::rename(root.join("new.txt"), root.join(PREVIOUS).join("new.txt")).unwrap();
    fs::write(root.join("still-here.txt"), "original").unwrap();
    fs::create_dir(root.join(STAGING)).unwrap();
    clean_restore_leftovers(root).unwrap();
    assert_eq!(names(root), ["lost+found", "new.txt", "still-here.txt"]);

    // Interrupted in phase two: all originals aside, some restored entries already moved in.
    fs::create_dir(root.join(PREVIOUS)).unwrap();
    for name in ["new.txt", "still-here.txt"] {
        fs::rename(root.join(name), root.join(PREVIOUS).join(name)).unwrap();
    }
    fs::write(root.join(PHASE_TWO), "").unwrap();
    fs::write(root.join("restored.txt"), "restored").unwrap();
    fs::create_dir(root.join(STAGING)).unwrap();
    clean_restore_leftovers(root).unwrap();
    assert_eq!(names(root), ["lost+found", "new.txt", "still-here.txt"]);
    assert_eq!(
        fs::read_to_string(root.join("still-here.txt")).unwrap(),
        "original"
    );
}

#[test]
fn pterodactyl_style_archives_with_dot_prefixes_import_and_escapes_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("volume");
    fs::create_dir_all(source.join("world/region")).unwrap();
    fs::write(source.join("world/region/r.0.0.mca"), vec![3u8; 5000]).unwrap();
    fs::write(source.join("server.properties"), "server-port=25565\n").unwrap();
    let archive = dir.path().join("server.tar.gz");
    // The command an operator runs on a copied Pterodactyl volume.
    let status = std::process::Command::new("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .arg(".")
        .status()
        .unwrap();
    assert!(status.success());
    assert!(verify_archive(fs::File::open(&archive).unwrap()).unwrap() >= 4);
    let staging = dir.path().join("staging");
    extract_archive(fs::File::open(&archive).unwrap(), &staging).unwrap();
    assert_eq!(
        fs::read(staging.join("world/region/r.0.0.mca"))
            .unwrap()
            .len(),
        5000
    );
    assert_eq!(
        fs::read_to_string(staging.join("server.properties")).unwrap(),
        "server-port=25565\n"
    );
    // A leading "./" does not open a way out of the directory.
    let file = tar::EntryType::Regular;
    for name in ["./../escape", "./a/../../escape"] {
        let hostile = hostile(&[(name, file, &b"x"[..], 0o644, None)]);
        assert!(verify_archive(&hostile[..]).is_err(), "{name}");
    }
}
