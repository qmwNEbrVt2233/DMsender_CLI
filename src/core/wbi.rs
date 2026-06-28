use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::errors::{AppError, AppResult};

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
}

impl WbiKeys {
    /// 从 nav 接口获取 WBI 密钥
    pub async fn fetch() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

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

        let body: NavResponse = resp.json().await?;

        let wbi_img = body
            .data
            .and_then(|d| d.wbi_img)
            .ok_or_else(|| AppError::Business("无法获取 WBI 签名密钥".to_string()))?;

        let img_key = extract_key_from_url(&wbi_img.img_url)?;
        let sub_key = extract_key_from_url(&wbi_img.sub_url)?;
        let raw = img_key.clone() + &sub_key;
        let mixin_key = get_mixin_key(raw.as_bytes());

        Ok(WbiKeys {
            img_key,
            sub_key,
            mixin_key,
        })
    }
}

/// 从 WBI 图片 URL 中提取文件名（即 key）
fn extract_key_from_url(url: &str) -> AppResult<String> {
    // URL 格式: https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png
    let filename = url
        .rsplit('/')
        .next()
        .and_then(|s| s.split('.').next())
        .ok_or_else(|| AppError::Business(format!("无法解析 WBI URL: {url}")))?;
    Ok(filename.to_string())
}

/// 对 img_key + sub_key 进行字符顺序打乱编码，取前 32 位
fn get_mixin_key(orig: &[u8]) -> String {
    MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .map(|&i| orig.get(i).copied().unwrap_or(b'\0') as char)
        .collect::<String>()
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
        let mixin = get_mixin_key(raw.as_bytes());
        assert_eq!(mixin, "ea1db124af3c7062474693fa704f4ff8");
    }

    #[test]
    fn test_extract_key_from_url() {
        let url = "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png";
        let key = extract_key_from_url(url).unwrap();
        assert_eq!(key, "7cd084941338484aae1ad9425b84077c");
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
