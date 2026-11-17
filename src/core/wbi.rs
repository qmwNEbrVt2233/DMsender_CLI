use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::api::{http_client, parse_json_response};
use crate::errors::{AppError, AppResult};

/// WBI 密钥刷新阈值（6 小时）
const WBI_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// WBI 重排映射表
const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// WBI 密钥对
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct WbiKeys {
    pub img_key: String,
    pub sub_key: String,
    /// 计算后的 mixin_key
    pub mixin_key: String,
    /// 获取密钥的时间点，用于长任务定期刷新
    pub fetched_at: Instant,
}

impl WbiKeys {
    /// 从 nav 接口获取 WBI 密钥
    pub async fn fetch() -> AppResult<Self> {
        let client = http_client()?;

        #[derive(Deserialize)]
        struct NavResponse {
            data: Option<NavData>,
        }

        #[derive(Deserialize)]
        struct NavData {
            wbi_img: Option<WbiImg>,
        }

        #[derive(Deserialize)]
        struct WbiImg {
            img_url: String,
            sub_url: String,
        }

        let resp = client
            .get("https://api.bilibili.com/x/web-interface/nav")
            .send()
            .await?;

        let body: NavResponse = parse_json_response(resp).await?;

        let wbi_img = body
            .data
            .and_then(|d| d.wbi_img)
            .ok_or_else(|| AppError::Business("无法获取 WBI 签名密钥".to_string()))?;

        let img_key = extract_key_from_url(&wbi_img.img_url)?;
        let sub_key = extract_key_from_url(&wbi_img.sub_url)?;
        let raw = img_key.clone() + &sub_key;
        let mixin_key = get_mixin_key(raw.as_bytes())?;

        Ok(WbiKeys {
            img_key,
            sub_key,
            mixin_key,
            fetched_at: Instant::now(),
        })
    }

    /// 检查密钥是否已超过刷新周期
    pub fn is_stale(&self) -> bool {
        self.fetched_at.elapsed() >= WBI_REFRESH_INTERVAL
    }
}

/// 从 WBI 图片 URL 中提取文件名（即 key）
fn extract_key_from_url(url: &str) -> AppResult<String> {
    // URL 格式: https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png
    let trimmed = url.trim();
    let filename = trimmed
        .rsplit('/')
        .next()
        .and_then(|s| s.split('.').next())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Business(format!("无法解析 WBI URL: {url}")))?;
    Ok(filename.to_string())
}

/// 对 img_key + sub_key 进行字符顺序打乱编码，取前 32 位
fn get_mixin_key(orig: &[u8]) -> AppResult<String> {
    if orig.len() < MIXIN_KEY_ENC_TAB.len() {
        return Err(AppError::Business(format!(
            "WBI 原始密钥长度不足（期望至少 {} 字节，实际 {} 字节）",
            MIXIN_KEY_ENC_TAB.len(),
            orig.len()
        )));
    }
    let key = MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .map(|&i| orig[i] as char)
        .collect::<String>();
    Ok(key)
}

/// URL 编码（按 WBI 规范：大写十六进制，空格编码为 %20）
fn wbi_url_encode(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
                Some(c.to_string())
            } else if "!'()".contains(c) {
                // 过滤这些字符（WBI 协议规定）
                None
            } else {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                let hex = encoded
                    .bytes()
                    .map(|b| format!("%{:02X}", b))
                    .collect::<String>();
                Some(hex)
            }
        })
        .collect::<String>()
}

/// 为请求参数计算 WBI 签名
///
/// 返回 (w_rid, wts) 元组
pub fn sign_params(params: &[(&str, String)], mixin_key: &str) -> (String, String) {
    let wts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    // 复制参数列表，添加 wts
    let mut signed_params: Vec<(&str, String)> = params.to_vec();
    signed_params.push(("wts", wts.clone()));

    // 按 key 升序排序
    signed_params.sort_by(|a, b| a.0.cmp(b.0));

    // 拼接为 query string
    let query_string: String = signed_params
        .iter()
        .map(|(k, v)| format!("{}={}", wbi_url_encode(k), wbi_url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    // 拼接 mixin_key 并计算 MD5
    let sign_input = query_string + mixin_key;
    let w_rid = format!("{:x}", md5::compute(sign_input.as_bytes()));

    (w_rid, wts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_mixin_key() {
        let img_key = "7cd084941338484aae1ad9425b84077c";
        let sub_key = "4932caff0ff746eab6f01bf08b70ac45";
        let raw = img_key.to_string() + sub_key;
        let mixin = get_mixin_key(raw.as_bytes()).unwrap();
        assert_eq!(mixin, "ea1db124af3c7062474693fa704f4ff8");
    }

    #[test]
    fn test_get_mixin_key_rejects_short_input() {
        assert!(get_mixin_key(b"too_short").is_err());
    }

    #[test]
    fn test_extract_key_from_url() {
        let url = "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png";
        let key = extract_key_from_url(url).unwrap();
        assert_eq!(key, "7cd084941338484aae1ad9425b84077c");
    }

    #[test]
    fn test_extract_key_from_empty_or_invalid_url_fails() {
        assert!(extract_key_from_url("").is_err());
        assert!(extract_key_from_url("https://i0.hdslb.com/bfs/wbi/.png").is_err());
        assert!(extract_key_from_url("https://i0.hdslb.com/bfs/wbi/").is_err());
    }

    #[test]
    fn test_wbi_url_encode() {
        assert_eq!(wbi_url_encode("foo"), "foo");
        assert_eq!(wbi_url_encode("one one four"), "one%20one%20four");
        assert_eq!(wbi_url_encode("五一四"), "%E4%BA%94%E4%B8%80%E5%9B%9B");
    }

    #[test]
    fn test_sign_params() {
        let params = vec![
            ("foo", "114".to_string()),
            ("bar", "514".to_string()),
            ("zab", "1919810".to_string()),
        ];
        let mixin_key = "ea1db124af3c7062474693fa704f4ff8";
        let (w_rid, wts) = sign_params(&params, mixin_key);
        assert!(!w_rid.is_empty());
        assert!(!wts.is_empty());
        assert_eq!(w_rid.len(), 32);
    }
}
