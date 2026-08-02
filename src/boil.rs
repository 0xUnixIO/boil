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
pub struct ChangeIpResponse {
    /// 新版接口返回 `ok`，兼容旧版接口的 `success`。
    #[serde(alias = "ok")]
    pub success: bool,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub uses_left: Option<u64>,
    #[serde(default)]
    pub next_allowed_at: Option<i64>,
}

const BOIL_URL: &str = "https://ippanel.boil.network";

/// 判断错误是否为服务器限流。
pub fn is_rate_limited(err: &anyhow::Error) -> bool {
    let m = err.to_string();
    m.contains("頻率限制") || m.contains("频率限制") || m.contains("頻密") || m.contains("频密")
}

/// 检查 API 返回的业务层错误。
fn check_api_error(body: &str) -> anyhow::Result<()> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(());
    };

    if let Some(err_msg) = v.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("{err_msg}");
    }

    let failed = v.get("ok").and_then(|value| value.as_bool()) == Some(false)
        || v.get("success").and_then(|value| value.as_bool()) == Some(false);
    if failed {
        anyhow::bail!(
            "{}",
            v.get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("API 返回失败")
        );
    }

    Ok(())
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
        check_api_error(&body)?;

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
        check_api_error(&body)?;

        serde_json::from_str::<ChangeIpResponse>(&body)
            .with_context(|| format!("changeIP 响应解析失败: {}", &body[..body.len().min(200)]))
    }
}

#[cfg(test)]
mod tests {
    use super::ChangeIpResponse;

    #[test]
    fn parses_current_change_ip_response() {
        let response: ChangeIpResponse = serde_json::from_str(
            r#"{"ok":true,"message":"正在執行更換IP","uses_left":997,"next_allowed_at":1785610842}"#,
        )
        .expect("新接口响应应能解析");

        assert!(response.success);
        assert_eq!(response.message.as_deref(), Some("正在執行更換IP"));
        assert_eq!(response.uses_left, Some(997));
        assert_eq!(response.next_allowed_at, Some(1785610842));
    }

    #[test]
    fn parses_legacy_change_ip_response() {
        let response: ChangeIpResponse =
            serde_json::from_str(r#"{"success":true,"ip":"203.0.113.10"}"#)
                .expect("旧接口响应应能解析");

        assert!(response.success);
        assert_eq!(response.ip.as_deref(), Some("203.0.113.10"));
    }
}
