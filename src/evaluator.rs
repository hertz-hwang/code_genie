// =========================================================================
// ⚡ 评估器
// =========================================================================

use rand::prelude::*;
use std::collections::HashMap;

use crate::context::OptContext;
use crate::types::{KeyDistConfig, Metrics, SimpleMetrics, EQUIV_TABLE_SIZE};

// =========================================================================
// 简码评估器
// =========================================================================

/// 简码级别跟踪器
struct SimpleLevelTracker {
    /// 该级别的编码数
    code_num: usize,
    /// 编码到候选汉字的映射 (编码 -> [(汉字索引, 频率)])
    code_to_candidates: HashMap<usize, Vec<(usize, u64)>>,
    /// 已覆盖的频率
    covered_freq: u64,
    /// 加权等价值
    equiv_weighted: f64,
    /// 等价值频率总和
    equiv_freq_sum: u64,
    /// 键位使用统计
    key_usage: [f64; EQUIV_TABLE_SIZE],
    /// 键击次数
    key_presses: f64,
    /// 已分配的汉字列表
    assigned_chars: Vec<usize>,
}

/// 简码评估器
pub struct SimpleEvaluator {
    /// 各简码级别的跟踪器
    levels: Vec<SimpleLevelTracker>,
    /// 所有出简的汉字标记（跨级别），Vec<bool> 替代 HashSet 加速查找
    all_assigned_flags: Vec<bool>,
    /// 简码重码数：全码桶去掉出简字后仍有重码的数量
    simple_collision_count: usize,
    /// 简码重码率：全码桶去掉出简字后仍被重码的字频 / 总频
    simple_collision_rate: f64,
    /// 缓存的简码得分
    cached_simple_score: f64,
    /// 得分是否需要重新计算
    simple_score_dirty: bool,
    /// 按频率降序排列的汉字索引（缓存，频率不变所以排序不变）
    sorted_chars: Vec<usize>,
}

impl SimpleEvaluator {
    /// 创建新的简码评估器
    pub fn new(
        ctx: &OptContext,
        assignment: &[u8],
        full_code_to_chars: &[Vec<usize>],
    ) -> Self {
        let n_levels = ctx.simple_config.levels.len();

        let mut levels: Vec<SimpleLevelTracker> = ctx
            .simple_config
            .levels
            .iter()
            .map(|l| SimpleLevelTracker {
                code_num: l.code_num,
                code_to_candidates: HashMap::new(),
                covered_freq: 0,
                equiv_weighted: 0.0,
                equiv_freq_sum: 0,
                key_usage: [0.0; EQUIV_TABLE_SIZE],
                key_presses: 0.0,
                assigned_chars: Vec::new(),
            })
            .collect();

        let n_chars = ctx.char_infos.len();
        let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
        sorted_chars.sort_by(|&a, &b| {
            ctx.char_infos[b]
                .frequency
                .cmp(&ctx.char_infos[a].frequency)
        });

        let mut globally_assigned = vec![false; n_chars];

        for li in 0..n_levels {
            Self::build_level(
                ctx,
                assignment,
                &mut levels[li],
                li,
                &sorted_chars,
                &globally_assigned,
            );
            for &ci in &levels[li].assigned_chars {
                globally_assigned[ci] = true;
            }
        }

        // 计算简码重码
        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, full_code_to_chars, &globally_assigned);

        let mut se = Self {
            levels,
            all_assigned_flags: globally_assigned,
            simple_collision_count: sc_count,
            simple_collision_rate: sc_rate,
            cached_simple_score: 0.0,
            simple_score_dirty: true,
            sorted_chars,
        };
        se.cached_simple_score = se.compute_simple_score(ctx);
        se.simple_score_dirty = false;
        se
    }

    /// 构建单个简码级别
    fn build_level(
        ctx: &OptContext,
        assignment: &[u8],
        level: &mut SimpleLevelTracker,
        li: usize,
        sorted_chars: &[usize],
        excluded: &[bool],
    ) {
        level.code_to_candidates.clear();
        level.covered_freq = 0;
        level.equiv_weighted = 0.0;
        level.equiv_freq_sum = 0;
        level.key_usage = [0.0; EQUIV_TABLE_SIZE];
        level.key_presses = 0.0;
        level.assigned_chars.clear();

        for &ci in sorted_chars {
            if excluded[ci] {
                continue;
            }
            if let Some(code) = ctx.calc_simple_code(ci, li, assignment) {
                level
                    .code_to_candidates
                    .entry(code)
                    .or_default()
                    .push((ci, ctx.char_infos[ci].frequency));
            }
        }

        let all_assigned: Vec<usize> = level
            .code_to_candidates
            .values()
            .flat_map(|candidates| {
                candidates
                    .iter()
                    .take(level.code_num)
                    .filter(|(ci, _)| !excluded[*ci])
                    .map(|&(ci, _)| ci)
            })
            .collect();

        for ci in &all_assigned {
            let ci = *ci;
            let freq = ctx.char_infos[ci].frequency;
            level.covered_freq += freq;
            level.assigned_chars.push(ci);

            let eq = ctx.calc_simple_equiv(ci, li, assignment);
            level.equiv_weighted += eq * freq as f64;
            level.equiv_freq_sum += freq;

            if let Some(keys) = ctx.get_simple_keys(ci, li, assignment) {
                let freq_f = freq as f64;
                for &k in &keys {
                    level.key_usage[k as usize] += freq_f;
                }
                level.key_presses += freq_f * keys.len() as f64;
            }
        }
    }

    /// 计算简码重码数和简码重码率
    /// 遍历全码桶，去掉已出简的字，统计剩余重码
    fn compute_simple_collisions(
        ctx: &OptContext,
        full_code_to_chars: &[Vec<usize>],
        assigned: &[bool],
    ) -> (usize, f64) {
        let mut total_collision_count: usize = 0;
        let mut total_collision_freq: u64 = 0;

        for chars in full_code_to_chars.iter() {
            if chars.is_empty() {
                continue;
            }
            // 过滤掉已出简的字
            let mut n = 0usize;
            let mut max_freq = 0u64;
            let mut sum_freq = 0u64;
            for &ci in chars {
                if !assigned[ci] {
                    let f = ctx.char_infos[ci].frequency;
                    sum_freq += f;
                    if f > max_freq {
                        max_freq = f;
                    }
                    n += 1;
                }
            }

            if n >= 2 {
                total_collision_count += n - 1;
                total_collision_freq += sum_freq - max_freq;
            }
        }

        let rate = if ctx.total_frequency > 0 {
            total_collision_freq as f64 / ctx.total_frequency as f64
        } else {
            0.0
        };

        (total_collision_count, rate)
    }

    /// 完整重建简码评估
    pub fn full_rebuild(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_code_to_chars: &[Vec<usize>],
    ) {
        let n_levels = ctx.simple_config.levels.len();
        let n_chars = ctx.char_infos.len();

        // 重用 all_assigned_flags，清零
        self.all_assigned_flags.clear();
        self.all_assigned_flags.resize(n_chars, false);

        for li in 0..n_levels {
            Self::build_level(
                ctx,
                assignment,
                &mut self.levels[li],
                li,
                &self.sorted_chars,
                &self.all_assigned_flags,
            );
            for &ci in &self.levels[li].assigned_chars {
                self.all_assigned_flags[ci] = true;
            }
        }

        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, full_code_to_chars, &self.all_assigned_flags);
        self.simple_collision_count = sc_count;
        self.simple_collision_rate = sc_rate;

        self.simple_score_dirty = true;
    }

    /// 计算简码得分
    fn compute_simple_score(&self, ctx: &OptContext) -> f64 {
        let sm = self.get_simple_metrics(ctx);

        let freq_loss = (1.0 - sm.weighted_freq_coverage) * ctx.scale_config.simple_freq;
        let equiv_loss = sm.equiv_mean * ctx.scale_config.simple_equiv;
        let dist_loss = sm.dist_deviation * ctx.scale_config.simple_dist;
        let collision_count_loss =
            sm.collision_count as f64 * ctx.scale_config.simple_collision_count;
        let collision_rate_loss = sm.collision_rate * ctx.scale_config.simple_collision_rate;

        ctx.weights.simple_weight_freq * freq_loss
            + ctx.weights.simple_weight_equiv * equiv_loss
            + ctx.weights.simple_weight_dist * dist_loss
            + ctx.weights.simple_weight_collision_count * collision_count_loss
            + ctx.weights.simple_weight_collision_rate * collision_rate_loss
    }

    /// 获取简码得分
    pub fn get_simple_score(&mut self, ctx: &OptContext) -> f64 {
        if self.simple_score_dirty {
            self.cached_simple_score = self.compute_simple_score(ctx);
            self.simple_score_dirty = false;
        }
        self.cached_simple_score
    }

    /// 获取简码评估指标
    pub fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        let mut total_covered = 0u64;
        let mut total_equiv_weighted = 0.0f64;
        let mut total_equiv_freq = 0u64;
        let mut total_key_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for level in &self.levels {
            total_covered += level.covered_freq;
            total_equiv_weighted += level.equiv_weighted;
            total_equiv_freq += level.equiv_freq_sum;
            for k in 0..EQUIV_TABLE_SIZE {
                total_key_usage[k] += level.key_usage[k];
            }
            total_key_presses += level.key_presses;
        }

        let coverage = if ctx.total_frequency > 0 {
            total_covered as f64 / ctx.total_frequency as f64
        } else {
            0.0
        };

        let equiv_mean = if total_equiv_freq > 0 {
            total_equiv_weighted / total_equiv_freq as f64
        } else {
            0.0
        };

        let dist_deviation = if total_key_presses > 0.0 {
            let inv = 1.0 / total_key_presses;
            let mut dev = 0.0;
            for key in 0..EQUIV_TABLE_SIZE {
                let cfg = &ctx.key_dist_config[key];
                if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
                    continue;
                }
                let actual_pct = total_key_usage[key] * 100.0 * inv;
                let diff = actual_pct - cfg.target_rate;
                if diff < 0.0 {
                    dev += diff * diff * cfg.low_penalty;
                } else if diff > 0.0 {
                    dev += diff * diff * cfg.high_penalty;
                }
            }
            dev
        } else {
            0.0
        };

        SimpleMetrics {
            weighted_freq_coverage: coverage,
            equiv_mean,
            dist_deviation,
            collision_count: self.simple_collision_count,
            collision_rate: self.simple_collision_rate,
        }
    }
}

// =========================================================================
// 主评估器
// =========================================================================

/// 主评估器 - 评估整个编码方案
pub struct Evaluator {
    /// 当前编码列表
    current_codes: Vec<usize>,
    /// 当前等价值列表
    current_equiv_val: Vec<f64>,
    /// 编码到汉字的映射（直接索引，大小 = code_space）
    code_to_chars: Vec<Vec<usize>>,
    /// 每个汉字在其桶中的位置（用于 O(1) swap_remove）
    char_bucket_pos: Vec<usize>,
    /// 每个桶的频率总和
    bucket_freq_sum: Vec<u64>,
    /// 每个桶的最大频率（用于增量 collision_freq 计算）
    bucket_max_freq: Vec<u64>,

    /// 总重码数
    total_collisions: usize,
    /// 重码频率
    collision_frequency: u64,

    /// 加权等价值总和
    total_equiv_weighted: f64,
    /// 加权等价值平方总和
    total_equiv_sq_weighted: f64,

    /// 键位加权使用统计
    pub key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    /// 总键击次数
    pub total_key_presses: f64,

    /// 总频率
    pub total_frequency: u64,
    /// 总频率倒数
    pub inv_total_frequency: f64,
    /// 总键击次数倒数
    pub inv_total_key_presses: f64,

    /// 缓存的得分
    pub cached_score: f64,
    /// 得分是否需要重新计算
    pub score_dirty: bool,

    /// 简码评估器
    simple_eval: Option<SimpleEvaluator>,
}

impl Evaluator {
    /// 创建新的评估器
    pub fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let n = ctx.char_infos.len();
        let cs = ctx.code_space;
        let mut code_to_chars: Vec<Vec<usize>> = vec![Vec::new(); cs];
        let mut char_bucket_pos = vec![0usize; n];
        let mut bucket_freq_sum = vec![0u64; cs];
        let mut bucket_max_freq = vec![0u64; cs];
        let mut current_codes = Vec::with_capacity(n);
        let mut current_equiv_val = Vec::with_capacity(n);

        let mut total_equiv_weighted = 0.0f64;
        let mut total_equiv_sq_weighted = 0.0f64;
        let mut key_weighted_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for ci in 0..n {
            let info = &ctx.char_infos[ci];
            let freq_f = info.frequency as f64;

            let code = ctx.calc_code_only(ci, assignment);
            let equiv = ctx.calc_equiv_from_parts(ci, assignment);

            current_codes.push(code);
            current_equiv_val.push(equiv);

            let pos = code_to_chars[code].len();
            code_to_chars[code].push(ci);
            char_bucket_pos[ci] = pos;
            bucket_freq_sum[code] += info.frequency;
            if info.frequency > bucket_max_freq[code] {
                bucket_max_freq[code] = info.frequency;
            }

            total_equiv_weighted += equiv * freq_f;
            total_equiv_sq_weighted += equiv * equiv * freq_f;

            for &p in &info.parts {
                let k = ctx.resolve_key(p, assignment) as usize;
                key_weighted_usage[k] += freq_f;
            }
            total_key_presses += freq_f * info.parts.len() as f64;
        }

        let mut total_collisions = 0usize;
        let mut collision_frequency = 0u64;
        for code in 0..cs {
            let cnt = code_to_chars[code].len();
            if cnt >= 2 {
                total_collisions += cnt - 1;
                collision_frequency += bucket_freq_sum[code] - bucket_max_freq[code];
            }
        }

        let inv_tf = if ctx.total_frequency > 0 {
            1.0 / ctx.total_frequency as f64
        } else {
            0.0
        };
        let inv_tkp = if total_key_presses > 0.0 {
            1.0 / total_key_presses
        } else {
            0.0
        };

        let simple_eval = if ctx.enable_simple_code && !ctx.simple_config.levels.is_empty() {
            Some(SimpleEvaluator::new(ctx, assignment, &code_to_chars))
        } else {
            None
        };

        let mut e = Self {
            current_codes,
            current_equiv_val,
            code_to_chars,
            char_bucket_pos,
            bucket_freq_sum,
            bucket_max_freq,
            total_collisions,
            collision_frequency,
            total_equiv_weighted,
            total_equiv_sq_weighted,
            key_weighted_usage,
            total_key_presses,
            total_frequency: ctx.total_frequency,
            inv_total_frequency: inv_tf,
            inv_total_key_presses: inv_tkp,
            cached_score: 0.0,
            score_dirty: true,
            simple_eval,
        };
        e.cached_score = e.compute_score(ctx);
        e.score_dirty = false;
        e
    }

    /// 重新扫描桶的最大频率
    #[inline]
    fn rescan_bucket_max(&self, ctx: &OptContext, code: usize) -> u64 {
        let mut max_f = 0u64;
        for &ci in &self.code_to_chars[code] {
            let f = ctx.char_infos[ci].frequency;
            if f > max_f {
                max_f = f;
            }
        }
        max_f
    }

    /// 计算桶的重码频率（仅用于 SimpleEvaluator 等非热路径）
    #[inline]
    fn bucket_cf_static(ctx: &OptContext, chars: &[usize]) -> u64 {
        debug_assert!(chars.len() >= 2);
        let mut total = 0u64;
        let mut max_f = 0u64;
        for &ci in chars {
            let f = ctx.char_infos[ci].frequency;
            total += f;
            if f > max_f {
                max_f = f;
            }
        }
        total - max_f
    }

    /// 更新单个汉字的编码（增量更新碰撞计数）
    #[inline]
    pub fn update_char(&mut self, ctx: &OptContext, assignment: &[u8], ci: usize) {
        let old_code = self.current_codes[ci];
        let new_code = ctx.calc_code_only(ci, assignment);
        if old_code == new_code {
            return;
        }

        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        // 更新等价值
        let old_eq = self.current_equiv_val[ci];
        let new_eq = ctx.calc_equiv_from_parts(ci, assignment);
        self.total_equiv_weighted += (new_eq - old_eq) * freq_f;
        self.total_equiv_sq_weighted += (new_eq * new_eq - old_eq * old_eq) * freq_f;
        self.current_equiv_val[ci] = new_eq;

        // === 从旧桶移除 ===
        let old_len = self.code_to_chars[old_code].len();
        // 旧桶的碰撞贡献（移除前）
        let old_bucket_cc = old_len.saturating_sub(1);
        let old_bucket_cf = if old_len >= 2 {
            self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
        } else {
            0
        };

        // swap_remove: 用最后一个元素替换被移除的元素
        let pos = self.char_bucket_pos[ci];
        let last_idx = old_len - 1;
        if pos != last_idx {
            let moved_ci = self.code_to_chars[old_code][last_idx];
            self.code_to_chars[old_code][pos] = moved_ci;
            self.char_bucket_pos[moved_ci] = pos;
        }
        self.code_to_chars[old_code].pop();

        // 更新旧桶的频率统计
        self.bucket_freq_sum[old_code] -= freq;
        // 如果移除的是 max，需要重扫
        if freq >= self.bucket_max_freq[old_code] {
            self.bucket_max_freq[old_code] = if self.code_to_chars[old_code].is_empty() {
                0
            } else {
                self.rescan_bucket_max(ctx, old_code)
            };
        }

        let new_old_len = self.code_to_chars[old_code].len();
        let new_old_cc = new_old_len.saturating_sub(1);
        let new_old_cf = if new_old_len >= 2 {
            self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
        } else {
            0
        };

        // === 插入新桶 ===
        let new_len = self.code_to_chars[new_code].len();
        let new_bucket_cc = new_len.saturating_sub(1);
        let new_bucket_cf = if new_len >= 2 {
            self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
        } else {
            0
        };

        let new_pos = new_len;
        self.code_to_chars[new_code].push(ci);
        self.char_bucket_pos[ci] = new_pos;
        self.bucket_freq_sum[new_code] += freq;
        if freq > self.bucket_max_freq[new_code] {
            self.bucket_max_freq[new_code] = freq;
        }

        let after_new_len = new_len + 1;
        let after_new_cc = after_new_len.saturating_sub(1);
        let after_new_cf = if after_new_len >= 2 {
            self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
        } else {
            0
        };

        // 更新全局碰撞计数
        self.total_collisions = (self.total_collisions + new_old_cc + after_new_cc)
            - (old_bucket_cc + new_bucket_cc);
        self.collision_frequency = (self.collision_frequency + new_old_cf + after_new_cf)
            - (old_bucket_cf + new_bucket_cf);
        self.current_codes[ci] = new_code;
    }

    /// 计算全码得分
    #[inline(always)]
    pub fn compute_full_score(&self, ctx: &OptContext) -> f64 {
        let mut score = ctx.weights.weight_collision_count
            * self.total_collisions as f64
            * ctx.scale_config.collision_count;

        if ctx.weights.weight_collision_rate > 0.0 {
            let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
            score += ctx.weights.weight_collision_rate
                * collision_rate
                * ctx.scale_config.collision_rate;
        }

        if ctx.weights.weight_equivalence > 0.0 {
            let weighted_equiv = self.total_equiv_weighted * self.inv_total_frequency;
            score += ctx.weights.weight_equivalence
                * weighted_equiv
                * ctx.scale_config.equivalence;
        }

        if ctx.weights.weight_equiv_cv > 0.0 {
            let equiv_cv = self.calc_equiv_cv();
            score += ctx.weights.weight_equiv_cv
                * equiv_cv
                * ctx.scale_config.equiv_cv;
        }

        if ctx.weights.weight_distribution > 0.0 {
            let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);
            score += ctx.weights.weight_distribution
                * dist_deviation
                * ctx.scale_config.distribution;
        }

        score
    }

    /// 计算综合得分
    #[inline(always)]
    pub fn compute_score(&self, ctx: &OptContext) -> f64 {
        let full_score = self.compute_full_score(ctx);

        if ctx.enable_simple_code {
            if let Some(ref se) = self.simple_eval {
                let simple_score = se.cached_simple_score;
                ctx.weights.weight_full_code * full_score + ctx.weights.weight_simple_code * simple_score
            } else {
                full_score
            }
        } else {
            full_score
        }
    }

    /// 获取得分
    #[inline(always)]
    pub fn get_score(&mut self, ctx: &OptContext) -> f64 {
        if self.score_dirty {
            self.cached_score = self.compute_score(ctx);
            self.score_dirty = false;
        }
        self.cached_score
    }

    /// 计算等价值变异系数
    #[inline(always)]
    pub fn calc_equiv_cv(&self) -> f64 {
        let mean = self.total_equiv_weighted * self.inv_total_frequency;
        if mean <= 0.0 {
            return 0.0;
        }
        let mean_sq = self.total_equiv_sq_weighted * self.inv_total_frequency;
        let variance = mean_sq - mean * mean;
        if variance <= 0.0 {
            return 0.0;
        }
        variance.sqrt() / mean
    }

    /// 计算分布偏差
    #[inline(always)]
    pub fn calc_distribution_deviation(&self, kdc: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
        let mut dev = 0.0;
        for key in 0..EQUIV_TABLE_SIZE {
            let cfg = &kdc[key];
            if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
                continue;
            }
            let actual_pct = self.key_weighted_usage[key] * 100.0 * self.inv_total_key_presses;
            let diff = actual_pct - cfg.target_rate;
            if diff < 0.0 {
                dev += diff * diff * cfg.low_penalty;
            } else if diff > 0.0 {
                dev += diff * diff * cfg.high_penalty;
            }
        }
        dev
    }

    /// 获取评估指标
    pub fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        Metrics {
            collision_count: self.total_collisions,
            collision_rate: self.collision_frequency as f64 * self.inv_total_frequency,
            equiv_mean: self.total_equiv_weighted * self.inv_total_frequency,
            equiv_cv: self.calc_equiv_cv(),
            dist_deviation: self.calc_distribution_deviation(&ctx.key_dist_config),
        }
    }

    /// 获取简码评估指标
    pub fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        if let Some(ref se) = self.simple_eval {
            se.get_simple_metrics(ctx)
        } else {
            SimpleMetrics::default()
        }
    }

    /// 检查是否有简码影响
    pub fn has_simple_impact(&self, ctx: &OptContext, group: usize) -> bool {
        if !ctx.enable_simple_code || self.simple_eval.is_none() {
            return false;
        }
        !ctx.group_to_simple_affected[group].is_empty()
    }

    /// 重建简码评估
    pub fn rebuild_simple(&mut self, ctx: &OptContext, assignment: &[u8]) {
        if let Some(ref mut se) = self.simple_eval {
            se.full_rebuild(ctx, assignment, &self.code_to_chars);
            se.cached_simple_score = se.compute_simple_score(ctx);
            se.simple_score_dirty = false;
        }
    }

    /// 尝试移动（改变单个组的键位）
    #[inline(always)]
    pub fn try_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let old_key = assignment[r];
        if old_key == new_key {
            return false;
        }

        let old_score = self.get_score(ctx);
        let needs_simple = self.has_simple_impact(ctx, r);

        // O(1) key_weighted_usage 更新
        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;

        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }

        if needs_simple {
            self.rebuild_simple(ctx, assignment);
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;

            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }

            if needs_simple {
                self.rebuild_simple(ctx, assignment);
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            false
        }
    }

    /// 尝试交换（交换两个组的键位）
    #[inline(always)]
    pub fn try_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 {
            return false;
        }

        let old_score = self.get_score(ctx);
        let needs_simple = self.has_simple_impact(ctx, r1) || self.has_simple_impact(ctx, r2);

        // O(1) key_weighted_usage 更新
        let gfs1 = ctx.group_freq_sum[r1];
        let gfs2 = ctx.group_freq_sum[r2];
        self.key_weighted_usage[k1 as usize] -= gfs1;
        self.key_weighted_usage[k2 as usize] += gfs1;
        self.key_weighted_usage[k2 as usize] -= gfs2;
        self.key_weighted_usage[k1 as usize] += gfs2;

        assignment[r1] = k2;
        assignment[r2] = k1;
        for &ci in &ctx.group_to_chars[r1] {
            self.update_char(ctx, assignment, ci);
        }
        for &ci in &ctx.group_to_chars[r2] {
            self.update_char(ctx, assignment, ci);
        }

        if needs_simple {
            self.rebuild_simple(ctx, assignment);
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;

            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }

            if needs_simple {
                self.rebuild_simple(ctx, assignment);
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            false
        }
    }
}
