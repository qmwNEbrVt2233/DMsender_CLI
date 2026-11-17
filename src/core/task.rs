use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 单条弹幕任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DanmakuTask {
    /// 任务 ID（用于断点续传）
    pub id: u64,
    /// 弹幕模式
    pub mode: u8,
    /// 弹幕内容（原始文本）
    pub msg: String,
    /// 弹幕出现时间（毫秒）
    pub progress: u64,
    /// 弹幕颜色（十进制 RGB888）
    pub color: u32,
    /// 弹幕字号
    pub fontsize: u8,
}

/// 任务文件（顶层结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFile {
    /// 任务文件创建时的 Unix 时间戳（秒）
    pub created_at: u64,
    /// 上次发送进度 — 记录最后成功发送的弹幕 ID（创建时无此项）
    pub last_progress_id: Option<u64>,
    /// 稿件 bvid
    pub bvid: String,
    /// 视频 cid
    pub cid: u64,
    /// 视频标题
    pub title: String,
    /// SESSDATA
    pub sessdata: String,
    /// CSRF Token（bili_jct）
    pub csrf: String,
    /// 目标弹幕池
    pub pool: u8,
    /// 弹幕任务列表（按 date 排序后分配 ID）
    pub danmakus: Vec<DanmakuTask>,
}

impl TaskFile {
    /// 创建新的任务文件
    pub fn new(
        bvid: String,
        cid: u64,
        title: String,
        sessdata: String,
        csrf: String,
        pool: u8,
        danmakus: Vec<DanmakuTask>,
    ) -> Self {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        TaskFile {
            created_at,
            last_progress_id: None,
            bvid,
            cid,
            title,
            sessdata,
            csrf,
            pool,
            danmakus,
        }
    }

    /// 从文件读取任务文件
    pub fn from_file(path: &str) -> Result<Self, crate::errors::AppError> {
        let content = std::fs::read_to_string(path)?;
        let task_file: TaskFile = serde_json::from_str(&content)?;
        Ok(task_file)
    }

    /// 将任务文件原子写入磁盘（先写临时文件再重命名，避免中断导致文件损坏）
    pub fn to_file(&self, path: &str) -> Result<(), crate::errors::AppError> {
        let json = serde_json::to_string_pretty(self)?;
        let seq = TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = format!("{path}.tmp.{}.{seq}", std::process::id());

        let write_result = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp_path, path)?;
            Ok(())
        })();

        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        Ok(())
    }

    /// 更新发送进度并写入文件
    pub fn update_progress(
        &mut self,
        task_path: &str,
        last_id: u64,
    ) -> Result<(), crate::errors::AppError> {
        self.last_progress_id = Some(last_id);
        self.to_file(task_path)
    }

    /// 更新 SESSDATA 和 csrf 并写入文件
    pub fn update_credentials(
        &mut self,
        task_path: &str,
        sessdata: String,
        csrf: String,
    ) -> Result<(), crate::errors::AppError> {
        self.sessdata = sessdata;
        self.csrf = csrf;
        self.to_file(task_path)
    }

    /// 获取需发送的弹幕列表（从断点开始，返回克隆）
    pub fn get_pending_danmakus(&self) -> Vec<DanmakuTask> {
        let start_id = self.last_progress_id.unwrap_or(0);
        self.danmakus
            .iter()
            .filter(|d| d.id > start_id)
            .cloned()
            .collect()
    }

    /// 用修改后的弹幕数据替换任务文件中对应 id 的弹幕并保存
    pub fn replace_danmaku(
        &mut self,
        task_path: &str,
        modified: &DanmakuTask,
    ) -> Result<(), crate::errors::AppError> {
        if let Some(existing) = self.danmakus.iter_mut().find(|d| d.id == modified.id) {
            existing.mode = modified.mode;
            existing.msg = modified.msg.clone();
            existing.progress = modified.progress;
            existing.color = modified.color;
            existing.fontsize = modified.fontsize;
        }
        self.to_file(task_path)
    }
}

/// 检查任务文件是否超过一个周 — 用于提示更新 SESSDATA
#[allow(dead_code)]
pub fn is_task_file_expired(task_file: &TaskFile) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let one_week_secs: u64 = 7 * 24 * 60 * 60;
    now.saturating_sub(task_file.created_at) > one_week_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_file_atomic_save_and_overwrite() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("dmsender_test_task_{}.json", std::process::id()));
        let path_str = path.to_string_lossy().to_string();

        let mut task = TaskFile::new(
            "BV1xx411c7mD".to_string(),
            12345,
            "test".to_string(),
            "sess".to_string(),
            "csrf".to_string(),
            1,
            vec![DanmakuTask {
                id: 1,
                mode: 1,
                msg: "hello".to_string(),
                progress: 1000,
                color: 16777215,
                fontsize: 25,
            }],
        );
        task.to_file(&path_str).unwrap();
        task.update_progress(&path_str, 1).unwrap();

        let loaded = TaskFile::from_file(&path_str).unwrap();
        assert_eq!(loaded.last_progress_id, Some(1));
        assert_eq!(loaded.danmakus.len(), 1);

        let _ = std::fs::remove_file(&path);
    }
}
