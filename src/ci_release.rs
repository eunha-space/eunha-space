//! Central Eunha releases built by Eunha's Release workflow on GitHub Actions,
//! downloaded instead of compiled into the sibling checkout. Enabled by
//! `CONTROL_GITHUB_TOKEN`; without it the control plane builds as before.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::io::AsyncWriteExt;

const DEFAULT_REPOSITORY: &str = "eunha-space/eunha";
const WORKFLOW: &str = "release.yml";
const ARTIFACT: &str = "eunha-macos-arm64";
const TARBALL: &str = "eunha-macos-arm64.tar.gz";
/// Far above the ~40 MB a build is, so only a wrong file reaches it.
const MAX_ARTIFACT_BYTES: u64 = 1 << 30;

pub struct Source {
    repository: String,
    token: String,
    client: reqwest::Client,
}

/// The newest commit on `main` that the Release workflow built successfully.
#[derive(Debug, PartialEq)]
pub struct Built {
    pub reference: String,
    run_id: u64,
    html_url: String,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Artifact {
    archive_download_url: String,
    size_in_bytes: u64,
    #[serde(default)]
    expired: bool,
    /// `sha256:<hex>` of the zip; artifacts uploaded before GitHub recorded
    /// digests have none.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Run {
    id: u64,
    head_sha: String,
    html_url: String,
}

#[derive(Deserialize)]
struct Runs {
    workflow_runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Artifacts {
    artifacts: Vec<Artifact>,
}

impl Source {
    pub fn from_env() -> Result<Option<Self>, String> {
        let Ok(token) = std::env::var("CONTROL_GITHUB_TOKEN") else {
            return Ok(None);
        };
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Ok(None);
        }
        let repository =
            std::env::var("CONTROL_EUNHA_REPOSITORY").unwrap_or_else(|_| DEFAULT_REPOSITORY.into());
        if !is_repository(&repository) {
            return Err("CONTROL_EUNHA_REPOSITORY must be owner/name.".into());
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent("eunha-space")
            .build()
            .map_err(|_| "Could not prepare the GitHub client.".to_string())?;
        Ok(Some(Self {
            repository,
            token,
            client,
        }))
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    fn api(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!(
                "https://api.github.com/repos/{}/{path}",
                self.repository
            ))
            .bearer_auth(&self.token)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(Duration::from_secs(30))
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let response = self
            .api(path)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| "GitHub could not be reached.".to_string())?;
        if !response.status().is_success() {
            return Err(format!("GitHub answered {}.", response.status().as_u16()));
        }
        response
            .json()
            .await
            .map_err(|_| "GitHub's answer could not be read.".to_string())
    }

    /// Releases deploy this rather than the head of `main`, so they never wait
    /// for a build in progress, and a commit pushed while one is queued cannot
    /// change what it deploys.
    pub async fn latest_build(&self) -> Result<Option<Built>, String> {
        let runs: Runs = self
            .json(&format!(
                "actions/workflows/{WORKFLOW}/runs?branch=main&event=push&status=success&per_page=1"
            ))
            .await?;
        newest(runs.workflow_runs)
    }

    pub async fn artifact(&self, built: &Built) -> Result<Artifact, String> {
        let artifacts: Artifacts = self
            .json(&format!(
                "actions/runs/{}/artifacts?name={ARTIFACT}",
                built.run_id
            ))
            .await?;
        match artifacts.artifacts.into_iter().next() {
            Some(artifact) if artifact.expired => Err(format!(
                "CI's build has expired; re-run the Release workflow: {}",
                built.html_url
            )),
            Some(artifact) => Ok(artifact),
            None => Err(format!(
                "CI's build uploaded no {ARTIFACT} artifact: {}",
                built.html_url
            )),
        }
    }

    /// Unpack the build into `directory`, which must be empty: `eunha` and
    /// `frontend/dist`.
    pub async fn download(&self, artifact: &Artifact, directory: &Path) -> Result<(), String> {
        if artifact.size_in_bytes > MAX_ARTIFACT_BYTES {
            return Err("CI's build is implausibly large.".into());
        }
        // GitHub redirects to blob storage; reqwest drops the token when the
        // redirect leaves api.github.com.
        let mut response = self
            .client
            .get(&artifact.archive_download_url)
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|_| "CI's build could not be downloaded.".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Downloading CI's build answered {}.",
                response.status().as_u16()
            ));
        }
        let zip = directory.join("artifact.zip");
        let mut file = tokio::fs::File::create(&zip)
            .await
            .map_err(|_| "Could not stage CI's build.".to_string())?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "CI's build download was interrupted.".to_string())?
        {
            written += chunk.len() as u64;
            if written > MAX_ARTIFACT_BYTES {
                return Err("CI's build is implausibly large.".into());
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| "Could not stage CI's build.".to_string())?;
        }
        file.flush()
            .await
            .map_err(|_| "Could not stage CI's build.".to_string())?;
        drop(file);
        if let Some(expected) = &artifact.digest
            && !digest_matches(expected, &hasher.finalize())
        {
            return Err("CI's build does not match its recorded digest.".into());
        }
        let directory_text = directory.to_string_lossy();
        crate::command_status(
            "Artifact extraction",
            "/usr/bin/unzip",
            &["-q", "artifact.zip", TARBALL],
            Some(directory),
        )
        .await?;
        // bsdtar refuses absolute and `..` member names without -P.
        crate::command_status(
            "Release extraction",
            "/usr/bin/tar",
            &["-xzf", TARBALL, "-C", &directory_text, "eunha", "frontend"],
            Some(directory),
        )
        .await?;
        let _ = tokio::fs::remove_file(&zip).await;
        let _ = tokio::fs::remove_file(directory.join(TARBALL)).await;
        if !directory.join("eunha").is_file()
            || !directory.join("frontend/dist/index.html").is_file()
        {
            return Err("CI's build is missing the binary or the frontend.".into());
        }
        Ok(())
    }
}

fn newest(runs: Vec<Run>) -> Result<Option<Built>, String> {
    let Some(run) = runs.into_iter().next() else {
        return Ok(None);
    };
    if !crate::is_git_sha(&run.head_sha) {
        return Err("GitHub named an invalid commit.".into());
    }
    Ok(Some(Built {
        reference: run.head_sha,
        run_id: run.id,
        html_url: run.html_url,
    }))
}

fn digest_matches(expected: &str, actual: &[u8]) -> bool {
    let actual: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
    expected
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.eq_ignore_ascii_case(&actual))
}

fn is_repository(value: &str) -> bool {
    let mut parts = value.split('/');
    let valid = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    };
    valid(parts.next()) && valid(parts.next()) && parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_successful_run_names_the_release() {
        assert_eq!(newest(vec![]).unwrap(), None);
        let sha = "a609604ee461a0c8d3b1e2f4a5b6c7d8e9f0a1b2";
        let built = newest(vec![Run {
            id: 7,
            head_sha: sha.into(),
            html_url: "u".into(),
        }])
        .unwrap()
        .unwrap();
        assert_eq!((built.reference.as_str(), built.run_id), (sha, 7));
        let invalid = vec![Run {
            id: 7,
            head_sha: "main".into(),
            html_url: "u".into(),
        }];
        assert!(newest(invalid).is_err());
    }

    #[test]
    fn digests_compare_the_zip_hash() {
        let hash = Sha256::digest(b"zip");
        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(digest_matches(&format!("sha256:{hex}"), &hash));
        assert!(digest_matches(
            &format!("sha256:{}", hex.to_uppercase()),
            &hash
        ));
        assert!(!digest_matches(&hex, &hash));
        assert!(!digest_matches("sha256:00", &hash));
    }

    #[test]
    fn repositories_are_one_owner_and_one_name() {
        assert!(is_repository("limeburst/eunha"));
        assert!(is_repository("eunha-space/eunha.social"));
        assert!(!is_repository("limeburst"));
        assert!(!is_repository("a/b/c"));
        assert!(!is_repository("../eunha"));
        assert!(!is_repository("limeburst/eunha?x=1"));
    }
}
