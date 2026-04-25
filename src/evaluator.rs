// =========================================================================
// ⚡ 评估器
// =========================================================================

use rand::prelude::*;
use std::collections::HashMap;

use crate::context::OptContext;
use crate::types::{KeyDistConfig, Metrics, SimpleMetrics, WordMetrics, EQUIV_TABLE_SIZE};

// =========================================================================
// 简码评估器
// =========================================================================

/// 简码级别跟踪器
struct SimpleLevelTracker {
    code_num: usize,
    allowed_orig_length: usize,
    code_to_candidates: HashMap<usize, Vec<(usize, u64)>>,
    covered_freq: u64,
    equiv_weighted: f64,
    equiv_freq_sum: u64,
    key_usage: [f64; EQUIV_TABLE_SIZE],
    key_presses: f64,
    assigned_chars: Vec<usize>,
    /// Σ(freq_i * (simple_len_i - full_len_i)) for assigned chars at this level
    wkl_delta: f64,
}

/// 简码评估器
pub struct SimpleEvaluator {
    levels: Vec<SimpleLevelTracker>,
    all_assigned_flags: Vec<bool>,
    simple_collision_count: usize,
    simple_collision_rate: f64,
    cached_simple_score: f64,
    simple_score_dirty: bool,
    sorted_chars: Vec<usize>,
}

impl SimpleEvaluator {
    /// 创建新的简码评估器
    pub fn new(
        ctx: &OptContext,
        assignment: &[u8],
        populated_codes: &[usize],
        full_code_to_chars: &[Vec<usize>],
    ) -> Self {
        let n_levels = ctx.simple_config.levels.len();

        let mut levels: Vec<SimpleLevelTracker> = ctx
            .simple_config
            .levels
            .iter()
            .map(|l| SimpleLevelTracker {
                code_num: l.code_num,
                allowed_orig_length: l.allowed_orig_length,
                code_to_candidates: HashMap::new(),
                covered_freq: 0,
                equiv_weighted: 0.0,
                equiv_freq_sum: 0,
                key_usage: [0.0; EQUIV_TABLE_SIZE],
                key_presses: 0.0,
                assigned_chars: Vec::new(),
                wkl_delta: 0.0,
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
                full_code_to_chars,
            );
            for &ci in &levels[li].assigned_chars {
                globally_assigned[ci] = true;
            }
        }

        // 计算简码重码
        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, populated_codes, full_code_to_chars, &globally_assigned);

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

    /// 检查简码码位是否被全码占用
    /// 如果该码位上有任何未出简的全码字，则认为被占用
    #[inline]
    fn is_code_occupied_by_full(
        full_code_to_chars: &[Vec<usize>],
        code: usize,
        excluded: &[bool],
    ) -> bool {
        if code >= full_code_to_chars.len() {
            return false;
        }
        let chars = &full_code_to_chars[code];
        for &ci in chars {
            if !excluded[ci] {
                return true; // 该码位上有未出简的全码字
            }
        }
        false
    }

    /// 构建单个简码级别
    fn build_level(
        ctx: &OptContext,
        assignment: &[u8],
        level: &mut SimpleLevelTracker,
        li: usize,
        sorted_chars: &[usize],
        excluded: &[bool],
        full_code_to_chars: &[Vec<usize>],
    ) {
        level.code_to_candidates.clear();
        level.covered_freq = 0;
        level.equiv_weighted = 0.0;
        level.equiv_freq_sum = 0;
        level.key_usage = [0.0; EQUIV_TABLE_SIZE];
        level.key_presses = 0.0;
        level.assigned_chars.clear();
        level.wkl_delta = 0.0;

        for &ci in sorted_chars {
            if excluded[ci] {
                continue;
            }
            if level.allowed_orig_length != 0
                && ctx.char_infos[ci].parts_len as usize != level.allowed_orig_length
            {
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
            .iter()
            .flat_map(|(&code, candidates)| {
                // 检查该简码码位是否被全码占用
                if Self::is_code_occupied_by_full(full_code_to_chars, code, excluded) {
                    // 码位被全码字占据，不分配简码
                    return Vec::new();
                }
                candidates
                    .iter()
                    .take(level.code_num)
                    .filter(|(ci, _)| !excluded[*ci])
                    .map(|&(ci, _)| ci)
                    .collect::<Vec<_>>()
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
                let simple_len = keys.len() as f64;
                let full_len = ctx.char_infos[ci].parts_len as f64;
                level.wkl_delta += freq_f * (simple_len - full_len);
                for &k in &keys {
                    level.key_usage[k as usize] += freq_f;
                }
                level.key_presses += freq_f * simple_len;
            }
        }
    }

    /// 计算简码重码数和简码重码率
    /// 遍历非空全码桶，去掉已出简的字，统计剩余重码
    fn compute_simple_collisions(
        ctx: &OptContext,
        populated_codes: &[usize],
        code_to_chars: &[Vec<usize>],
        assigned: &[bool],
    ) -> (usize, f64) {
        let mut total_collision_count: usize = 0;
        let mut total_collision_freq: u64 = 0;

        for &code in populated_codes {
            let chars = &code_to_chars[code];
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
        populated_codes: &[usize],
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
                full_code_to_chars,
            );
            for &ci in &self.levels[li].assigned_chars {
                self.all_assigned_flags[ci] = true;
            }
        }

        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, populated_codes, full_code_to_chars, &self.all_assigned_flags);
        self.simple_collision_count = sc_count;
        self.simple_collision_rate = sc_rate;

        self.simple_score_dirty = true;
    }

    /// 计算简码得分
    fn compute_simple_score(&self, ctx: &OptContext) -> f64 {
        let sm = self.get_simple_metrics(ctx);

        let wkl_loss = sm.weighted_key_length * ctx.scale_config.simple_weighted_key_length;
        let equiv_loss = sm.equiv_mean * ctx.scale_config.simple_equivalence;
        let dist_loss = sm.dist_deviation * ctx.scale_config.simple_distribution;
        let collision_count_loss = sm.collision_count as f64 * ctx.scale_config.simple_collision_count;
        let collision_rate_loss = sm.collision_rate * ctx.scale_config.simple_collision_rate;

        ctx.weights.simple_weighted_key_length * wkl_loss
            + ctx.weights.simple_equivalence * equiv_loss
            + ctx.weights.simple_distribution * dist_loss
            + ctx.weights.simple_collision_count * collision_count_loss
            + ctx.weights.simple_collision_rate * collision_rate_loss
    }

    /// 获取简码得分
    #[allow(dead_code)]
    pub fn get_simple_score(&mut self, ctx: &OptContext) -> f64 {
        if self.simple_score_dirty {
            self.cached_simple_score = self.compute_simple_score(ctx);
            self.simple_score_dirty = false;
        }
        self.cached_simple_score
    }

    /// 获取简码评估指标
    pub fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        let mut total_equiv_weighted = 0.0f64;
        let mut total_equiv_freq = 0u64;
        let mut total_key_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;
        let mut total_wkl_delta = 0.0f64;

        for level in &self.levels {
            total_equiv_weighted += level.equiv_weighted;
            total_equiv_freq += level.equiv_freq_sum;
            for k in 0..EQUIV_TABLE_SIZE {
                total_key_usage[k] += level.key_usage[k];
            }
            total_key_presses += level.key_presses;
            total_wkl_delta += level.wkl_delta;
        }

        // 加权码长 = (base_wkl + delta) / total_frequency
        let weighted_key_length = if ctx.total_frequency > 0 {
            (ctx.base_wkl + total_wkl_delta) / ctx.total_frequency as f64
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
            weighted_key_length,
            equiv_mean,
            dist_deviation,
            collision_count: self.simple_collision_count,
            collision_rate: self.simple_collision_rate,
        }
    }
}

// =========================================================================
// 词码评估器
// =========================================================================

/// 词码评估器 - 增量维护多字词全码的碰撞/当量/分布指标
pub struct WordEvaluator {
    current_codes: Vec<usize>,
    current_equiv_val: Vec<f64>,
    code_to_words: Vec<Vec<usize>>,
    word_bucket_pos: Vec<usize>,
    bucket_freq_sum: Vec<u64>,
    bucket_max_freq: Vec<u64>,
    populated_codes: Vec<usize>,
    code_populated_pos: Vec<usize>,
    total_collisions: usize,
    collision_frequency: u64,
    top2000_collisions: usize,
    top10000_collisions: usize,
    total_equiv_weighted: f64,
    pub key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    total_key_presses: f64,
    total_frequency: u64,
    inv_total_frequency: f64,
}

impl WordEvaluator {
    pub fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let nw = ctx.word_infos.len();
        let cs = ctx.code_space;
        let mut code_to_words: Vec<Vec<usize>> = vec![Vec::new(); cs];
        let mut word_bucket_pos = vec![0usize; nw];
        let mut bucket_freq_sum = vec![0u64; cs];
        let mut bucket_max_freq = vec![0u64; cs];
        let mut current_codes = Vec::with_capacity(nw);
        let mut current_equiv_val = Vec::with_capacity(nw);
        let mut total_equiv_weighted = 0.0f64;
        let mut key_weighted_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for wi in 0..nw {
            let winfo = &ctx.word_infos[wi];
            let freq_f = winfo.frequency as f64;
            let code = ctx.calc_word_code(wi, assignment);
            let equiv = ctx.calc_word_equiv(wi, assignment);
            current_codes.push(code);
            current_equiv_val.push(equiv);
            let pos = code_to_words[code].len();
            code_to_words[code].push(wi);
            word_bucket_pos[wi] = pos;
            bucket_freq_sum[code] += winfo.frequency;
            if winfo.frequency > bucket_max_freq[code] {
                bucket_max_freq[code] = winfo.frequency;
            }
            total_equiv_weighted += equiv * freq_f;
            for &p in winfo.parts_slice() {
                let k = ctx.resolve_key(p, assignment) as usize;
                key_weighted_usage[k] += freq_f;
            }
            total_key_presses += freq_f * winfo.parts_len as f64;
        }

        let mut populated_codes = Vec::with_capacity(nw);
        let mut code_populated_pos = vec![usize::MAX; cs];
        let mut total_collisions = 0usize;
        let mut collision_frequency = 0u64;
        let mut top2000_collisions = 0usize;
        let mut top10000_collisions = 0usize;

        for code in 0..cs {
            let words = &code_to_words[code];
            if !words.is_empty() {
                code_populated_pos[code] = populated_codes.len();
                populated_codes.push(code);
            }
            if words.len() >= 2 {
                total_collisions += words.len() - 1;
                collision_frequency += bucket_freq_sum[code] - bucket_max_freq[code];
                for &wi in words {
                    if ctx.word_infos[wi].is_top2000 { top2000_collisions += 1; }
                    if ctx.word_infos[wi].is_top10000 { top10000_collisions += 1; }
                }
            }
        }

        let inv_tf = if ctx.word_total_frequency > 0 {
            1.0 / ctx.word_total_frequency as f64
        } else {
            0.0
        };

        Self {
            current_codes,
            current_equiv_val,
            code_to_words,
            word_bucket_pos,
            bucket_freq_sum,
            bucket_max_freq,
            populated_codes,
            code_populated_pos,
            total_collisions,
            collision_frequency,
            top2000_collisions,
            top10000_collisions,
            total_equiv_weighted,
            key_weighted_usage,
            total_key_presses,
            total_frequency: ctx.word_total_frequency,
            inv_total_frequency: inv_tf,
        }
    }

    /// 增量更新词 wi 从 old_code 移到 new_code
    #[inline]
    pub fn update_word(&mut self, ctx: &OptContext, wi: usize, old_code: usize, new_code: usize) {
        let winfo = &ctx.word_infos[wi];
        let freq = winfo.frequency;
        let freq_f = freq as f64;
        let _ = freq_f; // used in key_usage updates elsewhere

        // ── 从旧桶移除 ──
        let old_size = self.code_to_words[old_code].len();
        let old_bucket_cc = old_size.saturating_sub(1);

        // top-N 碰撞：旧桶 size >= 2 时 wi 在碰撞中
        if old_size >= 2 {
            if winfo.is_top2000 { self.top2000_collisions -= 1; }
            if winfo.is_top10000 { self.top10000_collisions -= 1; }
        }
        // 旧桶 size == 2 时，另一个词也失去碰撞状态
        if old_size == 2 {
            let pos = self.word_bucket_pos[wi];
            let other_wi = self.code_to_words[old_code][1 - pos];
            let other = &ctx.word_infos[other_wi];
            if other.is_top2000 { self.top2000_collisions -= 1; }
            if other.is_top10000 { self.top10000_collisions -= 1; }
        }

        // swap_remove
        let pos = self.word_bucket_pos[wi];
        let last_idx = old_size - 1;
        if pos != last_idx {
            let moved_wi = self.code_to_words[old_code][last_idx];
            self.code_to_words[old_code][pos] = moved_wi;
            self.word_bucket_pos[moved_wi] = pos;
        }
        self.code_to_words[old_code].pop();

        if self.code_to_words[old_code].is_empty() {
            let pop_pos = self.code_populated_pos[old_code];
            let last_pop = self.populated_codes.len() - 1;
            if pop_pos != last_pop {
                let moved_code = self.populated_codes[last_pop];
                self.populated_codes[pop_pos] = moved_code;
                self.code_populated_pos[moved_code] = pop_pos;
            }
            self.populated_codes.pop();
            self.code_populated_pos[old_code] = usize::MAX;
        }

        // 碰撞频率
        let old_bucket_cf = if old_size >= 2 {
            self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
        } else { 0 };
        self.bucket_freq_sum[old_code] -= freq;
        if freq >= self.bucket_max_freq[old_code] {
            self.bucket_max_freq[old_code] = if self.code_to_words[old_code].is_empty() {
                0
            } else {
                let mut max_f = 0u64;
                for &wi2 in &self.code_to_words[old_code] {
                    let f = ctx.word_infos[wi2].frequency;
                    if f > max_f { max_f = f; }
                }
                max_f
            };
        }
        let new_old_size = self.code_to_words[old_code].len();
        let new_old_cf = if new_old_size >= 2 {
            self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
        } else { 0 };
        let new_old_cc = new_old_size.saturating_sub(1);

        // ── 插入新桶 ──
        let new_size = self.code_to_words[new_code].len();
        let new_bucket_cc = new_size.saturating_sub(1);

        // top-N 碰撞：新桶 size >= 1 时 wi 将在碰撞中
        if new_size >= 1 {
            if winfo.is_top2000 { self.top2000_collisions += 1; }
            if winfo.is_top10000 { self.top10000_collisions += 1; }
        }
        // 新桶 size == 1 时，已有词获得碰撞状态
        if new_size == 1 {
            let existing_wi = self.code_to_words[new_code][0];
            let existing = &ctx.word_infos[existing_wi];
            if existing.is_top2000 { self.top2000_collisions += 1; }
            if existing.is_top10000 { self.top10000_collisions += 1; }
        }

        if new_size == 0 {
            self.code_populated_pos[new_code] = self.populated_codes.len();
            self.populated_codes.push(new_code);
        }
        let new_pos = new_size;
        self.code_to_words[new_code].push(wi);
        self.word_bucket_pos[wi] = new_pos;

        let new_bucket_cf = if new_size >= 2 {
            self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
        } else { 0 };
        self.bucket_freq_sum[new_code] += freq;
        if freq > self.bucket_max_freq[new_code] {
            self.bucket_max_freq[new_code] = freq;
        }
        let after_new_size = new_size + 1;
        let after_new_cf = if after_new_size >= 2 {
            self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
        } else { 0 };
        let after_new_cc = after_new_size.saturating_sub(1);

        self.collision_frequency = (self.collision_frequency + new_old_cf + after_new_cf)
            .wrapping_sub(old_bucket_cf + new_bucket_cf);
        self.total_collisions = (self.total_collisions + new_old_cc + after_new_cc)
            .wrapping_sub(old_bucket_cc + new_bucket_cc);

        self.current_codes[wi] = new_code;
    }

    /// 更新词 wi 的当量（在 assignment 改变后调用）
    #[inline]
    pub fn update_word_equiv(&mut self, ctx: &OptContext, wi: usize, assignment: &[u8]) {
        let freq_f = ctx.word_infos[wi].frequency as f64;
        let old_eq = self.current_equiv_val[wi];
        let new_eq = ctx.calc_word_equiv(wi, assignment);
        self.total_equiv_weighted += (new_eq - old_eq) * freq_f;
        self.current_equiv_val[wi] = new_eq;
    }

    /// 更新词 wi 的键位使用统计（在 assignment 改变后调用）
    #[allow(dead_code)]
    #[inline]
    pub fn update_word_key_usage(&mut self, ctx: &OptContext, wi: usize, old_key: u8, new_key: u8) {
        let winfo = &ctx.word_infos[wi];
        let freq_f = winfo.frequency as f64;
        // 统计该词中 old_key 出现次数
        let mut count = 0usize;
        for &p in winfo.parts_slice() {
            if p < 1000 && p as u8 == old_key {
                count += 1;
            }
        }
        if count > 0 {
            let delta = freq_f * count as f64;
            self.key_weighted_usage[old_key as usize] -= delta;
            self.key_weighted_usage[new_key as usize] += delta;
        }
    }

    /// 计算词码得分
    pub fn compute_word_score(&self, ctx: &OptContext) -> f64 {
        let wm = self.get_word_metrics(ctx);
        ctx.weights.word_top2000_collision * wm.top2000_collision_count as f64 * ctx.scale_config.word_top2000_collision
            + ctx.weights.word_top10000_collision * wm.top10000_collision_count as f64 * ctx.scale_config.word_top10000_collision
            + ctx.weights.word_collision_count * wm.collision_count as f64 * ctx.scale_config.word_collision_count
            + ctx.weights.word_collision_rate * wm.collision_rate * ctx.scale_config.word_collision_rate
            + ctx.weights.word_equivalence * wm.equiv_mean * ctx.scale_config.word_equivalence
            + ctx.weights.word_distribution * wm.dist_deviation * ctx.scale_config.word_distribution
    }

    /// 获取词码评估指标
    pub fn get_word_metrics(&self, ctx: &OptContext) -> WordMetrics {
        let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
        let equiv_mean = self.total_equiv_weighted * self.inv_total_frequency;
        let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);
        WordMetrics {
            top2000_collision_count: self.top2000_collisions,
            top10000_collision_count: self.top10000_collisions,
            collision_count: self.total_collisions,
            collision_rate,
            equiv_mean,
            dist_deviation,
        }
    }

    fn calc_distribution_deviation(&self, key_dist_config: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
        if self.total_key_presses == 0.0 { return 0.0; }
        let inv = 1.0 / self.total_key_presses;
        let mut dev = 0.0;
        for key in 0..EQUIV_TABLE_SIZE {
            let cfg = &key_dist_config[key];
            if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 { continue; }
            let actual_pct = self.key_weighted_usage[key] * 100.0 * inv;
            let diff = actual_pct - cfg.target_rate;
            if diff < 0.0 { dev += diff * diff * cfg.low_penalty; }
            else if diff > 0.0 { dev += diff * diff * cfg.high_penalty; }
        }
        dev
    }
}

// =========================================================================
// 主评估器
// =========================================================================

/// 主评估器 - 评估整个编码方案
pub struct Evaluator {
    current_codes: Vec<usize>,
    current_equiv_val: Vec<f64>,
    code_to_chars: Vec<Vec<usize>>,
    char_bucket_pos: Vec<usize>,
    bucket_freq_sum: Vec<u64>,
    bucket_max_freq: Vec<u64>,
    populated_codes: Vec<usize>,
    code_populated_pos: Vec<usize>,
    total_collisions: usize,
    collision_frequency: u64,
    /// 字频前 N 重码数（N 由 ctx.weights.full_top_n 决定）
    top_n_collisions: usize,
    total_equiv_weighted: f64,
    pub key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    #[allow(dead_code)]
    pub total_key_presses: f64,
    #[allow(dead_code)]
    pub total_frequency: u64,
    pub inv_total_frequency: f64,
    pub inv_total_key_presses: f64,
    pub cached_score: f64,
    pub score_dirty: bool,
    simple_eval: Option<SimpleEvaluator>,
    word_eval: Option<WordEvaluator>,
    bucket_count: Vec<u32>,
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
            for &p in info.parts_slice() {
                let k = ctx.resolve_key(p, assignment) as usize;
                key_weighted_usage[k] += freq_f;
            }
            total_key_presses += freq_f * info.parts_len as f64;
        }

        let mut populated_codes = Vec::with_capacity(n);
        let mut code_populated_pos = vec![usize::MAX; cs];
        let mut total_collisions = 0usize;
        let mut collision_frequency = 0u64;
        let mut top_n_collisions = 0usize;
        for code in 0..cs {
            if !code_to_chars[code].is_empty() {
                code_populated_pos[code] = populated_codes.len();
                populated_codes.push(code);
            }
            let cnt = code_to_chars[code].len();
            if cnt >= 2 {
                total_collisions += cnt - 1;
                collision_frequency += bucket_freq_sum[code] - bucket_max_freq[code];
                for &ci in &code_to_chars[code] {
                    if ctx.top_n_char_flags[ci] { top_n_collisions += 1; }
                }
            }
        }

        let inv_tf = if ctx.total_frequency > 0 { 1.0 / ctx.total_frequency as f64 } else { 0.0 };
        let inv_tkp = if total_key_presses > 0.0 { 1.0 / total_key_presses } else { 0.0 };

        let simple_eval = if ctx.enable_simple_code && !ctx.simple_config.levels.is_empty() {
            Some(SimpleEvaluator::new(ctx, assignment, &populated_codes, &code_to_chars))
        } else {
            None
        };

        let word_eval = if ctx.enable_word_code && !ctx.word_infos.is_empty() {
            Some(WordEvaluator::new(ctx, assignment))
        } else {
            None
        };

        let mut bucket_count = vec![0u32; cs];
        for code in 0..cs {
            bucket_count[code] = code_to_chars[code].len() as u32;
        }

        let mut e = Self {
            current_codes,
            current_equiv_val,
            code_to_chars,
            char_bucket_pos,
            bucket_freq_sum,
            bucket_max_freq,
            populated_codes,
            code_populated_pos,
            total_collisions,
            collision_frequency,
            top_n_collisions,
            total_equiv_weighted,
            key_weighted_usage,
            total_key_presses,
            total_frequency: ctx.total_frequency,
            inv_total_frequency: inv_tf,
            inv_total_key_presses: inv_tkp,
            cached_score: 0.0,
            score_dirty: true,
            simple_eval,
            word_eval,
            bucket_count,
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
    #[allow(dead_code)]
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
        let info = &ctx.char_infos[ci];
        let freq = info.frequency;

        // 直接计算新编码（不再分配临时 Vec）
        let new_code = ctx.calc_code_only(ci, assignment);
        let old_code = self.current_codes[ci];
        if old_code == new_code {
            return;
        }

        // 更新等价值（仅在需要时）
        if ctx.need_equiv {
            let freq_f = freq as f64;
            let old_eq = self.current_equiv_val[ci];
            let new_eq = ctx.calc_equiv_from_parts(ci, assignment);
            self.total_equiv_weighted += (new_eq - old_eq) * freq_f;
            self.current_equiv_val[ci] = new_eq;
        }

        if ctx.need_bucket_members {
            // === 完整桶管理路径（需要成员列表）===
            let old_len = self.code_to_chars[old_code].len();
            let old_bucket_cc = old_len.saturating_sub(1);

            // top-N 碰撞：旧桶 size >= 2 时 ci 在碰撞中
            if old_len >= 2 && ctx.top_n_char_flags[ci] {
                self.top_n_collisions -= 1;
            }
            // 旧桶 size == 2 时，另一个字也失去碰撞状态
            if old_len == 2 {
                let pos = self.char_bucket_pos[ci];
                let other_ci = self.code_to_chars[old_code][1 - pos];
                if ctx.top_n_char_flags[other_ci] { self.top_n_collisions -= 1; }
            }

            // swap_remove
            let pos = self.char_bucket_pos[ci];
            let last_idx = old_len - 1;
            if pos != last_idx {
                let moved_ci = self.code_to_chars[old_code][last_idx];
                self.code_to_chars[old_code][pos] = moved_ci;
                self.char_bucket_pos[moved_ci] = pos;
            }
            self.code_to_chars[old_code].pop();

            // 旧桶变空时，从 populated_codes 移除
            if self.code_to_chars[old_code].is_empty() {
                let pop_pos = self.code_populated_pos[old_code];
                let last_pop = self.populated_codes.len() - 1;
                if pop_pos != last_pop {
                    let moved_code = self.populated_codes[last_pop];
                    self.populated_codes[pop_pos] = moved_code;
                    self.code_populated_pos[moved_code] = pop_pos;
                }
                self.populated_codes.pop();
                self.code_populated_pos[old_code] = usize::MAX;
            }

            // 碰撞频率维护（仅在需要 collision_rate 时）
            let old_bucket_cf;
            let new_old_cf;
            if ctx.need_collision_rate {
                old_bucket_cf = if old_len >= 2 {
                    self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
                } else {
                    0
                };
                self.bucket_freq_sum[old_code] -= freq;
                if freq >= self.bucket_max_freq[old_code] {
                    self.bucket_max_freq[old_code] = if self.code_to_chars[old_code].is_empty() {
                        0
                    } else {
                        self.rescan_bucket_max(ctx, old_code)
                    };
                }
                let new_old_len = self.code_to_chars[old_code].len();
                new_old_cf = if new_old_len >= 2 {
                    self.bucket_freq_sum[old_code] - self.bucket_max_freq[old_code]
                } else {
                    0
                };
            } else {
                old_bucket_cf = 0;
                new_old_cf = 0;
            }

            let new_old_len = self.code_to_chars[old_code].len();
            let new_old_cc = new_old_len.saturating_sub(1);

            // === 插入新桶 ===
            let new_len = self.code_to_chars[new_code].len();
            let new_bucket_cc = new_len.saturating_sub(1);

            // top-N 碰撞：新桶 size >= 1 时 ci 将在碰撞中
            if new_len >= 1 && ctx.top_n_char_flags[ci] {
                self.top_n_collisions += 1;
            }
            // 新桶 size == 1 时，已有字获得碰撞状态
            if new_len == 1 {
                let existing_ci = self.code_to_chars[new_code][0];
                if ctx.top_n_char_flags[existing_ci] { self.top_n_collisions += 1; }
            }

            // 新桶从空变非空时，加入 populated_codes
            if new_len == 0 {
                self.code_populated_pos[new_code] = self.populated_codes.len();
                self.populated_codes.push(new_code);
            }

            let new_pos = new_len;
            self.code_to_chars[new_code].push(ci);
            self.char_bucket_pos[ci] = new_pos;

            let new_bucket_cf;
            let after_new_cf;
            if ctx.need_collision_rate {
                new_bucket_cf = if new_len >= 2 {
                    self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
                } else {
                    0
                };
                self.bucket_freq_sum[new_code] += freq;
                if freq > self.bucket_max_freq[new_code] {
                    self.bucket_max_freq[new_code] = freq;
                }
                let after_new_len = new_len + 1;
                after_new_cf = if after_new_len >= 2 {
                    self.bucket_freq_sum[new_code] - self.bucket_max_freq[new_code]
                } else {
                    0
                };
                self.collision_frequency = (self.collision_frequency + new_old_cf + after_new_cf)
                    - (old_bucket_cf + new_bucket_cf);
            }

            let after_new_len = new_len + 1;
            let after_new_cc = after_new_len.saturating_sub(1);

            // 更新全局碰撞计数
            self.total_collisions = (self.total_collisions + new_old_cc + after_new_cc)
                - (old_bucket_cc + new_bucket_cc);
        } else {
            // === 轻量路径：只用计数器维护碰撞数 ===
            let old_count = self.bucket_count[old_code];
            self.bucket_count[old_code] = old_count - 1;
            if old_count >= 2 {
                // 旧桶至少有 2 个，移除一个减少一次碰撞
                self.total_collisions -= 1;
            }

            let new_count = self.bucket_count[new_code];
            self.bucket_count[new_code] = new_count + 1;
            if new_count >= 1 {
                // 新桶原来有 >= 1 个，加入后增加一次碰撞
                self.total_collisions += 1;
            }
        }

        self.current_codes[ci] = new_code;
    }

    /// 计算全码得分
    #[inline(always)]
    pub fn compute_full_score(&self, ctx: &OptContext) -> f64 {
        let mut score = ctx.weights.full_top_n_collision
            * self.top_n_collisions as f64
            * ctx.scale_config.full_top_n_collision;

        score += ctx.weights.full_collision_count
            * self.total_collisions as f64
            * ctx.scale_config.full_collision_count;

        if ctx.weights.full_collision_rate > 0.0 {
            let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
            score += ctx.weights.full_collision_rate * collision_rate * ctx.scale_config.full_collision_rate;
        }

        if ctx.weights.full_equivalence > 0.0 {
            let weighted_equiv = self.total_equiv_weighted * self.inv_total_frequency;
            score += ctx.weights.full_equivalence * weighted_equiv * ctx.scale_config.full_equivalence;
        }

        if ctx.weights.full_distribution > 0.0 {
            let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);
            score += ctx.weights.full_distribution * dist_deviation * ctx.scale_config.full_distribution;
        }

        score
    }

    /// 计算综合得分
    #[inline(always)]
    pub fn compute_score(&self, ctx: &OptContext) -> f64 {
        let full_score = self.compute_full_score(ctx);

        let simple_score = if ctx.enable_simple_code {
            self.simple_eval.as_ref().map(|se| se.cached_simple_score).unwrap_or(0.0)
        } else {
            0.0
        };

        let word_score = if ctx.enable_word_code {
            self.word_eval.as_ref().map(|we| we.compute_word_score(ctx)).unwrap_or(0.0)
        } else {
            0.0
        };

        ctx.weights.weight_full_code * full_score
            + ctx.weights.weight_simple_code * simple_score
            + ctx.weights.weight_word_code * word_score
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

    /// 获取全码评估指标
    pub fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        Metrics {
            top_n_collision_count: self.top_n_collisions,
            collision_count: self.total_collisions,
            collision_rate: self.collision_frequency as f64 * self.inv_total_frequency,
            equiv_mean: self.total_equiv_weighted * self.inv_total_frequency,
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

    /// 获取词码评估指标
    pub fn get_word_metrics(&self, ctx: &OptContext) -> WordMetrics {
        if let Some(ref we) = self.word_eval {
            we.get_word_metrics(ctx)
        } else {
            WordMetrics::default()
        }
    }

    /// 检查是否有简码影响
    #[allow(dead_code)]
    pub fn has_simple_impact(&self, ctx: &OptContext, group: usize) -> bool {
        if !ctx.enable_simple_code || self.simple_eval.is_none() {
            return false;
        }
        !ctx.group_to_simple_affected[group].is_empty()
    }

    /// 重建简码评估
    pub fn rebuild_simple(&mut self, ctx: &OptContext, assignment: &[u8]) {
        if let Some(ref mut se) = self.simple_eval {
            se.full_rebuild(ctx, assignment, &self.populated_codes, &self.code_to_chars);
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

        // O(1) key_weighted_usage 更新
        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;

        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }

        // 词码增量更新
        if ctx.enable_word_code {
            if let Some(ref mut we) = self.word_eval {
                let key_delta = new_key as isize - old_key as isize;
                for &(wi, mask) in &ctx.word_gcm_data[ctx.word_gcm_offsets[r]..ctx.word_gcm_offsets[r + 1]] {
                    let old_code = we.current_codes[wi];
                    let new_code = (old_code as isize + mask as isize * key_delta) as usize;
                    if old_code != new_code {
                        we.update_word(ctx, wi, old_code, new_code);
                        we.update_word_equiv(ctx, wi, assignment);
                    }
                }
            }
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

            // 词码回滚
            if ctx.enable_word_code {
                if let Some(ref mut we) = self.word_eval {
                    let key_delta = old_key as isize - new_key as isize;
                    for &(wi, mask) in &ctx.word_gcm_data[ctx.word_gcm_offsets[r]..ctx.word_gcm_offsets[r + 1]] {
                        let cur_code = we.current_codes[wi];
                        let orig_code = (cur_code as isize + mask as isize * key_delta) as usize;
                        if cur_code != orig_code {
                            we.update_word(ctx, wi, cur_code, orig_code);
                            we.update_word_equiv(ctx, wi, assignment);
                        }
                    }
                }
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

        // 词码增量更新
        if ctx.enable_word_code {
            if let Some(ref mut we) = self.word_eval {
                for &r in &[r1, r2] {
                    let (old_k, new_k) = if r == r1 { (k1, k2) } else { (k2, k1) };
                    let key_delta = new_k as isize - old_k as isize;
                    for &(wi, mask) in &ctx.word_gcm_data[ctx.word_gcm_offsets[r]..ctx.word_gcm_offsets[r + 1]] {
                        let old_code = we.current_codes[wi];
                        let new_code = (old_code as isize + mask as isize * key_delta) as usize;
                        if old_code != new_code {
                            we.update_word(ctx, wi, old_code, new_code);
                            we.update_word_equiv(ctx, wi, assignment);
                        }
                    }
                }
            }
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

            // 词码回滚
            if ctx.enable_word_code {
                if let Some(ref mut we) = self.word_eval {
                    for &r in &[r1, r2] {
                        let (old_k, new_k) = if r == r1 { (k2, k1) } else { (k1, k2) };
                        let key_delta = new_k as isize - old_k as isize;
                        for &(wi, mask) in &ctx.word_gcm_data[ctx.word_gcm_offsets[r]..ctx.word_gcm_offsets[r + 1]] {
                            let cur_code = we.current_codes[wi];
                            let orig_code = (cur_code as isize + mask as isize * key_delta) as usize;
                            if cur_code != orig_code {
                                we.update_word(ctx, wi, cur_code, orig_code);
                                we.update_word_equiv(ctx, wi, assignment);
                            }
                        }
                    }
                }
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            false
        }
    }

    // =========================================================================
    // AMHB 专用 API：探测（probe）+ 应用（apply）
    // 探测：增量计算 delta_score 并自动回滚，不改变状态
    // 应用：增量提交变更，不回滚
    // =========================================================================

    /// 探测移动（probe）：增量计算 delta_score 并回滚，不改变 Evaluator/assignment 状态。
    /// 返回 delta_score = new_score - old_score（越小越好）。
    #[inline(always)]
    pub fn probe_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
    ) -> f64 {
        let old_key = assignment[r];
        if old_key == new_key {
            return 0.0;
        }

        // 快速路径：只优化 collision_count，用 V5 风格增量 hash
        if !ctx.need_bucket_members && !ctx.need_equiv {
            let key_delta = new_key as isize - old_key as isize;
            let mut delta_collisions: i32 = 0;

            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r]..ctx.gcm_offsets[r + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta) as usize;

                // 从旧桶移除
                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { delta_collisions -= 1; }

                // 加入新桶
                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { delta_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            // 回滚
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r]..ctx.gcm_offsets[r + 1]] {
                let cur_code = self.current_codes[ci];
                let orig_code = (cur_code as isize - mask as isize * key_delta) as usize;

                let cur_count = self.bucket_count[cur_code];
                self.bucket_count[cur_code] = cur_count - 1;

                let orig_count = self.bucket_count[orig_code];
                self.bucket_count[orig_code] = orig_count + 1;

                self.current_codes[ci] = orig_code;
            }

            // delta_score = weight * scale * delta_collisions
            return delta_collisions as f64
                * ctx.weights.full_collision_count
                * ctx.scale_config.full_collision_count;
        }

        // 通用路径：多目标优化
        let old_score = self.get_score(ctx);

        // 正向：应用变更
        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;
        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }
        self.score_dirty = true;
        let new_score = self.get_score(ctx);

        // 回滚
        self.key_weighted_usage[new_key as usize] -= gfs;
        self.key_weighted_usage[old_key as usize] += gfs;
        assignment[r] = old_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }
        self.cached_score = old_score;
        self.score_dirty = false;

        new_score - old_score
    }

    /// 探测交换（probe）：增量计算 delta_score 并回滚。
    #[inline(always)]
    pub fn probe_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
    ) -> f64 {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 {
            return 0.0;
        }

        // 快速路径：只优化 collision_count，用 V5 风格增量 hash
        if !ctx.need_bucket_members && !ctx.need_equiv {
            let mut delta_collisions: i32 = 0;

            // 第一步：r1 从 k1 变到 k2
            let key_delta1 = k2 as isize - k1 as isize;
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r1]..ctx.gcm_offsets[r1 + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta1) as usize;

                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { delta_collisions -= 1; }

                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { delta_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            // 第二步：r2 从 k2 变到 k1
            let key_delta2 = k1 as isize - k2 as isize;
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r2]..ctx.gcm_offsets[r2 + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta2) as usize;

                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { delta_collisions -= 1; }

                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { delta_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            // 回滚 r2
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r2]..ctx.gcm_offsets[r2 + 1]] {
                let cur_code = self.current_codes[ci];
                let orig_code = (cur_code as isize - mask as isize * key_delta2) as usize;

                self.bucket_count[cur_code] -= 1;
                self.bucket_count[orig_code] += 1;
                self.current_codes[ci] = orig_code;
            }

            // 回滚 r1
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r1]..ctx.gcm_offsets[r1 + 1]] {
                let cur_code = self.current_codes[ci];
                let orig_code = (cur_code as isize - mask as isize * key_delta1) as usize;

                self.bucket_count[cur_code] -= 1;
                self.bucket_count[orig_code] += 1;
                self.current_codes[ci] = orig_code;
            }

            return delta_collisions as f64
                * ctx.weights.full_collision_count
                * ctx.scale_config.full_collision_count;
        }

        // 通用路径：多目标优化
        let old_score = self.get_score(ctx);

        // 正向：应用交换
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
        self.score_dirty = true;
        let new_score = self.get_score(ctx);

        // 回滚
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
        self.cached_score = old_score;
        self.score_dirty = false;

        new_score - old_score
    }

    /// 应用移动（apply）：增量提交变更，不回滚。调用后 Evaluator 和 assignment 都进入新状态。
    #[inline(always)]
    pub fn apply_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
    ) {
        let old_key = assignment[r];
        if old_key == new_key {
            return;
        }

        // 快速路径
        if !ctx.need_bucket_members && !ctx.need_equiv {
            let key_delta = new_key as isize - old_key as isize;
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r]..ctx.gcm_offsets[r + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta) as usize;

                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { self.total_collisions -= 1; }

                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { self.total_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            let gfs = ctx.group_freq_sum[r];
            self.key_weighted_usage[old_key as usize] -= gfs;
            self.key_weighted_usage[new_key as usize] += gfs;
            assignment[r] = new_key;
            self.score_dirty = true;
            return;
        }

        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;
        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }
        self.score_dirty = true;
    }

    /// 应用交换（apply）：增量提交变更，不回滚。
    #[inline(always)]
    pub fn apply_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
    ) {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 {
            return;
        }

        // 快速路径
        if !ctx.need_bucket_members && !ctx.need_equiv {
            let key_delta1 = k2 as isize - k1 as isize;
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r1]..ctx.gcm_offsets[r1 + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta1) as usize;

                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { self.total_collisions -= 1; }

                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { self.total_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            let key_delta2 = k1 as isize - k2 as isize;
            for &(ci, mask) in &ctx.gcm_data[ctx.gcm_offsets[r2]..ctx.gcm_offsets[r2 + 1]] {
                let old_code = self.current_codes[ci];
                let new_code = (old_code as isize + mask as isize * key_delta2) as usize;

                let old_count = self.bucket_count[old_code];
                self.bucket_count[old_code] = old_count - 1;
                if old_count >= 2 { self.total_collisions -= 1; }

                let new_count = self.bucket_count[new_code];
                self.bucket_count[new_code] = new_count + 1;
                if new_count >= 1 { self.total_collisions += 1; }

                self.current_codes[ci] = new_code;
            }

            let gfs1 = ctx.group_freq_sum[r1];
            let gfs2 = ctx.group_freq_sum[r2];
            self.key_weighted_usage[k1 as usize] -= gfs1;
            self.key_weighted_usage[k2 as usize] += gfs1;
            self.key_weighted_usage[k2 as usize] -= gfs2;
            self.key_weighted_usage[k1 as usize] += gfs2;
            assignment[r1] = k2;
            assignment[r2] = k1;
            self.score_dirty = true;
            return;
        }

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
        self.score_dirty = true;
    }
}
