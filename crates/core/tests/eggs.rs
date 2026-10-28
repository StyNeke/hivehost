use manager_core::eggs::{normalize_egg, validate_variable};
use serde_json::{Value, json};

const JAVA: &[u8] = include_bytes!("../../../tests/fixtures/eggs/java.json");
const BEDROCK: &[u8] = include_bytes!("../../../tests/fixtures/eggs/bedrock.json");

#[test]
fn pinned_java_and_bedrock_normalize_without_running_scripts() {
    let java = normalize_egg(JAVA).unwrap();
    assert_eq!(java.name, "Paper");
    assert_eq!(java.images[0].0, "Java 25");
    assert_eq!(java.stop, "stop");
    assert!(java.startup.contains("{{SERVER_JARFILE}}"));
    assert_eq!(java.files["server.properties"].parser, "properties");
    assert!(!java.installer.script.is_empty());
    let bedrock = normalize_egg(BEDROCK).unwrap();
    assert_eq!(bedrock.name, "Vanilla Bedrock");
    assert_eq!(bedrock.ready_when, "Server started");
    for egg in [java, bedrock] {
        for variable in &egg.variables {
            validate_variable(variable, &variable.default_value).unwrap();
        }
    }
}

#[test]
fn unsupported_features_and_parsers_fail_with_actionable_paths() {
    let mut raw: Value = serde_json::from_slice(JAVA).unwrap();
    raw["config"]["files"] = json!(r#"{"server.yml":{"parser":"yaml","find":{"port":"25565"}}}"#);
    let issues = normalize_egg(&serde_json::to_vec(&raw).unwrap()).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.path.contains("config.files") && i.message.contains("parser"))
    );
    raw = serde_json::from_slice(JAVA).unwrap();
    raw["variables"][0]["rules"] = json!("required|custom_rule");
    let issues = normalize_egg(&serde_json::to_vec(&raw).unwrap()).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.path.contains("variables") && i.message.contains("custom_rule"))
    );
    raw = serde_json::from_slice(JAVA).unwrap();
    raw["privileged"] = json!(true);
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw = serde_json::from_slice(JAVA).unwrap();
    raw["meta"]["version"] = json!("unknown");
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
}

#[test]
fn variables_reject_invalid_jar_names_enums_and_excessive_length() {
    let java = normalize_egg(JAVA).unwrap();
    let jar = java
        .variables
        .iter()
        .find(|v| v.env_variable == "SERVER_JARFILE")
        .unwrap();
    assert!(validate_variable(jar, "server.jar").is_ok());
    assert!(validate_variable(jar, "../evil.jar").is_err());
    assert!(validate_variable(jar, "server.jar;command").is_err());
    let bedrock = normalize_egg(BEDROCK).unwrap();
    let mode = bedrock
        .variables
        .iter()
        .find(|v| v.env_variable == "GAMEMODE")
        .unwrap();
    assert!(validate_variable(mode, "creative").is_ok());
    assert!(validate_variable(mode, "unrecognized").is_err());
    let version = &java.variables[0];
    assert!(validate_variable(version, &"x".repeat(21)).is_err());
}

#[test]
fn malformed_oversized_or_escaping_configuration_is_rejected() {
    assert!(normalize_egg(b"invalid JSON").is_err());
    assert!(normalize_egg(&vec![b' '; 1024 * 1024 + 1]).is_err());
    let mut raw: Value = serde_json::from_slice(JAVA).unwrap();
    raw["config"]["files"] = json!(r#"{"../server.properties":{"parser":"properties","find":{}}}"#);
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw = serde_json::from_slice(JAVA).unwrap();
    raw["variables"][1]["env_variable"] = raw["variables"][0]["env_variable"].clone();
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
}

#[test]
fn unsupported_or_repeated_rules_cannot_silently_weaken_validation() {
    let egg = normalize_egg(JAVA).unwrap();
    let mut variable = egg.variables[0].clone();
    variable.rules = "string|max:3|max:20".into();
    assert!(validate_variable(&variable, "four").is_err());
    variable.rules = "regex:/^x$/|regex:/^y$/".into();
    assert!(validate_variable(&variable, "x").is_err());
    variable.rules = "regex:/^(yes|no)$/".into();
    assert!(validate_variable(&variable, "yes").is_ok());
    assert!(validate_variable(&variable, "maybe").is_err());
}

const FABRIC: &[u8] = include_bytes!("../../../tests/fixtures/eggs/fabric.json");

#[test]
fn pinned_fabric_egg_normalizes_with_its_interaction_hints_and_length_rules() {
    let fabric = normalize_egg(FABRIC).unwrap();
    assert_eq!(fabric.name, "Fabric");
    assert_eq!(fabric.ready_when, ")! For help, type ");
    assert_eq!(fabric.installer.container, "eclipse-temurin:21-jdk-jammy");
    assert!(fabric.features.contains(&"eula".to_string()));
    for variable in &fabric.variables {
        validate_variable(variable, &variable.default_value).unwrap();
    }
    let version = fabric
        .variables
        .iter()
        .find(|v| v.env_variable == "MC_VERSION")
        .unwrap();
    assert!(validate_variable(version, "1.21.4").is_ok());
    assert!(validate_variable(version, "1.").is_err());
    assert!(validate_variable(version, &"1".repeat(16)).is_err());
}

#[test]
fn length_rules_and_log_settings_fail_closed_when_malformed() {
    let egg = normalize_egg(JAVA).unwrap();
    let mut variable = egg.variables[0].clone();
    variable.rules = "string|min:2".into();
    assert!(validate_variable(&variable, "a").is_err());
    assert!(validate_variable(&variable, "ab").is_ok());
    for rules in [
        "between:5,3",
        "between:3",
        "between:a,b",
        "min:x",
        "between:1,9|min:6",
        "between:1,9|max:2",
    ] {
        variable.rules = rules.into();
        assert!(validate_variable(&variable, "abcd").is_err(), "{rules}");
    }
    let mut raw: Value = serde_json::from_slice(FABRIC).unwrap();
    raw["config"]["logs"] = json!(r#"{"custom":true,"location":"logs/latest.log"}"#);
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw["config"]["logs"] = json!(r#"{"custom":false,"extra":1}"#);
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw = serde_json::from_slice(FABRIC).unwrap();
    raw["config"]["startup"] = json!(r#"{"done":"ready","userInteraction":"not a list"}"#);
    assert!(normalize_egg(&serde_json::to_vec(&raw).unwrap()).is_err());
}

#[test]
fn pinned_vanilla_forge_and_neoforge_eggs_normalize() {
    for (bytes, name) in [
        (
            &include_bytes!("../../../tests/fixtures/eggs/vanilla.json")[..],
            "Vanilla Minecraft",
        ),
        (
            &include_bytes!("../../../tests/fixtures/eggs/forge.json")[..],
            "Forge Enhanced",
        ),
        (
            &include_bytes!("../../../tests/fixtures/eggs/neoforge.json")[..],
            "NeoForge",
        ),
    ] {
        let egg = normalize_egg(bytes).unwrap_or_else(|issues| panic!("{name}: {issues:?}"));
        assert_eq!(egg.name, name);
        assert!(!egg.installer.script.is_empty());
        for variable in &egg.variables {
            validate_variable(variable, &variable.default_value)
                .unwrap_or_else(|e| panic!("{name} {}: {e}", variable.env_variable));
        }
    }
}
