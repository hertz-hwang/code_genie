// =========================================================================
// 📐 自动校准模块
// =========================================================================

use crate::types::{Metrics, ScaleConfig, SimpleMetrics, WeightConfig, WordMetrics};

/// 根据初始状态自动校准缩放因子
///
/// 使得不同量纲的指标在得分计算中具有相当的权重（各指标初始贡献 ≈ 1.0）
pub fn calibrate_scales(
    initial_metrics: &Metrics,
    initial_simple: &SimpleMetrics,
    initial_word: &WordMetrics,
    weights: &WeightConfig,
) -> ScaleConfig {
    let eps = 1e-9;

    // ── 全码缩放 ──
    let full_top_n_collision = 1.0 / (initial_metrics.top_n_collision_count as f64 + eps);
    let full_collision_count = 1.0 / (initial_metrics.collision_count as f64 + eps);
    let full_collision_rate = 1.0 / (initial_metrics.collision_rate + eps);
    let full_equivalence = 1.0 / (initial_metrics.equiv_mean + eps);
    let full_distribution = 1.0 / (initial_metrics.dist_deviation + eps);

    // ── 简码缩放 ──
    let (simple_weighted_key_length, simple_collision_count, simple_collision_rate,
         simple_equivalence, simple_distribution) = if weights.enable_simple_code {
        (
            1.0 / (initial_simple.weighted_key_length + eps),
            1.0 / (initial_simple.collision_count as f64 + eps),
            1.0 / (initial_simple.collision_rate + eps),
            1.0 / (initial_simple.equiv_mean + eps),
            1.0 / (initial_simple.dist_deviation + eps),
        )
    } else {
        (1.0, 1.0, 1.0, 1.0, 1.0)
    };

    // ── 词码缩放 ──
    let (word_top2000_collision, word_top10000_collision, word_collision_count,
         word_collision_rate, word_equivalence, word_distribution) = if weights.enable_word_code {
        (
            1.0 / (initial_word.top2000_collision_count as f64 + eps),
            1.0 / (initial_word.top10000_collision_count as f64 + eps),
            1.0 / (initial_word.collision_count as f64 + eps),
            1.0 / (initial_word.collision_rate + eps),
            1.0 / (initial_word.equiv_mean + eps),
            1.0 / (initial_word.dist_deviation + eps),
        )
    } else {
        (1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
    };

    ScaleConfig {
        full_top_n_collision,
        full_collision_count,
        full_collision_rate,
        full_equivalence,
        full_distribution,
        simple_weighted_key_length,
        simple_collision_count,
        simple_collision_rate,
        simple_equivalence,
        simple_distribution,
        word_top2000_collision,
        word_top10000_collision,
        word_collision_count,
        word_collision_rate,
        word_equivalence,
        word_distribution,
    }
}
