// =========================================================================
// 💾 断点续算 — 检查点保存与恢复
// =========================================================================

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::{Metrics, ScaleConfig, SimpleMetrics};

/// 单个 SA 线程的检查点状态
#[derive(Clone, Serialize, Deserialize)]
pub struct ThreadCheckpoint {
    /// 线程 ID
    pub thread_id: usize,
    /// 当前解（group → key 映射）
    pub assignment: Vec<u8>,
    /// 历史最优解
    pub best_assignment: Vec<u8>,
    /// 历史最优得分
    pub best_score: f64,
    /// 历史最优指标
    pub best_metrics: Metrics,
    /// 历史最优简码指标
    pub best_simple_metrics: SimpleMetrics,
    /// 当前已完成的步数
    pub current_step: usize,
    /// 温度乘数（重热状态）
    pub temp_multiplier: f64,
    /// 连续未改进步数
    pub steps_since_improve: usize,
    /// 上次报告的最优得分
    pub last_best_score: f64,
}

/// 全局检查点：包含所有线程 + 元信息
#[derive(Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 格式版本（兼容性检测）
    pub version: u32,
    /// 保存时间戳
    pub timestamp: String,
    /// 配置文件路径
    pub config_path: String,
    /// 校准得到的 ScaleConfig（避免重新校准）
    pub scale_config: ScaleConfig,
    /// 自动校准得到的起始温度（0 表示使用配置值）
    pub actual_temp_start: f64,
    /// 自动校准得到的舒适温度（0 表示使用配置值）
    pub actual_comfort_temp: f64,
    /// 总步数（来自配置）
    pub total_steps: usize,
    /// 线程数
    pub num_threads: usize,
    /// 是否使用 keysoul
    pub use_keysoul: bool,
    /// 各线程检查点
    pub threads: Vec<ThreadCheckpoint>,
}

/// 当前检查点文件格式版本
pub const CHECKPOINT_VERSION: u32 = 1;

/// 默认检查点文件名
pub const CHECKPOINT_FILENAME: &str = "checkpoint.json";

/// 保存检查点到文件
pub fn save_checkpoint(checkpoint: &Checkpoint, path: &Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(checkpoint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // 先写临时文件再 rename，防止写入中途崩溃导致文件损坏
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

/// 从文件加载检查点
pub fn load_checkpoint(path: &Path) -> Result<Checkpoint, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取检查点文件 {}: {}", path.display(), e))?;

    let checkpoint: Checkpoint = serde_json::from_str(&content)
        .map_err(|e| format!("检查点文件格式错误: {}", e))?;

    if checkpoint.version != CHECKPOINT_VERSION {
        return Err(format!(
            "检查点版本不兼容: 文件版本 {}, 当前版本 {}",
            checkpoint.version, CHECKPOINT_VERSION
        ));
    }

    Ok(checkpoint)
}
