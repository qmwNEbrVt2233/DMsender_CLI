use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::wbi::{WbiKeys, sign_params};
use crate::errors::{AppError, AppResult};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const CLI_USER_AGENT: &str = concat!("DMsender_CLI/", env!("CARGO_PKG_VERSION"));

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// 获取带超时与连接池复用的共享 HTTP 客户端
pub fn http_client() -> AppResult<reqwest::Client> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let _ = HTTP_CLIENT.set(client.clone());
    Ok(HTTP_CLIENT.get().cloned().unwrap_or(client))
}

/// 检查 HTTP 状态码并反序列化 JSON 响应
pub async fn parse_json_response<T: DeserializeOwned>(resp: reqwest::Response) -> AppResult<T> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let preview: String = text.chars().take(200).collect();
        return Err(AppError::Http(format!(
            "HTTP 状态异常 {}: {}",
            status.as_u16(),
            preview.trim()
        )));
    }
    serde_json::from_str::<T>(&text).map_err(|e| {
        let preview: String = text.chars().take(200).collect();
        AppError::Http(format!(
            "响应 JSON 解析失败: {e}（响应片段: {}）",
            preview.trim()
        ))
    })
}

/// 视频信息响应
#[derive(Debug, Deserialize)]
struct VideoInfoResponse {
    code: i32,
    #[serde(default)]
    message: String,
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
    #[serde(default)]
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

/// 获取临时 buvid3（用于在触发 412 风控时携带浏览器设备指纹 Cookie 重试）
async fn fetch_buvid3(client: &reqwest::Client) -> AppResult<String> {
    #[derive(Deserialize)]
    struct SpiResponse {
        code: i32,
        data: Option<SpiData>,
    }

    #[derive(Deserialize)]
    struct SpiData {
        b_3: String,
    }

    let resp = client
        .get("https://api.bilibili.com/x/frontend/finger/spi")
        .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
        .send()
        .await?;
    let body: SpiResponse = parse_json_response(resp).await?;
    if body.code != 0 {
        return Err(AppError::Http(format!(
            "获取 buvid3 失败 (code={})",
            body.code
        )));
    }
    body.data
        .map(|d| d.b_3)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Http("获取 buvid3 返回空数据".to_string()))
}

/// 通过 bvid 查询视频信息（cid 和标题），需传入 SESSDATA 以查看私密视频
pub async fn query_video_info(bvid: &str, sessdata: &str) -> AppResult<(Vec<PageInfo>, String)> {
    let url = "https://api.bilibili.com/x/web-interface/view";
    let client = http_client()?;

    // B 站 view 接口会对无 buvid3 的浏览器 UA 返回 412，默认先使用 CLI UA（与 curl 一致）
    let mut req = client
        .get(url)
        .query(&[("bvid", bvid)])
        .header(reqwest::header::USER_AGENT, CLI_USER_AGENT);
    if !sessdata.is_empty() {
        req = req.header(reqwest::header::COOKIE, format!("SESSDATA={sessdata}"));
    }

    let mut resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED
        && let Ok(buvid3) = fetch_buvid3(&client).await
    {
        let cookie = if sessdata.is_empty() {
            format!("buvid3={buvid3}")
        } else {
            format!("buvid3={buvid3}; SESSDATA={sessdata}")
        };
        resp = client
            .get(url)
            .query(&[("bvid", bvid)])
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .header(reqwest::header::REFERER, "https://www.bilibili.com/")
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await?;
    }

    let body: VideoInfoResponse = parse_json_response(resp).await?;

    if body.code != 0 {
        let msg = match body.code {
            -404 => format!("视频不存在，bvid={bvid} 可能无效"),
            -403 => format!("没有权限访问该视频，bvid={bvid} 可能是私密视频且未登录或无权查看"),
            _ => format!(
                "查询视频信息失败，bvid={bvid} (code={}, message={})",
                body.code, body.message
            ),
        };
        return Err(AppError::Business(msg));
    }

    match body.data {
        Some(data) => Ok((data.pages, data.title)),
        None => Err(AppError::Business(format!(
            "未找到视频信息，bvid={bvid} 返回空数据"
        ))),
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
    /// 需要重新认证（-101/-102/-111/-400）
    NeedReAuth,
    /// 其他错误
    Error(String),
}

/// 检测高级弹幕发送权限
pub async fn check_advanced_permission(sessdata: &str, cid: u64) -> AdvPermissionResult {
    let url = "https://api.bilibili.com/x/dm/adv/state";

    let client = match http_client() {
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

    let body: AdvStateResponse = match parse_json_response(resp).await {
        Ok(b) => b,
        Err(e) => return AdvPermissionResult::Error(format!("解析响应失败: {e}")),
    };

    match body.code {
        0 => match body.data {
            Some(data) if data.confirm == 1 && data.has_buy => AdvPermissionResult::Granted(data),
            Some(data) => AdvPermissionResult::Denied(data),
            None => AdvPermissionResult::Error("查询高级弹幕权限返回空数据".to_string()),
        },
        -101 | -102 | -111 | -400 => AdvPermissionResult::NeedReAuth,
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
    let mut params_for_sign: Vec<(&str, String)> = vec![
        ("type", "1".to_string()),
        ("oid", cid.to_string()),
        ("msg", msg.to_string()),
        ("progress", progress.to_string()),
        ("color", color.to_string()),
        ("fontsize", fontsize.to_string()),
        ("pool", pool.to_string()),
        ("mode", mode.to_string()),
        ("rnd", rnd.clone()),
        ("csrf", csrf.to_string()),
    ];
    if !bvid.is_empty() {
        params_for_sign.push(("bvid", bvid.to_string()));
    }

    let (w_rid, wts) = sign_params(&params_for_sign, &wbi_keys.mixin_key);

    let client = http_client()?;

    let mut form = params_for_sign;
    form.push(("w_rid", w_rid));
    form.push(("wts", wts));

    let resp = client
        .post(url)
        .header("Cookie", format!("SESSDATA={sessdata}"))
        .form(&form)
        .send()
        .await?;

    let body: DanmakuPostResponse = parse_json_response(resp).await?;
    Ok(body)
}
