#![cfg(target_os = "linux")]

use manager_agent::files::{
    as_server, chown_tree, detect_game, list_files, read_text, remove, render, resolve,
    set_properties, write_atomic,
};
use manager_core::host::{PortBinding, Transport};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

const UID: u32 = 54321;

fn is_root() -> bool {
    fs::metadata("/proc/self").unwrap().uid() == 0
}

#[test]
fn paths_cannot_escape_or_follow_links() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for bad in [
        "",
        "/etc/passwd",
        "../x",
        "a/../../x",
        "a\\b",
        "./x",
        "a//b/",
    ] {
        assert!(resolve(root, bad).is_err(), "{bad:?}");
    }
    fs::create_dir(root.join("real")).unwrap();
    symlink("/etc", root.join("link")).unwrap();
    symlink("/etc/hostname", root.join("server.properties")).unwrap();
    assert!(resolve(root, "link/passwd").is_err());
    assert!(resolve(root, "server.properties").is_err());
    assert!(read_text(root, "server.properties").is_err());
    assert!(write_atomic(root, "server.properties", b"x").is_err());
    assert!(write_atomic(root, "link/new", b"x").is_err());
    assert_eq!(
        resolve(root, "real/new/file").unwrap(),
        root.join("real/new/file")
    );
    assert!(root.join("real/new").is_dir());
}

#[test]
fn atomic_writes_replace_regular_files_and_size_limits_hold() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_atomic(root, "eula.txt", b"eula=true\n").unwrap();
    assert_eq!(read_text(root, "eula.txt").unwrap().unwrap(), "eula=true\n");
    write_atomic(root, "eula.txt", b"eula=false\n").unwrap();
    assert_eq!(
        read_text(root, "eula.txt").unwrap().unwrap(),
        "eula=false\n"
    );
    assert_eq!(read_text(root, "missing.txt").unwrap(), None);
    fs::write(root.join("big.txt"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    assert!(read_text(root, "big.txt").is_err());
    assert_eq!(
        fs::read_dir(root).unwrap().count(),
        2,
        "no temporary files left behind"
    );
    remove(root, "eula.txt").unwrap();
    remove(root, "eula.txt").unwrap();
}

#[test]
fn file_identity_blocks_access_outside_the_servers_own_files() {
    if !is_root() {
        eprintln!("skipped: setfsuid boundary needs root");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    fs::write(&secret, "root only").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let volume = dir.path().join("volume");
    fs::create_dir(&volume).unwrap();
    std::os::unix::fs::chown(&volume, Some(UID), Some(UID)).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let handle = std::thread::spawn(move || {
        let denied = as_server(UID, || fs::read_to_string(&secret)).unwrap_err();
        assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
        as_server(UID, || write_atomic(&volume, "mods/a.jar", b"jar")).unwrap();
        let owner = fs::metadata(volume.join("mods/a.jar")).unwrap().uid();
        // Identity is restored afterwards on the same thread.
        assert!(fs::read_to_string(&secret).is_ok());
        owner
    });
    assert_eq!(handle.join().unwrap(), UID);
}

#[test]
fn properties_transforms_replace_keys_keep_comments_and_append_missing() {
    let mut values = BTreeMap::new();
    values.insert("server-port".to_string(), "25600".to_string());
    values.insert("server-ip".to_string(), "0.0.0.0".to_string());
    let output = set_properties(
        "#comment\nmotd=hi\nserver-port = 25565\nserver-port=1\n",
        &values,
    );
    assert_eq!(
        output,
        "#comment\nmotd=hi\nserver-port=25600\nserver-port=1\nserver-ip=0.0.0.0\n"
    );
    assert_eq!(
        set_properties("", &values),
        "server-ip=0.0.0.0\nserver-port=25600\n"
    );
}

#[test]
fn placeholders_render_known_values_and_reject_unknown_ones() {
    let binding = PortBinding {
        ip: "100.116.183.201".parse().unwrap(),
        port: 25600,
        transport: Transport::Tcp,
    };
    let mut variables = BTreeMap::new();
    variables.insert("MOTD".to_string(), "Hello".to_string());
    assert_eq!(
        render("{{server.build.default.port}}", &binding, &variables).unwrap(),
        "25600"
    );
    assert_eq!(
        render(
            "{{server.build.default.ip}}:{{env.MOTD}}",
            &binding,
            &variables
        )
        .unwrap(),
        "100.116.183.201:Hello"
    );
    assert_eq!(
        render("{{ server.build.env.MOTD }}!", &binding, &variables).unwrap(),
        "Hello!"
    );
    assert_eq!(render("0.0.0.0", &binding, &variables).unwrap(), "0.0.0.0");
    assert!(render("{{server.build.memory}}", &binding, &variables).is_err());
    assert!(render("{{server.build.default.port", &binding, &variables).is_err());
}

fn jar(path: &std::path::Path, version_json: Option<&str>) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    if let Some(json) = version_json {
        writer
            .start_file("version.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(json.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn installed_game_is_detected_from_files_not_from_variables() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(detect_game(root).unwrap(), None);
    jar(
        &root.join("server.jar"),
        Some(r#"{"id":"1.21.4","name":"1.21.4"}"#),
    );
    let vanilla = detect_game(root).unwrap().unwrap();
    assert_eq!((vanilla.version.as_str(), vanilla.loader), ("1.21.4", None));
    fs::write(
        root.join("fabric-server-launcher.properties"),
        "serverJar=minecraft-server.jar\n",
    )
    .unwrap();
    jar(
        &root.join("minecraft-server.jar"),
        Some(r#"{"id":"1.21.10"}"#),
    );
    let fabric = detect_game(root).unwrap().unwrap();
    assert_eq!(fabric.version, "1.21.10");
    assert_eq!(fabric.loader.as_deref(), Some("fabric"));
    jar(
        &root.join("minecraft-server.jar"),
        Some(r#"{"id":"1.21\n<script>"}"#),
    );
    assert_eq!(detect_game(root).unwrap(), None);
    fs::write(
        root.join("fabric-server-launcher.properties"),
        "serverJar=../../etc/passwd\n",
    )
    .unwrap();
    assert!(detect_game(root).is_err());
}

#[test]
fn ownership_fixup_does_not_follow_links_out_of_the_volume() {
    if !is_root() {
        eprintln!("skipped: chown needs root");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "x").unwrap();
    let volume = dir.path().join("volume");
    fs::create_dir_all(volume.join("world/region")).unwrap();
    fs::write(volume.join("world/region/r.0.0.mca"), "x").unwrap();
    symlink(&outside, volume.join("escape")).unwrap();
    chown_tree(&volume, UID).unwrap();
    assert_eq!(
        fs::metadata(volume.join("world/region/r.0.0.mca"))
            .unwrap()
            .uid(),
        UID
    );
    assert_eq!(
        fs::symlink_metadata(volume.join("escape")).unwrap().uid(),
        UID
    );
    assert_eq!(fs::metadata(&outside).unwrap().uid(), 0);
    assert_eq!(
        list_files(&volume, "world/region", ".mca").unwrap(),
        vec!["r.0.0.mca"]
    );
    assert!(list_files(&volume, "escape", ".jar").is_err());
}
