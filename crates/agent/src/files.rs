// File operations inside a server's volume. Every call runs with that server's own filesystem
// identity (setfsuid) and refuses symlinks on every path component, so a game or installer that
// plants `server.properties -> /etc/shadow` or a link into another server's volume gains nothing:
// the agent can only touch what the server's own UID could touch. Callers only use these while
// the server's container is stopped, so no process of that UID can race the checks.
use manager_core::host::PortBinding;
use manager_protocol::DetectedGame;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub const MAX_TEXT_FILE: u64 = 1024 * 1024;

// Runs `action` on the current thread with fsuid/fsgid switched to the server's identity.
// The fs identity is per-thread on Linux, so this must run on a dedicated blocking thread.
#[cfg(target_os = "linux")]
pub fn as_server<T>(uid: u32, action: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    // SAFETY: setfsuid/setfsgid only change this thread's filesystem credentials.
    unsafe {
        libc::setfsgid(uid);
        libc::setfsuid(uid);
        let current_uid = libc::setfsuid(u32::MAX) as u32;
        let current_gid = libc::setfsgid(u32::MAX) as u32;
        if current_uid != uid || current_gid != uid {
            libc::setfsuid(0);
            libc::setfsgid(0);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "agent cannot assume the server's file identity",
            ));
        }
    }
    let result = action();
    // SAFETY: restore root filesystem credentials for this thread.
    unsafe {
        libc::setfsuid(0);
        libc::setfsgid(0);
    }
    result
}

// Validates a relative path and walks it without following symlinks. Returns the full path;
// the final component may not exist yet but, if it does, must not be a symlink.
pub fn resolve(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let candidate = Path::new(relative);
    if relative.is_empty()
        || relative.len() > 512
        || relative.contains(['\0', '\\'])
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || candidate
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("path must stay inside the server directory"));
    }
    let mut current = root.to_path_buf();
    let parts: Vec<_> = candidate.components().collect();
    for (index, part) in parts.iter().enumerate() {
        current.push(part.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "symbolic links are not followed",
                ));
            }
            Ok(meta) if index + 1 < parts.len() && !meta.is_dir() => {
                return Err(invalid("path component is not a directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if index + 1 < parts.len() {
                    fs::create_dir(&current)?;
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(current)
}

fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

pub fn read_text(root: &Path, relative: &str) -> io::Result<Option<String>> {
    let path = resolve(root, relative)?;
    let file = match open_no_follow(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid("not a regular file"));
    }
    let mut text = String::new();
    file.take(MAX_TEXT_FILE + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_TEXT_FILE {
        return Err(invalid("file is too large to edit"));
    }
    Ok(Some(text))
}

// Writes through a fresh temporary file in the same directory, then renames over the target.
pub fn write_atomic(root: &Path, relative: &str, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = resolve(root, relative)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("invalid file name"))?;
    let temporary = path.with_file_name(format!(".{name}.gm-{}", uuid::Uuid::new_v4().simple()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    let written = file.write_all(bytes).and_then(|_| file.sync_all());
    if let Err(error) = written.and_then(|_| fs::rename(&temporary, &path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

pub fn remove(root: &Path, relative: &str) -> io::Result<()> {
    let path = resolve(root, relative)?;
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub fn list_files(root: &Path, folder: &str, extension: &str) -> io::Result<Vec<String>> {
    let directory = resolve(root, folder)?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for entry in entries.take(10_000) {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && let Some(name) = entry.file_name().to_str()
            && name.ends_with(extension)
            && !name.starts_with('.')
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

// Renders the placeholders Pterodactyl eggs use in configuration transforms. Anything unknown
// fails closed instead of writing a literal "{{...}}" into game configuration.
pub fn render(
    template: &str,
    binding: &PortBinding,
    variables: &BTreeMap<String, String>,
) -> io::Result<String> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let end = rest[start..]
            .find("}}")
            .ok_or_else(|| invalid("unterminated placeholder"))?;
        let key = rest[start + 2..start + end].trim();
        let value = match key {
            "server.build.default.port" | "server.build.default.port_number" => {
                binding.port.to_string()
            }
            "server.build.default.ip" => binding.ip.to_string(),
            _ => {
                let name = key
                    .strip_prefix("server.build.env.")
                    .or_else(|| key.strip_prefix("env."))
                    .unwrap_or(key);
                variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| invalid("configuration uses an unsupported placeholder"))?
            }
        };
        output.push_str(&value);
        rest = &rest[start + end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

// Sets `key=value` lines in a Java properties file, replacing existing keys and appending new ones.
pub fn set_properties(existing: &str, values: &BTreeMap<String, String>) -> String {
    let mut remaining = values.clone();
    let mut output = String::with_capacity(existing.len() + 64);
    for line in existing.lines() {
        let trimmed = line.trim_start();
        let key = if trimmed.starts_with('#') || trimmed.starts_with('!') {
            None
        } else {
            trimmed
                .split_once(['=', ':'])
                .map(|(key, _)| key.trim_end().to_owned())
        };
        match key.and_then(|key| remaining.remove_entry(&key)) {
            Some((key, value)) => output.push_str(&format!("{key}={value}")),
            None => output.push_str(line),
        }
        output.push('\n');
    }
    for (key, value) in remaining {
        output.push_str(&format!("{key}={value}\n"));
    }
    output
}

fn jar_version(root: &Path, relative: &str) -> io::Result<Option<String>> {
    let path = resolve(root, relative)?;
    let file = match open_no_follow(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut archive = zip::ZipArchive::new(file).map_err(|_| invalid("unreadable jar"))?;
    let Ok(entry) = archive.by_name("version.json") else {
        return Ok(None);
    };
    let mut text = String::new();
    entry.take(64 * 1024).read_to_string(&mut text)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| invalid("invalid version.json"))?;
    Ok(parsed
        .get("id")
        .and_then(|id| id.as_str())
        .and_then(safe_version))
}

// Subdirectory names of `relative`, without creating anything or following links on the way.
fn directories(root: &Path, relative: &str) -> io::Result<Vec<String>> {
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(vec![]),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error),
        }
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&current)?.take(1000) {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn safe_version(id: &str) -> Option<String> {
    (!id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+ ".contains(&b)))
    .then(|| id.to_owned())
}

// Forge and NeoForge keep the unmodified game in libraries/net/minecraft/server/<version>[-<mcp>].
fn library_game_version(root: &Path) -> io::Result<Option<String>> {
    Ok(directories(root, "libraries/net/minecraft/server")?
        .iter()
        .filter_map(|name| safe_version(name.split('-').next().unwrap_or_default()))
        .next_back())
}

// Paper-family jars (paperclip) carry the game's version.json or list `paper-<version>.jar`.
fn paperclip_version(root: &Path, relative: &str) -> io::Result<Option<String>> {
    let path = resolve(root, relative)?;
    let file = match open_no_follow(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return Ok(None);
    };
    if !archive
        .file_names()
        .any(|n| n.starts_with("io/papermc/paperclip/"))
    {
        return Ok(None);
    }
    if let Ok(entry) = archive.by_name("version.json") {
        let mut text = String::new();
        entry.take(64 * 1024).read_to_string(&mut text)?;
        if let Some(id) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("id")
                    .and_then(|id| id.as_str())
                    .and_then(safe_version)
            })
        {
            return Ok(Some(id));
        }
    }
    let mut text = String::new();
    if let Ok(entry) = archive.by_name("META-INF/versions.list") {
        entry.take(64 * 1024).read_to_string(&mut text)?;
    }
    Ok(text.lines().find_map(|line| {
        let name = line.split_whitespace().last()?.rsplit('/').next()?;
        let version = name.strip_suffix(".jar")?.split_once('-')?.1;
        safe_version(version)
    }))
}

// Detects the installed Minecraft version and loader from files, not from egg variables, so a
// "latest" choice is recorded as the version that was actually installed.
pub fn detect_game(root: &Path, server_jar: &str) -> io::Result<Option<DetectedGame>> {
    if let Some(properties) = read_text(root, "fabric-server-launcher.properties")? {
        let jar = properties
            .lines()
            .find_map(|line| line.trim().strip_prefix("serverJar="))
            .unwrap_or("server.jar")
            .trim()
            .to_owned();
        if let Some(version) = jar_version(root, &jar)? {
            return Ok(Some(DetectedGame {
                version,
                loader: Some("fabric".into()),
            }));
        }
        return Ok(None);
    }
    for (folder, loader) in [
        ("libraries/net/neoforged/neoforge", "neoforge"),
        ("libraries/net/neoforged/forge", "neoforge"),
        ("libraries/net/minecraftforge/forge", "forge"),
    ] {
        if !directories(root, folder)?.is_empty() {
            return Ok(library_game_version(root)?.map(|version| DetectedGame {
                version,
                loader: Some(loader.into()),
            }));
        }
    }
    if let Ok(meta) = fs::symlink_metadata(root.join("bedrock_server"))
        && meta.is_file()
    {
        return Ok(Some(DetectedGame {
            version: bedrock_version(root)?.unwrap_or_else(|| "Bedrock".into()),
            loader: Some("bedrock".into()),
        }));
    }
    if let Some(version) = paperclip_version(root, server_jar)? {
        return Ok(Some(DetectedGame {
            version,
            loader: Some("paper".into()),
        }));
    }
    Ok(jar_version(root, server_jar)?.map(|version| DetectedGame {
        version,
        loader: None,
    }))
}

// Bedrock packages do not carry a version file; the release notes start with the version.
fn bedrock_version(root: &Path) -> io::Result<Option<String>> {
    let text = match read_text(root, "release-notes.txt") {
        Ok(text) => text.unwrap_or_default(),
        Err(_) => String::new(),
    };
    Ok(text
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| part.matches('.').count() >= 2 && part.starts_with('1'))
        .and_then(safe_version))
}

// Gives every file the installer created to the server's UID without following links or
// leaving the volume's filesystem. Runs as root, after the installer container has exited.
#[cfg(target_os = "linux")]
pub fn chown_tree(root: &Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let device = fs::symlink_metadata(root)?.dev();
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > 2_000_000 {
            return Err(io::Error::other("too many files to take ownership of"));
        }
        let meta = fs::symlink_metadata(&path)?;
        if meta.dev() != device {
            continue;
        }
        std::os::unix::fs::lchown(&path, Some(uid), Some(uid))?;
        if meta.is_dir() {
            for entry in fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}
