use manager_agent::commands::{read_limited, run_checked};
use std::{ffi::OsString, io::ErrorKind, path::Path, time::Duration};

#[tokio::test]
async fn command_output_is_bounded_before_parsing() {
    assert_eq!(read_limited(&b"hello"[..], 5).await.unwrap(), b"hello");
    assert_eq!(
        read_limited(&b"too large"[..], 3).await.unwrap_err().kind(),
        ErrorKind::InvalidData
    );
    assert!(read_limited(&b""[..], usize::MAX).await.is_err());
}

#[tokio::test]
async fn unsuccessful_commands_do_not_look_like_successful_inventory() {
    #[cfg(windows)]
    let (program, arguments) = (
        Path::new("cmd.exe"),
        vec![OsString::from("/C"), OsString::from("exit /b 7")],
    );
    #[cfg(not(windows))]
    let (program, arguments) = (
        Path::new("/bin/sh"),
        vec![OsString::from("-c"), OsString::from("exit 7")],
    );
    assert!(
        run_checked(program, &arguments, Duration::from_secs(5), 1024)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stalled_commands_are_terminated_at_the_deadline() {
    #[cfg(windows)]
    let (program, arguments) = (
        Path::new("powershell.exe"),
        vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from("[System.Threading.Thread]::Sleep(10000)"),
        ],
    );
    #[cfg(not(windows))]
    let (program, arguments) = (Path::new("/bin/sleep"), vec![OsString::from("10")]);
    assert_eq!(
        run_checked(program, &arguments, Duration::from_millis(100), 1024)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::TimedOut
    );
}
