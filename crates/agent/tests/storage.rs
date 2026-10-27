#![cfg(target_os = "linux")]

use manager_agent::{
    files::as_server,
    storage::{
        Layout, create_volume, disk_budget, ensure_mounted, is_mount_point, lock_instance, mounted,
        quota_tools_available, remove_volume, volume_usage,
    },
};
use std::{fs, io::Write, os::unix::fs::MetadataExt, path::Path};
use uuid::Uuid;

#[test]
fn mount_points_are_matched_exactly_including_escaped_spaces() {
    let info = "36 35 98:0 / /var/lib/game\\040manager/volumes/a rw,nodev - ext4 /dev/loop0 rw\n";
    assert!(is_mount_point(
        info,
        Path::new("/var/lib/game manager/volumes/a")
    ));
    assert!(!is_mount_point(
        info,
        Path::new("/var/lib/game manager/volumes")
    ));
    assert!(!is_mount_point(
        info,
        Path::new("/var/lib/game manager/volumes/ab")
    ));
}

#[test]
fn disk_budget_keeps_the_reserve_and_counts_owned_images_once() {
    let gib = 1024 * 1024 * 1024;
    assert_eq!(disk_budget(30 * gib, 10 * gib, 0), 20 * gib);
    assert_eq!(disk_budget(30 * gib, 10 * gib, 8 * gib), 28 * gib);
    assert_eq!(disk_budget(5 * gib, 10 * gib, 0), 0);
    assert_eq!(disk_budget(u64::MAX, 0, u64::MAX), u64::MAX);
}

#[tokio::test]
async fn volume_is_a_hard_quota_owned_by_the_server_and_removable() {
    if fs::metadata("/proc/self").unwrap().uid() != 0 || !quota_tools_available() {
        eprintln!("skipped: loop-mounted quota volumes need root and e2fsprogs");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout {
        data_root: dir.path().to_path_buf(),
    };
    layout.prepare().unwrap();
    let id = Uuid::new_v4();
    let uid = 54322;
    let limit = 64 * 1024 * 1024;
    assert!(create_volume(&layout, id, uid, 1024).await.is_err());
    create_volume(&layout, id, uid, limit).await.unwrap();
    let volume = layout.volume(id);
    assert!(mounted(&volume).unwrap());
    assert_eq!(fs::metadata(&volume).unwrap().uid(), uid);
    assert_eq!(fs::metadata(&volume).unwrap().mode() & 0o777, 0o700);
    assert!(
        create_volume(&layout, id, uid, limit).await.is_err(),
        "existing storage is not replaced"
    );
    let (_, total) = volume_usage(&volume).unwrap();
    assert!(total <= limit);
    let target = volume.clone();
    let outcome = std::thread::spawn(move || {
        as_server(uid, || {
            let mut file = fs::File::create(target.join("fill"))?;
            let block = vec![0u8; 1024 * 1024];
            for _ in 0..80 {
                file.write_all(&block)?;
            }
            file.sync_all()
        })
    })
    .join()
    .unwrap();
    assert_eq!(outcome.unwrap_err().raw_os_error(), Some(libc::ENOSPC));
    // A remount (for example after reboot) keeps the data.
    std::process::Command::new("umount")
        .arg(&volume)
        .status()
        .unwrap();
    assert!(!mounted(&volume).unwrap());
    ensure_mounted(&layout, id).await.unwrap();
    assert!(volume.join("fill").exists());
    remove_volume(&layout, id).await.unwrap();
    assert!(!mounted(&volume).unwrap());
    assert!(!layout.image(id).exists());
    assert!(!volume.exists());
}

#[test]
fn only_one_agent_can_hold_a_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout {
        data_root: dir.path().to_path_buf(),
    };
    let first = lock_instance(&layout).unwrap();
    assert_eq!(
        lock_instance(&layout).err().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(first);
    assert!(lock_instance(&layout).is_ok());
}
