// =========================================================================
// 📐 自动校准模块
// =========================================================================

use crate::config;
use crate::types::{Metrics, ScaleConfig, SimpleMetrics};

/// 根据初始状态自动校准缩放因子
/// 
/// 使得不同量纲的指标在得分计算中具有相当的权重
pub fn calibrate_scales(initial_metrics: &Metrics, initial_simple: &SimpleMetrics) -> ScaleConfig {
    let eps = 1e-9;

    // 计算活跃的全码指标数量
    let active_count = [
        config::WEIGHT_COLLISION_COUNT,
        config::WEIGHT_COLLISION_RATE,
        config::WEIGHT_EQUIVALENCE,
        config::WEIGHT_EQUIV_CV,
        config::WEIGHT_DISTRIBUTION,
    ]
    .iter()
    .filter(|&&w| w > 0.0)
    .count();

    // 如果只有一个活跃指标，使用默认缩放
    let base = if active_count <= 1 {
        ScaleConfig::default()
    } else {
        // 根据初始值计算缩放因子
        ScaleConfig {
            collision_count: 1.0 / (initial_metrics.collision_count as f64 + eps),
            collision_rate: 1.0 / (initial_metrics.collision_rate + eps),
            equivalence: 1.0 / (initial_metrics.equiv_mean + eps),
            equiv_cv: 1.0 / (initial_metrics.equiv_cv + eps),
            distribution: 1.0 / (initial_metrics.dist_deviation + eps),
            ..ScaleConfig::default()
        }
    };

    // 如果未启用简码，返回基础配置
    if !config::ENABLE_SIMPLE_CODE {
        return base;
    }

    // 简码频率覆盖损失 = 1 - 覆盖率
    let freq_coverage_loss = 1.0 - initial_simple.weighted_freq_coverage;
    
    // 合并简码缩放因子
    ScaleConfig {
        simple_freq: 1.0 / (freq_coverage_loss + eps),
        simple_equiv: 1.0 / (initial_simple.equiv_mean + eps),
        simple_dist: 1.0 / (initial_simple.dist_deviation + eps),
        simple_collision_count: 1.0 / (initial_simple.collision_count as f64 + eps),
        simple_collision_rate: 1.0 / (initial_simple.collision_rate + eps),
        ..base
    }
}
