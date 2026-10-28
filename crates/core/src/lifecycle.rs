// Crash restarts: at most MAX_RESTARTS within RESTART_WINDOW_SECS, then the server stays stopped.
// A requested stop, failed install or host shutdown never reaches this policy.
pub const MAX_RESTARTS: u32 = 3;
pub const RESTART_WINDOW_SECS: i64 = 600;
const BASE_DELAY_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashDecision {
    Restart { delay_secs: u64 },
    GiveUp,
}

// `history` holds the times of restarts already granted and is persisted by the caller, so the
// limit survives agent restarts. Timestamps after `now` (a clock that went backwards) still count.
pub fn on_unexpected_exit(history: &mut Vec<i64>, now: i64) -> CrashDecision {
    history.retain(|at| now.saturating_sub(*at) < RESTART_WINDOW_SECS);
    if history.len() >= MAX_RESTARTS as usize {
        return CrashDecision::GiveUp;
    }
    history.push(now);
    CrashDecision::Restart {
        delay_secs: BASE_DELAY_SECS << (history.len() - 1),
    }
}

// A recognised cause of an exit, read from the last console lines. `permanent` failures recur on
// every start (wrong Java, refused EULA, port taken), so restarting them only hides the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitCause {
    pub message: String,
    pub permanent: bool,
}

pub fn explain_exit(lines: &[String]) -> Option<ExitCause> {
    for line in lines.iter().rev() {
        if let Some(rest) = line.split("class file version ").nth(1) {
            let needed = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|v| (45..200).contains(v));
            if line.contains("UnsupportedClassVersionError") || needed.is_some() {
                let java =
                    needed.map_or("a newer Java".to_string(), |v| format!("Java {}", v - 44));
                return Some(ExitCause {
                    message: format!(
                        "This game version needs {java} or newer. Choose a matching runtime image in Settings, then start the server."
                    ),
                    permanent: true,
                });
            }
        }
        if line.contains("FAILED TO BIND TO PORT") || line.contains("Address already in use") {
            return Some(ExitCause {
                message: "The game could not listen on its port because something else uses it."
                    .into(),
                permanent: true,
            });
        }
        if line.contains("You need to agree to the EULA") {
            return Some(ExitCause {
                message: "The Minecraft EULA has not been accepted for this server.".into(),
                permanent: true,
            });
        }
        if line.contains("java.lang.OutOfMemoryError") {
            return Some(ExitCause {
                message: "The game ran out of memory. Raise the memory limit in Settings or remove heavy mods.".into(),
                permanent: false,
            });
        }
    }
    None
}
