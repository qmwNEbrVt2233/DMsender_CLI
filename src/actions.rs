use std::process;
use std::sync::Arc;

use crate::core::api::{self, AdvPermissionResult};
use crate::core::task::{DanmakuTask, TaskFile};
use crate::core::wbi::WbiKeys;
use crate::core::xml_parser;

use crate::cli::prompt::{self, ModifyChoice};
use crate::errors::{AppError, AppResult};

// ═══════════════════════════════════════════════════════════════
//  create 命令
// ═══════════════════════════════════════════════════════════════
pub async fn handle_create(
    xml_path: String,
    rigor: bool,
    output_path: Option<String>,
    time_offset_ms: Option<i64>,
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

    // 2. 询问 SESSDATA 和 bili_jct（在询问 bvid 之前，以便查询私密视频）
    let mut sessdata = loop {
        let input = prompt::ask_hidden_input("请输入 SESSDATA（Cookie 中的 SESSDATA）")?;
        if input.trim().is_empty() {
            println!("⚠ SESSDATA 不能为空，请重新输入");
            continue;
        }
        break input;
    };

    let mut csrf = loop {
        let input = prompt::ask_hidden_input("请输入 bili_jct（Cookie 中的 bili_jct / csrf）")?;
        if input.trim().is_empty() {
            println!("⚠ bili_jct 不能为空，请重新输入");
            continue;
        }
        break input;
    };

    // 3. 询问 bvid（带重试循环）
    let (bvid, page, cid, page_title) = loop {
        let bvid = prompt::ask_input("请输入视频 bvid")?;
        if bvid.trim().is_empty() {
            println!("⚠ bvid 不能为空，请重新输入");
            continue;
        }
        if !bvid.to_uppercase().starts_with("BV") {
            println!("⚠ bvid 格式不正确（应以 BV 开头），请重新输入");
            continue;
        }

        // 4. 询问分页号
        let page = prompt::ask_number("请输入分页号")?;

        // 5. 查询视频信息（带上 SESSDATA 以支持私密视频）
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
    };

    // 6. 询问目标弹幕池
    let pool_str =
        prompt::ask_input_with_default("请输入目标弹幕池（0:普通池 1:字幕池 2:特殊池）", "1")?;
    let pool: u8 = pool_str.parse().unwrap_or(1);

    // 7. 解析 XML
    println!();
    if rigor {
        println!("正在解析 XML 弹幕数据（严格校验模式）...");
    } else {
        println!("正在解析 XML 弹幕数据...");
    }
    let parse_result = xml_parser::parse_danmaku_xml(&xml_content, rigor, time_offset_ms)?;

    println!("✓ 解析完成:");
    println!("  - 有效弹幕: {} 条", parse_result.tasks.len());
    if parse_result.removed_mode8_count > 0 {
        println!(
            "  - 已剔除 mode=8 弹幕: {} 条（代码弹幕不可发送）",
            parse_result.removed_mode8_count
        );
    }
    if parse_result.removed_mode9_count > 0 {
        println!(
            "  - 已剔除 mode=9 弹幕: {} 条（BAS弹幕暂不支持）",
            parse_result.removed_mode9_count
        );
    }
    if parse_result.rigor_removed_count > 0 {
        println!(
            "  - 严格校验剔除: {} 条（ color/size 不合法）",
            parse_result.rigor_removed_count
        );
    }

    // 8. 检测 mode=7 高级弹幕权限
    if parse_result.has_mode7 {
        println!();
        if auto_mode {
            println!("检测到 mode=7 高级弹幕（自动模式：跳过权限检查，继续创建）");
        } else {
            println!("检测到 mode=7 高级弹幕，正在检查发送权限...");

            let mut sessdata_for_check = sessdata.clone();
            let mut csrf_for_check = csrf.clone();

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
                        println!("⚠ 账号未登录或凭据过期/格式无效 (code=-101/-400)");
                        sessdata_for_check = prompt::ask_hidden_input("请重新输入 SESSDATA")?;
                        csrf_for_check = prompt::ask_hidden_input("请重新输入 bili_jct")?;
                        // 更新主变量，使后续任务文件使用新凭据
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

            // 如果重新输入了凭据，更新后续使用的值
            if sessdata_for_check != sessdata {
                sessdata = sessdata_for_check;
                csrf = csrf_for_check;
            }
        }
    }

    // 9. 组装并保存
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
    let output_path_str = save_task_file_robust(&task_file, output_path.as_deref(), &bvid, page)?;
    let output_path_display = std::path::PathBuf::from(&output_path_str);

    println!("═══════════════════════════════════════");
    println!("✓ 任务文件已创建: {}", output_path_display.display());
    println!("  - 视频: {} (BV: {})", task_file.title, task_file.bvid);
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

pub async fn handle_send(task_path: String, auto_mode: bool) -> AppResult<()> {
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

    // 1. 加载任务文件
    let mut task_file = TaskFile::from_file(&task_path)?;
    println!("✓ 任务文件已加载: {task_path}");
    println!("  - 视频: {} (BV: {})", task_file.title, task_file.bvid);
    println!("  - 弹幕总数: {} 条", task_file.danmakus.len());

    // 2. 断点续传
    if let Some(_last_id) = task_file.last_progress_id {
        let pending = task_file.get_pending_danmakus().len();
        let done = task_file.danmakus.len() - pending;
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

    // 3. 检查时间戳
    if crate::core::task::is_task_file_expired(&task_file) {
        println!();
        println!("⚠ 任务文件已创建超过一周");
        if auto_mode {
            println!("  自动模式：跳过凭据更新");
        } else {
            let update = prompt::ask_confirm("是否更新 SESSDATA 和 bili_jct？")?;
            if update {
                let new_sessdata = prompt::ask_hidden_input("请输入新的 SESSDATA")?;
                let new_csrf = prompt::ask_hidden_input("请输入新的 bili_jct")?;
                task_file.update_credentials(&task_path, new_sessdata, new_csrf)?;
                println!("✓ 凭据已更新");
            }
        }
    }

    // 4. 发送间隔
    let interval_secs: u64 = if auto_mode {
        println!("发送间隔: 10s（自动模式默认值）");
        10
    } else {
        let interval_str = prompt::ask_input_with_default("请输入发送间隔（秒）", "10")?;
        interval_str.parse().unwrap_or(10)
    };

    // 5. 获取 WBI 密钥
    println!();
    println!("正在获取 WBI 签名密钥...");
    let wbi_keys = WbiKeys::fetch().await?;
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
    println!("  开始发送弹幕（共 {total} 条，间隔 {interval_secs}s）");
    println!("  按 p 暂停 | 按 q 退出");
    println!("═══════════════════════════════════════");
    println!();

    // 共享状态: (paused, should_exit)
    let state = Arc::new(tokio::sync::Mutex::new((false, false)));
    let mut keyboard_task = Some(spawn_keyboard_listener(Arc::clone(&state)));

    // 将主发送逻辑放在独立函数中，确保键盘监听器总是在这里被清理
    let result = run_send_loop(
        &task_path,
        &mut task_file,
        &wbi_keys,
        &pending,
        total,
        interval_secs,
        &mut keyboard_task,
        Arc::clone(&state),
        auto_mode,
    )
    .await;

    // 无论成功/失败/取消，都要清理键盘监听器
    stop_keyboard_listener(&mut keyboard_task).await;
    result
}

/// 主发送循环（从 handle_send 中提取，以便外层统一清理键盘监听器）
async fn run_send_loop(
    task_path: &str,
    task_file: &mut TaskFile,
    wbi_keys: &WbiKeys,
    pending: &[DanmakuTask],
    total: usize,
    interval_secs: u64,
    keyboard_task: &mut Option<tokio::task::JoinHandle<()>>,
    state: Arc<tokio::sync::Mutex<(bool, bool)>>,
    auto_mode: bool,
) -> AppResult<()> {
    let mut success_count = 0u64;
    let mut skip_count = 0u64;
    let mut fail_count = 0u64;
    let mut pause_notified = false;

    for dm in pending {
        // ── 等待直到不暂停 ──
        loop {
            let (paused, should_exit) = *state.lock().await;
            if should_exit {
                break;
            }
            if !paused {
                break;
            }
            if !pause_notified {
                println!();
                println!("⏸  暂停执行完成，是否继续？(按 c 继续 / 按 q 退出)");
                pause_notified = true;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        let (_paused, should_exit) = *state.lock().await;
        if should_exit {
            print_summary(success_count, skip_count, fail_count);
            return Ok(());
        }
        pause_notified = false;

        // ── 发送弹幕 ──
        let idx = success_count + skip_count + fail_count + 1;
        print!("[{idx}/{total}] ID={} mode={} | ", dm.id, dm.mode);
        let msg_preview: String = if dm.msg.len() > 30 {
            dm.msg.chars().take(30).collect::<String>() + "..."
        } else {
            dm.msg.clone()
        };
        print!("\"{msg_preview}\"");
        let _ = std::io::Write::flush(&mut std::io::stdout());

        match send_one_danmaku(task_file, dm, wbi_keys).await {
            Ok(_) => {
                println!("✓ 成功");
                success_count += 1;
                task_file.update_progress(task_path, dm.id)?;
            }
            Err(SendError::Fatal(code, msg)) => {
                println!("✗ [{code}] {msg}");
                fail_count += 1;
                if auto_mode {
                    println!("  自动模式：遇到致命错误，直接退出");
                    print_summary(success_count, skip_count, fail_count);
                    return Ok(());
                }
                let exit = prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
                    prompt::ask_confirm_default_no("是否退出发送程序？")
                })
                .await?;
                if exit {
                    print_summary(success_count, skip_count, fail_count);
                    return Ok(());
                }
            }
            Err(SendError::ReAuth(msg)) => {
                println!("⚠ {msg}");
                if auto_mode {
                    println!("  自动模式：需要重新认证，无法继续，退出");
                    fail_count += 1;
                    print_summary(success_count, skip_count, fail_count);
                    return Ok(());
                }
                let new_sessdata =
                    prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
                        prompt::ask_hidden_input("请输入新的 SESSDATA")
                    })
                    .await?;
                let new_csrf =
                    prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
                        prompt::ask_hidden_input("请输入新的 bili_jct")
                    })
                    .await?;
                task_file.update_credentials(task_path, new_sessdata, new_csrf)?;
                println!("✓ 凭据已更新，重试当前弹幕...");

                match send_one_danmaku(task_file, dm, wbi_keys).await {
                    Ok(_) => {
                        println!("  ✓ 重试成功");
                        success_count += 1;
                        task_file.update_progress(task_path, dm.id)?;
                    }
                    Err(e2) => {
                        println!("  ✗ 重试失败: {e2}");
                        fail_count += 1;
                    }
                }
            }
            Err(SendError::Retry(code, msg)) => {
                println!("[{code}] {msg}，进入重试流程（自动尝试5次）...");
                let retry_result = retry_loop_with_pause(
                    &state,
                    10,
                    dm,
                    task_file,
                    task_path,
                    wbi_keys,
                    keyboard_task,
                    Arc::clone(&state),
                    auto_mode,
                )
                .await;
                match retry_result {
                    RetryOutcome::Success => {
                        println!("  ✓ 重试成功");
                        success_count += 1;
                        task_file.update_progress(task_path, dm.id)?;
                    }
                    RetryOutcome::Skipped => {
                        println!("  ⏭ 已跳过");
                        skip_count += 1;
                        task_file.update_progress(task_path, dm.id)?;
                    }
                    RetryOutcome::Exited => {
                        print_summary(success_count, skip_count, fail_count);
                        return Ok(());
                    }
                }
            }
            Err(SendError::Modify(code, msg)) => {
                println!("[{code}] {msg}");
                if auto_mode {
                    println!("  自动模式：直接跳过此条弹幕");
                    skip_count += 1;
                    task_file.update_progress(task_path, dm.id)?;
                    // 继续下一条弹幕
                } else {
                    // 打印失败任务元数据
                    let meta =
                        serde_json::to_string_pretty(dm).unwrap_or_else(|_| format!("{dm:?}"));
                    println!("  失败弹幕数据:");
                    println!("{meta}");

                    // 修改循环：支持反复修改 → 重试 → 重新分类错误
                    'modify_loop: loop {
                        let choice =
                            prompt_with_keyboard_paused(keyboard_task, Arc::clone(&state), || {
                                prompt::ask_modify_choice()
                            })
                            .await?;
                        match choice {
                            ModifyChoice::Skip => {
                                println!("  ⏭ 已跳过");
                                skip_count += 1;
                                task_file.update_progress(task_path, dm.id)?;
                                break 'modify_loop;
                            }
                            ModifyChoice::Exit => {
                                print_summary(success_count, skip_count, fail_count);
                                return Ok(());
                            }
                            ModifyChoice::Modify => {
                                // 收集修改后的参数
                                let new_mode = prompt_with_keyboard_paused(
                                    keyboard_task,
                                    Arc::clone(&state),
                                    || prompt::ask_input_with_default("mode", &dm.mode.to_string()),
                                )
                                .await?
                                .parse()
                                .unwrap_or(dm.mode);
                                let new_msg = prompt_with_keyboard_paused(
                                    keyboard_task,
                                    Arc::clone(&state),
                                    || prompt::ask_input_with_default("msg", &dm.msg),
                                )
                                .await?;
                                let new_progress = prompt_with_keyboard_paused(
                                    keyboard_task,
                                    Arc::clone(&state),
                                    || {
                                        prompt::ask_input_with_default(
                                            "progress/ms",
                                            &dm.progress.to_string(),
                                        )
                                    },
                                )
                                .await?
                                .parse()
                                .unwrap_or(dm.progress);
                                let new_color = prompt_with_keyboard_paused(
                                    keyboard_task,
                                    Arc::clone(&state),
                                    || {
                                        prompt::ask_input_with_default(
                                            "color",
                                            &dm.color.to_string(),
                                        )
                                    },
                                )
                                .await?
                                .parse()
                                .unwrap_or(dm.color);
                                let new_fontsize = prompt_with_keyboard_paused(
                                    keyboard_task,
                                    Arc::clone(&state),
                                    || {
                                        prompt::ask_input_with_default(
                                            "fontsize",
                                            &dm.fontsize.to_string(),
                                        )
                                    },
                                )
                                .await?
                                .parse()
                                .unwrap_or(dm.fontsize);

                                let modified = DanmakuTask {
                                    id: dm.id,
                                    mode: new_mode,
                                    msg: new_msg,
                                    progress: new_progress,
                                    color: new_color,
                                    fontsize: new_fontsize,
                                };

                                // 发送修改后的弹幕，并对返回的错误重新分类处理
                                match send_one_danmaku(task_file, &modified, wbi_keys).await {
                                    Ok(_) => {
                                        println!("  ✓ 修改后发送成功");
                                        // 将修改后的数据写回任务文件
                                        task_file.replace_danmaku(task_path, &modified)?;
                                        success_count += 1;
                                        task_file.update_progress(task_path, dm.id)?;
                                        break 'modify_loop;
                                    }
                                    Err(e) => {
                                        println!("  ✗ 修改后仍失败: {e}");
                                        // 重新分类错误，走主发送流程的处理方式
                                        match e {
                                            SendError::Fatal(code, msg) => {
                                                println!("    [{code}] {msg}");
                                                let exit = prompt_with_keyboard_paused(
                                                    keyboard_task,
                                                    Arc::clone(&state),
                                                    || {
                                                        prompt::ask_confirm_default_no(
                                                            "是否退出发送程序？",
                                                        )
                                                    },
                                                )
                                                .await?;
                                                if exit {
                                                    print_summary(
                                                        success_count,
                                                        skip_count,
                                                        fail_count,
                                                    );
                                                    return Ok(());
                                                }
                                                // 不退出 → 回到修改循环，让用户选择 skip/modify/exit
                                                continue 'modify_loop;
                                            }
                                            SendError::ReAuth(msg) => {
                                                println!("    ⚠ {msg}");
                                                let new_sessdata = prompt_with_keyboard_paused(
                                                    keyboard_task,
                                                    Arc::clone(&state),
                                                    || {
                                                        prompt::ask_hidden_input(
                                                            "请输入新的 SESSDATA",
                                                        )
                                                    },
                                                )
                                                .await?;
                                                let new_csrf = prompt_with_keyboard_paused(
                                                    keyboard_task,
                                                    Arc::clone(&state),
                                                    || {
                                                        prompt::ask_hidden_input(
                                                            "请输入新的 bili_jct",
                                                        )
                                                    },
                                                )
                                                .await?;
                                                task_file.update_credentials(
                                                    task_path,
                                                    new_sessdata,
                                                    new_csrf,
                                                )?;
                                                println!("    ✓ 凭据已更新，重试修改后的弹幕...");

                                                match send_one_danmaku(
                                                    task_file, &modified, wbi_keys,
                                                )
                                                .await
                                                {
                                                    Ok(_) => {
                                                        println!("    ✓ 重试成功");
                                                        task_file.replace_danmaku(
                                                            task_path, &modified,
                                                        )?;
                                                        success_count += 1;
                                                        task_file
                                                            .update_progress(task_path, dm.id)?;
                                                        break 'modify_loop;
                                                    }
                                                    Err(e3) => {
                                                        println!("    ✗ 重试失败: {e3}");
                                                        // 继续循环，让用户再次选择
                                                        continue 'modify_loop;
                                                    }
                                                }
                                            }
                                            SendError::Retry(code, msg) => {
                                                println!("    [{code}] {msg}，进入重试流程...");
                                                let retry_result = retry_loop_with_pause(
                                                    &state,
                                                    10,
                                                    &modified,
                                                    task_file,
                                                    task_path,
                                                    wbi_keys,
                                                    keyboard_task,
                                                    Arc::clone(&state),
                                                    auto_mode,
                                                )
                                                .await;
                                                match retry_result {
                                                    RetryOutcome::Success => {
                                                        println!("    ✓ 重试成功");
                                                        task_file.replace_danmaku(
                                                            task_path, &modified,
                                                        )?;
                                                        success_count += 1;
                                                        task_file
                                                            .update_progress(task_path, dm.id)?;
                                                        break 'modify_loop;
                                                    }
                                                    RetryOutcome::Skipped => {
                                                        println!("    ⏭ 用户跳过");
                                                        skip_count += 1;
                                                        task_file
                                                            .update_progress(task_path, dm.id)?;
                                                        break 'modify_loop;
                                                    }
                                                    RetryOutcome::Exited => {
                                                        print_summary(
                                                            success_count,
                                                            skip_count,
                                                            fail_count,
                                                        );
                                                        return Ok(());
                                                    }
                                                }
                                            }
                                            SendError::Modify(_, _) => {
                                                // 仍为可修改类错误，回到修改循环
                                                continue 'modify_loop;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } // end else (non-auto modify handling)
            }
        }

        // ── 等间隔（分段，便于响应按键） ──
        if interval_secs > 0 {
            let mut elapsed = 0u64;
            while elapsed < interval_secs * 2 {
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
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
                pause_notified = false;
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                elapsed += 1;
            }
        }
    }

    print_summary(success_count, skip_count, fail_count);
    Ok(())
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
    force_exit(130);
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

/// 带暂停/退出响应的重试循环（5次一批）
/// 目标要求：默认自动重试5次，若全部失败询问是否继续，选Y则再试5次，循环
/// auto_mode: 5次全部失败直接跳过，不询问用户
async fn retry_loop_with_pause(
    state: &tokio::sync::Mutex<(bool, bool)>,
    wait_secs: u64,
    dm: &DanmakuTask,
    task_file: &TaskFile,
    _task_path: &str,
    wbi_keys: &WbiKeys,
    keyboard_task: &mut Option<tokio::task::JoinHandle<()>>,
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
    auto_mode: bool,
) -> RetryOutcome {
    loop {
        for attempt in 1..=5 {
            // 等待并响应暂停/退出
            if attempt > 1 {
                let result = wait_with_pause(state, wait_secs).await;
                match result {
                    WaitResult::Timeout => {}
                    WaitResult::Exited => return RetryOutcome::Exited,
                    WaitResult::Paused => match wait_for_resume(state).await {
                        ResumeResult::Resumed => {}
                        ResumeResult::Exited => return RetryOutcome::Exited,
                    },
                }
                println!("    重试 {attempt}/5...");
            }

            match send_one_danmaku(task_file, dm, wbi_keys).await {
                Ok(_) => return RetryOutcome::Success,
                Err(e) => {
                    println!("    ✗ {e}");
                }
            }
        }

        // 5次全部失败
        println!("  ⚠ 5次重试均失败");
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
async fn wait_with_pause(state: &tokio::sync::Mutex<(bool, bool)>, secs: u64) -> WaitResult {
    let mut elapsed = 0u64;
    while elapsed < secs * 2 {
        let (paused, should_exit) = *state.lock().await;
        if should_exit {
            return WaitResult::Exited;
        }
        if paused {
            return WaitResult::Paused;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        elapsed += 1;
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
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
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
    // 利用 RandomState 产生随机 u64，无需额外依赖
    let random: u64 = {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(42); // seed
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
    // 忽略创建错误（比如已存在），后面写文件时再报错
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
        // 目录 → 确保存在，追加默认文件名
        std::fs::create_dir_all(&path)?;
        let filename = generate_default_filename(bvid, page);
        Ok(path.join(filename).to_string_lossy().to_string())
    } else {
        // 文件 → 确保父目录存在
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(trimmed.to_string())
    }
}

/// 保存任务文件：优先使用 --output 指定路径，失败则回退到默认 tasks/ 目录
fn save_task_file_robust(
    task_file: &TaskFile,
    custom_path: Option<&str>,
    bvid: &str,
    page: u32,
) -> AppResult<String> {
    // 若用户指定了路径，尝试解析并使用
    if let Some(raw) = custom_path {
        let resolved = resolve_custom_output_path(raw, bvid, page)?;
        match task_file.to_file(&resolved) {
            Ok(()) => return Ok(resolved),
            Err(AppError::Io(e)) => {
                println!("⚠ 写入指定路径失败: {}，回退到默认 tasks/ 目录", e);
            }
            Err(other) => return Err(other),
        }
    }

    // 回退：使用默认 tasks/ 目录
    let tasks_dir = ensure_default_tasks_dir();
    let filename = generate_default_filename(bvid, page);
    let default_path = tasks_dir.join(&filename);
    let default_str = default_path.to_string_lossy().to_string();
    task_file.to_file(&default_str)?;
    Ok(default_str)
}

async fn read_xml_file(path: &str) -> AppResult<String> {
    if path.starts_with("http://") || path.starts_with("https://") {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;
        let resp = client.get(path).send().await?;
        let content = resp.text().await?;
        Ok(content)
    } else {
        let content = tokio::fs::read_to_string(path).await?;
        Ok(content)
    }
}

fn spawn_keyboard_listener(
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;

        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut buf = [0u8; 1];

        loop {
            match stdin.read_exact(&mut buf).await {
                Ok(_) => {
                    let c = buf[0];
                    let mut st = state_clone.lock().await;
                    match c {
                        b'p' | b'P' => {
                            *st = (true, false);
                            println!("\n暂停信号已接收，等待当前操作完成...");
                        }
                        b'c' | b'C' => {
                            *st = (false, false);
                            println!("\n▶  继续发送...");
                        }
                        b'q' | b'Q' => {
                            *st = (false, true);
                            println!("\n🛑 退出信号已接收...");
                            break;
                        }
                        _ => {}
                    }
                }
                Err(_) => break,
            }
        }
    })
}

async fn stop_keyboard_listener(keyboard_task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = keyboard_task.take() {
        handle.abort();
        let _ = handle.await;
    }
}

async fn prompt_with_keyboard_paused<T, F>(
    keyboard_task: &mut Option<tokio::task::JoinHandle<()>>,
    state_clone: Arc<tokio::sync::Mutex<(bool, bool)>>,
    prompt_fn: F,
) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T>,
{
    stop_keyboard_listener(keyboard_task).await;
    let result = prompt_fn();
    if keyboard_task.is_none() {
        *keyboard_task = Some(spawn_keyboard_listener(state_clone));
    }
    result
}

/// 通用强行退出函数
fn force_exit(code: i32) -> ! {
    process::exit(code);
}
