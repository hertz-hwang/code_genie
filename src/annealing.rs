// =========================================================================
// 🧠 混合优化算法（模拟退火 + 冲突导向邻域）
// =========================================================================

use rand::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::checkpoint::ThreadCheckpoint;
use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::schedule::TemperatureSchedule;
use crate::types::{char_to_key_index, Metrics, SimpleMetrics, WordMetrics, GROUP_MARKER, KEY_SPACE};

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

    let mut group_info: Vec<(usize, f64)> = (0..n)
        .map(|gi| {
            let weight = ctx.group_to_chars[gi].len() as f64;
            let noise = 1.0 + rng.gen::<f64>() * 0.25;
            (gi, weight * noise)
        })
        .collect();
    group_info.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut key_freq_load = vec![0.0f64; KEY_SPACE];
    let mut key_group_count = vec![0usize; KEY_SPACE];

    for &(gi, weight) in &group_info {
        let allowed = &ctx.groups[gi].allowed_keys;

        let mut best_key = allowed[0];
        let mut best_cost = f64::MAX;

        for &k in allowed {
            let ki = k as usize;
            let collision_cost = key_freq_load[ki] * weight;
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

/// 分散优先贪心
fn spread_greedy_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);

    let mut key_last_used = vec![0usize; KEY_SPACE];

    for (step, &gi) in order.iter().enumerate() {
        let allowed = &ctx.groups[gi].allowed_keys;
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
// 🔍 冲突分析
// =========================================================================

/// 构建编码到汉字的反向索引
fn build_code_to_chars(ctx: &OptContext, assignment: &[u8]) -> HashMap<usize, Vec<usize>> {
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..ctx.char_infos.len() {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }
    code_to_chars
}

/// 找出所有重码冲突的字根组对
fn find_collision_groups(
    ctx: &OptContext,
    assignment: &[u8],
) -> Vec<(usize, usize, usize)> {
    let code_to_chars = build_code_to_chars(ctx, assignment);
    let mut collisions: Vec<(usize, usize, usize)> = Vec::new();

    for chars in code_to_chars.values() {
        if chars.len() < 2 {
            continue;
        }
        let mut groups_in_conflict: HashSet<usize> = HashSet::new();
        for &ci in chars {
            let info = &ctx.char_infos[ci];
            for &p in info.parts_slice() {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    groups_in_conflict.insert(gi);
                }
            }
        }

        let groups: Vec<usize> = groups_in_conflict.into_iter().collect();
        for i in 0..groups.len() {
            for j in (i + 1)..groups.len() {
                collisions.push((groups[i], groups[j], chars.len()));
            }
        }
    }

    collisions.sort_by(|a, b| b.2.cmp(&a.2));
    collisions
}

/// 找出特定键位上的所有字根组
fn find_groups_on_key(ctx: &OptContext, assignment: &[u8], key: u8) -> Vec<usize> {
    (0..ctx.num_groups)
        .filter(|&gi| assignment[gi] == key)
        .collect()
}

// =========================================================================
// 🎯 冲突导向的邻域操作
// =========================================================================

/// 尝试解决冲突：将冲突组中的一个移动到新键位
fn try_resolve_conflict(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    collisions: &[(usize, usize, usize)],
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    if collisions.is_empty() {
        return false;
    }

    let idx = rng.gen_range(0..collisions.len().min(20));
    let (g1, g2, _) = collisions[idx];

    let groups_to_try = if rng.gen_bool(0.5) { vec![g1, g2] } else { vec![g2, g1] };

    for &gi in &groups_to_try {
        let current_key = assignment[gi];
        let allowed = &ctx.groups[gi].allowed_keys;

        let other_keys: Vec<u8> = allowed
            .iter()
            .filter(|&&k| k != current_key)
            .copied()
            .collect();

        if other_keys.is_empty() {
            continue;
        }

        let new_key = other_keys[rng.gen_range(0..other_keys.len())];
        if evaluator.try_move(ctx, assignment, gi, new_key, temp, rng) {
            return true;
        }
    }

    false
}

/// 尝试键位重组
fn try_key_reorganization(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    let n = assignment.len();
    if n < 2 {
        return false;
    }

    let k1 = assignment[rng.gen_range(0..n)];
    let groups_on_k1: Vec<usize> = find_groups_on_key(ctx, assignment, k1);

    if groups_on_k1.is_empty() {
        return false;
    }

    let gi = groups_on_k1[rng.gen_range(0..groups_on_k1.len())];
    let allowed = &ctx.groups[gi].allowed_keys;

    let other_keys: Vec<u8> = allowed
        .iter()
        .filter(|&&k| k != k1)
        .copied()
        .collect();

    if other_keys.is_empty() {
        return false;
    }

    let new_key = other_keys[rng.gen_range(0..other_keys.len())];
    evaluator.try_move(ctx, assignment, gi, new_key, temp, rng)
}

/// 三组循环交换 (g1←k2, g2←k3, g3←k1) — 增量评估
fn try_triple_swap(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    let n = assignment.len();
    if n < 3 {
        return false;
    }

    let indices: Vec<usize> = (0..n).choose_multiple(rng, 3);
    if indices.len() < 3 {
        return false;
    }

    let [g1, g2, g3] = [indices[0], indices[1], indices[2]];
    let [k1, k2, k3] = [assignment[g1], assignment[g2], assignment[g3]];

    // 三个键都相同则无意义
    if k1 == k2 && k2 == k3 {
        return false;
    }

    // 检查循环交换的合法性: g1→k2, g2→k3, g3→k1
    if !ctx.groups[g1].allowed_keys.contains(&k2)
        || !ctx.groups[g2].allowed_keys.contains(&k3)
        || !ctx.groups[g3].allowed_keys.contains(&k1)
    {
        return false;
    }

    let old_score = evaluator.get_score(ctx);

    // 更新 key_weighted_usage
    for &(gi, old_k, new_k) in &[(g1, k1, k2), (g2, k2, k3), (g3, k3, k1)] {
        if old_k == new_k {
            continue;
        }
        for &ci in &ctx.group_to_chars[gi] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in ctx.char_infos[ci].parts_slice() {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                    evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                    evaluator.key_weighted_usage[new_k as usize] += freq_f;
                }
            }
        }
    }

    // 执行交换
    assignment[g1] = k2;
    assignment[g2] = k3;
    assignment[g3] = k1;

    // 增量更新受影响的汉字编码
    for &gi in &[g1, g2, g3] {
        for &ci in &ctx.group_to_chars[gi] {
            evaluator.update_char(ctx, assignment, ci);
        }
    }

    evaluator.score_dirty = true;
    let new_score = evaluator.get_score(ctx);
    let delta = new_score - old_score;

    if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
        true
    } else {
        // 回滚 key_weighted_usage
        for &(gi, old_k, new_k) in &[(g1, k2, k1), (g2, k3, k2), (g3, k1, k3)] {
            if old_k == new_k {
                continue;
            }
            for &ci in &ctx.group_to_chars[gi] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in ctx.char_infos[ci].parts_slice() {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                        evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                        evaluator.key_weighted_usage[new_k as usize] += freq_f;
                    }
                }
            }
        }

        // 回滚 assignment
        assignment[g1] = k1;
        assignment[g2] = k2;
        assignment[g3] = k3;

        // 回滚编码
        for &gi in &[g1, g2, g3] {
            for &ci in &ctx.group_to_chars[gi] {
                evaluator.update_char(ctx, assignment, ci);
            }
        }

        evaluator.cached_score = old_score;
        evaluator.score_dirty = false;
        false
    }
}

// =========================================================================
// 🔧 增强版爬山算法
// =========================================================================

/// 增强版爬山：结合冲突导向的邻域操作
fn enhanced_hill_climb(
    ctx: &OptContext,
    init: Vec<u8>,
    rng: &mut ThreadRng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let n = assignment.len();
    if n == 0 {
        return (assignment, evaluator.get_score(ctx));
    }

    let zero_temp = 1e-15;
    let mut no_improve_count = 0usize;
    let mut collisions = find_collision_groups(ctx, &assignment);

    for step in 0..max_steps {
        let op_type = step % 10;

        let success = match op_type {
            0..=3 => {
                if !collisions.is_empty() {
                    try_resolve_conflict(ctx, &mut assignment, &mut evaluator, &collisions, zero_temp, rng)
                } else {
                    false
                }
            }
            4..=6 => {
                if n >= 2 {
                    let r1 = rng.gen_range(0..n);
                    let r2 = rng.gen_range(0..n - 1);
                    let r2 = if r2 >= r1 { r2 + 1 } else { r2 };
                    let k1 = assignment[r1];
                    let k2 = assignment[r2];
                    if ctx.groups[r1].allowed_keys.contains(&k2)
                        && ctx.groups[r2].allowed_keys.contains(&k1)
                    {
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, zero_temp, rng)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            7 => try_triple_swap(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            8 => try_key_reorganization(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            _ => {
                let r = rng.gen_range(0..n);
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.gen_range(0..allowed.len())];
                evaluator.try_move(ctx, &mut assignment, r, new_k, zero_temp, rng)
            }
        };

        // At zero_temp, success means score improved (only strict improvements accepted)
        if success {
            no_improve_count = 0;
            if step % 200 == 0 {
                collisions = find_collision_groups(ctx, &assignment);
            }
        } else {
            no_improve_count += 1;
        }

        if no_improve_count > n * 10 {
            break;
        }
    }

    (assignment, evaluator.get_score(ctx))
}

fn hill_climb_warmup(
    ctx: &OptContext,
    init: Vec<u8>,
    rng: &mut ThreadRng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    enhanced_hill_climb(ctx, init, rng, max_steps)
}

// =========================================================================
// 坐标下降
// =========================================================================

fn coordinate_descent(ctx: &OptContext, init: Vec<u8>) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let n = assignment.len();
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut improved = true;

    while improved {
        improved = false;
        for gi in 0..n {
            let current_key = assignment[gi];
            let current_score = evaluator.get_score(ctx);

            let mut best_key = current_key;
            let mut best_score = current_score;

            for &k in &ctx.groups[gi].allowed_keys {
                if k == current_key {
                    continue;
                }

                // 增量前向：移动到候选键
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in ctx.char_infos[ci].parts_slice() {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[assignment[gi] as usize] -= freq_f;
                            evaluator.key_weighted_usage[k as usize] += freq_f;
                        }
                    }
                }

                let prev_key = assignment[gi];
                assignment[gi] = k;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.score_dirty = true;
                let score = evaluator.get_score(ctx);

                if score < best_score - 1e-12 {
                    best_score = score;
                    best_key = k;
                }

                // 回滚
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in ctx.char_infos[ci].parts_slice() {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[k as usize] -= freq_f;
                            evaluator.key_weighted_usage[prev_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = prev_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.cached_score = current_score;
                evaluator.score_dirty = false;
            }

            if best_key != current_key {
                // 应用最优移动
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in ctx.char_infos[ci].parts_slice() {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[current_key as usize] -= freq_f;
                            evaluator.key_weighted_usage[best_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = best_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.score_dirty = true;
                improved = true;
            }
        }
    }

    // 坐标下降完成后做一次简码重建，确保最终得分包含简码
    evaluator.rebuild_simple(ctx, &assignment);
    evaluator.score_dirty = true;
    let score = evaluator.get_score(ctx);
    (assignment, score)
}

// =========================================================================
// 🎯 多起点初始化入口
// =========================================================================

pub fn multi_start_init(ctx: &OptContext, cfg: &Config, thread_id: usize) -> Vec<u8> {
    let mut rng = thread_rng();
    let n = ctx.num_groups;
    if n == 0 {
        return vec![];
    }

    let n_candidates: usize = if n < 100 { 16 } else if n < 1000 { 12 } else { 8 };
    let warmup_steps = (n * 20).max(1000).min(20_000);

    let mut best_assignment: Option<Vec<u8>> = None;
    let mut best_score = f64::MAX;

    for trial in 0..n_candidates {
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

        let (refined, score) = hill_climb_warmup(ctx, candidate, &mut rng, warmup_steps);

        if score < best_score {
            best_score = score;
            best_assignment = Some(refined);

            if thread_id == 0 {
                println!(
                    "   [Init T0] 候选 {}/{} 策略={} → 预热后得分: {:.4} ✓",
                    trial + 1, n_candidates, strategy_name, score
                );
            }
        }
    }

    let best = best_assignment.unwrap();

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

#[allow(dead_code)]
pub fn smart_init(ctx: &OptContext, cfg: &Config) -> Vec<u8> {
    multi_start_init(ctx, cfg, usize::MAX)
}

/// 纯随机初始化（供校准阶段使用，无需精确优化）
pub fn random_init(ctx: &OptContext) -> Vec<u8> {
    let mut rng = thread_rng();
    random_valid_init(ctx, &mut rng)
}

// =========================================================================
// 🌡️ 起始温度自动校准
// =========================================================================

/// 采样随机 move/swap 操作的 delta 值，计算使初始接受率达到目标值的起始温度。
///
/// 原理：对于 Boltzmann 接受准则 P = exp(-Δ/T)，给定目标接受率 p，
/// 取采样到的恶化 delta 的中位数 Δ_med，解出 T = -Δ_med / ln(p)。
///
/// 参数：
/// - `target_accept_rate`: 目标初始接受率，推荐 0.3-0.5
/// - `n_samples`: 采样次数，推荐 2000-5000
pub fn calibrate_temperature(
    ctx: &OptContext,
    assignment: &[u8],
    target_accept_rate: f64,
    n_samples: usize,
) -> f64 {
    let mut rng = thread_rng();
    let n_groups = assignment.len();
    if n_groups == 0 {
        return 1.0;
    }

    let mut assignment = assignment.to_vec();
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut deltas: Vec<f64> = Vec::with_capacity(n_samples);

    for i in 0..n_samples {
        let base_score = evaluator.get_score(ctx);

        // 交替使用 move 和 swap 采样
        if i % 3 == 0 && n_groups >= 2 {
            // swap 采样
            let r1 = rng.gen_range(0..n_groups);
            let r2 = rng.gen_range(0..n_groups - 1);
            let r2 = if r2 >= r1 { r2 + 1 } else { r2 };
            let k1 = assignment[r1];
            let k2 = assignment[r2];
            if k1 != k2
                && ctx.groups[r1].allowed_keys.contains(&k2)
                && ctx.groups[r2].allowed_keys.contains(&k1)
            {
                // 执行 swap
                let gfs1 = ctx.group_freq_sum[r1];
                let gfs2 = ctx.group_freq_sum[r2];
                evaluator.key_weighted_usage[k1 as usize] -= gfs1;
                evaluator.key_weighted_usage[k2 as usize] += gfs1;
                evaluator.key_weighted_usage[k2 as usize] -= gfs2;
                evaluator.key_weighted_usage[k1 as usize] += gfs2;

                assignment[r1] = k2;
                assignment[r2] = k1;
                for &ci in &ctx.group_to_chars[r1] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                for &ci in &ctx.group_to_chars[r2] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.score_dirty = true;
                let new_score = evaluator.get_score(ctx);
                let delta = new_score - base_score;
                if delta > 0.0 {
                    deltas.push(delta);
                }

                // 回滚
                evaluator.key_weighted_usage[k2 as usize] -= gfs1;
                evaluator.key_weighted_usage[k1 as usize] += gfs1;
                evaluator.key_weighted_usage[k1 as usize] -= gfs2;
                evaluator.key_weighted_usage[k2 as usize] += gfs2;
                assignment[r1] = k1;
                assignment[r2] = k2;
                for &ci in &ctx.group_to_chars[r1] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                for &ci in &ctx.group_to_chars[r2] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.cached_score = base_score;
                evaluator.score_dirty = false;
            }
        } else {
            // move 采样
            let r = rng.gen_range(0..n_groups);
            let allowed = &ctx.groups[r].allowed_keys;
            let new_key = allowed[rng.gen_range(0..allowed.len())];
            let old_key = assignment[r];
            if new_key == old_key {
                continue;
            }

            let gfs = ctx.group_freq_sum[r];
            evaluator.key_weighted_usage[old_key as usize] -= gfs;
            evaluator.key_weighted_usage[new_key as usize] += gfs;

            assignment[r] = new_key;
            for &ci in &ctx.group_to_chars[r] {
                evaluator.update_char(ctx, &assignment, ci);
            }
            evaluator.score_dirty = true;
            let new_score = evaluator.get_score(ctx);
            let delta = new_score - base_score;
            if delta > 0.0 {
                deltas.push(delta);
            }

            // 回滚
            evaluator.key_weighted_usage[new_key as usize] -= gfs;
            evaluator.key_weighted_usage[old_key as usize] += gfs;
            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                evaluator.update_char(ctx, &assignment, ci);
            }
            evaluator.cached_score = base_score;
            evaluator.score_dirty = false;
        }
    }

    if deltas.is_empty() {
        return 0.01; // 极端情况：所有移动都不恶化
    }

    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 用中位数 delta 和目标接受率反解温度: T = -Δ_med / ln(p)
    let median_idx = deltas.len() / 2;
    let delta_median = deltas[median_idx];

    // 同时看 P75 delta 用于参考
    let p75_idx = deltas.len() * 3 / 4;
    let delta_p75 = deltas[p75_idx];

    // T = -delta / ln(accept_rate)，ln(accept_rate) 为负数所以 T 为正
    let t_from_median = -delta_median / target_accept_rate.ln();
    let t_from_p75 = -delta_p75 / target_accept_rate.ln();

    // 使用中位数作为主校准值
    let calibrated_temp = t_from_median;

    println!("  🌡️ 温度校准 (采样 {} 次, {} 个恶化 delta):", n_samples, deltas.len());
    println!(
        "    delta 分布: min={:.6}, P25={:.6}, median={:.6}, P75={:.6}, max={:.6}",
        deltas[0],
        deltas[deltas.len() / 4],
        delta_median,
        delta_p75,
        deltas[deltas.len() - 1]
    );
    println!(
        "    目标接受率: {:.0}% → 校准 T_start={:.6} (P75参考: {:.6})",
        target_accept_rate * 100.0, calibrated_temp, t_from_p75
    );

    calibrated_temp
}

// =========================================================================
// 🔥 模拟退火主循环
// =========================================================================

/// SA 主循环返回类型
pub struct SaResult {
    pub assignment: Vec<u8>,
    pub score: f64,
    pub metrics: Metrics,
    pub simple_metrics: SimpleMetrics,
    pub word_metrics: WordMetrics,
    /// 若被中断，保存线程检查点
    pub checkpoint: Option<ThreadCheckpoint>,
    pub actual_temp_start: f64,
    pub actual_comfort_temp: f64,
}

pub fn simulated_annealing(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
) -> (Vec<u8>, f64, Metrics, SimpleMetrics) {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let result = simulated_annealing_resumable(ctx, cfg, thread_id, &stop_flag, None, None);
    (result.assignment, result.score, result.metrics, result.simple_metrics)
}

/// 可断点续算的 SA 主循环
///
/// - `stop_flag`: 外部信号，为 true 时主循环提前退出并生成检查点
/// - `resume`: 若非 None，从该检查点恢复继续
/// - `schedule_override`: 覆盖温度调度参数 (actual_temp_start, actual_comfort_temp)
pub fn simulated_annealing_resumable(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
    stop_flag: &Arc<AtomicBool>,
    resume: Option<&ThreadCheckpoint>,
    schedule_override: Option<(f64, f64)>,
) -> SaResult {
    let mut rng = thread_rng();

    // ---- 初始化或从检查点恢复 ----
    let start_step;
    let mut assignment;
    let mut best_assignment;
    let mut best_score;
    let mut best_metrics;
    let mut best_simple_metrics;
    let mut best_word_metrics;
    let mut temp_multiplier;
    let mut steps_since_improve;
    let mut last_best_score;

    if let Some(ckpt) = resume {
        assignment = ckpt.assignment.clone();
        best_assignment = ckpt.best_assignment.clone();
        best_score = ckpt.best_score;
        best_metrics = ckpt.best_metrics;
        best_simple_metrics = ckpt.best_simple_metrics;
        best_word_metrics = ckpt.best_word_metrics;
        start_step = ckpt.current_step;
        temp_multiplier = ckpt.temp_multiplier;
        steps_since_improve = ckpt.steps_since_improve;
        last_best_score = ckpt.last_best_score;

        if thread_id == 0 {
            println!(
                "   [T0] 从检查点恢复 | 步数: {}/{} | 最优得分: {:.4}",
                start_step, cfg.annealing.total_steps, best_score
            );
        }
    } else {
        // 正常初始化
        assignment = multi_start_init(ctx, cfg, thread_id);
        best_assignment = assignment.clone();
        start_step = 0;
        temp_multiplier = 1.0;
        steps_since_improve = 0;

        let mut evaluator_init = Evaluator::new(ctx, &assignment);
        best_score = evaluator_init.get_score(ctx);
        best_metrics = evaluator_init.get_metrics(ctx);
        best_simple_metrics = evaluator_init.get_simple_metrics(ctx);
        best_word_metrics = evaluator_init.get_word_metrics(ctx);
        last_best_score = best_score;

        if thread_id == 0 {
            let m = &best_metrics;
            println!(
                "   [T0] 初始化完成 | 得分: {:.4} | 重码: {} 重码率: {:.4}% 当量: {:.4}",
                best_score, m.collision_count, m.collision_rate * 100.0, m.equiv_mean
            );
        }
    }

    let mut evaluator = Evaluator::new(ctx, &assignment);

    let steps = cfg.annealing.total_steps;
    let n_groups = assignment.len();
    if n_groups == 0 {
        return SaResult {
            assignment: best_assignment,
            score: best_score,
            metrics: best_metrics,
            simple_metrics: best_simple_metrics,
            word_metrics: best_word_metrics,
            checkpoint: None,
            actual_temp_start: 0.0,
            actual_comfort_temp: 0.0,
        };
    }

    // 温度校准：优先使用 schedule_override，否则自动校准
    let (actual_temp_start, actual_comfort_temp) = if let Some((ts, ct)) = schedule_override {
        (ts, ct)
    } else if cfg.annealing.temp_start <= 0.0 {
        let calibrated = calibrate_temperature(ctx, &assignment, 0.4, 3000);
        let comfort = calibrated * 0.2;
        if thread_id == 0 {
            println!(
                "    自动 comfort_temp={:.6} (T_start × 0.2)",
                comfort
            );
        }
        (calibrated, comfort)
    } else {
        (cfg.annealing.temp_start, cfg.annealing.comfort_temp)
    };

    let schedule = TemperatureSchedule::build(
        actual_temp_start,
        cfg.annealing.temp_end,
        actual_comfort_temp,
        cfg.annealing.comfort_width,
        cfg.annealing.comfort_slowdown,
    );

    if thread_id == 0 {
        schedule.print_preview(steps);
    }

    // temp_multiplier / steps_since_improve / last_best_score 已在上方初始化或恢复
    let min_improve_steps = cfg.min_improve_steps();
    let reheat_decay = if min_improve_steps > 0 {
        (0.01f64).powf(1.0 / min_improve_steps as f64)
    } else {
        0.99
    };

    let report_interval = (steps / 20).max(1);
    let perturb_interval = cfg.perturb_interval();

    let swap_prob_base = cfg.annealing.swap_probability;

    let sa_start = Instant::now();

    // 主循环（从 start_step 开始，支持断点续算）
    for step in start_step..steps {
        // 检查外部停止信号（每 10000 步检查一次）
        if step % 10000 == 0 && step > start_step && stop_flag.load(Ordering::Relaxed) {
            if thread_id == 0 {
                println!("\n   [T0] 收到停止信号，正在保存检查点...");
            }
            // 保存当前线程检查点
            let ckpt = ThreadCheckpoint {
                thread_id,
                assignment: assignment.clone(),
                best_assignment: best_assignment.clone(),
                best_score,
                best_metrics,
                best_simple_metrics,
                best_word_metrics,
                current_step: step,
                temp_multiplier,
                steps_since_improve,
                last_best_score,
            };
            return SaResult {
                assignment: best_assignment,
                score: best_score,
                metrics: best_metrics,
                simple_metrics: best_simple_metrics,
                word_metrics: best_word_metrics,
                checkpoint: Some(ckpt),
                actual_temp_start,
                actual_comfort_temp,
            };
        }

        let _progress = step as f64 / steps as f64;
        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        let swap_prob = swap_prob_base + (1.0 - swap_prob_base) * (step as f64 / steps as f64) * 0.3;

        if rng.gen::<f64>() < swap_prob && n_groups >= 2 {
            let r1 = rng.gen_range(0..n_groups);
            let r2 = rng.gen_range(0..n_groups - 1);
            let r2 = if r2 >= r1 { r2 + 1 } else { r2 };

            let k1 = assignment[r1];
            let k2 = assignment[r2];
            if k1 != k2
                && ctx.groups[r1].allowed_keys.contains(&k2)
                && ctx.groups[r2].allowed_keys.contains(&k1)
            {
                evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
            } else {
                let r = r1;
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.gen_range(0..allowed.len())];
                evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
            }
        } else {
            let r = rng.gen_range(0..n_groups);
            let allowed = &ctx.groups[r].allowed_keys;
            let new_k = allowed[rng.gen_range(0..allowed.len())];
            evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
        }

        // 周期性刷新简码得分（每 10000 步一次，避免每步全量重建）
        if ctx.enable_simple_code && step % 10000 == 0 {
            evaluator.rebuild_simple(ctx, &assignment);
            evaluator.score_dirty = true;
        }

        let current_score = evaluator.get_score(ctx);
        if current_score < best_score {
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            best_simple_metrics = evaluator.get_simple_metrics(ctx);
            best_word_metrics = evaluator.get_word_metrics(ctx);
            steps_since_improve = 0;

            if thread_id == 0 && best_score <= last_best_score - 0.9 {
                let m = best_metrics;
                let elapsed = sa_start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
                println!(
                    "   [T0] 步数 {}/{} | {:.1} 万步/分钟 | 温度 {:.6} | 重码:{} 重码率:{:.4}% 当量:{:.4} | 得分: {:.4}",
                    step, steps, speed * 60.0 / 10000.0, temp, m.collision_count, m.collision_rate * 100.0,
                    m.equiv_mean, best_score
                );
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        if steps_since_improve > min_improve_steps {
            temp_multiplier = cfg.annealing.reheat_factor;
            steps_since_improve = 0;

            if thread_id == 0 {
                let elapsed = sa_start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
                println!(
                    "   [T0] 步数 {} | {:.1} 万步/分钟: Reheat ×{:.1} (基温 {:.6})",
                    step, speed * 60.0 / 10000.0, cfg.annealing.reheat_factor, base_temp
                );
            }
        }

        // 智能低温扰动
        if perturb_interval > 0 && step > 0 && step % perturb_interval == 0 && base_temp < cfg.annealing.comfort_temp * 0.01 {
            let collisions = find_collision_groups(ctx, &assignment);
            let n_perturb = (n_groups as f64 * cfg.annealing.perturb_strength) as usize;
            
            if !collisions.is_empty() {
                let mut perturbed_groups: HashSet<usize> = HashSet::new();
                for (g1, g2, _) in collisions.iter().take(10) {
                    perturbed_groups.insert(*g1);
                    perturbed_groups.insert(*g2);
                }
                let groups: Vec<usize> = perturbed_groups.into_iter().collect();
                for &gi in groups.iter().take(n_perturb) {
                    let allowed = &ctx.groups[gi].allowed_keys;
                    if allowed.len() > 1 {
                        let new_k = allowed[rng.gen_range(0..allowed.len())];
                        evaluator.try_move(ctx, &mut assignment, gi, new_k, temp * 2.0, &mut rng);
                    }
                }
            } else {
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
            }

            if thread_id == 0 {
                let m = evaluator.get_metrics(ctx);
                println!(
                    "   [T0] 步数 {}: 智能扰动 | 重码={} | 当前: {:.4}",
                    step, m.collision_count, evaluator.get_score(ctx)
                );
            }
        }

        if thread_id == 0 && step % report_interval == 0 && step > 0 {
            let pct = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            let elapsed = sa_start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
            println!(
                "   [T0] 进度: {}% | {:.1} 万步/分钟 | 基温: {:.6} | 重码={} 当量={:.4} | 当前: {:.4} 🏆最优: {:.4}",
                pct, speed * 60.0 / 10000.0, base_temp, m.collision_count, m.equiv_mean,
                evaluator.get_score(ctx), best_score
            );
        }
    }

    // 最终精炼
    if thread_id == 0 {
        println!("   [T0] SA 完成，执行最终精炼...");
    }

    let final_warmup_steps = (n_groups * 50).max(5000).min(100_000);
    let (final_assignment, final_score) =
        hill_climb_warmup(ctx, best_assignment.clone(), &mut rng, final_warmup_steps);

    if final_score < best_score {
        best_assignment = final_assignment;
        best_score = final_score;
        let eval = Evaluator::new(ctx, &best_assignment);
        best_metrics = eval.get_metrics(ctx);
        best_simple_metrics = eval.get_simple_metrics(ctx);
        best_word_metrics = eval.get_word_metrics(ctx);

        if thread_id == 0 {
            println!("   [T0] 最终爬山改进 → 得分: {:.4}", best_score);
        }
    }

    if n_groups <= 500 {
        let score_before_cd = best_score;
        let (cd_assignment, cd_score) = coordinate_descent(ctx, best_assignment.clone());
        if cd_score < best_score {
            best_assignment = cd_assignment;
            best_score = cd_score;
            let eval = Evaluator::new(ctx, &best_assignment);
            best_metrics = eval.get_metrics(ctx);
            best_simple_metrics = eval.get_simple_metrics(ctx);
            best_word_metrics = eval.get_word_metrics(ctx);

            if thread_id == 0 {
                println!("   [T0] 坐标下降精炼: {:.4} → {:.4}", score_before_cd, best_score);
            }
        }
    }

    if thread_id == 0 {
        println!("   [T0] 最终得分: {:.4} 重码: {}", best_score, best_metrics.collision_count);
    }

    SaResult {
        assignment: best_assignment,
        score: best_score,
        metrics: best_metrics,
        simple_metrics: best_simple_metrics,
        word_metrics: best_word_metrics,
        checkpoint: None,
        actual_temp_start,
        actual_comfort_temp,
    }
}