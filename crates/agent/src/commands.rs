use std::{ffi::OsString, io, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

pub async fn read_limited(reader: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    if limit == 0 || limit > 8 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid command output limit",
        ));
    }
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "command output limit exceeded",
        ));
    }
    Ok(bytes)
}

// A short single-line excerpt of a failed command's stderr, without control characters.
pub fn error_excerpt(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let mut excerpt: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    if excerpt.is_empty() {
        excerpt.push_str("no error output");
    }
    excerpt
}

pub async fn run_checked(
    program: &Path,
    arguments: &[OsString],
    deadline: Duration,
    output_limit: usize,
) -> io::Result<Vec<u8>> {
    if deadline.is_zero() || output_limit == 0 || output_limit > 8 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid host command bounds",
        ));
    }
    let mut command = Command::new(program);
    #[cfg(target_os = "linux")]
    command.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    command
        .args(arguments)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("stderr pipe unavailable"))?;
    let outcome = tokio::time::timeout(deadline, async {
        let (output, errors) = tokio::try_join!(
            read_limited(stdout, output_limit),
            read_limited(stderr, 16 * 1024)
        )?;
        let status = child.wait().await?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "host command exited unsuccessfully: {}",
                error_excerpt(&errors)
            )));
        }
        Ok(output)
    })
    .await;
    match outcome {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "host command deadline exceeded",
            ))
        }
    }
}
