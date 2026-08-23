use clap::{Parser, Subcommand};

/// DMsender_CLI — Bilibili 弹幕发送命令行工具
#[derive(Debug, Parser)]
#[command(name = "DMsender", version, about = "Bilibili 弹幕发送命令行工具")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 根据 XML 创建任务文件
    /// 用法: DMsender create "XMLFILEURL"
    Create {
        /// XML 文件的路径（本地路径或 URL）
        xml_path: String,
        /// 启用严格校验模式，过滤非法数据
        #[arg(short = 'r', long = "rigor")]
        rigor: bool,
        /// 指定任务文件的输出路径
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
        /// 创建完成后直接启动发送流程
        #[arg(long = "sendafter")]
        sendafter: bool,
        /// 对转换后的 progress 进行偏移（单位 ms，支持 +/-）
        #[arg(long = "timeoffset")]
        timeoffset: Option<String>,
        /// 自动模式：所有需要用户选择的地方自动跳过
        /// （仅在同时指定 --sendafter 时有效）
        #[arg(long = "auto")]
        auto: bool,
        /// SESSDATA（接口模式；与 --bili_jct 及视频信息一起使用）
        #[arg(long = "sessdata")]
        sessdata: Option<String>,
        /// bili_jct / csrf（接口模式；与 --sessdata 及视频信息一起使用）
        #[arg(long = "bili_jct")]
        bili_jct: Option<String>,
        /// 视频 bvid（接口模式；需与 --page 一起使用，或改用 --cid）
        #[arg(long = "bvid")]
        bvid: Option<String>,
        /// 视频分页号（接口模式；需与 --bvid 一起使用）
        #[arg(long = "page")]
        page: Option<u32>,
        /// 视频 cid（接口模式；可替代 --bvid + --page）
        #[arg(long = "cid")]
        cid: Option<u64>,
        /// 目标弹幕池（接口模式必需，0:普通池 1:字幕池 2:特殊池）
        #[arg(long = "pool")]
        pool: Option<u8>,
        /// 指定移除项；无此标识默认移除 mode=8,9，仅写 --remove 则不移除任何项
        #[arg(long = "remove", num_args = 0..=1, require_equals = true)]
        remove: Option<Option<String>>,
        /// 创建后发送时的发送间隔（毫秒，仅 --sendafter 时有效）
        #[arg(long = "interval")]
        interval: Option<u64>,
        /// 创建后发送时的起始发送 ID，或 last 表示从上次进度继续（仅 --sendafter 时有效）
        #[arg(long = "sendfrom")]
        sendfrom: Option<String>,
        /// 创建后发送时的重试间隔（毫秒，仅 --sendafter 时有效）
        #[arg(long = "retryinterval")]
        retryinterval: Option<u64>,
        /// 创建后发送时的自动重试次数，或 inf 表示无限重试（仅 --sendafter 时有效）
        #[arg(long = "autoretryfrequency")]
        autoretryfrequency: Option<String>,
    },
    /// 选择任务文件发起网络请求发送弹幕
    /// 用法: DMsender send "TASKFILEURL"
    Send {
        /// 任务文件路径
        task_path: String,
        /// 自动模式：Retry 默认重试5次后跳过，Fatal/ReAuth 直接退出，Modify 直接跳过
        #[arg(long = "auto")]
        auto: bool,
        /// 发送间隔（毫秒）
        #[arg(long = "interval")]
        interval: Option<u64>,
        /// 起始发送 ID，或 last 表示从上次进度继续
        #[arg(long = "sendfrom")]
        sendfrom: Option<String>,
        /// 重试间隔（毫秒）
        #[arg(long = "retryinterval")]
        retryinterval: Option<u64>,
        /// 自动重试次数，或 inf 表示无限重试
        #[arg(long = "autoretryfrequency")]
        autoretryfrequency: Option<String>,
    },
}
