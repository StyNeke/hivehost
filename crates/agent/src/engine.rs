// Minimal Docker Engine API client over the local Unix socket. Used for the two operations the
// Docker CLI cannot do safely: writing to a container's stdin without a client that never exits,
// and reading ordered stdout/stderr logs with a hard size bound.
use std::{io, path::Path, time::Duration};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn valid_container_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// Decodes an HTTP/1.1 chunked body. Rejects truncated or malformed framing.
pub fn decode_chunked(mut input: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| invalid("truncated chunk header"))?;
        let header = std::str::from_utf8(&input[..line_end]).map_err(|_| invalid("bad chunk"))?;
        let size_text = header.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| invalid("bad chunk size"))?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err(invalid("truncated chunk"));
        }
        if output.len() + size > limit {
            return Err(invalid("response exceeds limit"));
        }
        output.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
}

// Docker multiplexes stdout/stderr for non-TTY containers as [stream, 0, 0, 0, len_be32, payload].
pub fn demultiplex(mut input: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len());
    while !input.is_empty() {
        if input.len() < 8 || !matches!(input[0], 0..=2) || input[1..4] != [0, 0, 0] {
            return Err(invalid("malformed log frame"));
        }
        let size = u32::from_be_bytes([input[4], input[5], input[6], input[7]]) as usize;
        if input.len() < 8 + size {
            return Err(invalid("truncated log frame"));
        }
        output.extend_from_slice(&input[8..8 + size]);
        input = &input[8 + size..];
    }
    Ok(output)
}

// Removes terminal escape sequences and control characters so console text is plain and safe.
pub fn console_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        // CSI: parameters/intermediates until a final byte in @..~
                        for next in chars.by_ref() {
                            if ('@'..='~').contains(&next) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        chars.next();
                        // OSC: until BEL or ST
                        while let Some(next) = chars.next() {
                            if next == '\u{7}' || (next == '\u{1b}' && chars.peek() == Some(&'\\'))
                            {
                                if next == '\u{1b}' {
                                    chars.next();
                                }
                                break;
                            }
                        }
                    }
                    _ => {
                        chars.next();
                    }
                }
            } else if c == '\t' || !c.is_control() {
                line.push(c);
            }
        }
        lines.push(line);
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

fn parse_head(head: &[u8]) -> io::Result<(u16, bool, Option<usize>)> {
    let text = std::str::from_utf8(head).map_err(|_| invalid("bad response head"))?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or_else(|| invalid("missing status"))?;
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("bad status"))?;
    let mut chunked = false;
    let mut length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.eq_ignore_ascii_case("chunked");
            } else if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.parse().map_err(|_| invalid("bad length"))?);
            }
        }
    }
    Ok((status, chunked, length))
}

#[cfg(unix)]
async fn read_head(stream: &mut tokio::net::UnixStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            buffer.truncate(end);
            return Ok((buffer, rest));
        }
        if buffer.len() > 64 * 1024 {
            return Err(invalid("response head too large"));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(invalid("connection closed before response"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(unix)]
pub async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    limit: usize,
    deadline: Duration,
) -> io::Result<Response> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(deadline, async {
        let mut stream = tokio::net::UnixStream::connect(socket).await?;
        stream
            .write_all(
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: docker\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let (head, mut body) = read_head(&mut stream).await?;
        let (status, chunked, length) = parse_head(&head)?;
        let mut chunk = [0u8; 16384];
        let bound = if chunked { limit + limit / 8 + 1024 } else { limit };
        loop {
            if length.is_some_and(|length| body.len() >= length) {
                break;
            }
            if body.len() > bound {
                return Err(invalid("response exceeds limit"));
            }
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
        let body = if chunked {
            decode_chunked(&body, limit)?
        } else {
            if body.len() > limit || length.is_some_and(|length| body.len() < length) {
                return Err(invalid("response length mismatch"));
            }
            body.truncate(length.unwrap_or(body.len()));
            body
        };
        Ok(Response { status, body })
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Docker API timed out"))?
}

// Writes to a running container's stdin through a hijacked attach stream, then disconnects.
// Containers are created with OpenStdin and StdinOnce=false, so disconnecting keeps stdin open.
#[cfg(unix)]
pub async fn write_stdin(socket: &Path, container: &str, data: &[u8]) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    if !valid_container_name(container) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid container name",
        ));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = tokio::net::UnixStream::connect(socket).await?;
        stream
            .write_all(
                format!(
                    "POST /containers/{container}/attach?stream=1&stdin=1 HTTP/1.1\r\nHost: docker\r\nConnection: Upgrade\r\nUpgrade: tcp\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let (head, _) = read_head(&mut stream).await?;
        let (status, _, _) = parse_head(&head)?;
        if status != 101 && status != 200 {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "container is not accepting input",
            ));
        }
        stream.write_all(data).await?;
        stream.flush().await?;
        // Give the daemon a moment to forward the bytes before the connection closes.
        tokio::time::sleep(Duration::from_millis(200)).await;
        stream.shutdown().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Docker attach timed out"))?
}

#[cfg(unix)]
pub async fn logs(
    socket: &Path,
    container: &str,
    query: &str,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if !valid_container_name(container) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid container name",
        ));
    }
    let response = request(
        socket,
        "GET",
        &format!("/containers/{container}/logs?stdout=1&stderr=1&{query}"),
        limit,
        Duration::from_secs(10),
    )
    .await?;
    match response.status {
        200 => demultiplex(&response.body),
        404 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "container not found",
        )),
        _ => Err(io::Error::other("Docker refused the log request")),
    }
}
