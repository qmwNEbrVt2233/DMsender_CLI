use std::fmt;

/// 应用错误类型
#[derive(Debug)]
pub enum AppError {
    /// 网络请求错误
    Http(String),
    /// XML 解析错误
    XmlParse(String),
    /// 文件 I/O 错误
    Io(std::io::Error),
    /// JSON 序列化/反序列化错误
    Json(serde_json::Error),
    /// 用户取消操作
    Cancelled(String),
    /// 业务逻辑错误
    Business(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Http(msg) => write!(f, "HTTP 请求错误: {msg}"),
            AppError::XmlParse(msg) => write!(f, "XML 解析错误: {msg}"),
            AppError::Io(e) => write!(f, "IO 错误: {e}"),
            AppError::Json(e) => write!(f, "JSON 错误: {e}"),
            AppError::Cancelled(msg) => write!(f, "操作已取消: {msg}"),
            AppError::Business(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Json(e)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Http(e.to_string())
    }
}

impl From<quick_xml::Error> for AppError {
    fn from(e: quick_xml::Error) -> Self {
        AppError::XmlParse(e.to_string())
    }
}

/// 类型别名
pub type AppResult<T> = Result<T, AppError>;
