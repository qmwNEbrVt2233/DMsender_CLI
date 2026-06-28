use serde::Deserialize;

use super::wbi::{WbiKeys, sign_params};
use crate::errors::{AppError, AppResult};

/// 视频信息响应
#[derive(Debug, Deserialize)]
struct VideoInfoResponse {
    code: i32,
    data: Option<VideoData>,
}

#[derive(Debug, Deserialize)]
struct VideoData {
    /// 视频标题
    pub title: String,
    /// 分页列表
    pub pages: Vec<PageInfo>,
}

#[derive(Debug, Deserialize)]
pub struct PageInfo {
    /// 分页 cid
    pub cid: u64,
    /// 分页号
    pub page: u32,
    /// 分页标题
    pub part: String,
}

/// 高级弹幕权限响应
#[derive(Debug, Deserialize)]
struct AdvStateResponse {
    code: i32,
    data: Option<AdvStateData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AdvStateData {
    /// 需要的硬币数
    #[serde(default)]
    pub coins: u64,
    /// 是否同意（1:同意 2:未同意）
    #[serde(default)]
    pub confirm: u8,
    /// 是否允许申请
    #[serde(default)]
    pub accept: bool,
    /// 是否已购买
    #[serde(default, rename = "hasBuy")]
    pub has_buy: bool,
}

/// 发送弹幕响应
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DanmakuPostResponse {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<DanmakuPostData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DanmakuPostData {
    #[serde(default)]
    pub dmid: u64,
    #[serde(default)]
    pub dmid_str: String,
}

/// 通过 bvid 查询视频信息（cid 和标题），需传入 SESSDATA 以查看私密视频
pub async fn query_video_info(bvid: &str, sessdata: &str) -> AppResult<(Vec<PageInfo>, String)> {
    let url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()?;

    let mut req = client.get(&url);
    if !sessdata.is_empty() {
        req = req.header("Cookie", format!("SESSDATA={sessdata}"));
    }

    let resp = req.send().await?;
    let body: VideoInfoResponse = resp.json().await?;

    match body.data {
        Some(data) => Ok((data.pages, data.title)),
        None => {
            let msg = match body.code {
                -404 => format!("视频不存在，bvid={bvid} 可能无效"),
                -403 => format!("没有权限访问该视频，bvid={bvid} 可能是私密视频且未登录或无权查看"),
                _ => format!("未找到视频信息，bvid={bvid} 可能无效 (code={})", body.code),
            };
            Err(AppError::Business(msg))
        }
    }
}

/// 通过 bvid 和分页号获取 cid 和分页标题
pub async fn get_cid_by_page(bvid: &str, page: u32, sessdata: &str) -> AppResult<(u64, String)> {
    let (pages, _title) = query_video_info(bvid, sessdata).await?;

    pages
        .iter()
        .find(|p| p.page == page)
        .map(|p| (p.cid, p.part.clone()))
        .ok_or_else(|| {
            AppError::Business(format!(
                "未找到分页号 {page}，该视频共有 {} 个分页",
                pages.len()
            ))
        })
}

#[allow(dead_code)]
/// 检测是否有有效的 bvid
pub async fn check_bvid_exists(bvid: &str, sessdata: &str) -> AppResult<bool> {
    match query_video_info(bvid, sessdata).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// 高级弹幕权限检查结果
#[derive(Debug)]
#[allow(dead_code)]
pub enum AdvPermissionResult {
    /// 有权限
    Granted(AdvStateData),
    /// 无权限
    Denied(AdvStateData),
    /// 需要重新认证（-101）
    NeedReAuth,
    /// 其他错误
    Error(String),
}

/// 检测高级弹幕发送权限
pub async fn check_advanced_permission(sessdata: &str, cid: u64) -> AdvPermissionResult {
    let url = "https://api.bilibili.com/x/dm/adv/state";

    let client = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
    {
        Ok(c) => c,
        Err(e) => return AdvPermissionResult::Error(format!("创建 HTTP 客户端失败: {e}")),
    };

    let resp = match client
        .get(url)
        .query(&[("cid", cid.to_string()), ("mode", "sp".to_string())])
        .header("Cookie", format!("SESSDATA={sessdata}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return AdvPermissionResult::Error(format!("网络请求失败: {e}")),
    };

    let body: AdvStateResponse = match resp.json().await {
        Ok(b) => b,
        Err(e) => return AdvPermissionResult::Error(format!("解析响应失败: {e}")),
    };

    match body.code {
        0 => {
            let has_permission = body
                .data
                .as_ref()
                .map_or(false, |d| d.confirm == 1 && d.has_buy);
            if has_permission {
                AdvPermissionResult::Granted(body.data.unwrap_or(AdvStateData {
                    coins: 0,
                    confirm: 0,
                    accept: false,
                    has_buy: false,
                }))
            } else {
                AdvPermissionResult::Denied(body.data.unwrap_or(AdvStateData {
                    coins: 0,
                    confirm: 0,
                    accept: false,
                    has_buy: false,
                }))
            }
        }
        -101 | -400 => AdvPermissionResult::NeedReAuth,
        code => AdvPermissionResult::Error(format!("查询高级弹幕权限失败: code={code}")),
    }
}

/// 发送一条弹幕（带 WBI 签名）
pub async fn post_danmaku(
    sessdata: &str,
    csrf: &str,
    bvid: &str,
    cid: u64,
    pool: u8,
    mode: u8,
    msg: &str,
    progress: u64,
    color: u32,
    fontsize: u8,
    wbi_keys: &WbiKeys,
) -> AppResult<DanmakuPostResponse> {
    let url = "https://api.bilibili.com/x/v2/dm/post";

    let rnd = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
        + "000000";

    // 构建参数列表（用于 WBI 签名）
    let params_for_sign: Vec<(&str, String)> = vec![
        ("type", "1".to_string()),
        ("oid", cid.to_string()),
        ("msg", msg.to_string()),
        ("bvid", bvid.to_string()),
        ("progress", progress.to_string()),
        ("color", color.to_string()),
        ("fontsize", fontsize.to_string()),
        ("pool", pool.to_string()),
        ("mode", mode.to_string()),
        ("rnd", rnd.clone()),
        ("csrf", csrf.to_string()),
    ];

    let (w_rid, wts) = sign_params(&params_for_sign, &wbi_keys.mixin_key);

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()?;

    let resp = client
        .post(url)
        .header("Cookie", format!("SESSDATA={sessdata}"))
        .form(&[
            ("type", "1"),
            ("oid", &cid.to_string()),
            ("msg", msg),
            ("bvid", bvid),
            ("progress", &progress.to_string()),
            ("color", &color.to_string()),
            ("fontsize", &fontsize.to_string()),
            ("pool", &pool.to_string()),
            ("mode", &mode.to_string()),
            ("rnd", &rnd),
            ("csrf", csrf),
            ("w_rid", &w_rid),
            ("wts", &wts),
        ])
        .send()
        .await?;

    let body: DanmakuPostResponse = resp.json().await?;
    Ok(body)
}
