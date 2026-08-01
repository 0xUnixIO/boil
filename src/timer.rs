use std::sync::Arc;

use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use crate::{boil::BoilClient, config::Config, core::do_reconnect};

/// 定时换 IP 管理器：持有运行中的调度器，支持运行时动态增删任务（无需重启进程）。
pub struct TimerManager {
    sched: JobScheduler,
    config: Arc<Config>,
    job_id: Option<Uuid>,
    /// 当前生效的 cron 表达式（None 表示未启用），供查询展示
    current: Option<String>,
}

impl TimerManager {
    /// 创建并启动一个空调度器（尚无任务）
    pub async fn new(config: Arc<Config>) -> anyhow::Result<Self> {
        let sched = JobScheduler::new().await?;
        sched.start().await?;
        Ok(Self {
            sched,
            config,
            job_id: None,
            current: None,
        })
    }

    /// 当前生效的 cron 表达式
    pub fn current(&self) -> Option<String> {
        self.current.clone()
    }

    /// 设置/替换定时任务，立即生效（先移除旧任务再添加新任务）
    pub async fn set(&mut self, expr: &str) -> anyhow::Result<()> {
        self.clear().await?;

        // tokio-cron-scheduler 用 6字段（秒 分 时 日 月 周），我们在前面补 "0 "
        let full_expr = format!("0 {}", expr.trim());
        let cfg = self.config.clone();
        // 按北京时间（Asia/Shanghai）解析 cron，否则默认走 UTC，"3 点" 会变成北京 11 点
        let job = Job::new_async_tz(
            &full_expr,
            chrono_tz::Asia::Shanghai,
            move |_uuid, _lock| {
                let cfg = cfg.clone();
                Box::pin(async move {
                    run_auto_change(&cfg).await;
                })
            },
        )?;

        self.job_id = Some(self.sched.add(job).await?);
        self.current = Some(expr.trim().to_string());
        log::info!("定时换 IP 已生效，cron: {expr}");
        Ok(())
    }

    /// 清除当前定时任务，立即生效
    pub async fn clear(&mut self) -> anyhow::Result<()> {
        if let Some(id) = self.job_id.take() {
            self.sched.remove(&id).await?;
            log::info!("定时换 IP 已清除");
        }
        self.current = None;
        Ok(())
    }
}

/// 纯定时守护模式入口（无 TG）：按配置的 cron 启动一个长驻调度器
pub async fn start(config: Arc<Config>) -> anyhow::Result<TimerManager> {
    let expr = match &config.change_cron {
        Some(e) => e.clone(),
        None => anyhow::bail!("未配置 CHANGE_CRON"),
    };
    let mut mgr = TimerManager::new(config).await?;
    mgr.set(&expr).await?;
    Ok(mgr)
}

async fn run_auto_change(config: &Config) {
    let c = match BoilClient::new(&config.boil_api_token) {
        Ok(c) => c,
        Err(e) => {
            log::error!("定时换 IP 失败: {e}");
            return;
        }
    };

    log::info!("定时换 IP 触发");

    match do_reconnect(&c).await {
        Ok(res) => {
            let msg = match &res.new_ip {
                Some(new_ip) => {
                    let quality_info = res
                        .quality
                        .as_ref()
                        .map(|q| {
                            format!(
                                "\n类型: {} | CF 风险: {}",
                                q.ip_type(),
                                q.cf_risk()
                            )
                        })
                        .unwrap_or_default();
                    format!(
                        "⏰ 定时换 IP 完成\n旧 IP: {}\n新 IP: {}{}",
                        res.old_ip.as_deref().unwrap_or("未知"),
                        new_ip,
                        quality_info,
                    )
                }
                None => format!(
                    "⚠️ 定时换 IP：重拨触发但 IP 未变化（旧 IP: {}）",
                    res.old_ip.as_deref().unwrap_or("未知")
                ),
            };
            tg_notify(config, &msg).await;
        }
        Err(e) => {
            tg_notify(config, &format!("❌ 定时换 IP 失败: {e}")).await;
        }
    }
}

async fn tg_notify(config: &Config, msg: &str) {
    let (token, chat_id) = match (&config.tg_token, &config.tg_chat_id) {
        (Some(t), Some(c)) => (t, c),
        _ => return,
    };
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let _ = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "chat_id": chat_id, "text": msg }))
        .send()
        .await;
}
