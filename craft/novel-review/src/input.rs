//! 输入收集与输出写入工具。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

///  positional 与 `-i/--input` 合并为输入列表；
/// 空列表直接报错，退出码 1。
pub fn resolve_inputs(positional: &[PathBuf], optional: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut inputs: Vec<PathBuf> = positional.to_vec();
    inputs.extend(optional.iter().cloned());
    anyhow::ensure!(!inputs.is_empty(), "No input paths provided.");
    Ok(inputs)
}

/// 写文本文件，父目录不存在时自动创建（`lib/io.write_text`）。
pub fn write_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录 {}", parent.display()))?;
        }
    }
    fs::write(path, content).with_context(|| format!("无法写入文件 {}", path.display()))
}

/// 写 JSON 文件：给定序列化 JSON 字符串原样写入并追加末尾换行。
pub fn write_json_line(path: &Path, json: &str) -> Result<()> {
    write_text(path, &format!("{json}\n"))
}

/// 写 JSON 文件（2 空格缩进美化、非 ASCII 原样输出，末尾追加换行）。
pub fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    write_text(path, &format!("{body}\n"))
}
