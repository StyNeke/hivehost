use manager_agent::engine::{console_lines, decode_chunked, demultiplex, valid_container_name};

#[test]
fn chunked_bodies_decode_and_malformed_framing_fails() {
    assert_eq!(
        decode_chunked(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n", 64).unwrap(),
        b"hello world"
    );
    assert_eq!(
        decode_chunked(b"5;ext=1\r\nhello\r\n0\r\n\r\n", 64).unwrap(),
        b"hello"
    );
    assert!(decode_chunked(b"5\r\nhel", 64).is_err());
    assert!(decode_chunked(b"zz\r\nhello\r\n0\r\n\r\n", 64).is_err());
    assert!(decode_chunked(b"5\r\nhelloXX6\r\n", 64).is_err());
    assert!(decode_chunked(b"5\r\nhello\r\n0\r\n\r\n", 4).is_err());
}

#[test]
fn log_frames_are_demultiplexed_in_order_and_truncation_fails() {
    let mut frames = vec![1, 0, 0, 0, 0, 0, 0, 3];
    frames.extend_from_slice(b"ab\n");
    frames.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 2]);
    frames.extend_from_slice(b"c\n");
    assert_eq!(demultiplex(&frames).unwrap(), b"ab\nc\n");
    assert!(demultiplex(&frames[..frames.len() - 1]).is_err());
    assert!(demultiplex(&[9, 0, 0, 0, 0, 0, 0, 0]).is_err());
}

#[test]
fn console_text_drops_escape_and_control_sequences() {
    let lines = console_lines(b"\x1b[1m\x1b[33mcontainer@host~ \x1b[0mjava\r\n\x1b]0;title\x07Done (1.2s)! For help\x08\n");
    assert_eq!(lines, vec!["container@host~ java", "Done (1.2s)! For help"]);
    assert_eq!(console_lines(b"\xff\xfeok\n"), vec!["\u{fffd}\u{fffd}ok"]);
}

#[test]
fn container_names_are_restricted() {
    assert!(valid_container_name("gm-4f1c"));
    for name in ["", "a/b", "a b", "../x", "a?b"] {
        assert!(!valid_container_name(name));
    }
}
