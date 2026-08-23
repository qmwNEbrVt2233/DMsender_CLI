mod actions;
mod cli;
mod core;
mod errors;

use clap::Parser;
use cli::args::{Cli, Command};

use errors::{AppError, AppResult};

use crate::actions::{AutoRetry, CreateInputs, CreateVideoInput, SendFrom, SendOptions};

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

fn parse_remove_modes(raw: Option<Option<String>>) -> AppResult<Vec<u8>> {
    let Some(raw) = raw else {
        return Ok(vec![8, 9]);
    };
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let modes = trimmed.strip_prefix("mode=").ok_or_else(|| {
        AppError::Business("--remove 格式错误，应为 --remove=\"mode=1,2,3\"".to_string())
    })?;
    if modes.trim().is_empty() {
        return Err(AppError::Business(
            "--remove 至少需要指定一个 mode".to_string(),
        ));
    }

    let mut parsed = Vec::new();
    for mode in modes.split(',') {
        let mode = mode.trim();
        if mode.is_empty() {
            return Err(AppError::Business(
                "--remove 中存在空的 mode 项".to_string(),
            ));
        }
        let mode = mode
            .parse::<u8>()
            .map_err(|_| AppError::Business(format!("--remove 中的 mode={mode} 不是合法数字")))?;
        if !parsed.contains(&mode) {
            parsed.push(mode);
        }
    }
    Ok(parsed)
}

fn parse_create_inputs(
    sessdata: Option<String>,
    bili_jct: Option<String>,
    bvid: Option<String>,
    page: Option<u32>,
    cid: Option<u64>,
    pool: Option<u8>,
) -> AppResult<Option<CreateInputs>> {
    let interface_mode = sessdata.is_some()
        || bili_jct.is_some()
        || bvid.is_some()
        || page.is_some()
        || cid.is_some()
        || pool.is_some();
    if !interface_mode {
        return Ok(None);
    }

    let sessdata = required_non_empty(sessdata, "--sessdata")?;
    let csrf = required_non_empty(bili_jct, "--bili_jct")?;
    let pool = pool.ok_or_else(|| AppError::Business("接口模式缺少必需参数 --pool".to_string()))?;
    validate_pool(pool)?;

    let video = match (bvid, page, cid) {
        (Some(bvid), Some(page), None) => {
            let bvid = bvid.trim().to_string();
            if bvid.is_empty() {
                return Err(AppError::Business("--bvid 不能为空".to_string()));
            }
            if !bvid.to_uppercase().starts_with("BV") {
                return Err(AppError::Business(
                    "--bvid 格式不正确，应以 BV 开头".to_string(),
                ));
            }
            CreateVideoInput::BvidPage { bvid, page }
        }
        (None, None, Some(cid)) => CreateVideoInput::Cid(cid),
        (Some(_), Some(_), Some(_)) => {
            return Err(AppError::Business(
                "视频信息只能指定 --bvid + --page 或 --cid，不能同时使用".to_string(),
            ));
        }
        _ => {
            return Err(AppError::Business(
                "接口模式缺少视频信息：请指定 --bvid <BVID> --page <PAGE>，或直接指定 --cid <CID>"
                    .to_string(),
            ));
        }
    };

    Ok(Some(CreateInputs {
        sessdata,
        csrf,
        video,
        pool,
    }))
}

fn validate_pool(pool: u8) -> AppResult<()> {
    if pool > 2 {
        return Err(AppError::Business("--pool 必须为 0、1 或 2".to_string()));
    }
    Ok(())
}

fn required_non_empty(value: Option<String>, name: &str) -> AppResult<String> {
    let value = value.ok_or_else(|| AppError::Business(format!("接口模式缺少必需参数 {name}")))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(AppError::Business(format!("{name} 不能为空")));
    }
    Ok(value)
}

fn parse_send_options(
    interval: Option<u64>,
    sendfrom: Option<String>,
    retryinterval: Option<u64>,
    autoretryfrequency: Option<String>,
) -> AppResult<SendOptions> {
    Ok(SendOptions {
        interval_ms: interval,
        send_from: parse_send_from(sendfrom)?,
        retry_interval_ms: retryinterval.unwrap_or(10_000),
        auto_retry: parse_auto_retry(autoretryfrequency)?,
    })
}

fn parse_send_from(raw: Option<String>) -> AppResult<Option<SendFrom>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("last") {
        return Ok(Some(SendFrom::Last));
    }
    let id = raw
        .parse::<u64>()
        .map_err(|_| AppError::Business("--sendfrom 应为 last 或正整数 ID".to_string()))?;
    if id == 0 {
        return Err(AppError::Business("--sendfrom ID 必须大于 0".to_string()));
    }
    Ok(Some(SendFrom::Id(id)))
}

fn parse_auto_retry(raw: Option<String>) -> AppResult<AutoRetry> {
    let Some(raw) = raw else {
        return Ok(AutoRetry::Finite(5));
    };
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("inf") {
        return Ok(AutoRetry::Infinite);
    }
    let attempts = raw
        .parse::<u32>()
        .map_err(|_| AppError::Business("--autoretryfrequency 应为正整数或 inf".to_string()))?;
    if attempts == 0 {
        return Err(AppError::Business(
            "--autoretryfrequency 必须大于 0，或使用 inf".to_string(),
        ));
    }
    Ok(AutoRetry::Finite(attempts))
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
            sessdata,
            bili_jct,
            bvid,
            page,
            cid,
            pool,
            remove,
            interval,
            sendfrom,
            retryinterval,
            autoretryfrequency,
        } => {
            let create_inputs = parse_create_inputs(sessdata, bili_jct, bvid, page, cid, pool)?;
            let remove_modes = parse_remove_modes(remove)?;
            let send_options =
                parse_send_options(interval, sendfrom, retryinterval, autoretryfrequency)?;
            let task_path = actions::handle_create(
                xml_path,
                rigor,
                output,
                parse_timeoffset(timeoffset),
                create_inputs,
                remove_modes,
                auto && sendafter,
            )
            .await?;
            if sendafter {
                actions::handle_send(task_path, auto, send_options).await?;
            }
        }
        Command::Send {
            task_path,
            auto,
            interval,
            sendfrom,
            retryinterval,
            autoretryfrequency,
        } => {
            let send_options =
                parse_send_options(interval, sendfrom, retryinterval, autoretryfrequency)?;
            actions::handle_send(task_path, auto, send_options).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_remove_modes_defaults_to_unsupported_modes() {
        let modes = parse_remove_modes(None).unwrap();
        assert_eq!(modes, vec![8, 9]);
    }

    #[test]
    fn test_parse_remove_modes_empty_flag_keeps_all_modes() {
        let modes = parse_remove_modes(Some(None)).unwrap();
        assert!(modes.is_empty());

        let modes = parse_remove_modes(Some(Some(String::new()))).unwrap();
        assert!(modes.is_empty());
    }

    #[test]
    fn test_parse_remove_modes_custom_rule() {
        let modes = parse_remove_modes(Some(Some("mode=1,2,3,8,9".to_string()))).unwrap();
        assert_eq!(modes, vec![1, 2, 3, 8, 9]);
    }

    #[test]
    fn test_parse_create_inputs_requires_pool_in_interface_mode() {
        let result = parse_create_inputs(
            Some("sess".to_string()),
            Some("csrf".to_string()),
            None,
            None,
            Some(123),
            None,
        );
        assert!(result.is_err());
    }
}
