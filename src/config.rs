// =========================================================================
// 🔧 配置模块
// =========================================================================

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
}

/// 键位配置
#[derive(Debug, Clone, Deserialize)]
pub struct KeysConfig {
    pub allowed: String,
    pub display_order: String,
}

/// 权重配置
#[derive(Debug, Clone, Deserialize)]
pub struct WeightsConfig {
    pub full_code: FullCodeWeights,
    pub simple_code: SimpleCodeWeights,
}

/// 全码权重
#[derive(Debug, Clone, Deserialize)]
pub struct FullCodeWeights {
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
}

/// 简码权重
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleCodeWeights {
    pub enabled: bool,
    pub full_code_weight: f64,
    pub simple_code_weight: f64,
    pub freq: f64,
    pub equiv: f64,
    pub dist: f64,
    pub collision_count: f64,
    pub collision_rate: f64,
}

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
}

/// 简码级别配置（TOML 格式）
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleLevelConfig {
    pub level: usize,
    pub code_num: usize,
    pub rules: Vec<String>,
}

// =========================================================================
// 📥 配置加载
// =========================================================================

impl Config {
    /// 从 config.toml 加载配置
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
                }
            })
            .filter(|l| !l.rule_candidates.is_empty())
            .collect();

        SimpleCodeConfig { levels }
    }

    /// 验证权重配置是否合理
    pub fn validate_weights(&self) {
        let total_full = self.weights.full_code.collision_count
            + self.weights.full_code.collision_rate
            + self.weights.full_code.equivalence
            + self.weights.full_code.equiv_cv
            + self.weights.full_code.distribution;
        if (total_full - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：全码权重总和不为 1.0 (当前: {:.3})",
                total_full
            );
        }

        let total_simple = self.weights.simple_code.freq
            + self.weights.simple_code.equiv
            + self.weights.simple_code.dist
            + self.weights.simple_code.collision_count
            + self.weights.simple_code.collision_rate;
        if self.weights.simple_code.enabled && (total_simple - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：简码子权重总和不为 1.0 (当前: {:.3})",
                total_simple
            );
        }

        let total_main = self.weights.simple_code.full_code_weight
            + self.weights.simple_code.simple_code_weight;
        if self.weights.simple_code.enabled && (total_main - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：全码/简码总权重不为 1.0 (当前: {:.3})",
                total_main
            );
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
        WeightConfig {
            weight_collision_count: self.weights.full_code.collision_count,
            weight_collision_rate: self.weights.full_code.collision_rate,
            weight_equivalence: self.weights.full_code.equivalence,
            weight_equiv_cv: self.weights.full_code.equiv_cv,
            weight_distribution: self.weights.full_code.distribution,
            enable_simple_code: self.weights.simple_code.enabled,
            weight_full_code: self.weights.simple_code.full_code_weight,
            weight_simple_code: self.weights.simple_code.simple_code_weight,
            simple_weight_freq: self.weights.simple_code.freq,
            simple_weight_equiv: self.weights.simple_code.equiv,
            simple_weight_dist: self.weights.simple_code.dist,
            simple_weight_collision_count: self.weights.simple_code.collision_count,
            simple_weight_collision_rate: self.weights.simple_code.collision_rate,
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
            },
            keys: KeysConfig {
                allowed: "qwertyuiopasdfghjklzxcvbnm".to_string(),
                display_order: "qwertyuiopasdfghjklzxcvbnm".to_string(),
            },
            weights: WeightsConfig {
                full_code: FullCodeWeights {
                    collision_count: 0.07,
                    collision_rate: 0.62,
                    equivalence: 0.2,
                    equiv_cv: 0.01,
                    distribution: 0.1,
                },
                simple_code: SimpleCodeWeights {
                    enabled: true,
                    full_code_weight: 0.7,
                    simple_code_weight: 0.3,
                    freq: 0.5,
                    equiv: 0.15,
                    dist: 0.05,
                    collision_count: 0.05,
                    collision_rate: 0.25,
                },
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
            },
            simple_levels: vec![
                SimpleLevelConfig {
                    level: 1,
                    code_num: 0,
                    rules: vec!["Aa".to_string()],
                },
                SimpleLevelConfig {
                    level: 2,
                    code_num: 1,
                    rules: vec!["AaBa".to_string()],
                },
                SimpleLevelConfig {
                    level: 3,
                    code_num: 1,
                    rules: vec!["AaBaCa".to_string()],
                },
            ],
        }
    }
}