use anyhow::Context as _;
use dialoguer::{Input, Password, Select};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub boil_api_token: String,
    pub tg_token: Option<String>,
    pub tg_chat_id: Option<String>,
    /// 定时换 IP 的 cron 表达式（5字段），None 表示不启用
    pub change_cron: Option<String>,
}

impl Config {
    pub fn has_tg(&self) -> bool {
        self.tg_token.is_some() && self.tg_chat_id.is_some()
    }
}

/// 验证 cron 表达式是否合法（5字段：min hour day month weekday）
pub fn validate_cron(expr: &str) -> anyhow::Result<()> {
    use tokio_cron_scheduler::Job;
    // tokio-cron-scheduler 用 6字段（加秒），我们在前面补 0 秒
    let full = format!("0 {}", expr.trim());
    Job::new(&full, |_, _| {}).map_err(|e| anyhow::anyhow!("cron 表达式无效: {e}"))?;
    Ok(())
}

/// 将 cron 表达式写入 config.env（None 表示清除）
pub fn save_cron(cron: Option<&str>) -> anyhow::Result<()> {
    let path = config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();

    let filtered: String = content
        .lines()
        .filter(|l| !l.starts_with("CHANGE_CRON="))
        .map(|l| format!("{l}\n"))
        .collect();

    let new_content = match cron {
        Some(expr) => format!("{filtered}CHANGE_CRON='{expr}'\n"),
        None => filtered,
    };
    // 兜底路径可能是尚不存在的 /etc/boil/，先确保父目录存在再写
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, new_content)?;
    Ok(())
}

fn config_path() -> PathBuf {
    // 优先级：/etc/boil/ > exe 同目录 > 当前目录
    let candidates = [
        PathBuf::from("/etc/boil/config.env"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("config.env")))
            .unwrap_or_else(|| PathBuf::from("config.env")),
        PathBuf::from("config.env"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("/etc/boil/config.env"))
}

/// setup 向导写入配置的目标路径（优先写到 /etc/boil/，不存在则写当前目录）
fn setup_save_path() -> PathBuf {
    let etc = PathBuf::from("/etc/boil");
    if etc.exists() || std::fs::create_dir_all(&etc).is_ok() {
        etc.join("config.env")
    } else {
        PathBuf::from("config.env")
    }
}

/// 构建配置文件内容：始终用新 token，保留已有 CHANGE_CRON；
/// tg 为 Some 时写入新 TG 配置（覆盖旧的），为 None 时保留已有 TG 配置。
/// 关键：旧 TG 行不会与新行并存，避免 dotenvy「同名 key 取第一个」导致新配置失效。
fn build_config_content(
    existing: &str,
    token: &str,
    tg: Option<(&str, &str)>,
) -> String {
    let cron_line: String = existing
        .lines()
        .find(|l| l.starts_with("CHANGE_CRON="))
        .map(|l| format!("{l}\n"))
        .unwrap_or_default();

    let mut content = format!("BOIL_API_TOKEN='{}'\n{}", token, cron_line);

    match tg {
        // 写入新 TG 配置，旧的丢弃
        Some((tg_token, chat_id)) => {
            content.push_str(&format!(
                "TG_TOKEN='{tg_token}'\nTG_CHAT_ID='{chat_id}'\n"
            ));
        }
        // 未配置 TG：原样保留已有 TG 行
        None => {
            let tg_lines: String = existing
                .lines()
                .filter(|l| l.starts_with("TG_"))
                .map(|l| format!("{l}\n"))
                .collect();
            content.push_str(&tg_lines);
        }
    }
    content
}

pub fn load() -> anyhow::Result<Config> {
    let path = config_path();
    if path.exists() {
        dotenvy::from_path(&path).ok();
    }
    dotenvy::dotenv().ok();

    Ok(Config {
        boil_api_token: std::env::var("BOIL_API_TOKEN")
            .or_else(|_| {
                // 兼容旧配置：自动从 BOIL_ACCOUNT/BOIL_PASSWORD 迁移提示
                std::env::var("BOIL_ACCOUNT").map(|_| String::new())
            })
            .context(
                "缺少 BOIL_API_TOKEN 配置\n\
                 请前往 https://ippanel.boil.network/ 登录后点击「獲取API」生成 Token，\
                 然后运行 boil setup 重新配置",
            )?,
        tg_token: std::env::var("TG_TOKEN").ok(),
        tg_chat_id: std::env::var("TG_CHAT_ID").ok(),
        change_cron: std::env::var("CHANGE_CRON").ok(),
    })
}

pub async fn load_or_setup() -> anyhow::Result<Config> {
    // 清理旧版残留文件（v1 session cookie 在新版 Bearer Token 下已无用）
    cleanup_old_artifacts();

    match load() {
        Ok(cfg) => {
            // 检查是否从旧配置迁移过来的空 token
            if cfg.boil_api_token.is_empty() {
                println!("检测到旧版配置（BOIL_ACCOUNT/BOIL_PASSWORD），新版已改用 API Token。");
                println!("请前往 https://ippanel.boil.network/ 登录后点击「獲取API」生成 Token。\n");
                run_setup_wizard().await?;
                return load();
            }
            Ok(cfg)
        }
        Err(_) => {
            println!("未找到配置，启动首次配置向导...\n");
            run_setup_wizard().await?;
            load()
        }
    }
}

/// 清理 v1 旧版残留文件（session.cookie 等），静默执行，不存在则跳过。
fn cleanup_old_artifacts() {
    for dir in ["/etc/boil", "."] {
        let cookie_path = std::path::PathBuf::from(dir).join("session.cookie");
        if cookie_path.exists() {
            let _ = std::fs::remove_file(&cookie_path);
            log::info!("已清理旧版残留: {:?}", cookie_path);
        }
    }
}

pub async fn run_setup_wizard() -> anyhow::Result<()> {
    println!("请前往 https://ippanel.boil.network/ 登录后点击「獲取API」获取 Token。\n");

    let token: String = Password::new()
        .with_prompt("Boil API Token")
        .interact()?;

    println!("\n验证 Token 中...");
    let client = crate::boil::BoilClient::new(&token)?;
    match client.get_ip().await {
        Ok(resp) => println!("✅ Token 验证成功，当前 IP: {}\n", resp.ip),
        Err(e) => {
            anyhow::bail!("Token 验证失败: {e}\n请检查 Token 是否正确，是否已过期");
        }
    }

    // 保存 token（保留已有的 CHANGE_CRON 与 TG 配置）
    let save_path = setup_save_path();
    let existing = std::fs::read_to_string(&save_path).unwrap_or_default();
    std::fs::write(&save_path, build_config_content(&existing, &token, None))?;
    println!("✅ Token 已保存到 {}\n", save_path.display());

    // TG 可选
    let want_tg = Select::new()
        .with_prompt("配置 Telegram Bot（用于远程控制）")
        .items(&["是，现在配置", "否，跳过（之后可用 boil setup 补充）"])
        .default(0)
        .interact()?
        == 0;

    if want_tg {
        let tg_token: String = Input::new()
            .with_prompt("Bot Token（从 @BotFather 获取）")
            .interact_text()?;

        let chat_id = loop {
            let _: String = Input::new()
                .with_prompt("先向机器人发任意消息，然后按回车检测")
                .allow_empty(true)
                .interact_text()?;

            match detect_chat_id(&tg_token).await {
                Ok(id) => {
                    println!("✅ 检测到 chat_id: {id}\n");
                    break id;
                }
                Err(_) => {
                    println!("⚠️  未检测到消息，请先在 Telegram 向机器人发一条消息，然后再按回车");
                }
            }
        };

        // 用新 TG 配置覆盖写入（替换旧的，避免重复 key 导致新配置不生效）
        std::fs::write(
            &save_path,
            build_config_content(&existing, &token, Some((tg_token.as_str(), chat_id.as_str()))),
        )?;
        println!("✅ TG 配置已保存\n");
    } else {
        println!("已跳过 Telegram 配置，可使用 boil status/change 命令行操作\n");
    }
    println!("常用命令:");
    println!("  boil status    查看当前 IP");
    println!("  boil check     检查 IP 质量和流媒体解锁");
    println!("  boil change    换 IP");
    println!();
    Ok(())
}

async fn detect_chat_id(tg_token: &str) -> anyhow::Result<String> {
    let url = format!(
        "https://api.telegram.org/bot{}/getUpdates?offset=-1&limit=1",
        tg_token
    );
    let resp: serde_json::Value = reqwest::get(&url).await?.json().await?;
    resp["result"][0]["message"]["from"]["id"]
        .as_i64()
        .map(|id| id.to_string())
        .context("未检测到消息")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复现并验证修复：重新配置 TG 时不应产生重复的 TG_ 行，且新值生效。
    #[test]
    fn reconfigure_tg_no_duplicate() {
        let existing =
            "BOIL_API_TOKEN='oldtoken'\nTG_TOKEN='oldtg'\nTG_CHAT_ID='111'\n";
        let out = build_config_content(existing, "newtoken", Some(("newtg", "222")));

        assert_eq!(out.matches("TG_TOKEN=").count(), 1, "TG_TOKEN 应只出现一次");
        assert_eq!(out.matches("TG_CHAT_ID=").count(), 1, "TG_CHAT_ID 应只出现一次");
        assert!(out.contains("TG_TOKEN='newtg'"));
        assert!(out.contains("TG_CHAT_ID='222'"));
        assert!(!out.contains("oldtg"), "旧 TG token 不应残留");
        assert!(out.contains("BOIL_API_TOKEN='newtoken'"));
    }

    /// 跳过 TG 配置时，应保留已有的 TG 配置。
    #[test]
    fn skip_tg_keeps_existing() {
        let existing = "BOIL_API_TOKEN='t'\nTG_TOKEN='keep'\nTG_CHAT_ID='1'\n";
        let out = build_config_content(existing, "t2", None);
        assert!(out.contains("TG_TOKEN='keep'"));
        assert_eq!(out.matches("TG_TOKEN=").count(), 1);
    }

    /// 重配 Token/TG 时，已有的 CHANGE_CRON 定时设置应被保留。
    #[test]
    fn keeps_cron_when_configuring_tg() {
        let existing = "BOIL_API_TOKEN='t'\nCHANGE_CRON='0 */6 * * *'\n";
        let out = build_config_content(existing, "t2", Some(("tg", "c")));
        assert!(out.contains("CHANGE_CRON='0 */6 * * *'"));
        assert_eq!(out.matches("CHANGE_CRON=").count(), 1);
    }
}
