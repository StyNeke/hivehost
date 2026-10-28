// File manager operations on a server volume. They may run while the game runs, so path
// containment is enforced by the kernel (openat2 with RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS) and
// every create/rename/delete uses a directory handle plus a single name: a game that swaps a
// directory for a symlink mid-operation cannot redirect it. Callers also run these with the
// server's file identity (files::as_server).
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, ResolveFlags};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, OwnedFd},
    path::Path,
};

pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 5000;
const MAX_DEPTH: usize = 64;
const UPLOAD_PREFIX: &str = ".gm-upload-";

fn resolve_flags() -> ResolveFlags {
    ResolveFlags::BENEATH
        | ResolveFlags::NO_SYMLINKS
        | ResolveFlags::NO_MAGICLINKS
        | ResolveFlags::NO_XDEV
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn from_rustix(error: rustix::io::Errno) -> io::Error {
    let error = io::Error::from(error);
    if error.raw_os_error() == Some(libc::ELOOP) || error.raw_os_error() == Some(libc::EXDEV) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symbolic links are not followed",
        );
    }
    error
}

pub use manager_protocol::{FileEntry as Entry, FileKind as EntryKind};

// Normalises a user path ("" is the server root). Internal names are never reachable.
pub fn validate(path: &str, allow_root: bool) -> io::Result<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return if allow_root {
            Ok(String::new())
        } else {
            Err(invalid("choose a file or folder"))
        };
    }
    if trimmed.len() > 1024 || trimmed.contains(['\0', '\\']) {
        return Err(invalid("invalid path"));
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() > MAX_DEPTH
        || parts.iter().any(|p| {
            p.is_empty() || *p == "." || *p == ".." || p.starts_with(".gm-") || p.len() > 255
        })
        || parts[0] == "lost+found"
    {
        return Err(invalid("path must stay inside the server directory"));
    }
    Ok(trimmed.to_owned())
}

fn split(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent, name),
        None => ("", path),
    }
}

pub struct Volume(OwnedFd);

impl Volume {
    pub fn open(root: &Path) -> io::Result<Self> {
        rustix::fs::open(
            root,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Self)
        .map_err(io::Error::from)
    }

    fn at(&self, path: &str, flags: OFlags, mode: Mode) -> io::Result<OwnedFd> {
        let target = if path.is_empty() { "." } else { path };
        rustix::fs::openat2(
            self.0.as_fd(),
            target,
            flags | OFlags::CLOEXEC,
            mode,
            resolve_flags(),
        )
        .map_err(from_rustix)
    }

    fn parent(&self, path: &str) -> io::Result<(OwnedFd, String)> {
        let (parent, name) = split(path);
        Ok((
            self.at(parent, OFlags::PATH | OFlags::DIRECTORY, Mode::empty())?,
            name.to_owned(),
        ))
    }

    pub fn list(&self, path: &str) -> io::Result<Vec<Entry>> {
        let directory = self.at(path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?;
        let mut entries = vec![];
        for entry in Dir::read_from(&directory).map_err(io::Error::from)? {
            let entry = entry.map_err(io::Error::from)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "."
                || name == ".."
                || name.starts_with(".gm-")
                || (path.is_empty() && name == "lost+found")
            {
                continue;
            }
            let stat = match rustix::fs::statat(
                &directory,
                entry.file_name(),
                AtFlags::SYMLINK_NOFOLLOW,
            ) {
                Ok(stat) => stat,
                Err(_) => continue, // removed while listing
            };
            let kind = match FileType::from_raw_mode(stat.st_mode) {
                FileType::RegularFile => EntryKind::File,
                FileType::Directory => EntryKind::Directory,
                FileType::Symlink => EntryKind::Link,
                _ => EntryKind::Other,
            };
            entries.push(Entry {
                name,
                kind,
                size: stat.st_size as u64,
                modified: stat.st_mtime,
            });
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
        entries.sort_by(|a, b| {
            (b.kind == EntryKind::Directory)
                .cmp(&(a.kind == EntryKind::Directory))
                .then(a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    fn open_regular(&self, path: &str) -> io::Result<File> {
        let file = File::from(self.at(path, OFlags::RDONLY, Mode::empty())?);
        if !file.metadata()?.is_file() {
            return Err(invalid("not a regular file"));
        }
        Ok(file)
    }

    // Returns the text and a version (SHA-256) for conflict detection on save.
    pub fn read_text(&self, path: &str) -> io::Result<(String, String)> {
        let file = self.open_regular(path)?;
        if file.metadata()?.len() > MAX_TEXT_BYTES as u64 {
            return Err(invalid(
                "the file is too large to edit here; download it instead",
            ));
        }
        let mut bytes = vec![];
        file.take(MAX_TEXT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_TEXT_BYTES || bytes.contains(&0) {
            return Err(invalid(
                "the file is binary or too large to edit here; download it instead",
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| invalid("the file is not UTF-8 text; download it instead"))?;
        let version = version(text.as_bytes());
        Ok((text, version))
    }

    // Saves text atomically. `expected` is the version that was read; None creates a new file.
    pub fn write_text(&self, path: &str, text: &str, expected: Option<&str>) -> io::Result<String> {
        if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
            return Err(invalid("text is too large or contains NUL characters"));
        }
        let current = match self.open_regular(path) {
            Ok(mut file) => {
                let mut bytes = vec![];
                (&mut file)
                    .take(MAX_TEXT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                Some(version(&bytes))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if current.as_deref() != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the file changed since it was opened; reload it before saving",
            ));
        }
        let (parent, name) = self.parent(path)?;
        let temporary = format!("{UPLOAD_PREFIX}{}", uuid::Uuid::new_v4().simple());
        let fd = rustix::fs::openat2(
            &parent,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
            resolve_flags(),
        )
        .map_err(from_rustix)?;
        let mut file = File::from(fd);
        let written = file
            .write_all(text.as_bytes())
            .and_then(|_| file.sync_all());
        if let Err(error) = written.and_then(|_| {
            rustix::fs::renameat(&parent, temporary.as_str(), &parent, name.as_str())
                .map_err(from_rustix)
        }) {
            let _ = rustix::fs::unlinkat(&parent, temporary.as_str(), AtFlags::empty());
            return Err(error);
        }
        Ok(version(text.as_bytes()))
    }

    pub fn read_chunk(&self, path: &str, offset: u64, length: usize) -> io::Result<(Vec<u8>, u64)> {
        let file = self.open_regular(path)?;
        let size = file.metadata()?.len();
        let mut buffer = vec![0u8; length.min(MAX_CHUNK_BYTES)];
        let read = rustix::io::pread(&file, &mut buffer[..], offset).map_err(io::Error::from)?;
        buffer.truncate(read);
        Ok((buffer, size))
    }

    // Uploads go to a hidden staging file next to the destination and are renamed into place
    // with the last chunk, so readers never see a half-written file.
    pub fn write_chunk(
        &self,
        upload: uuid::Uuid,
        path: &str,
        offset: u64,
        data: &[u8],
        last: bool,
    ) -> io::Result<()> {
        if data.len() > MAX_CHUNK_BYTES {
            return Err(invalid("chunk too large"));
        }
        let (parent, name) = self.parent(path)?;
        let staging = format!("{UPLOAD_PREFIX}{}", upload.simple());
        // openat2 rejects a mode unless the file is being created.
        let (flags, mode) = if offset == 0 {
            (
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                Mode::from_raw_mode(0o644),
            )
        } else {
            (OFlags::WRONLY, Mode::empty())
        };
        let fd = rustix::fs::openat2(
            &parent,
            staging.as_str(),
            flags | OFlags::CLOEXEC,
            mode,
            resolve_flags(),
        )
        .map_err(from_rustix)?;
        let file = File::from(fd);
        if file.metadata()?.len() != offset {
            return Err(invalid("upload chunks arrived out of order"));
        }
        let result = (|| {
            let mut written = 0;
            while written < data.len() {
                written += rustix::io::pwrite(&file, &data[written..], offset + written as u64)
                    .map_err(io::Error::from)?;
            }
            if last {
                file.sync_all()?;
                if rustix::fs::statat(&parent, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .is_ok_and(|s| FileType::from_raw_mode(s.st_mode) == FileType::Directory)
                {
                    return Err(invalid("a folder with that name already exists"));
                }
                rustix::fs::renameat(&parent, staging.as_str(), &parent, name.as_str())
                    .map_err(from_rustix)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&parent, staging.as_str(), AtFlags::empty());
        }
        result
    }

    pub fn create_directory(&self, path: &str) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        rustix::fs::mkdirat(&parent, name.as_str(), Mode::from_raw_mode(0o755)).map_err(from_rustix)
    }

    // Never replaces an existing entry.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let (source, old) = self.parent(from)?;
        let (target, new) = self.parent(to)?;
        rustix::fs::renameat_with(
            &source,
            old.as_str(),
            &target,
            new.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(from_rustix)
    }

    pub fn delete(&self, path: &str) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        remove(&parent, &name, 0)
    }

    // Removes upload staging files left by interrupted uploads.
    pub fn clean_uploads(&self) -> io::Result<()> {
        clean(
            &self.at("", OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?,
            0,
        )
    }
}

fn version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// Deletes `name` inside `parent` without following links: directories are opened through the
// same no-symlink resolution and emptied first.
fn remove(parent: &OwnedFd, name: &str, depth: usize) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid("folder tree is too deep"));
    }
    let stat =
        rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return rustix::fs::unlinkat(parent, name, AtFlags::empty()).map_err(io::Error::from);
    }
    let directory = rustix::fs::openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve_flags(),
    )
    .map_err(from_rustix)?;
    let names: Vec<String> = Dir::read_from(&directory)
        .map_err(io::Error::from)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "." && n != "..")
        .collect();
    for child in names {
        remove(&directory, &child, depth + 1)?;
    }
    rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)
}

fn clean(directory: &OwnedFd, depth: usize) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries: Vec<(String, FileType)> = Dir::read_from(directory)
        .map_err(io::Error::from)?
        .filter_map(Result::ok)
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type()))
        .filter(|(n, _)| n != "." && n != "..")
        .collect();
    for (name, kind) in entries {
        if name.starts_with(UPLOAD_PREFIX) {
            let _ = rustix::fs::unlinkat(directory, name.as_str(), AtFlags::empty());
        } else if kind == FileType::Directory && name != "lost+found" {
            if let Ok(child) = rustix::fs::openat2(
                directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                resolve_flags(),
            ) {
                clean(&child, depth + 1)?;
            }
        }
    }
    Ok(())
}
