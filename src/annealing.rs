// =========================================================================
// 🧠 模拟退火算法
// =========================================================================

use rand::prelude::*;

use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::schedule::TemperatureSchedule;
use crate::types::{char_to_key_index, Metrics, SimpleMetrics, KEY_SPACE};

/// 智能初始化 - 使用贪心算法生成初始分配
pub fn smart_init(ctx: &OptContext, cfg: &Config) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_groups];
    let mut rng = thread_rng();

    // 按组内汉字数量降序排列
    let mut group_freq: Vec<(usize, usize)> = ctx
        .group_to_chars
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    group_freq.sort_by(|a, b| b.1.cmp(&a.1));

    let max_ki = cfg
        .keys
        .allowed
        .chars()
        .filter_map(char_to_key_index)
        .max()
        .unwrap_or(25);
    let mut key_counts = vec![0usize; max_ki + 1];

    for (gi, _) in group_freq {
        let allowed = &ctx.groups[gi].allowed_keys;
        let min_count = allowed
            .iter()
            .map(|&k| key_counts.get(k as usize).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);

        let candidates: Vec<u8> = allowed
            .iter()
            .filter(|&&k| key_counts.get(k as usize).copied().unwrap_or(0) == min_count)
            .copied()
            .collect();

        let best = if candidates.is_empty() {
            allowed[0]
        } else {
            candidates[rng.gen_range(0..candidates.len())]
        };

        assignment[gi] = best;
        if (best as usize) < key_counts.len() {
            key_counts[best as usize] += 1;
        }
    }
    assignment
}

/// 模拟退火优化
/// 
/// # 返回值
/// - (最佳分配, 最佳得分, 最佳指标, 最佳简码指标)
pub fn simulated_annealing(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
) -> (Vec<u8>, f64, Metrics, SimpleMetrics) {
    let mut rng = thread_rng();

    // 智能初始化
    let mut assignment = smart_init(ctx, cfg);
    
    // 10% 概率随机扰动
    if rng.gen_bool(0.5) {
        for i in 0..assignment.len() {
            if rng.gen_bool(0.1) {
                let allowed = &ctx.groups[i].allowed_keys;
                assignment[i] = allowed[rng.gen_range(0..allowed.len())];
            }
        }
    }

    // 创建评估器
    let mut evaluator = Evaluator::new(ctx, &assignment);
    
    // 记录最佳状态
    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);
    let mut best_simple_metrics = evaluator.get_simple_metrics(ctx);

    let steps = cfg.annealing.total_steps;

    // 构建温度调度器
    let schedule = TemperatureSchedule::build(
        cfg.annealing.temp_start,
        cfg.annealing.temp_end,
        cfg.annealing.comfort_temp,
        cfg.annealing.comfort_width,
        cfg.annealing.comfort_slowdown,
    );

    // 主线程打印预览
    if thread_id == 0 {
        schedule.print_preview(steps);
    }

    // 重新加热参数
    let mut temp_multiplier = 1.0f64;
    let min_improve_steps = cfg.min_improve_steps();
    let reheat_decay = if min_improve_steps > 0 {
        (0.01f64).powf(1.0 / min_improve_steps as f64)
    } else {
        0.99
    };

    let mut steps_since_improve = 0usize;
    let mut last_best_score = best_score;

    let n_groups = assignment.len();
    if n_groups == 0 {
        return (
            best_assignment,
            best_score,
            best_metrics,
            best_simple_metrics,
        );
    }

    let report_interval = (steps / 20).max(1);
    let perturb_interval = cfg.perturb_interval();

    // 主循环
    for step in 0..steps {
        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        // 重新加热衰减
        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        // 选择操作：交换或移动
        if rng.gen::<f64>() < cfg.annealing.swap_probability && n_groups >= 2 {
            let r1 = rng.gen_range(0..n_groups);
            let r2 = rng.gen_range(0..n_groups - 1);
            let r2 = if r2 >= r1 { r2 + 1 } else { r2 };

            let k1 = assignment[r1];
            let k2 = assignment[r2];
            let can_swap = ctx.groups[r1].allowed_keys.contains(&k2)
                && ctx.groups[r2].allowed_keys.contains(&k1);
            if can_swap {
                evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
            }
        } else {
            let r = rng.gen_range(0..n_groups);
            let allowed = &ctx.groups[r].allowed_keys;
            let new_k = allowed[rng.gen_range(0..allowed.len())];
            evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
        }

        // 检查是否找到更好的解
        let current_score = evaluator.get_score(ctx);
        if current_score < best_score {
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            best_simple_metrics = evaluator.get_simple_metrics(ctx);
            steps_since_improve = 0;

            // 主线程打印重大改进
            if thread_id == 0 && best_score <= last_best_score - 0.9 {
                let m = best_metrics;
                let sm = best_simple_metrics;
                println!(
                    "   [T0] 步数 {}/{} | 温度 {:.9} | 重码:{} 重码率:{:.4}% 当量:{:.4} | 简码覆盖:{:.2}% 简码重码:{} 简码重码率:{:.4}% | 得分: {:.4}",
                    step, steps, temp, m.collision_count, m.collision_rate * 100.0,
                    m.equiv_mean, sm.weighted_freq_coverage * 100.0,
                    sm.collision_count, sm.collision_rate * 100.0,
                    best_score
                );
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        // 长时间无改进则重新加热
        if steps_since_improve > min_improve_steps {
            temp_multiplier = cfg.annealing.reheat_factor;
            steps_since_improve = 0;

            if thread_id == 0 {
                println!(
                    "   [T0] 步数 {}: Reheat ×{:.1} (基温 {:.6})",
                    step,
                    cfg.annealing.reheat_factor,
                    base_temp
                );
            }
        }

        // 低温扰动
        if step > 0
            && step % perturb_interval == 0
            && base_temp < cfg.annealing.comfort_temp * 0.01
        {
            let n_perturb = (n_groups as f64 * cfg.annealing.perturb_strength) as usize;
            for _ in 0..n_perturb {
                let r1 = rng.gen_range(0..n_groups);
                let r2 = rng.gen_range(0..n_groups);
                if r1 != r2 {
                    let ka = assignment[r1];
                    let kb = assignment[r2];
                    let can = ctx.groups[r1].allowed_keys.contains(&kb)
                        && ctx.groups[r2].allowed_keys.contains(&ka);
                    if can {
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, temp * 2.0, &mut rng);
                    }
                }
            }

            if thread_id == 0 {
                let m = evaluator.get_metrics(ctx);
                println!(
                    "   [T0] 步数 {}: 扰动 | 重码={} 当量={:.4} | 当前: {:.4}",
                    step,
                    m.collision_count,
                    m.equiv_mean,
                    evaluator.get_score(ctx)
                );
            }
        }

        // 定期报告进度
        if thread_id == 0 && step % report_interval == 0 && step > 0 {
            let progress = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            let sm = evaluator.get_simple_metrics(ctx);
            println!(
                "   [T0] 进度: {}% | 基温: {:.6} | 重码={} 当量={:.4} | 简码覆盖:{:.2}% 简码重码:{} | 当前: {:.4} 🏆最优: {:.4}",
                progress, base_temp,
                m.collision_count, m.equiv_mean,
                sm.weighted_freq_coverage * 100.0,
                sm.collision_count,
                evaluator.get_score(ctx), best_score
            );
        }
    }

    (
        best_assignment,
        best_score,
        best_metrics,
        best_simple_metrics,
    )
}