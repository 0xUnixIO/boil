use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;

pub struct BoilClient {
    client: reqwest::Client,
    token: String,
}

#[derive(Deserialize, Debug)]
pub struct GetIpResponse {
    pub ip: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ChangeIpResponse {
    pub success: bool,
    #[serde(default)]
    pub ip: Option<String>,
}

const BOIL_URL: &str = "https://ippanel.boil.network";

/// 判断错误是否为服务器限流。
pub fn is_rate_limited(err: &anyhow::Error) -> bool {
    let m = err.to_string();
    m.contains("頻率限制") || m.contains("频率限制") || m.contains("頻密") || m.contains("频密")
}

impl BoilClient {
    pub fn new(token: &str) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            token: token.to_string(),
        })
    }

    /// 获取当前 IP
    pub async fn get_ip(&self) -> anyhow::Result<GetIpResponse> {
        let body = self
            .client
            .post(format!("{BOIL_URL}/api/v1/getIP"))
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await
            .context("getIP 请求失败")?
            .text()
            .await
            .context("getIP 读取响应失败")?;

        // 先检查业务层错误（token 失效、限流等）
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(err_msg) = v.get("error").and_then(|e| e.as_str()) {
                anyhow::bail!("{}", err_msg);
            }
        }

        serde_json::from_str::<GetIpResponse>(&body)
            .with_context(|| format!("getIP 响应解析失败: {}", &body[..body.len().min(200)]))
    }

    /// 更换 IP，成功返回新的 IP 信息
    pub async fn change_ip(&self) -> anyhow::Result<ChangeIpResponse> {
        let body = self
            .client
            .post(format!("{BOIL_URL}/api/v1/changeIP/"))
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await
            .context("changeIP 请求失败")?
            .text()
            .await
            .context("changeIP 读取响应失败")?;

        // 先检查业务层错误
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(err_msg) = v.get("error").and_then(|e| e.as_str()) {
                anyhow::bail!("{}", err_msg);
            }
        }

        serde_json::from_str::<ChangeIpResponse>(&body)
            .with_context(|| format!("changeIP 响应解析失败: {}", &body[..body.len().min(200)]))
    }
}
