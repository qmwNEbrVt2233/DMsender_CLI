use crate::errors::AppResult;
use dialoguer::Password;
use inquire::{Confirm, CustomType, Select, Text};

/// 将 inquire 的 InquireError 转换为 AppError
fn map_inquire_err(e: inquire::InquireError) -> crate::errors::AppError {
    crate::errors::AppError::Cancelled(format!("用户取消: {e}"))
}

/// 询问用户输入字符串
pub fn ask_input(prompt: &str) -> AppResult<String> {
    Text::new(prompt)
        .prompt()
        .map_err(map_inquire_err)
        .map(|s| s.trim().to_string())
}

/// 询问用户输入字符串，带默认值
pub fn ask_input_with_default(prompt: &str, default: &str) -> AppResult<String> {
    Text::new(prompt)
        .with_default(default)
        .prompt()
        .map_err(map_inquire_err)
        .map(|s| s.trim().to_string())
}

/// 询问用户输入字符串，隐藏输入
pub fn ask_hidden_input(prompt: &str) -> AppResult<String> {
    let input = Password::new()
        .with_prompt(prompt)
        .interact()
        .map_err(|e| crate::errors::AppError::Cancelled(format!("用户取消输入: {e}")))?;

    Ok(input.trim().to_string())
}

/// 询问用户输入数字
pub fn ask_number(prompt: &str) -> AppResult<u32> {
    CustomType::<u32>::new(prompt)
        .with_error_message("请输入有效的数字")
        .prompt()
        .map_err(map_inquire_err)
}

/// 询问用户确认 (Y/N)
pub fn ask_confirm(prompt: &str) -> AppResult<bool> {
    Confirm::new(prompt)
        .with_default(true)
        .prompt()
        .map_err(map_inquire_err)
}

/// 询问用户确认 (Y/N)，默认值为 false
#[allow(dead_code)]
pub fn ask_confirm_default_no(prompt: &str) -> AppResult<bool> {
    Confirm::new(prompt)
        .with_default(false)
        .prompt()
        .map_err(map_inquire_err)
}

/// 弹幕修改选项
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifyChoice {
    Skip,
    Modify,
    Exit,
}

impl std::fmt::Display for ModifyChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModifyChoice::Skip => write!(f, "跳过此条弹幕"),
            ModifyChoice::Modify => write!(f, "修改弹幕参数后重试"),
            ModifyChoice::Exit => write!(f, "退出发送程序"),
        }
    }
}

/// 询问用户：跳过 / 修改 / 退出
pub fn ask_modify_choice() -> AppResult<ModifyChoice> {
    let items = vec![ModifyChoice::Skip, ModifyChoice::Modify, ModifyChoice::Exit];

    Select::new("请选择操作", items)
        .prompt()
        .map_err(map_inquire_err)
}
