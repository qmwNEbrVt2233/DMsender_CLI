mod actions;
mod cli;
mod core;
mod errors;

use clap::Parser;
use cli::args::{Cli, Command};

use errors::AppResult;

/// 解析 --timeoffset 参数，格式为 +/-TIME(ms)
fn parse_timeoffset(raw: Option<String>) -> Option<i64> {
    raw.as_deref().and_then(|s| {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // 支持 +1000, -500, 1000 等形式
        if let Some(stripped) = s.strip_prefix('+') {
            stripped.parse::<i64>().ok()
        } else {
            s.parse::<i64>().ok()
        }
    })
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Create {
            xml_path,
            rigor,
            output,
            sendafter,
            timeoffset,
            auto,
        } => {
            let task_path = actions::handle_create(
                xml_path,
                rigor,
                output,
                parse_timeoffset(timeoffset),
                auto && sendafter,
            )
            .await?;
            if sendafter {
                actions::handle_send(task_path, auto).await?;
            }
        }
        Command::Send { task_path, auto } => {
            actions::handle_send(task_path, auto).await?;
        }
    }

    Ok(())
}
