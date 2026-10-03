//! Shared services handed to the loader, the web panel, and the control bot.

use std::sync::Arc;

use crate::account_tools::AccountRegistry;
use crate::database::Database;
use crate::jobs::{Job, JobManager, JobStatus};
use crate::logs::LogBuffer;
use crate::notify::Notifier;
use crate::runtime::RuntimeState;
use crate::settings::AppConfig;
use crate::web::auth::PanelAuth;

pub struct Services {
    pub config: AppConfig,
    pub db: Arc<Database>,
    pub runtime: Arc<RuntimeState>,
    pub panel_auth: Arc<PanelAuth>,
    pub logs: Arc<LogBuffer>,
    pub notifier: Arc<Notifier>,
    pub accounts: Arc<AccountRegistry>,
    pub jobs: Arc<JobManager>,
    pub automations: Arc<crate::automations::Engine>,
}

impl Services {
    pub async fn panel_link(&self) -> String {
        let token = self.panel_auth.issue_link_token().await;
        format!("{}/auth/token?t={token}", self.config.web.public_url())
    }

    /// Runs a job in the background and reports its result through the control bot.
    pub fn spawn_job<F>(self: &Arc<Self>, job: Job, work: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let services = Arc::clone(self);
        tokio::spawn(async move {
            work.await;
            let Some(snapshot) = services.jobs.get(job.id()).await else {
                return;
            };
            let icon = match snapshot.status {
                JobStatus::Done => "✅",
                JobStatus::Failed => "❌",
                JobStatus::Cancelled => "⏹",
                JobStatus::Running => "⏳",
            };
            let html = format!(
                "{icon} <b>{}</b> · {}\n{}",
                crate::notify::escape_html(&snapshot.title),
                crate::notify::escape_html(&snapshot.account),
                crate::notify::escape_html(snapshot.summary.as_deref().unwrap_or_default())
            );
            services
                .notifier
                .send(crate::notify::Topic::Jobs, &html)
                .await;
        });
    }
}

#[cfg(test)]
pub async fn test_services() -> Arc<Services> {
    let path = std::env::temp_dir().join(format!("fly_services_{}.json", uuid::Uuid::new_v4()));
    let security = Arc::new(tokio::sync::RwLock::new(None));
    let db = Arc::new(
        Database::load_with_state(&path, security)
            .await
            .expect("temp database"),
    );
    let runtime = RuntimeState::new();
    Arc::new(Services {
        config: AppConfig::default(),
        notifier: Notifier::new(Arc::clone(&db), Arc::clone(&runtime)),
        db,
        runtime,
        panel_auth: PanelAuth::new(),
        logs: LogBuffer::new(),
        accounts: AccountRegistry::new(),
        jobs: JobManager::new(),
        automations: crate::automations::Engine::new(),
    })
}
