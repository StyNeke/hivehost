#![cfg(target_os = "linux")]

use manager_agent::runtime::{
    ContainerPlan, environment, installer_args, parse_docker_time, runtime_args, server_memory_mib,
    valid_command, valid_extension_name, valid_remote_file, validate_spec,
};
use manager_core::{
    capacity::Resources,
    eggs::{FileTransform, Installer},
    host::{PortBinding, Transport},
};
use manager_protocol::{RemoteFile, ServerSpec};
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

const GIB: u64 = 1024 * 1024 * 1024;

fn spec() -> ServerSpec {
    let mut files = BTreeMap::new();
    files.insert(
        "server.properties".to_string(),
        FileTransform {
            parser: "properties".into(),
            find: [(
                "server-port".to_string(),
                "{{server.build.default.port}}".to_string(),
            )]
            .into(),
        },
    );
    ServerSpec {
        server_id: Uuid::new_v4(),
        image: "ghcr.io/ptero-eggs/yolks:java_21".into(),
        startup: "java -Xmx{{SERVER_MEMORY}}M -jar {{SERVER_JARFILE}}".into(),
        stop: "stop".into(),
        ready_when: ")! For help, type ".into(),
        installer: Installer {
            script: "#!/bin/bash\necho hi".into(),
            container: "eclipse-temurin:21-jdk-jammy".into(),
            entrypoint: "bash".into(),
        },
        variables: [("SERVER_JARFILE".to_string(), "server.jar".to_string())].into(),
        limits: Resources {
            memory_bytes: 4 * GIB,
            disk_bytes: 10 * GIB,
            cpu_millis: 1500,
        },
        binding: PortBinding {
            ip: "127.0.0.1".parse().unwrap(),
            port: 25600,
            transport: Transport::Tcp,
        },
        files,
        accept_eula: true,
    }
}

fn strings(args: Vec<std::ffi::OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.into_string().unwrap()).collect()
}

#[test]
fn game_containers_are_unprivileged_bounded_and_mount_only_their_volume() {
    let spec = spec();
    let volume = Path::new("/var/lib/game-manager/volumes/x");
    let plan = ContainerPlan {
        spec: &spec,
        uid: 200001,
        image: "ghcr.io/ptero-eggs/yolks@sha256:abc",
        volume,
        network: "gm-games",
        proxy: None,
        stop_grace_secs: 120,
    };
    let args = strings(runtime_args(&plan));
    let joined = args.join(" ");
    for required in [
        "--user 200001:200001",
        "--cap-drop ALL",
        "--security-opt no-new-privileges",
        "--restart no",
        "--read-only",
        "--interactive",
        "--network gm-games",
        "--publish 127.0.0.1:25600:25600/tcp",
        "--memory 4294967296",
        "--memory-swap 4294967296",
        "--cpus 1.500",
        "--pids-limit 512",
        "type=bind,source=/var/lib/game-manager/volumes/x,target=/home/container",
    ] {
        assert!(joined.contains(required), "missing {required}");
    }
    for forbidden in [
        "--privileged",
        "--cap-add",
        "--pid",
        "--ipc",
        "--attach",
        "--volume",
        "--device",
        "--userns",
    ] {
        assert!(
            !args
                .iter()
                .any(|a| a == forbidden || a.starts_with(&format!("{forbidden}="))),
            "unexpected {forbidden}"
        );
    }
    assert!(!joined.contains("docker.sock") && !joined.contains("--network host"));
    assert_eq!(
        args.iter().filter(|a| a.starts_with("type=bind")).count(),
        1
    );
    assert_eq!(
        &args[args.len() - 2..],
        ["--", "ghcr.io/ptero-eggs/yolks@sha256:abc"]
    );
    assert!(args.contains(&format!("STARTUP={}", spec.startup)));
}

#[test]
fn installers_get_only_package_manager_capabilities_and_a_read_only_script() {
    let spec = spec();
    let plan = ContainerPlan {
        spec: &spec,
        uid: 200001,
        image: "x",
        volume: Path::new("/v"),
        network: "gm-games",
        proxy: Some(&("172.30.250.1".to_string(), 3128)),
        stop_grace_secs: 120,
    };
    let args = strings(installer_args(&plan, Path::new("/s")));
    let joined = args.join(" ");
    assert!(joined.contains("type=bind,source=/v,target=/mnt/server"));
    assert!(joined.contains("type=bind,source=/s,target=/mnt/install,readonly"));
    assert!(joined.contains("--entrypoint bash"));
    let added: Vec<_> = args
        .windows(2)
        .filter(|w| w[0] == "--cap-add")
        .map(|w| w[1].as_str())
        .collect();
    assert_eq!(
        added,
        [
            "CHOWN",
            "DAC_OVERRIDE",
            "FOWNER",
            "FSETID",
            "SETUID",
            "SETGID"
        ]
    );
    assert!(!joined.contains("--privileged") && !joined.contains("docker.sock"));
    assert!(args.contains(&"HTTPS_PROXY=http://172.30.250.1:3128".to_string()));
    assert!(joined.contains("-Dhttps.proxyHost=172.30.250.1 -Dhttps.proxyPort=3128"));
    assert_eq!(
        &args[args.len() - 3..],
        [
            "--",
            "eclipse-temurin:21-jdk-jammy",
            "/mnt/install/install.sh"
        ]
    );
}

#[test]
fn specs_that_could_inject_arguments_or_escape_are_rejected() {
    assert!(validate_spec(&spec()).is_ok());
    type Mutation = Box<dyn Fn(&mut ServerSpec)>;
    let cases: Vec<Mutation> = vec![
        Box::new(|s| s.image = "--privileged".into()),
        Box::new(|s| s.image = "a b".into()),
        Box::new(|s| s.installer.container = "-v/:/host".into()),
        Box::new(|s| s.installer.entrypoint = "-c".into()),
        Box::new(|s| s.installer.entrypoint = "bash -c".into()),
        Box::new(|s| s.startup = "java\nrm".into()),
        Box::new(|s| s.stop = "stop\n".into()),
        Box::new(|s| {
            s.variables.insert("STARTUP".into(), "x".into());
        }),
        Box::new(|s| {
            s.variables.insert("JAVA_TOOL_OPTIONS".into(), "x".into());
        }),
        Box::new(|s| {
            s.variables.insert("lower".into(), "x".into());
        }),
        Box::new(|s| {
            s.variables.insert("NUL".into(), "a\0b".into());
        }),
        Box::new(|s| s.binding.ip = "8.8.8.8".parse().unwrap()),
        Box::new(|s| s.binding.port = 0),
        Box::new(|s| s.limits.memory_bytes = 1),
        Box::new(|s| s.limits.disk_bytes = 1),
        Box::new(|s| s.limits.memory_bytes = u64::MAX),
        Box::new(|s| {
            s.files.insert(
                "../etc/passwd".into(),
                FileTransform {
                    parser: "properties".into(),
                    find: BTreeMap::new(),
                },
            );
        }),
        Box::new(|s| {
            s.files.insert(
                "config.yml".into(),
                FileTransform {
                    parser: "yaml".into(),
                    find: BTreeMap::new(),
                },
            );
        }),
    ];
    for (index, case) in cases.iter().enumerate() {
        let mut candidate = spec();
        case(&mut candidate);
        assert!(
            validate_spec(&candidate).is_err(),
            "case {index} should be rejected"
        );
    }
}

#[test]
fn environment_puts_agent_values_last_and_memory_leaves_headroom() {
    let env = environment(&spec(), None);
    assert!(env.contains(&("SERVER_PORT".to_string(), "25600".to_string())));
    assert!(env.contains(&("SERVER_IP".to_string(), "0.0.0.0".to_string())));
    assert!(!env.iter().any(|(k, _)| k.contains("PROXY")));
    assert_eq!(server_memory_mib(4 * GIB), 3481);
    assert_eq!(server_memory_mib(512 * 1024 * 1024), 435);
    assert_eq!(server_memory_mib(32 * GIB), 31744);
}

#[test]
fn docker_times_parse_and_invalid_values_are_ignored() {
    assert_eq!(parse_docker_time("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_docker_time("2026-10-27T22:51:03.123456789Z"),
        Some(1793141463)
    );
    assert_eq!(parse_docker_time("2024-02-29T12:00:00Z"), Some(1709208000));
    for bad in [
        "0001-01-01T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-10-27T22:51:03+01:00",
        "garbage",
        "",
    ] {
        assert_eq!(parse_docker_time(bad), None, "{bad}");
    }
}

#[test]
fn commands_and_mod_files_are_strictly_validated() {
    assert!(valid_command("player bot1 spawn"));
    for bad in ["", "   ", "stop\nop me", "say \u{1b}[2J", &"x".repeat(513)] {
        assert!(!valid_command(bad), "{bad:?}");
    }
    assert!(valid_extension_name(
        "fabric-carpet-1.21.4-1.4.161+v241203.jar"
    ));
    for bad in [
        ".hidden.jar",
        "../x.jar",
        "x.zip",
        "a b.jar",
        ".jar",
        "x/y.jar",
    ] {
        assert!(!valid_extension_name(bad), "{bad}");
    }
    let file = RemoteFile {
        url: "https://cdn.modrinth.com/data/TQTTVgYE/versions/abc/carpet.jar".into(),
        sha512: "a".repeat(128),
        size: 1024,
        filename: "carpet.jar".into(),
    };
    assert!(valid_remote_file(&file));
    for bad in [
        RemoteFile {
            url: "http://cdn.modrinth.com/data/x.jar".into(),
            ..file.clone()
        },
        RemoteFile {
            url: "https://evil.example/data/x.jar".into(),
            ..file.clone()
        },
        RemoteFile {
            url: "https://cdn.modrinth.com.evil.example/data/x.jar".into(),
            ..file.clone()
        },
        RemoteFile {
            url: "https://cdn.modrinth.com/data/../../x.jar".into(),
            ..file.clone()
        },
        RemoteFile {
            sha512: "z".repeat(128),
            ..file.clone()
        },
        RemoteFile {
            size: 0,
            ..file.clone()
        },
        RemoteFile {
            size: 1 << 40,
            ..file.clone()
        },
        RemoteFile {
            filename: "../carpet.jar".into(),
            ..file.clone()
        },
    ] {
        assert!(!valid_remote_file(&bad), "{bad:?}");
    }
}
