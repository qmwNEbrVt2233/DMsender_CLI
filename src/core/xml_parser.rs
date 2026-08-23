use quick_xml::Reader;
use quick_xml::events::Event;

use super::task::DanmakuTask;
use crate::errors::{AppError, AppResult};

/// XML 解析后的原始弹幕数据
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RawDanmaku {
    /// 弹幕出现时间（秒，来自 XML 的 stime 字段）
    stime: f64,
    /// 弹幕模式
    mode: u8,
    /// 字号
    size: u8,
    /// 颜色（十进制）
    color: u32,
    /// Unix 时间戳（用于排序）
    date: u64,
    /// 弹幕池
    pool: u8,
    /// 发送者 uhash
    uhash: String,
    /// 弹幕 dmid
    dmid: String,
    /// 权重
    weight: u32,
    /// 弹幕文本
    text: String,
    /// 是否存在解析时使用了默认值的字段（严格校验时据此剔除）
    has_parse_defaults: bool,
}

/// XML 解析结果
pub struct ParseResult {
    /// 转换后的弹幕任务列表
    pub tasks: Vec<DanmakuTask>,
    /// 是否存在 mode=7 的高级弹幕
    pub has_mode7: bool,
    /// 按 --remove 规则剔除的弹幕数量（mode, count）
    pub removed_modes: Vec<(u8, usize)>,
    /// 严格校验时剔除的弹幕数量
    pub rigor_removed_count: usize,
}

/// 严格校验单条弹幕数据，返回 true 表示通过
fn validate_danmaku_rigor(raw: &RawDanmaku, time_offset_ms: Option<i64>) -> bool {
    // 任何字段在解析时使用了默认值 → 源数据非法，剔除
    if raw.has_parse_defaults {
        return false;
    }
    let rawstime = (raw.stime * 1000.0) as i64 + time_offset_ms.unwrap_or(0);
    // color: 0 ~ 16777215
    if raw.color > 16777215 {
        return false;
    }
    // size: 10 ~ 127
    if raw.size < 10 || raw.size > 127 {
        return false;
    }
    // stime: >= 0
    if rawstime < 0 {
        return false;
    }
    true
}

/// 解析弹幕 XML 文件
///
/// - 剔除 mode=8 的弹幕（代码弹幕，不可发送）
/// - 暂时剔除 mode=9 的弹幕（BAS弹幕）
/// - 检测是否存在 mode=7 的高级弹幕
/// - 若 rigor=true，额外验证 color/size/stime 合法性
/// - 按 date 字段排序并分配任务 ID
/// - 若 time_offset_ms 有值，对转换后的 progress 进行 +/- 偏移
pub fn parse_danmaku_xml(
    xml_content: &str,
    rigor: bool,
    time_offset_ms: Option<i64>,
    remove_modes: &[u8],
) -> AppResult<ParseResult> {
    let mut reader = Reader::from_str(xml_content);
    reader.config_mut().trim_text(true);

    let mut raw_danmakus: Vec<RawDanmaku> = Vec::new();
    let mut has_mode7 = false;
    let mut removed_modes: Vec<(u8, usize)> = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                // 自闭合标签 <d ... />，无文本
                if e.name().as_ref() == b"d" {
                    if let Some(mut raw) = parse_d_element_attr(e) {
                        raw.text = String::new();
                        classify_and_push(
                            &mut raw_danmakus,
                            raw,
                            &mut has_mode7,
                            &mut removed_modes,
                            remove_modes,
                        );
                    }
                }
            }
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"d" {
                    if let Some(mut raw) = parse_d_element_attr(e) {
                        // 读取文本内容直到 </d>
                        let text = read_d_text(&mut reader, &mut buf)?;
                        raw.text = text;

                        classify_and_push(
                            &mut raw_danmakus,
                            raw,
                            &mut has_mode7,
                            &mut removed_modes,
                            remove_modes,
                        );
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(AppError::XmlParse(format!("XML 解析失败: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    if raw_danmakus.is_empty() {
        return Err(AppError::XmlParse(
            "XML 中未找到任何有效弹幕数据".to_string(),
        ));
    }

    // 严格校验（若启用）
    let mut rigor_removed_count = 0usize;
    if rigor {
        let before = raw_danmakus.len();
        raw_danmakus.retain(|d| validate_danmaku_rigor(d, time_offset_ms));
        rigor_removed_count = before - raw_danmakus.len();
        if raw_danmakus.is_empty() {
            return Err(AppError::XmlParse("严格校验后无有效弹幕数据".to_string()));
        }
    }

    // 按 date 排序
    raw_danmakus.sort_by_key(|d| d.date);

    // 转换为 DanmakuTask 并分配 ID
    let offset = time_offset_ms.unwrap_or(0);
    let tasks: Vec<DanmakuTask> = raw_danmakus
        .into_iter()
        .enumerate()
        .map(|(idx, raw)| {
            // stime 单位是秒，progress 单位是毫秒
            let base_progress = (raw.stime * 1000.0) as i64;
            let adjusted = (base_progress + offset).max(0) as u64;
            DanmakuTask {
                id: (idx + 1) as u64,
                mode: raw.mode,
                msg: raw.text,
                progress: adjusted,
                color: raw.color,
                fontsize: raw.size.max(1),
            }
        })
        .collect();

    Ok(ParseResult {
        tasks,
        has_mode7,
        removed_modes,
        rigor_removed_count,
    })
}

/// 解析 p 属性字符串
/// 格式: "{stime},{mode},{size},{color},{date},{pool},{uhash},{dmid},{weight}"
fn parse_p_attribute(p: &str) -> Option<RawDanmaku> {
    let parts: Vec<&str> = p.split(',').collect();
    if parts.len() < 9 {
        return None;
    }

    let mut has_parse_defaults = false;

    let stime: f64 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            0.0
        }
    };
    let mode: u8 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            1
        }
    };
    let size: u8 = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            25
        }
    };
    let color: u32 = match parts[3].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            16777215
        }
    };
    let date: u64 = match parts[4].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            0
        }
    };
    let pool: u8 = match parts[5].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            0
        }
    };
    let weight: u32 = match parts[8].parse() {
        Ok(v) => v,
        Err(_) => {
            has_parse_defaults = true;
            0
        }
    };

    Some(RawDanmaku {
        stime,
        mode,
        size,
        color,
        date,
        pool,
        uhash: parts[6].to_string(),
        dmid: parts[7].to_string(),
        weight,
        text: String::new(),
        has_parse_defaults,
    })
}

/// 解析 <d> 元素的 p 属性，返回 RawDanmaku（不含 text）
fn parse_d_element_attr(e: &quick_xml::events::BytesStart) -> Option<RawDanmaku> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == b"p")
        .and_then(|attr| {
            let p_value = String::from_utf8_lossy(&attr.value);
            parse_p_attribute(&p_value)
        })
}

/// 根据 mode 分类弹幕：剔除 mode=8/9，记录 mode=7，否则推入列表
fn classify_and_push(
    danmakus: &mut Vec<RawDanmaku>,
    raw: RawDanmaku,
    has_mode7: &mut bool,
    removed_modes: &mut Vec<(u8, usize)>,
    remove_modes_rule: &[u8],
) {
    if remove_modes_rule.contains(&raw.mode) {
        if let Some((_, count)) = removed_modes.iter_mut().find(|(mode, _)| *mode == raw.mode) {
            *count += 1;
        } else {
            removed_modes.push((raw.mode, 1));
        }
        return;
    }

    if raw.mode == 7 {
        *has_mode7 = true;
    }
    danmakus.push(raw);
}

/// 读取 <d> 标签内的文本内容
fn read_d_text(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> AppResult<String> {
    let mut text = String::new();
    loop {
        match reader.read_event_into(buf) {
            Ok(Event::Text(e)) => {
                text.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"d" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(AppError::XmlParse(format!("读取弹幕文本失败: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_p_attribute() {
        let p = "12.5,1,25,16777215,1594818540,0,abc123,456789,5";
        let result = parse_p_attribute(p).unwrap();
        assert_eq!(result.stime, 12.5);
        assert_eq!(result.mode, 1);
        assert_eq!(result.size, 25);
        assert_eq!(result.color, 16777215);
        assert_eq!(result.date, 1594818540);
        assert_eq!(result.pool, 0);
    }

    #[test]
    fn test_parse_xml_with_mode8_filter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="1.0,1,25,16777215,1000,0,hash1,dmid1,1">弹幕1</d>
    <d p="2.0,8,25,16777215,2000,0,hash2,dmid2,1">代码弹幕</d>
    <d p="3.0,1,25,16777215,3000,0,hash3,dmid3,1">弹幕3</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, None, &[8]).unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert_eq!(result.removed_modes, vec![(8, 1)]);
        assert_eq!(result.tasks[0].msg, "弹幕1");
        assert_eq!(result.tasks[1].msg, "弹幕3");
        // 按 date 排序后 ID 从 1 开始
        assert_eq!(result.tasks[0].id, 1);
        assert_eq!(result.tasks[1].id, 2);
    }

    #[test]
    fn test_parse_xml_with_empty_remove_rule_keeps_modes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="1.0,8,25,16777215,1000,0,hash1,dmid1,1">代码弹幕</d>
    <d p="2.0,9,25,16777215,2000,0,hash2,dmid2,1">BAS弹幕</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, None, &[]).unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert!(result.removed_modes.is_empty());
    }

    #[test]
    fn test_parse_xml_with_mode9() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="1.0,9,25,16777215,1000,0,hash1,dmid1,1">BAS弹幕</d>
    <d p="2.0,1,25,16777215,2000,0,hash2,dmid2,1">普通弹幕</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, None, &[9]).unwrap();
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.removed_modes, vec![(9, 1)]);
    }

    #[test]
    fn test_parse_xml_with_mode7() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="1.0,7,25,16777215,1000,0,hash1,dmid1,1">高级弹幕</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, None, &[]).unwrap();
        assert!(result.has_mode7);
        assert_eq!(result.tasks.len(), 1);
    }

    #[test]
    fn test_stime_to_progress_conversion() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="5.5,1,25,16777215,1000,0,hash1,dmid1,1">测试</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, None, &[]).unwrap();
        assert_eq!(result.tasks[0].progress, 5500); // 5.5s → 5500ms
    }

    #[test]
    fn test_timeoffset_positive() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="5.5,1,25,16777215,1000,0,hash1,dmid1,1">测试</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, Some(500), &[]).unwrap();
        assert_eq!(result.tasks[0].progress, 6000); // 5500 + 500 = 6000ms
    }

    #[test]
    fn test_timeoffset_negative() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="5.5,1,25,16777215,1000,0,hash1,dmid1,1">测试</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, Some(-2000), &[]).unwrap();
        assert_eq!(result.tasks[0].progress, 3500); // 5500 - 2000 = 3500ms
    }

    #[test]
    fn test_timeoffset_clamp_zero() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <d p="0.5,1,25,16777215,1000,0,hash1,dmid1,1">测试</d>
</i>"#;

        let result = parse_danmaku_xml(xml, false, Some(-1000), &[]).unwrap();
        assert_eq!(result.tasks[0].progress, 0); // 不能小于 0
    }
}
