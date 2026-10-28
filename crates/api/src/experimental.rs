// Experimental Minecraft server implementations: their bundled eggs and sourced upstream status
// from GitHub (releases, recent commits, open milestones with their own issue counts). Status is
// cached for an hour and kept, labelled stale, when GitHub is unreachable or rate limited. Nothing
// here estimates completeness: counts are shown only with the milestone they belong to.
use crate::database;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

pub const GITHUB_API: &str = "https://api.github.com";
pub const FRESH_SECS: i64 = 3600;
// Background refreshes and the manual refresh button share this minimum spacing.
pub const MIN_ATTEMPT_SPACING_SECS: i64 = 60;

pub struct Project {
    pub key: &'static str,
    pub name: &'static str,
    pub repository: &'static str,
    pub website: &'static str,
    pub summary: &'static str,
    pub egg: &'static [u8],
}

pub const PROJECTS: [Project; 2] = [
    Project {
        key: "pumpkin",
        name: "Pumpkin",
        repository: "Pumpkin-MC/Pumpkin",
        website: "https://pumpkinmc.org/",
        summary: "Minecraft Java Edition server written in Rust. Upstream egg, installs the published nightly musl build.",
        egg: include_bytes!("../../../eggs/experimental/pumpkin.json"),
    },
    Project {
        key: "steelmc",
        name: "SteelMC",
        repository: "Steel-Foundation/SteelMC",
        website: "https://steelmc.dev/",
        summary: "Minecraft Java Edition server written in Rust. Installs the latest published Linux release build.",
        egg: include_bytes!("../../../eggs/experimental/steelmc.json"),
    },
];

pub fn project(key: &str) -> Option<&'static Project> {
    PROJECTS.iter().find(|p| p.key == key)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Release {
    pub tag: String,
    pub name: Option<String>,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub url: String,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub message: String,
    pub date: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Milestone {
    pub title: String,
    pub open_issues: u64,
    pub closed_issues: u64,
    pub due_on: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Upstream {
    pub description: Option<String>,
    pub url: String,
    pub pushed_at: Option<String>,
    pub releases: Vec<Release>,
    pub commits: Vec<Commit>,
    pub milestones: Vec<Milestone>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub key: &'static str,
    pub name: &'static str,
    pub repository: &'static str,
    pub website: &'static str,
    pub summary: &'static str,
    pub upstream: Option<Upstream>,
    pub fetched_at: Option<i64>,
    pub stale: bool,
    pub error: Option<String>,
    pub egg_id: Option<String>,
}

pub struct StatusStore(Connection);

impl StatusStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Ok(Self(database::open(path)?))
    }

    pub fn entries(&self, now: i64) -> rusqlite::Result<Vec<Entry>> {
        PROJECTS
            .iter()
            .map(|project| {
                let row: Option<(Option<String>, Option<i64>, Option<String>)> = self
                    .0
                    .query_row(
                        "SELECT data, fetched_at, error FROM experimental_status WHERE project = ?1",
                        [project.key],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let (data, fetched_at, error) = row.unwrap_or((None, None, None));
                let egg_id = self
                    .0
                    .query_row(
                        "SELECT egg_id FROM experimental_eggs WHERE project = ?1 LIMIT 1",
                        [project.key],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(Entry {
                    key: project.key,
                    name: project.name,
                    repository: project.repository,
                    website: project.website,
                    summary: project.summary,
                    upstream: data.and_then(|d| serde_json::from_str(&d).ok()),
                    stale: fetched_at.is_none_or(|at| now - at > FRESH_SECS) || error.is_some(),
                    fetched_at,
                    error,
                    egg_id,
                })
            })
            .collect()
    }

    // Claims a refresh attempt; false when one happened within the minimum spacing.
    pub fn claim_attempt(&mut self, now: i64, only_if_due: bool) -> rusqlite::Result<bool> {
        let transaction = self
            .0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (last_attempt, oldest_fetch): (Option<i64>, Option<i64>) = transaction.query_row(
            "SELECT max(attempted_at), min(COALESCE(fetched_at, 0)) FROM experimental_status",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let known: i64 =
            transaction.query_row("SELECT COUNT(*) FROM experimental_status", [], |row| {
                row.get(0)
            })?;
        if last_attempt.is_some_and(|at| now - at < MIN_ATTEMPT_SPACING_SECS) {
            return Ok(false);
        }
        let due =
            known < PROJECTS.len() as i64 || oldest_fetch.is_none_or(|at| now - at > FRESH_SECS);
        if only_if_due && !due {
            return Ok(false);
        }
        for project in &PROJECTS {
            transaction.execute(
                "INSERT INTO experimental_status (project, attempted_at) VALUES (?1, ?2)
                 ON CONFLICT(project) DO UPDATE SET attempted_at = excluded.attempted_at",
                params![project.key, now],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn save(
        &mut self,
        project: &str,
        result: &Result<Upstream, String>,
        now: i64,
    ) -> rusqlite::Result<()> {
        match result {
            Ok(upstream) => self.0.execute(
                "UPDATE experimental_status SET data = ?1, fetched_at = ?2, error = NULL WHERE project = ?3",
                params![serde_json::to_string(upstream).unwrap_or_default(), now, project],
            )?,
            // The last good data stays; the error explains why it is stale.
            Err(error) => self.0.execute(
                "UPDATE experimental_status SET error = ?1 WHERE project = ?2",
                params![error, project],
            )?,
        };
        Ok(())
    }

    pub fn mark_egg(&mut self, egg: &str, project: &str) -> rusqlite::Result<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO experimental_eggs (egg_id, project) VALUES (?1, ?2)",
            params![egg, project],
        )?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct GitHub {
    client: reqwest::Client,
    base: String,
}

#[derive(Deserialize)]
struct RawRepository {
    description: Option<String>,
    html_url: String,
    pushed_at: Option<String>,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
    prerelease: bool,
    html_url: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
}

#[derive(Deserialize)]
struct RawCommit {
    sha: String,
    html_url: String,
    commit: RawCommitDetail,
}

#[derive(Deserialize)]
struct RawCommitDetail {
    message: String,
    committer: Option<RawSignature>,
}

#[derive(Deserialize)]
struct RawSignature {
    date: Option<String>,
}

#[derive(Deserialize)]
struct RawMilestone {
    title: String,
    open_issues: u64,
    closed_issues: u64,
    due_on: Option<String>,
    html_url: String,
}

fn clip(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

impl GitHub {
    pub fn new(base: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("hivehost (experimental status)")
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client configuration is static");
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, String> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| "GitHub could not be reached".to_string())?;
        let status = response.status().as_u16();
        let remaining = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if (status == 403 || status == 429) && remaining.as_deref() == Some("0") || status == 429 {
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok());
            return Err(match reset {
                Some(at) => format!("GitHub rate limit reached; it resets at unix time {at}"),
                None => "GitHub rate limit reached".into(),
            });
        }
        if status != 200 {
            return Err(format!("GitHub answered with status {status}"));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "GitHub could not be reached".to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("GitHub answer was too large".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "GitHub answer was not understood".into())
    }

    pub async fn fetch(&self, repository: &str) -> Result<Upstream, String> {
        let repo: RawRepository = self.get(&format!("/repos/{repository}")).await?;
        let releases: Vec<RawRelease> = self
            .get(&format!("/repos/{repository}/releases?per_page=5"))
            .await?;
        let commits: Vec<RawCommit> = self
            .get(&format!("/repos/{repository}/commits?per_page=5"))
            .await?;
        let milestones: Vec<RawMilestone> = self
            .get(&format!(
                "/repos/{repository}/milestones?state=open&per_page=10"
            ))
            .await?;
        Ok(Upstream {
            description: repo.description.map(|d| clip(&d, 300)),
            url: repo.html_url,
            pushed_at: repo.pushed_at,
            releases: releases
                .into_iter()
                .map(|r| Release {
                    tag: clip(&r.tag_name, 80),
                    name: r.name.map(|n| clip(&n, 120)),
                    published_at: r.published_at,
                    prerelease: r.prerelease,
                    url: r.html_url,
                    assets: r
                        .assets
                        .into_iter()
                        .take(20)
                        .map(|a| clip(&a.name, 120))
                        .collect(),
                })
                .collect(),
            commits: commits
                .into_iter()
                .map(|c| Commit {
                    sha: c.sha.chars().take(12).collect(),
                    message: clip(c.commit.message.lines().next().unwrap_or_default(), 160),
                    date: c.commit.committer.and_then(|s| s.date),
                    url: c.html_url,
                })
                .collect(),
            milestones: milestones
                .into_iter()
                .map(|m| Milestone {
                    title: clip(&m.title, 120),
                    open_issues: m.open_issues,
                    closed_issues: m.closed_issues,
                    due_on: m.due_on,
                    url: m.html_url,
                })
                .collect(),
        })
    }

    // Refreshes every project and stores the results (errors keep the last good data).
    pub async fn refresh(&self, database: &Path, now: i64) -> rusqlite::Result<()> {
        for project in &PROJECTS {
            let result = self.fetch(project.repository).await;
            let path = database.to_path_buf();
            tokio::task::spawn_blocking(move || {
                StatusStore::open(&path)?.save(project.key, &result, now)
            })
            .await
            .map_err(|_| rusqlite::Error::InvalidQuery)??;
        }
        Ok(())
    }
}
