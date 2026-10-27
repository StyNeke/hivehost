use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityIssue {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EggRevision {
    pub name: String,
    pub author: String,
    pub description: String,
    pub images: Vec<(String, String)>,
    pub startup: String,
    pub stop: String,
    pub ready_when: String,
    pub installer: Installer,
    pub variables: Vec<Variable>,
    pub files: BTreeMap<String, FileTransform>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installer {
    pub script: String,
    pub container: String,
    pub entrypoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    pub description: String,
    pub env_variable: String,
    pub default_value: String,
    pub user_viewable: bool,
    pub user_editable: bool,
    pub rules: String,
    #[serde(default)]
    pub field_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransform {
    pub parser: String,
    pub find: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEgg {
    #[serde(rename = "_comment")]
    _comment: Option<String>,
    meta: Meta,
    #[serde(rename = "exported_at")]
    _exported_at: Option<String>,
    name: String,
    author: String,
    description: Option<String>,
    features: Option<Vec<String>>,
    docker_images: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    file_denylist: Vec<String>,
    startup: String,
    config: RawConfig,
    scripts: Scripts,
    variables: Vec<Variable>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    version: String,
    #[serde(rename = "update_url")]
    _update_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    files: String,
    startup: String,
    logs: String,
    stop: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scripts {
    installation: Installer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Readiness {
    done: String,
    // Hints for prompts such as the EULA; the EULA itself is handled by the "eula" feature.
    #[serde(default, rename = "userInteraction")]
    _user_interaction: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogSettings {
    custom: bool,
    #[serde(rename = "location")]
    _location: Option<String>,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> Vec<CompatibilityIssue> {
    vec![CompatibilityIssue {
        path: path.into(),
        message: message.into(),
    }]
}

pub fn normalize_egg(raw: &[u8]) -> Result<EggRevision, Vec<CompatibilityIssue>> {
    if raw.len() > 1024 * 1024 {
        return Err(issue("$", "egg exceeds 1 MiB import limit"));
    }
    let egg: RawEgg = serde_json::from_slice(raw).map_err(|e| issue("$", e.to_string()))?;
    if egg.meta.version != "PTDL_v2" {
        return Err(issue(
            "meta.version",
            "only PTDL_v2 is qualified for parsing",
        ));
    }
    if egg.name.trim().is_empty() || egg.name.len() > 200 {
        return Err(issue("name", "name must be 1–200 bytes"));
    }
    if !egg.file_denylist.is_empty() {
        return Err(issue(
            "file_denylist",
            "denylist patterns are not supported yet",
        ));
    }
    let features = egg.features.unwrap_or_default();
    for feature in &features {
        if !["eula", "java_version", "pid_limit"].contains(&feature.as_str()) {
            return Err(issue("features", format!("unsupported feature: {feature}")));
        }
    }
    let mut images = Vec::new();
    if egg.docker_images.is_empty() || egg.docker_images.len() > 32 {
        return Err(issue("docker_images", "supply between 1 and 32 images"));
    }
    for (label, value) in egg.docker_images {
        let image = value
            .as_str()
            .filter(|v| valid_image(v))
            .ok_or_else(|| issue("docker_images", "invalid image reference"))?;
        images.push((label, image.to_owned()));
    }
    if !valid_image(&egg.scripts.installation.container) {
        return Err(issue(
            "scripts.installation.container",
            "invalid installer image",
        ));
    }
    if egg.scripts.installation.script.len() > 256 * 1024
        || egg.scripts.installation.script.contains('\0')
    {
        return Err(issue(
            "scripts.installation.script",
            "installer script is invalid or too large",
        ));
    }
    let entrypoint = &egg.scripts.installation.entrypoint;
    if entrypoint.is_empty()
        || !entrypoint
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    {
        return Err(issue(
            "scripts.installation.entrypoint",
            "entrypoint must be one executable path",
        ));
    }
    if egg.startup.trim().is_empty()
        || egg.startup.contains('\0')
        || egg.config.stop.trim().is_empty()
        || egg.config.stop.contains(['\0', '\r', '\n'])
    {
        return Err(issue(
            "config.stop/startup",
            "startup and stop command must be nonempty and valid",
        ));
    }
    // "{}" and {"custom": false} both mean the console stream is the log; custom log files are not qualified.
    let logs: Option<LogSettings> = if egg.config.logs.trim() == "{}" {
        None
    } else {
        Some(
            serde_json::from_str(&egg.config.logs)
                .map_err(|e| issue("config.logs", e.to_string()))?,
        )
    };
    if logs.is_some_and(|logs| logs.custom) {
        return Err(issue(
            "config.logs",
            "custom log configuration is unsupported",
        ));
    }
    let files: BTreeMap<String, FileTransform> = serde_json::from_str(&egg.config.files)
        .map_err(|e| issue("config.files", e.to_string()))?;
    for (path, transform) in &files {
        if path.starts_with('/')
            || path.contains(['\\', '\0', ':'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(issue(
                "config.files",
                "file path must stay within the server root",
            ));
        }
        if transform.parser != "properties" {
            return Err(issue(
                "config.files",
                format!("unsupported parser: {}", transform.parser),
            ));
        }
        for (key, value) in &transform.find {
            if key.is_empty()
                || key.contains(['\n', '\r', '\0', '='])
                || value.contains(['\n', '\r', '\0'])
            {
                return Err(issue("config.files", "invalid properties replacement"));
            }
        }
    }
    let readiness: Readiness = serde_json::from_str(&egg.config.startup)
        .map_err(|e| issue("config.startup", e.to_string()))?;
    if readiness.done.is_empty() || readiness.done.contains('\0') {
        return Err(issue(
            "config.startup.done",
            "readiness marker must be nonempty",
        ));
    }
    if egg.variables.len() > 128 {
        return Err(issue("variables", "maximum 128 variables"));
    }
    let mut names = std::collections::HashSet::new();
    for (index, variable) in egg.variables.iter().enumerate() {
        let path = format!("variables[{index}]");
        let name = &variable.env_variable;
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || name.as_bytes()[0].is_ascii_digit()
            || !names.insert(name.clone())
        {
            return Err(issue(
                &path,
                "environment names must be unique uppercase identifiers",
            ));
        }
        if [
            "SERVER_MEMORY",
            "SERVER_IP",
            "SERVER_PORT",
            "STARTUP",
            "P_SERVER_UUID",
        ]
        .contains(&name.as_str())
        {
            return Err(issue(
                &path,
                "system environment variable cannot be overridden",
            ));
        }
        if variable
            .field_type
            .as_deref()
            .is_some_and(|field| field != "text")
        {
            return Err(issue(&path, "unsupported field_type"));
        }
        parse_rules(&variable.rules).map_err(|e| issue(format!("{path}.rules"), e))?;
        if !variable.default_value.is_empty() {
            validate_variable(variable, &variable.default_value)
                .map_err(|e| issue(format!("{path}.default_value"), e))?;
        }
    }
    Ok(EggRevision {
        name: egg.name,
        author: egg.author,
        description: egg.description.unwrap_or_default(),
        images,
        startup: egg.startup,
        stop: egg.config.stop,
        ready_when: readiness.done,
        installer: egg.scripts.installation,
        variables: egg.variables,
        files,
        features,
    })
}

fn valid_image(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 512
        && image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.:@-".contains(&b))
}

#[derive(Default)]
struct Rules {
    required: bool,
    nullable: bool,
    max: Option<usize>,
    min: Option<usize>,
    choices: Option<Vec<String>>,
    pattern: Option<regex::Regex>,
}

fn parse_rules(input: &str) -> Result<Rules, String> {
    let mut rules = Rules::default();
    let (simple, pattern) = if let Some(at) = input.find("regex:") {
        if at != 0 && input.as_bytes()[at - 1] != b'|' {
            return Err("invalid regex rule".into());
        }
        (&input[..at].trim_end_matches('|'), Some(&input[at + 6..]))
    } else {
        (&input, None)
    };
    let mut seen = std::collections::HashSet::new();
    for rule in simple.split('|').filter(|r| !r.is_empty()) {
        if !seen.insert(rule.split(':').next().unwrap_or(rule)) {
            return Err(format!("repeated validation rule: {rule}"));
        }
        match rule {
            "required" => rules.required = true,
            "nullable" => rules.nullable = true,
            "string" => {}
            _ if rule.starts_with("max:") && rules.max.is_none() => {
                rules.max = Some(rule[4..].parse().map_err(|_| "invalid max rule")?)
            }
            _ if rule.starts_with("min:") && rules.min.is_none() => {
                rules.min = Some(rule[4..].parse().map_err(|_| "invalid min rule")?)
            }
            // Laravel's between:a,b on strings bounds the length inclusively.
            _ if rule.starts_with("between:") => {
                let (low, high) = rule[8..].split_once(',').ok_or("invalid between rule")?;
                let low: usize = low.parse().map_err(|_| "invalid between rule")?;
                let high: usize = high.parse().map_err(|_| "invalid between rule")?;
                if low > high || rules.min.is_some() || rules.max.is_some() {
                    return Err("invalid between rule".into());
                }
                rules.min = Some(low);
                rules.max = Some(high);
            }
            _ if rule.starts_with("in:") => {
                rules.choices = Some(rule[3..].split(',').map(str::to_owned).collect())
            }
            _ => return Err(format!("unsupported validation rule: {rule}")),
        }
    }
    if let Some(pattern) = pattern {
        // Only terminal /pattern/ rules are qualified; unsupported modifiers/rule tails fail closed.
        let source = pattern
            .strip_prefix('/')
            .and_then(|p| p.strip_suffix('/'))
            .ok_or("regex must be a terminal /pattern/ rule")?;
        let mut escaped = false;
        for byte in source.bytes() {
            if byte == b'/' && !escaped {
                return Err("regex must end before any following rule".into());
            }
            escaped = byte == b'\\' && !escaped;
        }
        if source.len() > 1024 {
            return Err("regex too large".into());
        }
        rules.pattern = Some(
            regex::RegexBuilder::new(source)
                .size_limit(1024 * 1024)
                .build()
                .map_err(|e| format!("unsupported regex: {e}"))?,
        );
    }
    Ok(rules)
}

pub fn validate_variable(variable: &Variable, value: &str) -> Result<(), String> {
    let rules = parse_rules(&variable.rules)?;
    if value.len() > 4096 || value.contains('\0') {
        return Err("variable exceeds supported size or contains NUL".into());
    }
    if rules.required && value.trim().is_empty() {
        return Err("value is required".into());
    }
    if value.is_empty() && rules.nullable {
        return Ok(());
    }
    if rules.max.is_some_and(|max| value.chars().count() > max) {
        return Err("value exceeds maximum length".into());
    }
    if rules.min.is_some_and(|min| value.chars().count() < min) {
        return Err("value is shorter than the minimum length".into());
    }
    if rules
        .choices
        .as_ref()
        .is_some_and(|choices| !choices.iter().any(|v| v == value))
    {
        return Err("value is not an allowed choice".into());
    }
    if rules
        .pattern
        .as_ref()
        .is_some_and(|pattern| !pattern.is_match(value))
    {
        return Err("value does not match required pattern".into());
    }
    Ok(())
}
