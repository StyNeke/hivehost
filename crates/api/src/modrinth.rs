// Modrinth lookups for individual mods/plugins. Compatibility comes from the installed game version
// and loader the agent detected on disk, never from what a user typed. Every value returned by
// Modrinth is validated before it reaches the agent, which validates it again.
use crate::runtime::ExtensionRecord;
use manager_protocol::{DetectedGame, ExtensionFolder, RemoteFile};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

const USER_AGENT: &str = "hivehost/0.1 (private game server manager)";
const MAX_FILES: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModrinthError {
    #[error("Modrinth could not be reached")]
    Unavailable,
    #[error("{0}")]
    Unsupported(String),
    #[error("project not found")]
    NotFound,
}

#[derive(Clone)]
pub struct Modrinth {
    client: reqwest::Client,
    base: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub folder: ExtensionFolder,
    pub files: Vec<RemoteFile>,
    pub records: Vec<ExtensionRecord>,
    pub remove: Vec<String>,
}

// The loader names Modrinth uses for an installed server, and where its extensions live.
pub fn target(game: &DetectedGame) -> Result<(Vec<&'static str>, ExtensionFolder), ModrinthError> {
    match game.loader.as_deref() {
        Some("fabric") => Ok((vec!["fabric"], ExtensionFolder::Mods)),
        Some("paper") => Ok((vec!["paper", "spigot", "bukkit"], ExtensionFolder::Plugins)),
        _ => Err(ModrinthError::Unsupported(
            "this server has no supported mod or plugin loader; Modrinth installs are unavailable"
                .into(),
        )),
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    hits: Vec<RawHit>,
}

#[derive(Deserialize)]
struct RawHit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    icon_url: Option<String>,
    downloads: u64,
}

#[derive(Deserialize)]
struct Project {
    id: String,
    title: String,
    project_type: String,
    server_side: String,
}

#[derive(Debug, Deserialize)]
struct Version {
    id: String,
    project_id: String,
    version_number: String,
    version_type: String,
    files: Vec<VersionFile>,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct VersionFile {
    url: String,
    filename: String,
    primary: bool,
    size: u64,
    hashes: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_filename(name: &str) -> bool {
    name.len() > 4
        && name.len() <= 128
        && name.ends_with(".jar")
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

fn json_list(values: &[&str]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".into())
}

impl Modrinth {
    pub fn new(base: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client configuration is static");
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ModrinthError> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .query(query)
            .send()
            .await
            .map_err(|_| ModrinthError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            404 => return Err(ModrinthError::NotFound),
            _ => return Err(ModrinthError::Unavailable),
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| ModrinthError::Unavailable)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(ModrinthError::Unavailable);
        }
        serde_json::from_slice(&bytes).map_err(|_| ModrinthError::Unavailable)
    }

    pub async fn search(
        &self,
        query: &str,
        game: &DetectedGame,
    ) -> Result<Vec<SearchHit>, ModrinthError> {
        let (loaders, folder) = target(game)?;
        let kind = match folder {
            ExtensionFolder::Mods => "mod",
            ExtensionFolder::Plugins => "plugin",
        };
        let categories: Vec<String> = loaders
            .iter()
            .map(|l| format!("\"categories:{l}\""))
            .collect();
        let facets = format!(
            "[[{}],[\"versions:{}\"],[\"project_type:{kind}\"],[\"server_side:required\",\"server_side:optional\"]]",
            categories.join(","),
            game.version.replace('"', "")
        );
        let query: String = query
            .chars()
            .filter(|c| !c.is_control())
            .take(100)
            .collect();
        let response: SearchResponse = self
            .get(
                "/search",
                &[("query", query), ("facets", facets), ("limit", "20".into())],
            )
            .await?;
        Ok(response
            .hits
            .into_iter()
            .filter(|hit| valid_id(&hit.project_id))
            .map(|hit| SearchHit {
                project_id: hit.project_id,
                slug: hit.slug,
                title: hit.title.chars().take(200).collect(),
                description: hit.description.chars().take(500).collect(),
                icon_url: hit
                    .icon_url
                    .filter(|u| u.starts_with("https://cdn.modrinth.com/")),
                downloads: hit.downloads,
            })
            .collect())
    }

    async fn compatible_version(
        &self,
        project: &str,
        loaders: &[&str],
        game: &str,
    ) -> Result<Option<Version>, ModrinthError> {
        let versions: Vec<Version> = self
            .get(
                &format!("/project/{project}/version"),
                &[
                    ("loaders", json_list(loaders)),
                    ("game_versions", json_list(&[game])),
                ],
            )
            .await?;
        // Newest stable release first; fall back to beta, never to alpha builds.
        Ok(["release", "beta"]
            .iter()
            .find_map(|kind| versions.iter().position(|v| v.version_type == *kind))
            .and_then(|index| versions.into_iter().nth(index)))
    }

    // Resolves a project and its required dependencies to exact files for this installation.
    pub async fn plan(
        &self,
        project: &str,
        game: &DetectedGame,
        installed: &[ExtensionRecord],
    ) -> Result<Plan, ModrinthError> {
        if !valid_id(project) {
            return Err(ModrinthError::NotFound);
        }
        let (loaders, folder) = target(game)?;
        let mut queue = vec![(project.to_owned(), false)];
        let mut seen = std::collections::HashSet::new();
        let mut plan = Plan {
            folder,
            files: vec![],
            records: vec![],
            remove: vec![],
        };
        while let Some((wanted, dependency)) = queue.pop() {
            let details: Project = self.get(&format!("/project/{wanted}"), &[]).await?;
            if !seen.insert(details.id.clone()) {
                continue;
            }
            if !valid_id(&details.id) {
                return Err(ModrinthError::Unavailable);
            }
            let expected_type = match folder {
                ExtensionFolder::Mods => "mod",
                ExtensionFolder::Plugins => "plugin",
            };
            if details.project_type != expected_type {
                return Err(ModrinthError::Unsupported(format!(
                    "{} is a {}, not a {expected_type}",
                    details.title, details.project_type
                )));
            }
            if details.server_side == "unsupported" {
                return Err(ModrinthError::Unsupported(format!(
                    "{} only runs on game clients, not on servers",
                    details.title
                )));
            }
            let version = self
                .compatible_version(&details.id, &loaders, &game.version)
                .await?
                .ok_or_else(|| {
                    ModrinthError::Unsupported(format!(
                        "{} has no release for {} {}",
                        details.title, loaders[0], game.version
                    ))
                })?;
            if version.project_id != details.id || !valid_id(&version.id) {
                return Err(ModrinthError::Unavailable);
            }
            for dependency in &version.dependencies {
                let Some(other) = dependency.project_id.as_deref().filter(|id| valid_id(id)) else {
                    if dependency.dependency_type == "required" && dependency.version_id.is_some() {
                        return Err(ModrinthError::Unsupported(format!(
                            "{} requires a dependency that Modrinth only identifies by version; install it manually",
                            details.title
                        )));
                    }
                    continue;
                };
                match dependency.dependency_type.as_str() {
                    "required" => queue.push((other.to_owned(), true)),
                    "incompatible" if installed.iter().any(|e| e.project_id == other) => {
                        return Err(ModrinthError::Unsupported(format!(
                            "{} is incompatible with an installed extension",
                            details.title
                        )));
                    }
                    _ => {}
                }
            }
            if installed
                .iter()
                .any(|e| e.project_id == details.id && e.version_id == version.id)
            {
                continue;
            }
            let file = version
                .files
                .iter()
                .find(|f| f.primary)
                .or_else(|| version.files.first())
                .ok_or_else(|| {
                    ModrinthError::Unsupported(format!(
                        "{} has no downloadable file",
                        details.title
                    ))
                })?;
            let sha512 = file.hashes.get("sha512").cloned().unwrap_or_default();
            if !valid_filename(&file.filename)
                || !file.url.starts_with("https://cdn.modrinth.com/data/")
                || sha512.len() != 128
                || !sha512.bytes().all(|b| b.is_ascii_hexdigit())
                || file.size == 0
            {
                return Err(ModrinthError::Unsupported(format!(
                    "{} has a file that cannot be verified",
                    details.title
                )));
            }
            if let Some(previous) = installed.iter().find(|e| e.project_id == details.id) {
                plan.remove.push(previous.filename.clone());
            }
            plan.files.push(RemoteFile {
                url: file.url.clone(),
                sha512,
                size: file.size,
                filename: file.filename.clone(),
            });
            plan.records.push(ExtensionRecord {
                project_id: details.id.clone(),
                version_id: version.id.clone(),
                title: details.title.chars().take(200).collect(),
                version_number: version.version_number.chars().take(100).collect(),
                filename: file.filename.clone(),
                // A project the user picked is never demoted to a dependency.
                dependency: dependency
                    && !installed
                        .iter()
                        .any(|e| e.project_id == details.id && !e.dependency),
            });
            if plan.files.len() > MAX_FILES {
                return Err(ModrinthError::Unsupported(
                    "too many dependencies to install at once".into(),
                ));
            }
        }
        // A file that another installed project already uses would be overwritten silently.
        for file in &plan.files {
            if installed.iter().any(|e| {
                e.filename == file.filename
                    && !plan.records.iter().any(|r| r.project_id == e.project_id)
            }) {
                return Err(ModrinthError::Unsupported(format!(
                    "{} would replace a file owned by another extension",
                    file.filename
                )));
            }
        }
        Ok(plan)
    }
}
