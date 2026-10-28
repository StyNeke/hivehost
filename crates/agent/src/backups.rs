// Backup archives. Creation and extraction run with the server's own file identity (see
// files::as_server), never follow symlinks, and treat every archive entry as untrusted input.
use crate::files;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const STAGING: &str = ".gm-restore-staging";
pub const PREVIOUS: &str = ".gm-restore-previous";
// Present while restored entries are being moved in: every original is then in PREVIOUS.
pub const PHASE_TWO: &str = ".gm-restore-phase-two";
const MAX_DEPTH: usize = 64;
const MAX_LINK_TARGET: usize = 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// Top-level names the agent manages itself and never archives or replaces.
fn reserved(name: &std::ffi::OsStr) -> bool {
    name == "lost+found" || name == STAGING || name == PREVIOUS || name == PHASE_TWO
}

// Counts written bytes and lets the caller stop a write that would eat into the OS reserve.
pub struct Guarded<W, F> {
    inner: W,
    written: u64,
    next_check: u64,
    check: F,
}

impl<W: Write, F: FnMut(u64) -> io::Result<()>> Guarded<W, F> {
    pub fn new(inner: W, check: F) -> Self {
        Self {
            inner,
            written: 0,
            next_check: 0,
            check,
        }
    }
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write, F: FnMut(u64) -> io::Result<()>> Write for Guarded<W, F> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.written >= self.next_check {
            (self.check)(self.written)?;
            self.next_check = self.written + 64 * 1024 * 1024;
        }
        let written = self.inner.write(buffer)?;
        self.written += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// Writes a gzip tar of `root` into `output`. Directories, regular files and symlinks (as links)
// are stored; sockets, FIFOs and devices are skipped. Must run as the server's file identity.
pub fn write_archive(root: &Path, output: impl Write) -> io::Result<u64> {
    let mut builder = tar::Builder::new(GzEncoder::new(output, Compression::fast()));
    builder.follow_symlinks(false);
    builder.mode(tar::HeaderMode::Complete);
    let mut entries = 0u64;
    let mut pending: Vec<(PathBuf, usize)> = vec![];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !reserved(&entry.file_name()) {
            pending.push((PathBuf::from(entry.file_name()), 1));
        }
    }
    pending.sort();
    pending.reverse();
    while let Some((relative, depth)) = pending.pop() {
        let path = root.join(&relative);
        let meta = fs::symlink_metadata(&path)?;
        let kind = meta.file_type();
        if !(kind.is_dir() || kind.is_file() || kind.is_symlink()) {
            continue;
        }
        builder.append_path_with_name(&path, &relative)?;
        entries += 1;
        if kind.is_dir() {
            if depth >= MAX_DEPTH {
                return Err(invalid("directory tree is too deep to back up"));
            }
            let mut children: Vec<_> = fs::read_dir(&path)?
                .map(|e| e.map(|e| (relative.join(e.file_name()), depth + 1)))
                .collect::<Result<_, _>>()?;
            children.sort();
            children.reverse();
            pending.extend(children);
        }
    }
    let encoder = builder.into_inner()?;
    encoder.finish()?.flush()?;
    Ok(entries)
}

// Reads the whole archive back so a truncated or corrupt file is never kept as a backup.
pub fn verify_archive(input: impl Read) -> io::Result<u64> {
    let mut archive = tar::Archive::new(GzDecoder::new(input));
    let mut entries = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        io::copy(&mut entry, &mut io::sink())?;
        entries += 1;
    }
    Ok(entries)
}

fn safe_relative(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("archive path is not UTF-8"))?;
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty()
        || path.components().count() > MAX_DEPTH
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("archive entry escapes the server directory"));
    }
    Ok(trimmed.to_owned())
}

// Extracts into `staging` (which must not exist yet). Every entry is validated; nothing is
// written through a symlink; setuid/setgid/sticky bits are dropped. Run as the server identity.
pub fn extract_archive(input: impl Read, staging: &Path) -> io::Result<u64> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::create_dir(staging)?;
    let mut archive = tar::Archive::new(GzDecoder::new(input));
    let mut entries = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let relative = safe_relative(&entry.path()?)?;
        if relative
            .split('/')
            .next()
            .is_some_and(|first| reserved(first.as_ref()))
        {
            continue;
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                let path = files::resolve(staging, &relative)?;
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => return Err(invalid("archive entry conflicts with another entry")),
                    Err(_) => fs::create_dir(&path)?,
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o700))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let path = files::resolve(staging, &relative)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(mode | 0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                io::copy(&mut entry, &mut file)?;
                file.sync_all()?;
            }
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()?
                    .ok_or_else(|| invalid("symlink without target"))?
                    .into_owned();
                let length = target.as_os_str().len();
                if length == 0 || length > MAX_LINK_TARGET {
                    return Err(invalid("invalid symlink target"));
                }
                let path = files::resolve(staging, &relative)?;
                std::os::unix::fs::symlink(target, &path)?;
            }
            // Hard links, devices, FIFOs and other special entries are not restored.
            _ => continue,
        }
        entries += 1;
    }
    Ok(entries)
}

// Replaces the volume content with the staged content using renames on the same filesystem.
// If anything fails the original entries are moved back. Run as the server identity.
pub fn swap_in(root: &Path) -> io::Result<()> {
    let staging = root.join(STAGING);
    let previous = root.join(PREVIOUS);
    fs::create_dir(&previous)?;
    let current: Vec<_> = fs::read_dir(root)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|name| !reserved(name))
        .collect();
    let mut moved = vec![];
    let result = (|| {
        for name in &current {
            fs::rename(root.join(name), previous.join(name))?;
            moved.push(name.clone());
        }
        fs::File::create(root.join(PHASE_TWO))?.sync_all()?;
        let staged: Vec<_> = fs::read_dir(&staging)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        for name in staged {
            fs::rename(staging.join(&name), root.join(&name))?;
        }
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = result {
        // Put back whatever was already moved aside; staged entries that made it are removed.
        for name in fs::read_dir(root)?
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| !reserved(n))
        {
            if moved.contains(&name) {
                continue;
            }
            let path = root.join(&name);
            if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
        for name in moved {
            let _ = fs::rename(previous.join(&name), root.join(&name));
        }
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&previous);
        let _ = fs::remove_file(root.join(PHASE_TWO));
        return Err(error);
    }
    fs::remove_file(root.join(PHASE_TWO))?;
    fs::remove_dir_all(&staging)?;
    fs::remove_dir_all(&previous)
}

fn remove_entry(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

// Leftovers from an interrupted restore are resolved before anything else touches the volume,
// always back to the original data: in phase one the originals are split between the root and
// PREVIOUS; in phase two all of them are in PREVIOUS and the root holds restored entries.
pub fn clean_restore_leftovers(root: &Path) -> io::Result<()> {
    let previous = root.join(PREVIOUS);
    let marker = root.join(PHASE_TWO);
    if previous.exists() {
        if marker.exists() {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                if !reserved(&entry.file_name()) {
                    remove_entry(&entry.path())?;
                }
            }
        }
        for entry in fs::read_dir(&previous)? {
            let entry = entry?;
            fs::rename(entry.path(), root.join(entry.file_name()))?;
        }
        fs::remove_dir(&previous)?;
    }
    for leftover in [marker, root.join(STAGING)] {
        if fs::symlink_metadata(&leftover).is_ok() {
            remove_entry(&leftover)?;
        }
    }
    Ok(())
}
