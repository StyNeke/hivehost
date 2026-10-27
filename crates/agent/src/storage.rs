// Per-server storage: one fully allocated ext4 image per server, loop-mounted nodev/nosuid.
// The filesystem size is a hard quota that also works on hosts whose main ext4 filesystem has no
// project quota support, and hard links or rename tricks cannot leave the server's filesystem.
use crate::commands::run_checked;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Layout {
    pub data_root: PathBuf,
}

impl Layout {
    pub fn image(&self, id: Uuid) -> PathBuf {
        self.data_root.join("images").join(format!("{}.img", id))
    }
    pub fn volume(&self, id: Uuid) -> PathBuf {
        self.data_root.join("volumes").join(id.to_string())
    }
    pub fn registry(&self) -> PathBuf {
        self.data_root.join("registry")
    }
    pub fn scripts(&self, id: Uuid) -> PathBuf {
        self.data_root.join("scripts").join(id.to_string())
    }
    pub fn docker_config(&self) -> PathBuf {
        self.data_root.join("docker-client")
    }

    // Creates the agent-private directories. None of them is ever mounted into a container
    // except a single server's volume and its read-only install script directory.
    pub fn prepare(&self) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        for directory in ["images", "volumes", "registry", "scripts", "docker-client"] {
            let path = self.data_root.join(directory);
            std::fs::create_dir_all(&path)?;
            let mode = if directory == "volumes" { 0o711 } else { 0o700 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }
}

fn tool(name: &str) -> PathBuf {
    ["/usr/sbin", "/usr/bin", "/sbin", "/bin"]
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| Path::new("/usr/bin").join(name))
}

pub fn quota_tools_available() -> bool {
    ["mkfs.ext4", "fallocate", "mount", "umount"]
        .iter()
        .all(|name| tool(name).exists())
}

fn unescape_mount_path(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&field[index + 1..index + 4], 8)
        {
            output.push(value);
            index += 4;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

pub fn is_mount_point(mountinfo: &str, path: &Path) -> bool {
    mountinfo.lines().any(|line| {
        line.split(' ')
            .nth(4)
            .is_some_and(|field| Path::new(&unescape_mount_path(field)) == path)
    })
}

pub fn mounted(path: &Path) -> io::Result<bool> {
    Ok(is_mount_point(
        &std::fs::read_to_string("/proc/self/mountinfo")?,
        path,
    ))
}

// The admissible disk budget: what is still free after the OS reserve, plus space already held
// by this application's own images (those are counted again by the caller as reservations).
pub fn disk_budget(available: u64, reserve: u64, owned_images: u64) -> u64 {
    available
        .saturating_sub(reserve)
        .saturating_add(owned_images)
}

pub fn filesystem_space(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid path"))?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is a valid NUL-terminated string and stat is a valid out pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let fragment = stat.f_frsize as u64;
    let total = (stat.f_blocks as u64).saturating_mul(fragment);
    let available = (stat.f_bavail as u64).saturating_mul(fragment);
    Ok((total, available))
}

// Returns (used, total) bytes for a mounted server volume.
pub fn volume_usage(path: &Path) -> io::Result<(u64, u64)> {
    let (total, available) = filesystem_space(path)?;
    Ok((total.saturating_sub(available), total))
}

async fn run(name: &str, arguments: Vec<OsString>, seconds: u64) -> io::Result<()> {
    run_checked(
        &tool(name),
        &arguments,
        Duration::from_secs(seconds),
        64 * 1024,
    )
    .await
    .map(|_| ())
    .map_err(|error| io::Error::new(error.kind(), format!("{name} failed: {error}")))
}

pub async fn create_volume(layout: &Layout, id: Uuid, uid: u32, bytes: u64) -> io::Result<()> {
    if bytes < 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "disk limit must be at least 64 MiB",
        ));
    }
    let image = layout.image(id);
    let volume = layout.volume(id);
    if image.exists() || mounted(&volume)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "server storage already exists",
        ));
    }
    // Fully allocate so later writes cannot fail for lack of host space.
    let allocated = async {
        run(
            "fallocate",
            vec!["-l".into(), bytes.to_string().into(), image.clone().into()],
            300,
        )
        .await?;
        run(
            "mkfs.ext4",
            vec![
                "-q".into(),
                "-F".into(),
                "-m".into(),
                "0".into(),
                "-E".into(),
                format!("root_owner={uid}:{uid}").into(),
                image.clone().into(),
            ],
            600,
        )
        .await
    }
    .await;
    if let Err(error) = allocated {
        let _ = std::fs::remove_file(&image);
        return Err(error);
    }
    ensure_mounted(layout, id).await
}

pub async fn ensure_mounted(layout: &Layout, id: Uuid) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let image = layout.image(id);
    let volume = layout.volume(id);
    if mounted(&volume)? {
        return Ok(());
    }
    if !image.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "server storage image is missing",
        ));
    }
    std::fs::create_dir_all(&volume)?;
    std::fs::set_permissions(&volume, std::fs::Permissions::from_mode(0o700))?;
    run(
        "mount",
        vec![
            "-o".into(),
            "loop,nodev,nosuid".into(),
            image.into(),
            volume.clone().into(),
        ],
        60,
    )
    .await?;
    // Other servers' UIDs must not be able to traverse into this volume.
    std::fs::set_permissions(&volume, std::fs::Permissions::from_mode(0o700))
}

pub async fn remove_volume(layout: &Layout, id: Uuid) -> io::Result<()> {
    let volume = layout.volume(id);
    if mounted(&volume)? {
        run("umount", vec![volume.clone().into()], 120).await?;
    }
    match std::fs::remove_dir(&volume) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    match std::fs::remove_file(layout.image(id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
