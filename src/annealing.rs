// =========================================================================
// 🧠 模拟退火算法（多起点初始化 + 自适应优化）
// =========================================================================

use rand::prelude::*;

use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::schedule::TemperatureSchedule;
use crate::types::{char_to_key_index, Metrics, SimpleMetrics, KEY_SPACE};

// =========================================================================
// 初始化策略
// =========================================================================

/// 原始贪心初始化 - 按组大小降序，均衡分配到键位
fn greedy_balance_init(ctx: &OptContext, cfg: &Config, rng: &mut ThreadRng) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_groups];

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

    for (gi, _) in &group_freq {
        let gi = *gi;
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

/// 频率感知贪心 - 按组权重降序，最小化碰撞代价
fn frequency_greedy_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    // 计算每组的重要性权重（字符数 × 随机噪声以产生多样性）
    let mut group_info: Vec<(usize, f64)> = (0..n)
        .map(|gi| {
            let weight = ctx.group_to_chars[gi].len() as f64;
            let noise = 1.0 + rng.gen::<f64>() * 0.25;
            (gi, weight * noise)
        })
        .collect();
    group_info.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // 追踪每键的累计负载
    let mut key_freq_load = vec![0.0f64; KEY_SPACE];
    let mut key_group_count = vec![0usize; KEY_SPACE];

    for &(gi, weight) in &group_info {
        let allowed = &ctx.groups[gi].allowed_keys;

        let mut best_key = allowed[0];
        let mut best_cost = f64::MAX;

        for &k in allowed {
            let ki = k as usize;
            // 碰撞代价：当前键上已有负载 × 本组权重
            let collision_cost = key_freq_load[ki] * weight;
            // 均衡代价：轻微惩罚聚集
            let balance_cost = key_group_count[ki] as f64 * 0.05;
            let cost = collision_cost + balance_cost;

            if cost < best_cost {
                best_cost = cost;
                best_key = k;
            }
        }

        assignment[gi] = best_key;
        key_freq_load[best_key as usize] += weight;
        key_group_count[best_key as usize] += 1;
    }

    assignment
}

/// 分散优先贪心 - 尽可能让相邻分配的组落在不同键上
fn spread_greedy_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);

    let mut key_last_used = vec![0usize; KEY_SPACE];

    for (step, &gi) in order.iter().enumerate() {
        let allowed = &ctx.groups[gi].allowed_keys;
        // 选"最久未用"的键
        let best_key = *allowed
            .iter()
            .min_by_key(|&&k| key_last_used[k as usize])
            .unwrap();

        assignment[gi] = best_key;
        key_last_used[best_key as usize] = step + 1;
    }

    assignment
}

/// 纯随机有效解
fn random_valid_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        let allowed = &ctx.groups[gi].allowed_keys;
        assignment[gi] = allowed[rng.gen_range(0..allowed.len())];
    }
    assignment
}

// =========================================================================
// 爬山预热：零温局部搜索
// =========================================================================

/// 用极低温度做快速纯下降搜索
fn hill_climb_warmup(
    ctx: &OptContext,
    init: Vec<u8>,
    rng: &mut ThreadRng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let n = assignment.len();
    if n == 0 {
        let s = evaluator.get_score(ctx);
        return (assignment, s);
    }

    let zero_temp = 1e-15;
    let mut no_improve_count = 0usize;

    for _step in 0..max_steps {
        let score_before = evaluator.get_score(ctx);

        if rng.gen_bool(0.5) && n >= 2 {
            let r1 = rng.gen_range(0..n);
            let r2 = loop {
                let x = rng.gen_range(0..n);
                if x != r1 {
                    break x;
                }
            };
            let k1 = assignment[r1];
            let k2 = assignment[r2];
            if ctx.groups[r1].allowed_keys.contains(&k2)
                && ctx.groups[r2].allowed_keys.contains(&k1)
            {
                evaluator.try_swap(ctx, &mut assignment, r1, r2, zero_temp, rng);
            }
        } else {
            let r = rng.gen_range(0..n);
            let allowed = &ctx.groups[r].allowed_keys;
            let new_k = allowed[rng.gen_range(0..allowed.len())];
            evaluator.try_move(ctx, &mut assignment, r, new_k, zero_temp, rng);
        }

        let score_after = evaluator.get_score(ctx);
        if score_after < score_before - 1e-12 {
            no_improve_count = 0;
        } else {
            no_improve_count += 1;
        }

        // 连续无改进 → 已到局部最优
        if no_improve_count > n * 5 {
            break;
        }
    }

    let final_score = evaluator.get_score(ctx);
    (assignment, final_score)
}

// =========================================================================
// 坐标下降：系统性逐组精确优化
// =========================================================================

/// 对每个组遍历所有允许键选最优，重复直到收敛
fn coordinate_descent(ctx: &OptContext, init: Vec<u8>) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let n = assignment.len();
    let mut improved = true;

    while improved {
        improved = false;
        for gi in 0..n {
            let current_key = assignment[gi];
            let mut current_eval = Evaluator::new(ctx, &assignment);
            let current_score = current_eval.get_score(ctx);

            let mut best_key = current_key;
            let mut best_score = current_score;

            for &k in &ctx.groups[gi].allowed_keys {
                if k == current_key {
                    continue;
                }
                assignment[gi] = k;
                let mut eval = Evaluator::new(ctx, &assignment);
                let score = eval.get_score(ctx);
                if score < best_score - 1e-12 {
                    best_score = score;
                    best_key = k;
                }
            }

            assignment[gi] = best_key;
            if best_key != current_key {
                improved = true;
            }
        }
    }

    let mut eval = Evaluator::new(ctx, &assignment);
    let score = eval.get_score(ctx);
    (assignment, score)
}

// =========================================================================
// 🎯 多起点初始化入口
// =========================================================================

/// 多策略生成 + 爬山预热 + 择优
pub fn multi_start_init(ctx: &OptContext, cfg: &Config, thread_id: usize) -> Vec<u8> {
    let mut rng = thread_rng();
    let n = ctx.num_groups;
    if n == 0 {
        return vec![];
    }

    // 根据问题规模决定候选数量
    let n_candidates: usize = if n < 100 {
        32
    } else if n < 1000 {
        20
    } else {
        12
    };
    let warmup_steps = (n * 30).max(2000).min(50_000);

    let mut best_assignment: Option<Vec<u8>> = None;
    let mut best_score = f64::MAX;

    for trial in 0..n_candidates {
        // ── 生成候选 ──
        let strategy_name;
        let candidate = match trial % 5 {
            0 => {
                strategy_name = "均衡贪心";
                greedy_balance_init(ctx, cfg, &mut rng)
            }
            1 | 2 => {
                strategy_name = "频率贪心";
                frequency_greedy_init(ctx, &mut rng)
            }
            3 => {
                strategy_name = "分散贪心";
                spread_greedy_init(ctx, &mut rng)
            }
            _ => {
                strategy_name = "纯随机";
                random_valid_init(ctx, &mut rng)
            }
        };

        // ── 爬山预热 ──
        let (refined, score) = hill_climb_warmup(ctx, candidate, &mut rng, warmup_steps);

        if score < best_score {
            best_score = score;
            best_assignment = Some(refined);

            if thread_id == 0 {
                println!(
                    "   [Init T0] 候选 {}/{} 策略={} → 预热后得分: {:.4} ✓",
                    trial + 1,
                    n_candidates,
                    strategy_name,
                    score
                );
            }
        }
    }

    let best = best_assignment.unwrap();

    // 小规模问题再做坐标下降精炼
    if n <= 500 {
        let (polished, polished_score) = coordinate_descent(ctx, best.clone());
        if thread_id == 0 {
            println!(
                "   [Init T0] 坐标下降: {:.4} → {:.4}",
                best_score, polished_score
            );
        }
        if polished_score < best_score {
            return polished;
        }
    }

    best
}

// =========================================================================
// 兼容旧接口
// =========================================================================

/// 智能初始化（保留旧接口兼容性，内部调用多起点初始化）
pub fn smart_init(ctx: &OptContext, cfg: &Config) -> Vec<u8> {
    multi_start_init(ctx, cfg, usize::MAX) // usize::MAX 表示静默模式
}

// =========================================================================
// 🔥 模拟退火主循环
// =========================================================================

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

    // ━━━ 多起点初始化（替代原始 smart_init + 随机扰动）━━━
    let mut assignment = multi_start_init(ctx, cfg, thread_id);

    // 创建评估器
    let mut evaluator = Evaluator::new(ctx, &assignment);

    // 记录最佳状态
    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);
    let mut best_simple_metrics = evaluator.get_simple_metrics(ctx);

    if thread_id == 0 {
        let m = &best_metrics;
        let sm = &best_simple_metrics;
        println!(
            "   [T0] 初始化完成 | 得分: {:.4} | 重码: {} 重码率: {:.4}% 当量: {:.4} | 简码覆盖: {:.2}% 简码重码: {}",
            best_score,
            m.collision_count,
            m.collision_rate * 100.0,
            m.equiv_mean,
            sm.weighted_freq_coverage * 100.0,
            sm.collision_count
        );
    }

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

    // ━━━ 自适应邻域参数 ━━━
    // 高温阶段倾向大范围移动，低温阶段倾向精细交换
    let swap_prob_base = cfg.annealing.swap_probability;

    // ━━━ 主循环 ━━━
    for step in 0..steps {
        let progress = step as f64 / steps as f64;
        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        // 重新加热衰减
        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        // 自适应交换概率：后期增大交换比例以精细调优
        let swap_prob = swap_prob_base + (1.0 - swap_prob_base) * progress * 0.3;

        // 选择操作：交换或移动
        if rng.gen::<f64>() < swap_prob && n_groups >= 2 {
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
            let pct = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            let sm = evaluator.get_simple_metrics(ctx);
            println!(
                "   [T0] 进度: {}% | 基温: {:.6} | 重码={} 当量={:.4} | 简码覆盖:{:.2}% 简码重码:{} | 当前: {:.4} 🏆最优: {:.4}",
                pct, base_temp,
                m.collision_count, m.equiv_mean,
                sm.weighted_freq_coverage * 100.0,
                sm.collision_count,
                evaluator.get_score(ctx), best_score
            );
        }
    }

    // ━━━ SA 结束后：对最优解做最终精炼 ━━━
    if thread_id == 0 {
        println!("   [T0] SA 完成，执行最终精炼...");
    }

    // 最终爬山：从最优解出发做零温搜索
    let final_warmup_steps = (n_groups * 50).max(5000).min(100_000);
    let (final_assignment, final_score) =
        hill_climb_warmup(ctx, best_assignment.clone(), &mut rng, final_warmup_steps);

    if final_score < best_score {
        best_assignment = final_assignment;
        best_score = final_score;
        let eval = Evaluator::new(ctx, &best_assignment);
        best_metrics = eval.get_metrics(ctx);
        best_simple_metrics = eval.get_simple_metrics(ctx);

        if thread_id == 0 {
            println!(
                "   [T0] 最终精炼改进: {:.4} → {:.4}",
                best_score + (best_score - final_score).abs(),
                best_score
            );
        }
    }

    // 小规模再做坐标下降
    if n_groups <= 500 {
        let score_before_cd = best_score;
        let (cd_assignment, cd_score) = coordinate_descent(ctx, best_assignment.clone());
        if cd_score < best_score {
            best_assignment = cd_assignment;
            best_score = cd_score;
            let eval = Evaluator::new(ctx, &best_assignment);
            best_metrics = eval.get_metrics(ctx);
            best_simple_metrics = eval.get_simple_metrics(ctx);

            if thread_id == 0 {
                println!(
                    "   [T0] 坐标下降精炼: {:.4} → {:.4}",
                    score_before_cd, best_score
                );
            }
        }
    }

    (
        best_assignment,
        best_score,
        best_metrics,
        best_simple_metrics,
    )
}