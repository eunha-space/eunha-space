//! Bootstrap site-owned settings through Eunha's idempotent importer. The
//! private input is never installed as runtime configuration or logged.
use super::*;
use tokio::io::AsyncWriteExt;

#[derive(Serialize)]
pub(crate) struct Seed {
    title: String,
    description: String,
    short_description: String,
    contact_email: Option<String>,
    registrations_open: bool,
    approval_required: bool,
    privacy_policy: String,
    terms_of_service: String,
}

impl From<&StoredTenant> for Seed {
    fn from(tenant: &StoredTenant) -> Self {
        Self {
            title: tenant.title.clone(),
            description: tenant.description.clone(),
            short_description: tenant.short_description.clone(),
            contact_email: tenant.contact_email.clone(),
            registrations_open: tenant.registrations_open,
            approval_required: tenant.approval_required,
            privacy_policy: tenant.privacy_policy.clone(),
            terms_of_service: tenant.terms_of_service.clone(),
        }
    }
}

fn input(typed: &TenantToml, seed: &Seed) -> Result<String> {
    let mut config = toml::Value::try_from(typed)?;
    let instance = config
        .get_mut("instance")
        .and_then(toml::Value::as_table_mut)
        .context("missing instance configuration")?;
    let site = toml::Value::try_from(seed)?;
    instance.extend(site.as_table().context("invalid site settings")?.clone());
    Ok(toml::to_string_pretty(&config)?)
}

pub(crate) async fn import(typed: &TenantToml, seed: &Seed, tenants: &FsPath) -> Result<()> {
    let root = tenants
        .parent()
        .context("runtime directory has no parent")?;
    let temporary = root.join(format!(".site-settings-{}", Uuid::new_v4()));
    let mut directory = tokio::fs::DirBuilder::new();
    #[cfg(unix)]
    directory.mode(0o700);
    directory.create(&temporary).await?;
    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(temporary.join("config.toml")).await?;
        file.write_all(input(typed, seed)?.as_bytes()).await?;
        file.flush().await?;
        drop(file);
        let status = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(root.join("current/eunha"))
                .args(["settings", "import-config"])
                .env("DATABASE_URL", &typed.database_url)
                .current_dir(&temporary)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await??;
        anyhow::ensure!(status.success(), "site settings import failed");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = tokio::fs::remove_dir_all(&temporary).await;
    result?;
    cleanup?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (TenantToml, Seed) {
        let typed = toml::from_str(
            r#"
database_url = "postgres://test:test@localhost/test"
redis_url = "redis://localhost/"
bind_address = "127.0.0.1:3000"
[media_storage]
region = "test"
access_key_id = "test"
secret_access_key = "test"
base_url = "https://media.example.test"
[instance]
domain = "example.test"
vapid_private_key = "test"
vapid_public_key = "test"
"#,
        )
        .unwrap();
        let seed = Seed {
            title: "Example".into(),
            description: "Description".into(),
            short_description: "Short".into(),
            contact_email: Some("admin@example.test".into()),
            registrations_open: false,
            approval_required: true,
            privacy_policy: "Privacy".into(),
            terms_of_service: "Terms".into(),
        };
        (typed, seed)
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn importer_receives_private_seed_and_cleans_up_on_success_and_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("site-settings-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("current")).unwrap();
        let binary = root.join("current/eunha");
        let (typed, seed) = fixture();
        let runtime = toml::Value::try_from(&typed).unwrap();
        let bootstrap: toml::Value = toml::from_str(&input(&typed, &seed).unwrap()).unwrap();
        for key in [
            "title",
            "description",
            "short_description",
            "contact_email",
            "registrations_open",
            "approval_required",
            "privacy_policy",
            "terms_of_service",
        ] {
            assert!(
                runtime["instance"].get(key).is_none(),
                "{key} remains in runtime TOML"
            );
            assert!(
                bootstrap["instance"].get(key).is_some(),
                "{key} missing from bootstrap"
            );
        }
        for exit_code in [0, 1] {
            std::fs::write(&binary, format!("#!/bin/sh\n[ \"$1\" = settings ] && [ \"$2\" = import-config ] || exit 2\n[ -f config.toml ] || exit 3\n[ \"$(stat -f %Lp config.toml 2>/dev/null || stat -c %a config.toml)\" = 600 ] || exit 4\n[ \"$(stat -f %Lp . 2>/dev/null || stat -c %a .)\" = 700 ] || exit 5\nexit {exit_code}\n")).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = import(&typed, &seed, &root.join("tenants")).await;
            assert_eq!(result.is_ok(), exit_code == 0);
            assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".site-settings-")
            }));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
