use crate::{
    boil::BoilClient,
    config::{save_cron, validate_cron, Config},
    core::{check_ip_quality, check_reachable, do_reconnect},
};

pub async fn cmd_status(config: &Config) -> anyhow::Result<()> {
    let c = BoilClient::new(&config.boil_api_token)?;
    let resp = c.get_ip().await?;
    println!("📡 当前 IP: {}", resp.ip);
    Ok(())
}

pub async fn cmd_check(config: &Config) -> anyhow::Result<()> {
    let c = BoilClient::new(&config.boil_api_token)?;
    let resp = c.get_ip().await?;
    let ip = &resp.ip;

    let (reachable, quality) = tokio::join!(check_reachable(ip), check_ip_quality(ip));
    let reach = if reachable {
        "TCP 可达 ✅"
    } else {
        "TCP 未通 ⚠️"
    };
    println!("📍 IP: {}  {}", ip, reach);
    if let Some(q) = quality {
        println!(
            "   地区: {} | ISP: {}\n   类型: {} | CF 风险: {}",
            q.country,
            q.isp,
            q.ip_type(),
            q.cf_risk()
        );
    }
    println!();

    // 流媒体检测需要从 Boil VPS 的 IP 发出，先对比本机公网 IP
    let local_ip = get_local_public_ip().await;
    let on_boil_vps = local_ip.as_deref() == Some(ip.as_str());

    if on_boil_vps {
        println!("📺 流媒体检测中...");
        let results = crate::streaming::check_all().await;
        for r in &results {
            println!("   {:16} {}", r.service, r.status.display());
        }
    } else {
        println!("📺 流媒体检测跳过（当前运行于非 Boil VPS 机器，结果无意义）");
        println!("   如需检测，请在 Boil VPS 上直接运行 boil check");
    }
    println!();
    Ok(())
}

async fn get_local_public_ip() -> Option<String> {
    let client = reqwest::Client::new();
    // 多源兜底：单一服务超时/被墙时仍能拿到本机公网 IP，避免误判为非 VPS
    for url in [
        "https://api.ipify.org",
        "https://ifconfig.me/ip",
        "https://icanhazip.com",
    ] {
        if let Ok(resp) = client
            .get(url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            if let Ok(text) = resp.text().await {
                let ip = text.trim().to_string();
                if !ip.is_empty() {
                    return Some(ip);
                }
            }
        }
    }
    None
}

pub async fn cmd_change(config: &Config) -> anyhow::Result<()> {
    let c = BoilClient::new(&config.boil_api_token)?;

    println!("⏳ 换 IP 中...");
    let res = do_reconnect(&c).await?;

    match res.new_ip {
        Some(new_ip) => {
            let reach = if res.reachable {
                "TCP 可达 ✅"
            } else {
                "TCP 未通 ⚠️"
            };
            println!(
                "\n✅ 换 IP 完成\n   旧 IP: {}\n   新 IP: {}  {}\n",
                res.old_ip.as_deref().unwrap_or("未知"),
                new_ip,
                reach,
            );
            if let Some(q) = res.quality {
                println!(
                    "📊 IP 质量\n   地区: {}\n   ISP:  {}\n   类型: {}\n   CF 风险: {}",
                    q.country, q.isp, q.ip_type(), q.cf_risk()
                );
            }
        }
        None => {
            println!(
                "⚠️  重拨已触发，但未检测到 IP 变化\n   旧 IP: {}\n   请到面板手动确认",
                res.old_ip.as_deref().unwrap_or("未知")
            );
        }
    }
    Ok(())
}

/// arg: cron 表达式 / "off" / "" (查看)
pub fn cmd_timer(config: &Config, arg: &str) -> anyhow::Result<()> {
    if arg.is_empty() {
        match &config.change_cron {
            Some(cron) => println!("⏰ 当前定时换 IP: {cron}\n\n关闭: boil timer off"),
            None => println!(
                "⏰ 定时换 IP 未启用\n\n设置示例:\n  每6小时: boil timer \"0 */6 * * *\"\n  每天3点: boil timer \"0 3 * * *\""
            ),
        }
        return Ok(());
    }

    if arg.eq_ignore_ascii_case("off") {
        save_cron(None)?;
        println!("✅ 定时换 IP 已关闭");
        return Ok(());
    }

    validate_cron(arg)?;
    save_cron(Some(arg))?;
    println!("✅ 定时换 IP 已设置: {arg}\n重启 boil 后生效");
    Ok(())
}
