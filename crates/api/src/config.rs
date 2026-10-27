use std::net::SocketAddr;

pub fn validate_listener(bind: &str, origin: &str) -> Result<SocketAddr, &'static str> {
    let address: SocketAddr = bind.parse().map_err(|_| "invalid bind address")?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("the API must bind to a fixed loopback port behind private HTTPS");
    }
    let uri: axum::http::Uri = origin.parse().map_err(|_| "invalid origin")?;
    if uri.scheme_str() != Some("https")
        || uri.authority().is_none()
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        || uri
            .path_and_query()
            .is_some_and(|path| path.as_str() != "/" && !path.as_str().is_empty())
        || origin.ends_with('/')
    {
        return Err("origin must be an HTTPS origin without credentials, path, or trailing slash");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_is_loopback_and_browser_origin_is_a_bare_https_origin() {
        assert!(validate_listener("127.0.0.1:19090", "https://panel.example.test:8443").is_ok());
        assert!(validate_listener("[::1]:19090", "https://panel.example.test").is_ok());
        assert!(validate_listener("0.0.0.0:19090", "https://panel.example.test").is_err());
        assert!(validate_listener("[::]:19090", "https://panel.example.test").is_err());
        assert!(validate_listener("127.0.0.1:0", "https://panel.example.test").is_err());
        assert!(validate_listener("127.0.0.1:19090", "http://panel.example.test").is_err());
        assert!(validate_listener("127.0.0.1:19090", "https://panel.example.test/path").is_err());
        assert!(
            validate_listener("127.0.0.1:19090", "https://user:secret@panel.example.test").is_err()
        );
    }
}
