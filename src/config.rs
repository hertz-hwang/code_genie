// =========================================================================
// 🔧 配置区域
// =========================================================================

/// 固定字根文件
pub const FILE_FIXED: &str = "input-fixed.txt";
/// 动态字根文件
pub const FILE_DYNAMIC: &str = "input-roots.txt";
/// 拆分表文件
pub const FILE_SPLITS: &str = "input-division.txt";
/// 字根对当量文件
pub const FILE_PAIR_EQUIV: &str = "pair_equivalence.txt";
/// 用指分布文件
pub const FILE_KEY_DIST: &str = "key_distribution.txt";
/// 简码规则文件
pub const FILE_SIMPLE: &str = "input-simple.txt";

/// 允许的键位字符
pub const ALLOWED_KEYS: &str = "qwrtypsdfghjklzxcvbnm";
/// 键位显示顺序
pub const KEY_DISPLAY_ORDER: &str = "qwertyuiopasdfghjklzxcvbnm";

// =========================================================================
// 🎚️ 归一化权重配置 — 全码部分 (总和 = 1.0)
// =========================================================================

/// 重码数权重
pub const WEIGHT_COLLISION_COUNT: f64 = 0.07;
/// 重码率权重
pub const WEIGHT_COLLISION_RATE: f64 = 0.62;
/// 当量权重
pub const WEIGHT_EQUIVALENCE: f64 = 0.2;
/// 当量变异系数权重
pub const WEIGHT_EQUIV_CV: f64 = 0.01;
/// 分布偏差权重
pub const WEIGHT_DISTRIBUTION: f64 = 0.1;

// =========================================================================
// 🎚️ 简码优化开关与权重
// =========================================================================

/// 是否启用简码优化
pub const ENABLE_SIMPLE_CODE: bool = true;

/// 全码目标总权重 vs 简码目标总权重
pub const WEIGHT_FULL_CODE: f64 = 0.7;
pub const WEIGHT_SIMPLE_CODE: f64 = 0.3;

/// 简码内部子权重（总和 = 1.0）
pub const SIMPLE_WEIGHT_FREQ: f64 = 0.5;
pub const SIMPLE_WEIGHT_EQUIV: f64 = 0.15;
pub const SIMPLE_WEIGHT_DIST: f64 = 0.05;
pub const SIMPLE_WEIGHT_COLLISION_COUNT: f64 = 0.05;
pub const SIMPLE_WEIGHT_COLLISION_RATE: f64 = 0.25;

/// 验证权重配置是否合理
pub fn validate_weights() {
    let total_full = WEIGHT_COLLISION_COUNT
        + WEIGHT_COLLISION_RATE
        + WEIGHT_EQUIVALENCE
        + WEIGHT_EQUIV_CV
        + WEIGHT_DISTRIBUTION;
    if (total_full - 1.0).abs() > 0.001 {
        eprintln!("⚠️ 警告：全码权重总和不为 1.0 (当前: {:.3})", total_full);
    }
    let total_simple = SIMPLE_WEIGHT_FREQ
        + SIMPLE_WEIGHT_EQUIV
        + SIMPLE_WEIGHT_DIST
        + SIMPLE_WEIGHT_COLLISION_COUNT
        + SIMPLE_WEIGHT_COLLISION_RATE;
    if ENABLE_SIMPLE_CODE && (total_simple - 1.0).abs() > 0.001 {
        eprintln!(
            "⚠️ 警告：简码子权重总和不为 1.0 (当前: {:.3})",
            total_simple
        );
    }
    if ENABLE_SIMPLE_CODE && (WEIGHT_FULL_CODE + WEIGHT_SIMPLE_CODE - 1.0).abs() > 0.001 {
        eprintln!(
            "⚠️ 警告：全码/简码总权重不为 1.0 (当前: {:.3})",
            WEIGHT_FULL_CODE + WEIGHT_SIMPLE_CODE
        );
    }
}

// =========================================================================
// 🎚️ 模拟退火参数
// =========================================================================

/// 并行线程数
pub const NUM_THREADS: usize = 16;
/// 总迭代步数
pub const TOTAL_STEPS: usize = 100_000;
/// 初始温度
pub const TEMP_START: f64 = 100.0;
/// 结束温度
pub const TEMP_END: f64 = 0.000001;
/// 舒适温度区间
pub const COMFORT_TEMP: f64 = 0.2;
/// 舒适区宽度
pub const COMFORT_WIDTH: f64 = 0.15;
/// 舒适区减速因子
pub const COMFORT_SLOWDOWN: f64 = 0.8;

/// 交换操作概率
pub const SWAP_PROBABILITY: f64 = 0.3;

/// 最小改进步数
pub const MIN_IMPROVE_STEPS: usize = TOTAL_STEPS / 10;
/// 扰动间隔
pub const PERTURB_INTERVAL: usize = TOTAL_STEPS / 20;
/// 扰动强度
pub const PERTURB_STRENGTH: f64 = 0.15;
/// 重新加热因子
pub const REHEAT_FACTOR: f64 = 1.25;

/// 最大码长
pub const MAX_PARTS: usize = 5;