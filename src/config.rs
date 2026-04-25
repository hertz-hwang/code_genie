use serde::Deserialize;
use std::fs;

use crate::types::{SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep, WeightConfig};

// =========================================================================
// 📋 配置结构体定义
// =========================================================================

/// 主配置结构体
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub files: FilesConfig,
    pub keys: KeysConfig,
    pub weights: WeightsConfig,
    pub annealing: AnnealingConfig,
    pub amhb: AmhbConfig,
    pub simple_levels: Vec<SimpleLevelConfig>,
}

/// 文件路径配置
#[derive(Debug, Clone, Deserialize)]
pub struct FilesConfig {
    pub fixed: String,
    pub dynamic: String,
    pub splits: String,
    pub pair_equiv: String,
    pub key_dist: String,
    /// 多字词拆分文件（可选，启用词码优化时需要）
    #[serde(default)]
    pub word_div: Option<String>,
}

/// 键位配置
#[derive(Debug, Clone, Deserialize)]
pub struct KeysConfig {
    pub allowed: String,
    pub display_order: String,
}

/// 权重配置（顶层）
/// TOML 格式：
///   [weights]
///   full = 0.5        # 单字全码顶层权重
///   simple = 0.3      # 单字简码顶层权重
///   word = 0.2        # 多字词全码顶层权重
///   [weights.full_code]   # 单字全码子权重
///   [weights.simple_code] # 单字简码子权重
///   [weights.word_code]   # 多字词全码子权重
#[derive(Debug, Clone, Deserialize)]
pub struct WeightsConfig {
    /// 单字全码顶层权重
    #[serde(default = "default_w_full")]
    pub full: f64,
    /// 单字简码顶层权重
    #[serde(default = "default_w_simple")]
    pub simple: f64,
    /// 多字词全码顶层权重
    #[serde(default)]
    pub word: f64,
    /// 单字全码子权重
    pub full_code: FullCodeWeights,
    /// 单字简码子权重
    pub simple_code: SimpleCodeWeights,
    /// 多字词全码子权重
    #[serde(default)]
    pub word_code: WordCodeWeights,
}

fn default_w_full() -> f64 { 0.5 }
fn default_w_simple() -> f64 { 0.3 }

/// 单字全码子权重
#[derive(Debug, Clone, Deserialize)]
pub struct FullCodeWeights {
    /// 字频前 N 重码数的 N 值
    #[serde(default = "default_top_n")]
    pub top_n: usize,
    pub top_n_collision_count: f64,
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub distribution: f64,
}

fn default_top_n() -> usize { 1500 }

/// 单字简码子权重
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleCodeWeights {
    pub enabled: bool,
    pub weighted_key_length: f64,
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub distribution: f64,
}

/// 多字词全码子权重
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WordCodeWeights {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_word_top2000")]
    pub top2000_collision_count: f64,
    #[serde(default = "default_word_top10000")]
    pub top10000_collision_count: f64,
    #[serde(default = "default_word_collision_count")]
    pub collision_count: f64,
    #[serde(default = "default_word_collision_rate")]
    pub collision_rate: f64,
    #[serde(default = "default_word_equivalence")]
    pub equivalence: f64,
    #[serde(default = "default_word_distribution")]
    pub distribution: f64,
}

fn default_word_top2000() -> f64 { 0.2 }
fn default_word_top10000() -> f64 { 0.1 }
fn default_word_collision_count() -> f64 { 0.1 }
fn default_word_collision_rate() -> f64 { 0.3 }
fn default_word_equivalence() -> f64 { 0.2 }
fn default_word_distribution() -> f64 { 0.1 }

/// 模拟退火参数配置
#[derive(Debug, Clone, Deserialize)]
pub struct AnnealingConfig {
    pub threads: usize,
    pub total_steps: usize,
    pub temp_start: f64,
    pub temp_end: f64,
    pub comfort_temp: f64,
    pub comfort_width: f64,
    pub comfort_slowdown: f64,
    pub swap_probability: f64,
    pub min_improve_steps_ratio: f64,
    pub perturb_interval_ratio: f64,
    pub perturb_strength: f64,
    pub reheat_factor: f64,
    pub max_parts: usize,
    // AMHB 参数（已移至 [amhb] 节点，这里保留以支持旧配置）
    #[serde(default)]
    #[allow(dead_code)]
    pub total_neighbors: usize,
    #[serde(default)]
    #[allow(dead_code)]
    pub steal_threshold: i32,
}

/// AMHB 分段降温配置
/// 每个段定义一个温度阈值和冷却系数：当温度 > threshold 时，每步 temp *= factor
/// 段按 threshold 从高到低匹配，第一个满足的段生效
/// 若所有段都不匹配（温度已低于最小阈值），返回 -1.0 终止优化
#[derive(Debug, Clone, Deserialize)]
pub struct CoolingSegment {
    /// 温度阈值：当 temp > threshold 时使用此段的 factor
    pub threshold: f64,
    /// 冷却系数：每步 temp *= factor（应略小于 1.0）
    pub factor: f64,
}

/// AMHB 算法参数配置
#[derive(Debug, Clone, Deserialize)]
pub struct AmhbConfig {
    /// 算子池大小（总邻居数）
    pub total_neighbors: usize,
    /// 工作窃取阈值
    pub steal_threshold: i32,
    /// 最大迭代步数（达到后终止，即使温度未降完）
    #[serde(default)]
    pub total_steps: Option<usize>,
    /// 初始温度
    pub temp_start: f64,
    /// 分段降温参数（按 threshold 从高到低排列）
    pub cooling_segments: Vec<CoolingSegment>,
}

/// 简码级别配置（TOML 格式）
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleLevelConfig {
    pub level: usize,
    pub code_num: usize,
    pub rules: Vec<String>,
    #[serde(default)]
    pub allowed_orig_length: usize,
}

// =========================================================================
// 📥 配置加载
// =========================================================================

impl Config {
    /// 从 config.toml 加载配置
    #[allow(dead_code)]
    pub fn load() -> Self {
        Self::load_from_path("config.toml")
    }

    /// 从指定路径加载配置
    pub fn load_from_path(path: &str) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => {
                match toml::from_str(&content) {
                    Ok(config) => {
                        println!("✅ 已加载配置文件: {}", path);
                        config
                    }
                    Err(e) => {
                        eprintln!("⚠️ 配置文件解析失败: {}, 使用默认配置", e);
                        Self::default()
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠️ 无法读取配置文件 {}: {}, 使用默认配置", path, e);
                Self::default()
            }
        }
    }

    /// 获取简码配置（转换为内部格式）
    pub fn get_simple_code_config(&self) -> SimpleCodeConfig {
        let levels: Vec<SimpleCodeLevel> = self
            .simple_levels
            .iter()
            .filter(|l| l.code_num > 0)
            .map(|l| {
                let rule_candidates: Vec<Vec<SimpleCodeStep>> = l
                    .rules
                    .iter()
                    .filter_map(|rule| parse_rule_string(rule))
                    .collect();

                SimpleCodeLevel {
                    level: l.level,
                    code_num: l.code_num,
                    rule_candidates,
                    allowed_orig_length: l.allowed_orig_length,
                }
            })
            .filter(|l| !l.rule_candidates.is_empty())
            .collect();

        SimpleCodeConfig { levels }
    }

    /// 验证权重配置是否合理
    pub fn validate_weights(&self) {
        let w = &self.weights;
        let total_full = w.full_code.top_n_collision_count
            + w.full_code.collision_count
            + w.full_code.collision_rate
            + w.full_code.equivalence
            + w.full_code.distribution;
        if (total_full - 1.0).abs() > 0.001 {
            eprintln!("⚠️ 警告：全码子权重总和不为 1.0 (当前: {:.3})", total_full);
        }

        if w.simple_code.enabled {
            let total_simple = w.simple_code.weighted_key_length
                + w.simple_code.collision_count
                + w.simple_code.collision_rate
                + w.simple_code.equivalence
                + w.simple_code.distribution;
            if (total_simple - 1.0).abs() > 0.001 {
                eprintln!("⚠️ 警告：简码子权重总和不为 1.0 (当前: {:.3})", total_simple);
            }
        }

        if w.word_code.enabled {
            let total_word = w.word_code.top2000_collision_count
                + w.word_code.top10000_collision_count
                + w.word_code.collision_count
                + w.word_code.collision_rate
                + w.word_code.equivalence
                + w.word_code.distribution;
            if (total_word - 1.0).abs() > 0.001 {
                eprintln!("⚠️ 警告：词码子权重总和不为 1.0 (当前: {:.3})", total_word);
            }
        }

        let total_top = w.full + w.simple + w.word;
        if (total_top - 1.0).abs() > 0.001 {
            eprintln!("⚠️ 警告：顶层权重总和不为 1.0 (当前: {:.3})", total_top);
        }
    }

    /// 计算最小改进步数
    pub fn min_improve_steps(&self) -> usize {
        (self.annealing.total_steps as f64 * self.annealing.min_improve_steps_ratio) as usize
    }

    /// 计算扰动间隔
    pub fn perturb_interval(&self) -> usize {
        (self.annealing.total_steps as f64 * self.annealing.perturb_interval_ratio) as usize
    }

    /// 获取权重配置
    pub fn get_weight_config(&self) -> WeightConfig {
        let w = &self.weights;
        WeightConfig {
            weight_full_code: w.full,
            weight_simple_code: w.simple,
            weight_word_code: w.word,
            full_top_n: w.full_code.top_n,
            full_top_n_collision: w.full_code.top_n_collision_count,
            full_collision_count: w.full_code.collision_count,
            full_collision_rate: w.full_code.collision_rate,
            full_equivalence: w.full_code.equivalence,
            full_distribution: w.full_code.distribution,
            enable_simple_code: w.simple_code.enabled,
            simple_weighted_key_length: w.simple_code.weighted_key_length,
            simple_collision_count: w.simple_code.collision_count,
            simple_collision_rate: w.simple_code.collision_rate,
            simple_equivalence: w.simple_code.equivalence,
            simple_distribution: w.simple_code.distribution,
            enable_word_code: w.word_code.enabled,
            word_top2000_collision: w.word_code.top2000_collision_count,
            word_top10000_collision: w.word_code.top10000_collision_count,
            word_collision_count: w.word_code.collision_count,
            word_collision_rate: w.word_code.collision_rate,
            word_equivalence: w.word_code.equivalence,
            word_distribution: w.word_code.distribution,
        }
    }
}

/// 解析规则字符串为 SimpleCodeStep 列表
fn parse_rule_string(rule: &str) -> Option<Vec<SimpleCodeStep>> {
    let chars: Vec<char> = rule.trim().chars().collect();
    if chars.len() % 2 != 0 || chars.is_empty() {
        return None;
    }

    let mut steps = Vec::new();
    for chunk in chars.chunks(2) {
        steps.push(SimpleCodeStep {
            root_selector: chunk[0],
            code_selector: chunk[1],
        });
    }
    Some(steps)
}

// =========================================================================
// 🔧 默认配置（后备）
// =========================================================================

impl Default for Config {
    fn default() -> Self {
        Config {
            files: FilesConfig {
                fixed: "input-fixed.txt".to_string(),
                dynamic: "input-roots.txt".to_string(),
                splits: "input-division.txt".to_string(),
                pair_equiv: "pair_equivalence.txt".to_string(),
                key_dist: "key_distribution.txt".to_string(),
                word_div: None,
            },
            keys: KeysConfig {
                allowed: "qwertyuiopasdfghjklzxcvbnm".to_string(),
                display_order: "qwertyuiopasdfghjklzxcvbnm".to_string(),
            },
            weights: WeightsConfig {
                full: 0.5,
                simple: 0.3,
                word: 0.0,
                full_code: FullCodeWeights {
                    top_n: 1500,
                    top_n_collision_count: 0.1,
                    collision_count: 0.1,
                    collision_rate: 0.3,
                    equivalence: 0.3,
                    distribution: 0.2,
                },
                simple_code: SimpleCodeWeights {
                    enabled: true,
                    weighted_key_length: 0.3,
                    collision_count: 0.1,
                    collision_rate: 0.2,
                    equivalence: 0.2,
                    distribution: 0.2,
                },
                word_code: WordCodeWeights::default(),
            },
            annealing: AnnealingConfig {
                threads: 16,
                total_steps: 10_000,
                temp_start: 1.0,
                temp_end: 0.000001,
                comfort_temp: 0.2,
                comfort_width: 0.15,
                comfort_slowdown: 0.8,
                swap_probability: 0.3,
                min_improve_steps_ratio: 0.1,
                perturb_interval_ratio: 0.05,
                perturb_strength: 0.15,
                reheat_factor: 1.25,
                max_parts: 3,
                total_neighbors: 0,
                steal_threshold: 0,
            },
            amhb: AmhbConfig {
                total_neighbors: 256,
                steal_threshold: 1,
                total_steps: None,
                temp_start: 40.0,
                cooling_segments: vec![
                    CoolingSegment { threshold: 7.5, factor: 0.99999 },
                    CoolingSegment { threshold: 1.75, factor: 0.999999 },
                    CoolingSegment { threshold: 1.25, factor: 0.9999995 },
                    CoolingSegment { threshold: 0.15, factor: 0.9999999625 },
                ],
            },
            simple_levels: vec![
                SimpleLevelConfig {
                    level: 1,
                    code_num: 0,
                    rules: vec!["Aa".to_string()],
                    allowed_orig_length: 0,
                },
                SimpleLevelConfig {
                    level: 2,
                    code_num: 1,
                    rules: vec!["AaBa".to_string()],
                    allowed_orig_length: 0,
                },
                SimpleLevelConfig {
                    level: 3,
                    code_num: 1,
                    rules: vec!["AaBaCa".to_string()],
                    allowed_orig_length: 0,
                },
            ],
        }
    }
}