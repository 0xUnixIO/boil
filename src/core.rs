use std::time::Duration;

use tokio::time::sleep;

use crate::boil::BoilClient;

pub struct IpQuality {
    pub country: String,
    pub isp: String,
    pub is_proxy: bool,
    pub is_hosting: bool,
}

impl IpQuality {
    pub fn cf_risk(&self) -> &'static str {
        if self.is_proxy || self.is_hosting {
            "高 ⚠️"
        } else {
            "低 ✅"
        }
    }
    pub fn ip_type(&self) -> &'static str {
        if self.is_proxy {
            "代理 ❌"
        } else if self.is_hosting {
            "机房 ⚠️"
        } else {
            "住宅 ✅"
        }
    }
}

pub struct ReconnectResult {
    pub old_ip: Option<String>,
    pub new_ip: Option<String>,
    pub reachable: bool,
    pub quality: Option<IpQuality>,
}

/// 执行换 IP 并等待新 IP 生效
pub async fn do_reconnect(client: &BoilClient) -> anyhow::Result<ReconnectResult> {
    // 获取旧 IP
    let old_ip = client
        .get_ip()
        .await
        .ok()
        .map(|r| r.ip);

    // 触发换 IP
    client.change_ip().await?;
    sleep(Duration::from_secs(8)).await;

    // 轮询直到 IP 变化
    let mut new_ip: Option<String> = None;
    for _ in 0..10u8 {
        sleep(Duration::from_secs(6)).await;
        match client.get_ip().await {
            Ok(resp) => {
                if Some(&resp.ip) != old_ip.as_ref() {
                    new_ip = Some(resp.ip);
                    break;
                }
            }
            Err(e) => {
                if crate::boil::is_rate_limited(&e) {
                    log::warn!("轮询 getIP 限流，跳过本轮: {e}");
                    continue;
                }
                return Err(e);
            }
        }
    }

    let (reachable, quality) = match &new_ip {
        Some(ip) => tokio::join!(check_reachable(ip), check_ip_quality(ip)),
        None => (false, None),
    };

    Ok(ReconnectResult {
        old_ip,
        new_ip,
        reachable,
        quality,
    })
}

pub async fn check_ip_quality(ip: &str) -> Option<IpQuality> {
    let url = format!("http://ip-api.com/json/{ip}?fields=status,country,isp,proxy,hosting");
    let resp: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    if resp["status"].as_str() != Some("success") {
        return None;
    }
    Some(IpQuality {
        country: resp["country"].as_str().unwrap_or("未知").to_string(),
        isp: resp["isp"].as_str().unwrap_or("未知").to_string(),
        is_proxy: resp["proxy"].as_bool().unwrap_or(false),
        is_hosting: resp["hosting"].as_bool().unwrap_or(false),
    })
}

pub async fn check_reachable(ip: &str) -> bool {
    for port in [80u16, 443, 22] {
        if tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(format!("{ip}:{port}")),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
        {
            return true;
        }
    }
    false
}
