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
    },
    /// 选择任务文件发起网络请求发送弹幕
    /// 用法: DMsender send "TASKFILEURL"
    Send {
        /// 任务文件路径
        task_path: String,
        /// 自动模式：Retry 默认重试5次后跳过，Fatal/ReAuth 直接退出，Modify 直接跳过
        #[arg(long = "auto")]
        auto: bool,
    },
}
