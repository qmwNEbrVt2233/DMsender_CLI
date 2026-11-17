use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::core::api::{self, AdvPermissionResult};
use crate::core::task::{DanmakuTask, TaskFile};
use crate::core::wbi::WbiKeys;
use crate::core::xml_parser;

use crate::cli::prompt::{self, ModifyChoice};
use crate::errors::{AppError, AppResult};

#[derive(Debug, Clone)]
pub enum CreateVideoInput {
    BvidPage { bvid: String, page: u32 },
    Cid(u64),
}

#[derive(Debug, Clone)]
pub struct CreateInputs {
    pub sessdata: String,
    pub csrf: String,
    pub video: CreateVideoInput,
    pub pool: u8,
}

#[derive(Debug, Clone)]
pub enum SendFrom {
    Last,
    Id(u64),
}

#[derive(Debug, Clone)]
pub enum AutoRetry {
    Finite(u32),
    Infinite,
}

#[derive(Debug, Clone)]
pub struct SendOptions {
    pub interval_ms: Option<u64>,
    pub send_from: Option<SendFrom>,
    pub retry_interval_ms: u64,
    pub auto_retry: AutoRetry,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions {
            interval_ms: None,
            send_from: None,
            retry_interval_ms: 10_000,
            auto_retry: AutoRetry::Finite(5),
        }
    }
}

/// 键盘监听句柄，通过原子标志位优雅退出，避免强行 abort 阻塞读取抢占后续交互输入
struct KeyboardListener {
    stop_flag: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

// ═══════════════════════════════════════════════════════════════
//  create 命令
// ═══════════════════════════════════════════════════════════════
pub async fn handle_create(
    xml_path: String,
    rigor: bool,
    output_path: Option<String>,
    time_offset_ms: Option<i64>,
    create_inputs: Option<CreateInputs>,
    remove_modes: Vec<u8>,
    auto_mode: bool,
) -> AppResult<String> {
    println!("═══════════════════════════════════════");
    println!("      DMsender_CLI — 创建任务文件");
    println!("═══════════════════════════════════════");
    println!();

    // 1. 读取 XML
    let xml_content = read_xml_file(&xml_path).await?;
    println!(
        "✓ XML 文件已读取 ({:.1} KB)",
        xml_content.len() as f64 / 1024.0
    );

    // 2. 优先解析 XML，避免账号/视频信息都确认后才暴露语法或数据问题
    println!();
    if rigor {
        println!("正在解析 XML 弹幕数据（严格校验模式）...");
    } else {
        println!("正在解析 XML 弹幕数据...");
    }
    let parse_result =
        xml_parser::parse_danmaku_xml(&xml_content, rigor, time_offset_ms, &remove_modes)?;

    println!("✓ 解析完成:");
    println!("  - 有效弹幕: {} 条", parse_result.tasks.len());
    for (mode, count) in &parse_result.removed_modes {
        println!("  - 已按规则剔除 mode={mode} 弹幕: {count} 条");
    }
    if parse_result.rigor_removed_count > 0 {
        println!(
            "  - 严格校验剔除: {} 条（字段或内容不合法）",
            parse_result.rigor_removed_count
        );
    }

    let interface_mode = create_inputs.is_some();

    // 3. 获取 SESSDATA 和 bili_jct（接口模式不询问）
    let (mut sessdata, mut csrf, video_input, pool_input) = if let Some(inputs) = create_inputs {
        (
            inputs.sessdata,
            inputs.csrf,
            Some(inputs.video),
            Some(inputs.pool),
        )
    } else {
        let sessdata =
            prompt_non_empty_hidden("请输入 SESSDATA（Cookie 中的 SESSDATA）", "SESSDATA")?;
        let csrf =
            prompt_non_empty_hidden("请输入 bili_jct（Cookie 中的 bili_jct / csrf）", "bili_jct")?;
        (sessdata, csrf, None, None)
    };

    // 4. 获取视频信息（接口模式缺项已在入口处报错）
    let (bvid, page, cid, page_title) = match video_input {
        Some(CreateVideoInput::BvidPage { bvid, page }) => {
            println!();
            println!("正在查询视频信息...");
            let (cid, title) = api::get_cid_by_page(&bvid, page, &sessdata).await?;
            println!("✓ 视频标题: {title}");
            println!("✓ 分页 cid: {cid}");
            (bvid, page, cid, title)
        }
        Some(CreateVideoInput::Cid(cid)) => {
            println!("✓ 使用指定 cid: {cid}");
            (String::new(), 0, cid, format!("CID {cid}"))
        }
        None => loop {
            let bvid = prompt::ask_input("请输入视频 bvid")?;
            if bvid.trim().is_empty() {
                println!("⚠ bvid 不能为空，请重新输入");
                continue;
            }
            if !bvid.to_uppercase().starts_with("BV")
                || bvid.len() < 3
                || !bvid.chars().all(|c| c.is_ascii_alphanumeric())
            {
                println!("⚠ bvid 格式不正确（应以 BV 开头且仅包含字母和数字），请重新输入");
                continue;
            }

            let page = prompt::ask_number("请输入分页号")?;
            if page == 0 {
                println!("⚠ 分页号必须大于 0，请重新输入");
                continue;
            }

            println!();
            println!("正在查询视频信息...");
            match api::get_cid_by_page(&bvid, page, &sessdata).await {
                Ok((cid, title)) => {
                    println!("✓ 视频标题: {title}");
                    println!("✓ 分页 cid: {cid}");
                    break (bvid, page, cid, title);
                }
                Err(e) => {
                    println!("⚠ 查询失败: {e}");
                    let retry = prompt::ask_confirm("是否重新输入 bvid 和分页号？")?;
                    if !retry {
                        return Err(AppError::Cancelled("取消创建".to_string()));
                    }
                }
            }
        },
    };

    // 5. 获取目标弹幕池；接口模式缺项已在入口处报错
    let pool: u8 = if let Some(pool) = pool_input {
        println!("目标弹幕池: {pool}");
        pool
    } else {
        loop {
            let pool_str = prompt::ask_input_with_default(
                "请输入目标弹幕池（0:普通池 1:字幕池 2:特殊池）",
                "1",
            )?;
            match pool_str.parse::<u8>() {
                Ok(p) if p <= 2 => break p,
                _ => println!("⚠ 弹幕池必须为 0、1 或 2，请重新输入"),
            }
        }
    };

    // 6. 检测 mode=7 高级弹幕权限
    if parse_result.has_mode7 {
        println!();
        if auto_mode || interface_mode {
            println!("检测到 mode=7 高级弹幕（自动/接口模式：跳过权限检查，继续创建）");
        } else {
            println!("检测到 mode=7 高级弹幕，正在检查发送权限...");

            let mut sessdata_for_check = sessdata.clone();
            let mut csrf_for_check = csrf.clone();
            let mut credentials_updated = false;

            loop {
                match api::check_advanced_permission(&sessdata_for_check, cid).await {
                    AdvPermissionResult::Granted(_) => {
                        println!("✓ 高级弹幕发送权限已确认");
                        break;
                    }
                    AdvPermissionResult::Denied(_) => {
                        println!("⚠ 没有高级弹幕发送权限");
                        let proceed = prompt::ask_confirm(
                            "是否继续创建任务文件？(mode=7 弹幕将保留但发送时可能失败)",
                        )?;
                        if !proceed {
                            return Err(AppError::Cancelled("取消创建".to_string()));
                        }
                        break;
                    }
                    AdvPermissionResult::NeedReAuth => {
                        println!("⚠ 账号未登录或凭据过期/格式无效");
                        if !prompt::ask_confirm("是否重新输入凭据并重试权限检查？")?
                        {
                            return Err(AppError::Cancelled("取消创建".to_string()));
                        }
                        sessdata_for_check =
                            prompt_non_empty_hidden("请重新输入 SESSDATA", "SESSDATA")?;
                        csrf_for_check =
                            prompt_non_empty_hidden("请重新输入 bili_jct", "bili_jct")?;
                        credentials_updated = true;
                    }
                    AdvPermissionResult::Error(msg) => {
                        println!("⚠ 检查权限时出错: {msg}");
                        let proceed = prompt::ask_confirm("是否继续创建任务文件？")?;
                        if !proceed {
                            return Err(AppError::Cancelled("取消创建".to_string()));
                        }
                        break;
                    }
                }
            }

            if credentials_updated {
                sessdata = sessdata_for_check;
                csrf = csrf_for_check;
            }
        }
    }

    // 7. 组装并保存
    println!();
    let task_file = TaskFile::new(
        bvid.clone(),
        cid,
        page_title,
        sessdata,
        csrf,
        pool,
        parse_result.tasks,
    );

    // 确定输出路径并保存（自动回退）
    let filename_video_id = if bvid.is_empty() {
        format!("cid{cid}")
    } else {
        bvid.clone()
    };
    let output_path_str =
        save_task_file_robust(&task_file, output_path.as_deref(), &filename_video_id, page)?;
    let output_path_display = std::path::PathBuf::from(&output_path_str);

    println!("═══════════════════════════════════════");
    println!("✓ 任务文件已创建: {}", output_path_display.display());
    if task_file.bvid.is_empty() {
        println!("  - 视频: {} (CID: {})", task_file.title, task_file.cid);
    } else {
        println!("  - 视频: {} (BV: {})", task_file.title, task_file.bvid);
    }
    println!("  - 分页: {page}, CID: {}", task_file.cid);
    println!("  - 弹幕数: {} 条", task_file.danmakus.len());
    println!("  - 弹幕池: {}", task_file.pool);
    if let Some(offset) = time_offset_ms {
        if offset != 0 {
            println!("  - 时间偏移: {}ms", offset);
        }
    }
    println!("═══════════════════════════════════════");

    Ok(output_path_str)
}

// ═══════════════════════════════════════════════════════════════
//  send 命令
// ═══════════════════════════════════════════════════════════════

pub async fn handle_send(
    task_path: String,
    auto_mode: bool,
    options: SendOptions,
) -> AppResult<()> {
    if auto_mode {
        println!("═══════════════════════════════════════");
        println!("    DMsender_CLI — 发送弹幕（自动模式）");
        println!("═══════════════════════════════════════");
    } else {
        println!("═══════════════════════════════════════");
        println!("       DMsender_CLI — 发送弹幕");
        println!("═══════════════════════════════════════");
    }
    println!();

    // 1. 加载并校验任务文件基础字段
    let mut task_file = TaskFile::from_file(&task_path)?;
    if task_file.pool > 2 {
        return Err(AppError::Business(format!(
            "任务文件弹幕池配置无效: pool={}（必须为 0、1 或 2）",
            task_file.pool
        )));
    }
    println!("✓ 任务文件已加载: {task_path}");
    if task_file.bvid.is_empty() {
        println!("  - 视频: {} (CID: {})", task_file.title, task_file.cid);
    } else {
        println!("  - 视频: {} (BV: {})", task_file.title, task_file.bvid);
    }
    println!("  - 弹幕总数: {} 条", task_file.danmakus.len());

    if task_file.sessdata.trim().is_empty() || task_file.csrf.trim().is_empty() {
        if auto_mode {
            return Err(AppError::Business(
                "任务文件缺少有效的 SESSDATA 或 bili_jct 凭据".to_string(),
            ));
        }
        println!("⚠ 任务文件缺少有效的 SESSDATA 或 bili_jct 凭据，请补充");
        let new_sessdata = prompt_non_empty_hidden("请输入 SESSDATA", "SESSDATA")?;
        let new_csrf = prompt_non_empty_hidden("请输入 bili_jct", "bili_jct")?;
        task_file.update_credentials(&task_path, new_sessdata, new_csrf)?;
    }

    // 2. 断点续传（先在内存中计算，避免提前破坏磁盘上的历史进度）
    match options.send_from.clone() {
        Some(SendFrom::Last) => {
            if let Some(_last_id) = task_file.last_progress_id {
                let pending = task_file.get_pending_danmakus().len();
                let done = task_file.danmakus.len().saturating_sub(pending);
                println!("  - 指定从上次进度继续: 已完成 {done} 条，剩余 {pending} 条");
            } else {
                println!("  - 指定从上次进度继续，但任务文件没有历史进度，将从头发送");
            }
        }
        Some(SendFrom::Id(start_id)) => {
            let previous_progress = task_file.last_progress_id;
            task_file.last_progress_id = start_id.checked_sub(1);
            let pending_count = task_file.get_pending_danmakus().len();
            if pending_count == 0 && !task_file.danmakus.is_empty() {
                task_file.last_progress_id = previous_progress;
                println!("⚠ 指定的起始 ID={start_id} 大于任务文件中的最大弹幕 ID，未更改任务进度");
                return Ok(());
            }
            println!("  - 指定从 ID={start_id} 开始发送（剩余 {pending_count} 条）");
        }
        None => {
            if let Some(_last_id) = task_file.last_progress_id {
                let pending = task_file.get_pending_danmakus().len();
                let done = task_file.danmakus.len().saturating_sub(pending);
                println!("  - 上次进度: 已完成 {done} 条，剩余 {pending} 条");
                if auto_mode {
                    println!("  - 自动模式：从上次进度继续发送");
                } else {
                    let resume = prompt::ask_confirm("是否从上次进度继续发送？")?;
                    if !resume {
                        task_file.last_progress_id = None;
                        task_file.to_file(&task_path)?;
                        println!("✓ 已重置进度，将从头开始发送");
                    }
                }
            }
        }
    }

    // 3. 检查时间戳
    if crate::core::task::is_task_file_expired(&task_file) {
        println!();
        println!("⚠ 任务文件已创建超过一周");
        if auto_mode {
            println!("  自动模式：跳过凭据更新");
        } else {
            let update = prompt::ask_confirm("是否更新 SESSDATA 和 bili_jct？")?;
            if update {
                let new_sessdata = prompt_non_empty_hidden("请输入新的 SESSDATA", "SESSDATA")?;
                let new_csrf = prompt_non_empty_hidden("请输入新的 bili_jct", "bili_jct")?;
                task_file.update_credentials(&task_path, new_sessdata, new_csrf)?;
                println!("✓ 凭据已更新");
            }
        }
    }

    // 4. 发送间隔
    let interval_ms: u64 = if let Some(ms) = options.interval_ms {
        println!("发送间隔: {}", format_millis(ms));
        ms
    } else if auto_mode {
        println!("发送间隔: 10s（自动模式默认值）");
        10_000
    } else {
        loop {
            let interval_str = prompt::ask_input_with_default("请输入发送间隔（秒）", "10")?;
            match interval_str
                .parse::<u64>()
                .ok()
                .and_then(|s| s.checked_mul(1000))
            {
                Some(ms) => break ms,
                None => println!("⚠ 请输入有效的非负整数秒数（且不能溢出）"),
            }
        }
    };

    // 5. 获取 WBI 密钥
    println!();
    println!("正在获取 WBI 签名密钥...");
    let mut wbi_keys = WbiKeys::fetch().await?;
    println!("✓ WBI 密钥已就绪");

    // 6. 准备待发送列表
    let pending = task_file.get_pending_danmakus();
    let total = pending.len();
    if total == 0 {
        println!();
        println!("✓ 所有弹幕已发送完毕，无需再次发送");
        return Ok(());
    }

    println!();
    println!("═══════════════════════════════════════");
    println!(
        "  开始发送弹幕（共 {total} 条，间隔 {}）",
        format_millis(interval_ms)
    );
    println!("  按 p 暂停 | 按 q 退出");
    println!("═══════════════════════════════════════");
    println!();

    // 共享状态: (paused, should_exit)
    let state = Arc::new(tokio::sync::Mutex::new((false, false)));
    let mut keyboard_task = spawn_keyboard_listener(Arc::clone(&state));

    // 将主发送逻辑放在独立函数中，确保键盘监听器总是在这里被清理
    let result = run_send_loop(
        &task_path,
        &mut task_file,
        &mut wbi_keys,
        &pending,
        total,
        interval_ms,
        options.retry_interval_ms,
        options.auto_retry.clone(),
        &mut keyboard_task,
        Arc::clone(&state),
        auto_mode,
    )
    .await;

    // 无论成功/失败/取消，都要清理键盘监听器
    stop_keyboard_listener(&mut keyboard_task).await;
    result
}

enum DanmakuOutcome {
    Sent,
    Skipped,
    Failed,
    Exit { failed: bool },
}

/// 主发送循环（从 handle_send 中提取，以便外层统一清理键盘监听器）
#[allow(clippy::too_many_arguments)]
async fn run_send_loop(
    task_path: &str,
    task_file: &mut TaskFile,
    wbi_keys: &mut WbiKeys,
    pending: &[DanmakuTask],
    total: usize,
    interval_ms: u64,
    retry_interval_ms: u64,
    auto_retry: AutoRetry,
    keyboard_task: &mut Option<KeyboardListener>,
    state: Arc<tokio::sync::Mutex<(bool, bool)>>,
    auto_mode: bool,
) -> AppResult<()> {
    let mut success_count = 0u64;
    let mut skip_count = 0u64;
    let mut fail_count = 0u64;
    let mut pause_notified = false;

    for (pos, dm) in pending.iter().enumerate() {
        // ── 等待直到不暂停 ──
        loop {
            let (paused, should_exit) = *state.lock().await;
            if should_exit {
                print_summary(success_count, skip_count, fail_count);
                return Ok(());
            }
            if !paused {
                break;
            }
            if !pause_notified {
                println!();
                println!("⏸  暂停执行完成，是否继续？(按 c 继续 / 按 q 退出)");
                pause_notified = true;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        pause_notified = false;

        // 长任务跨时段时自动刷新 WBI 密钥
        if wbi_keys.is_stale() {
            match WbiKeys::fetch().await {
                Ok(fresh) => {
                    *wbi_keys = fresh;
                }
                Err(e) => {
                    println!("⚠ 刷新 WBI 密钥失败（继续使用旧密钥）: {e}");
                }
            }
        }

        // ── 发送弹幕 ──
        let idx = success_count + skip_count + fail_count + 1;
        print!("[{idx}/{total}] ID={} mode={} | ", dm.id, dm.mode);
        let msg_preview: String = if dm.msg.chars().count() > 30 {
            dm.msg.chars().take(30).collect::<String>() + "..."
        } else {
            dm.msg.clone()
        };
        print!("\"{msg_preview}\"");
        let _ = std::io::Write::flush(&mut std::io::stdout());

        let initial_result = send_one_danmaku(task_file, dm, wbi_keys).await;

        // 若在请求期间按下了 q，优先退出而不再弹出交互提示
        if state.lock().await.1 {
            match initial_result {
                Ok(_) => {
                    println!("✓ 成功");
                    success_count += 1;
                    task_file.update_progress(task_path, dm.id)?;
                }
                Err(e) => {
                    println!("✗ {e}");
                    fail_count += 1;
                }
            }
            print_summary(success_count, skip_count, fail_count);
            return Ok(());
        }

        let outcome = handle_danmaku_result(
            initial_result,
            dm,
            task_path,
            task_file,
            wbi_keys,
            retry_interval_ms,
            &auto_retry,
            keyboard_task,
            &state,
            auto_mode,
        )
        .await?;

        match outcome {
            DanmakuOutcome::Sent => success_count += 1,
            DanmakuOutcome::Skipped => skip_count += 1,
            DanmakuOutcome::Failed => fail_count += 1,
            DanmakuOutcome::Exit { failed } => {
                if failed {
                    fail_count += 1;
                }
                print_summary(success_count, skip_count, fail_count);
                return Ok(());
            }
        }

        // 最后一条弹幕发送完成后无需再等待发送间隔
        if pos + 1 < total && interval_ms > 0 {
            let mut elapsed = 0u64;
            while elapsed < interval_ms {
                let (paused, should_exit) = *state.lock().await;
                if should_exit {
                    print_summary(success_count, skip_count, fail_count);
                    return Ok(());
                }
                if paused {
                    if !pause_notified {
                        println!("⏸  暂停执行完成，是否继续？(按 c 继续 / 按 q 退出)");
                        pause_notified = true;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                pause_notified = false;
                let chunk_ms = (interval_ms - elapsed).min(200);
                tokio::time::sleep(Duration::from_millis(chunk_ms)).await;
                elapsed += chunk_ms;
            }
        }
    }

    print_summary(success_count, skip_count, fail_count);
    Ok(())
}

/// 统一处理单条弹幕的发送结果，支持 Retry 重分类与 ReAuth/Modify 闭环
#[allow(clippy::too_many_arguments)]
async fn handle_danmaku_result(
    initial_result: Result<(), SendError>,
    dm: &DanmakuTask,
    task_path: &str,
    task_file: &mut TaskFile,
    wbi_keys: &WbiKeys,
    retry_interval_ms: u64,
    auto_retry: &AutoRetry,
    keyboard_task: &mut Option<KeyboardListener>,
    state: &Arc<tokio::sync::Mutex<(bool, bool)>>,
    auto_mode: bool,
) -> AppResult<DanmakuOutcome> {
    let mut current_dm = dm.clone();
    let mut modified_any = false;
    let mut current_result = initial_result;
    let mut first_attempt = true;

    loop {
        match current_result {
            Ok(_) => {
                if first_attempt {
                    println!("✓ 成功");
                } else {
                    println!("  ✓ 发送成功");
                }
                if modified_any {
                    task_file.replace_danmaku(task_path, &current_dm)?;
                }
                task_file.update_progress(task_path, dm.id)?;
                return Ok(DanmakuOutcome::Sent);
            }
            Err(SendError::Fatal(code, msg)) => {
                println!("✗ [{code}] {msg}");
                if auto_mode {
                    println!("  自动模式：遇到致命错误");
                    return Ok(DanmakuOutcome::Exit { failed: true });
                }
                let exit = prompt_with_keyboard_paused(keyboard_task, Arc::clone(state), || {
                    prompt::ask_confirm_default_no("是否退出发送程序？")
                })
                .await?;
                if exit {
                    return Ok(DanmakuOutcome::Exit { failed: true });
                }
                return Ok(DanmakuOutcome::Failed);
            }
            Err(SendError::ReAuth(msg)) => {
                println!("⚠ {msg}");
                if auto_mode {
                    println!("  自动模式：需要重新认证");
                    return Ok(DanmakuOutcome::Exit { failed: true });
                }
                let new_sessdata =
                    prompt_with_keyboard_paused(keyboard_task, Arc::clone(state), || {
                        prompt_non_empty_hidden("请输入新的 SESSDATA", "SESSDATA")
                    })
                    .await?;
                let new_csrf =
                    prompt_with_keyboard_paused(keyboard_task, Arc::clone(state), || {
                        prompt_non_empty_hidden("请输入新的 bili_jct", "bili_jct")
                    })
                    .await?;
                task_file.update_credentials(task_path, new_sessdata, new_csrf)?;
                println!("✓ 凭据已更新，重试当前弹幕...");

                first_attempt = false;
                current_result = send_one_danmaku(task_file, &current_dm, wbi_keys).await;
                if let Err(SendError::ReAuth(ref err_msg)) = current_result {
                    println!("  ✗ 重试仍需要认证: {err_msg}");
                    let retry_auth =
                        prompt_with_keyboard_paused(keyboard_task, Arc::clone(state), || {
                            prompt::ask_confirm("凭据似乎仍无效，是否继续重新输入？")
                        })
                        .await?;
                    if !retry_auth {
                        return Ok(DanmakuOutcome::Failed);
                    }
                }
                continue;
            }
            Err(SendError::Retry(code, msg)) => {
                println!(
                    "[{code}] {msg}，进入重试流程（自动尝试{}）...",
                    format_retry_frequency(auto_retry)
                );
                let retry_result = retry_loop_with_pause(
                    state,
                    retry_interval_ms,
                    auto_retry.clone(),
                    &current_dm,
                    task_file,
                    wbi_keys,
                    keyboard_task,
                    Arc::clone(state),
                    auto_mode,
                )
                .await;
                match retry_result {
                    RetryOutcome::Success => {
                        first_attempt = false;
                        current_result = Ok(());
                        continue;
                    }
                    RetryOutcome::Skipped => {
                        println!("  ⏭ 已跳过");
                        task_file.update_progress(task_path, dm.id)?;
                        return Ok(DanmakuOutcome::Skipped);
                    }
                    RetryOutcome::Exited => {
                        return Ok(DanmakuOutcome::Exit { failed: false });
                    }
                    RetryOutcome::Reclassified(new_err) => {
                        println!("  ⚠ 重试期间错误类型变化，转为对应处理流程");
                        first_attempt = false;
                        current_result = Err(new_err);
                        continue;
                    }
                }
            }
            Err(SendError::Modify(code, msg)) => {
                println!("[{code}] {msg}");
                if auto_mode {
                    println!("  自动模式：直接跳过此条弹幕");
                    task_file.update_progress(task_path, dm.id)?;
                    return Ok(DanmakuOutcome::Skipped);
                }

                let meta = serde_json::to_string_pretty(&current_dm)
                    .unwrap_or_else(|_| format!("{current_dm:?}"));
                println!("  失败弹幕数据:");
                println!("{meta}");

                let choice = prompt_with_keyboard_paused(keyboard_task, Arc::clone(state), || {
                    prompt::ask_modify_choice()
                })
                .await?;

                match choice {
                    ModifyChoice::Skip => {
                        println!("  ⏭ 已跳过");
                        task_file.update_progress(task_path, dm.id)?;
                        return Ok(DanmakuOutcome::Skipped);
                    }
                    ModifyChoice::Exit => {
                        return Ok(DanmakuOutcome::Exit { failed: false });
                    }
                    ModifyChoice::Modify => {
                        current_dm =
                            prompt_modified_danmaku(&current_dm, keyboard_task, Arc::clone(state))
                                .await?;
                        modified_any = true;
                        first_attempt = false;
                        current_result = send_one_danmaku(task_file, &current_dm, wbi_keys).await;
                        continue;
                    }
                }
            }
        }
    }
}

async fn prompt_modified_danmaku(
    base: &DanmakuTask,
    keyboard_task: &mut Option<KeyboardListener>,
    state: Arc<tokio::sync::Mutex<(bool, bool)>>,
) -> AppResult<DanmakuTask> {
    let new_mode = prompt_parsed_with_default(
        "mode",
        &base.mode.to_string(),
        keyboard_task,
        Arc::clone(&state),
        |s| s.parse::<u8>().ok(),
        "请输入合法的 mode（0~255 整数）",
    )
    .await?;

    let new_msg = prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
        prompt::ask_input_with_default("msg", &base.msg)
    })
    .await?;

    let new_progress = prompt_parsed_with_default(
        "progress/ms",
        &base.progress.to_string(),
        keyboard_task,
        Arc::clone(&state),
        |s| s.parse::<u64>().ok(),
        "请输入合法的 progress（非负整数毫秒）",
    )
    .await?;

    let new_color = prompt_parsed_with_default(
        "color",
        &base.color.to_string(),
        keyboard_task,
        Arc::clone(&state),
        |s| s.parse::<u32>().ok().filter(|&c| c <= 16_777_215),
        "请输入合法的 color（0~16777215 整数）",
    )
    .await?;

    let new_fontsize = prompt_parsed_with_default(
        "fontsize",
        &base.fontsize.to_string(),
        keyboard_task,
        Arc::clone(&state),
        |s| s.parse::<u8>().ok().filter(|&f| f > 0),
        "请输入合法的 fontsize（1~255 整数）",
    )
    .await?;

    Ok(DanmakuTask {
        id: base.id,
        mode: new_mode,
        msg: new_msg,
        progress: new_progress,
        color: new_color,
        fontsize: new_fontsize,
    })
}

async fn prompt_parsed_with_default<T, P>(
    label: &str,
    default: &str,
    keyboard_task: &mut Option<KeyboardListener>,
    state: Arc<tokio::sync::Mutex<(bool, bool)>>,
    parser: P,
    err_msg: &str,
) -> AppResult<T>
where
    P: Fn(&str) -> Option<T>,
{
    loop {
        let raw = prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
            prompt::ask_input_with_default(label, default)
        })
        .await?;
        if let Some(val) = parser(&raw) {
            return Ok(val);
        }
        println!("  ⚠ {err_msg}");
    }
}

fn prompt_non_empty_hidden(prompt_text: &str, field_name: &str) -> AppResult<String> {
    loop {
        let input = prompt::ask_hidden_input(prompt_text)?;
        if input.trim().is_empty() {
            println!("⚠ {field_name} 不能为空，请重新输入");
            continue;
        }
        return Ok(input);
    }
}

/// 打印发送汇总
fn print_summary(success: u64, skip: u64, fail: u64) {
    println!();
    println!("═══════════════════════════════════════");
    println!("  发送结束");
    println!("  - 成功: {success} 条");
    println!("  - 跳过: {skip} 条");
    println!("  - 失败: {fail} 条");
    println!("═══════════════════════════════════════");
}

fn format_millis(ms: u64) -> String {
    if ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

fn format_retry_frequency(auto_retry: &AutoRetry) -> String {
    match auto_retry {
        AutoRetry::Finite(attempts) => format!("{attempts}次"),
        AutoRetry::Infinite => "无限次".to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════
//  发送辅助
// ═══════════════════════════════════════════════════════════════

enum SendError {
    Fatal(i32, String),
    ReAuth(String),
    Retry(i32, String),
    Modify(i32, String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Fatal(code, msg) => write!(f, "[{code}] {msg}"),
            SendError::ReAuth(msg) => write!(f, "{msg}"),
            SendError::Retry(code, msg) => write!(f, "[{code}] {msg}"),
            SendError::Modify(code, msg) => write!(f, "[{code}] {msg}"),
        }
    }
}

/// 重试循环的最终结果
enum RetryOutcome {
    Success,
    Skipped,
    Exited,
    Reclassified(SendError),
}

fn classify_response(code: i32, message: &str) -> SendError {
    match code {
        // 需要重新认证
        -101 | -102 | -111 | 36705 => {
            SendError::ReAuth(format!("需要重新认证 (code={code}): {message}"))
        }
        // 10秒后重试
        -404 | 36700 | 36703 => SendError::Retry(code, message.to_string()),
        // 36715 当日操作数量超过上限 — 重试无意义，归为致命
        36715 => SendError::Fatal(code, format!("当日操作数量超过上限: {message}")),
        // 询问跳过/修改
        -400 | 36701 | 36702 | 36706 | 36707 | 36708 | 36709 | 36710 | 36712 | 36714 | 36718 => {
            SendError::Modify(code, format!("弹幕内容问题: {message}"))
        }
        // 致命 — 询问退出
        36704 | 36711 | 36713 => SendError::Fatal(code, format!("视频状态异常: {message}")),
        _ => SendError::Fatal(code, format!("未知错误: {message}")),
    }
}

async fn send_one_danmaku(
    task_file: &TaskFile,
    dm: &DanmakuTask,
    wbi_keys: &WbiKeys,
) -> Result<(), SendError> {
    let resp = api::post_danmaku(
        &task_file.sessdata,
        &task_file.csrf,
        &task_file.bvid,
        task_file.cid,
        task_file.pool,
        dm.mode,
        &dm.msg,
        dm.progress,
        dm.color,
        dm.fontsize,
        wbi_keys,
    )
    .await
    .map_err(|e| SendError::Retry(0, format!("网络错误: {e}")))?;

    match resp.code {
        0 => Ok(()),
        code => Err(classify_response(code, &resp.message)),
    }
}

/// 带暂停/退出响应的重试循环
/// 默认自动重试5次，若全部失败询问是否继续，选Y则再试5次，循环
/// auto_mode: 有限次数全部失败后直接跳过，不询问用户
/// 若重试期间错误类型变为 Fatal/ReAuth/Modify，立即停止重试并转交外层处理
#[allow(clippy::too_many_arguments)]
async fn retry_loop_with_pause(
    state: &tokio::sync::Mutex<(bool, bool)>,
    wait_ms: u64,
    auto_retry: AutoRetry,
    dm: &DanmakuTask,
    task_file: &TaskFile,
    wbi_keys: &WbiKeys,
    keyboard_task: &mut Option<KeyboardListener>,
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
    auto_mode: bool,
) -> RetryOutcome {
    if matches!(auto_retry, AutoRetry::Infinite) {
        let mut attempt = 1u64;
        loop {
            match wait_with_pause(state, wait_ms).await {
                WaitResult::Timeout => {}
                WaitResult::Exited => return RetryOutcome::Exited,
                WaitResult::Paused => match wait_for_resume(state).await {
                    ResumeResult::Resumed => {}
                    ResumeResult::Exited => return RetryOutcome::Exited,
                },
            }

            println!("    重试 {attempt}/inf...");
            match send_one_danmaku(task_file, dm, wbi_keys).await {
                Ok(_) => return RetryOutcome::Success,
                Err(e @ SendError::Retry(_, _)) => println!("    ✗ {e}"),
                Err(other) => {
                    println!("    ✗ {other}");
                    return RetryOutcome::Reclassified(other);
                }
            }
            attempt += 1;
        }
    }

    let attempts = match auto_retry {
        AutoRetry::Finite(attempts) => attempts,
        AutoRetry::Infinite => unreachable!(),
    };

    loop {
        for attempt in 1..=attempts {
            match wait_with_pause(state, wait_ms).await {
                WaitResult::Timeout => {}
                WaitResult::Exited => return RetryOutcome::Exited,
                WaitResult::Paused => match wait_for_resume(state).await {
                    ResumeResult::Resumed => {}
                    ResumeResult::Exited => return RetryOutcome::Exited,
                },
            }

            println!("    重试 {attempt}/{attempts}...");
            match send_one_danmaku(task_file, dm, wbi_keys).await {
                Ok(_) => return RetryOutcome::Success,
                Err(e @ SendError::Retry(_, _)) => {
                    println!("    ✗ {e}");
                }
                Err(other) => {
                    println!("    ✗ {other}");
                    return RetryOutcome::Reclassified(other);
                }
            }
        }

        println!("  ⚠ {attempts}次重试均失败");
        if auto_mode {
            println!("  自动模式：直接跳过此条");
            return RetryOutcome::Skipped;
        }
        match prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state_clone), || {
            prompt::ask_confirm("是否继续重试？(N=跳过此条)")
        })
        .await
        {
            Ok(true) => continue,
            Ok(false) => return RetryOutcome::Skipped,
            Err(_) => return RetryOutcome::Exited,
        }
    }
}

enum WaitResult {
    Timeout,
    Exited,
    Paused,
}

enum ResumeResult {
    Resumed,
    Exited,
}

/// 分段等待，可响应暂停/退出
async fn wait_with_pause(state: &tokio::sync::Mutex<(bool, bool)>, ms: u64) -> WaitResult {
    let mut elapsed = 0u64;
    while elapsed < ms {
        let (paused, should_exit) = *state.lock().await;
        if should_exit {
            return WaitResult::Exited;
        }
        if paused {
            return WaitResult::Paused;
        }
        let chunk_ms = (ms - elapsed).min(200);
        tokio::time::sleep(Duration::from_millis(chunk_ms)).await;
        elapsed += chunk_ms;
    }
    // 处理 ms == 0 时也先检查一次退出/暂停状态
    let (paused, should_exit) = *state.lock().await;
    if should_exit {
        return WaitResult::Exited;
    }
    if paused {
        return WaitResult::Paused;
    }
    WaitResult::Timeout
}

/// 暂停后等待用户恢复或退出
async fn wait_for_resume(state: &tokio::sync::Mutex<(bool, bool)>) -> ResumeResult {
    println!("⏸  已暂停（按 c 继续 / 按 q 退出）");
    loop {
        let (paused, should_exit) = *state.lock().await;
        if should_exit {
            return ResumeResult::Exited;
        }
        if !paused {
            return ResumeResult::Resumed;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ═══════════════════════════════════════════════════════════════
//  工具函数
// ═══════════════════════════════════════════════════════════════

/// 生成默认输出文件名：bvid + 分页 + 时间戳 + 随机哈希 → 防止覆盖
fn generate_default_filename(bvid: &str, page: u32) -> String {
    use std::hash::{BuildHasher, Hasher};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ts = now.as_secs();
    let random: u64 = {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(42);
        h.write_u128(now.as_nanos());
        h.finish()
    };
    format!("{bvid}_p{page}_{ts}_{random:016x}.json")
}

/// 计算默认输出目录（exe 同级 tasks/）并确保存在
fn ensure_default_tasks_dir() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let tasks_dir = exe_dir.join("tasks");
    let _ = std::fs::create_dir_all(&tasks_dir);
    tasks_dir
}

/// 根据用户输入解析输出路径：
/// - 以 / 或 \ 结尾 → 视为目录，追加默认文件名
/// - 否则视为文件路径
fn resolve_custom_output_path(raw: &str, bvid: &str, page: u32) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Business("--output 路径不能为空".to_string()));
    }

    let is_dir = trimmed.ends_with('/') || trimmed.ends_with('\\');
    let path = std::path::PathBuf::from(trimmed);

    if is_dir {
        std::fs::create_dir_all(&path)?;
        let filename = generate_default_filename(bvid, page);
        Ok(path.join(filename).to_string_lossy().to_string())
    } else {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(trimmed.to_string())
    }
}

fn try_save_custom_path(
    task_file: &TaskFile,
    raw: &str,
    bvid: &str,
    page: u32,
) -> AppResult<String> {
    let resolved = resolve_custom_output_path(raw, bvid, page)?;
    task_file.to_file(&resolved)?;
    Ok(resolved)
}

/// 保存任务文件：优先使用 --output 指定路径，IO 失败（含目录创建失败）则回退到默认 tasks/ 目录
fn save_task_file_robust(
    task_file: &TaskFile,
    custom_path: Option<&str>,
    bvid: &str,
    page: u32,
) -> AppResult<String> {
    if let Some(raw) = custom_path {
        match try_save_custom_path(task_file, raw, bvid, page) {
            Ok(resolved) => return Ok(resolved),
            Err(AppError::Io(e)) => {
                println!("⚠ 写入指定路径失败: {}，回退到默认 tasks/ 目录", e);
            }
            Err(other) => return Err(other),
        }
    }

    let tasks_dir = ensure_default_tasks_dir();
    let filename = generate_default_filename(bvid, page);
    let default_path = tasks_dir.join(&filename);
    let default_str = default_path.to_string_lossy().to_string();
    task_file.to_file(&default_str)?;
    Ok(default_str)
}

async fn read_xml_file(path: &str) -> AppResult<String> {
    if path.starts_with("http://") || path.starts_with("https://") {
        let client = api::http_client()?;
        let resp = client.get(path).send().await?.error_for_status()?;
        let content = resp.text().await?;
        Ok(content)
    } else {
        let content = tokio::fs::read_to_string(path).await?;
        Ok(content)
    }
}

fn spawn_keyboard_listener(
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
) -> Option<KeyboardListener> {
    if !std::io::stdin().is_terminal() {
        return None;
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop_flag);

    let handle = tokio::task::spawn_blocking(move || {
        while !stop_for_thread.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key))
                        if key.kind == KeyEventKind::Press
                            && !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        if let KeyCode::Char(c) = key.code {
                            let mut st = state_clone.blocking_lock();
                            match c {
                                'p' | 'P' => {
                                    *st = (true, false);
                                    println!("\n暂停信号已接收，等待当前操作完成...");
                                }
                                'c' | 'C' => {
                                    *st = (false, false);
                                    println!("\n▶  继续发送...");
                                }
                                'q' | 'Q' => {
                                    *st = (false, true);
                                    println!("\n🛑 退出信号已接收...");
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });

    Some(KeyboardListener { stop_flag, handle })
}

async fn stop_keyboard_listener(keyboard_task: &mut Option<KeyboardListener>) {
    if let Some(listener) = keyboard_task.take() {
        listener.stop_flag.store(true, Ordering::Relaxed);
        let _ = listener.handle.await;
    }
}

async fn prompt_with_keyboard_paused<T, F>(
    keyboard_task: &mut Option<KeyboardListener>,
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
    prompt_fn: F,
) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T>,
{
    stop_keyboard_listener(keyboard_task).await;
    let result = prompt_fn();
    if keyboard_task.is_none() {
        *keyboard_task = spawn_keyboard_listener(state_clone);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_task_file_robust_falls_back_when_custom_parent_is_file() {
        let dir = std::env::temp_dir();
        let blocker_file = dir.join(format!("dmsender_blocker_{}", std::process::id()));
        std::fs::write(&blocker_file, b"not a directory").unwrap();

        let bad_output = blocker_file.join("nested").join("task.json");
        let bad_output_str = bad_output.to_string_lossy().to_string();

        let task_file = TaskFile::new(
            "BV1xx411c7mD".to_string(),
            12345,
            "test".to_string(),
            "sess".to_string(),
            "csrf".to_string(),
            1,
            vec![],
        );

        let saved_path =
            save_task_file_robust(&task_file, Some(&bad_output_str), "BV1xx411c7mD", 1).unwrap();
        assert_ne!(saved_path, bad_output_str);
        assert!(std::path::Path::new(&saved_path).exists());

        let _ = std::fs::remove_file(&saved_path);
        let _ = std::fs::remove_file(&blocker_file);
    }
}
