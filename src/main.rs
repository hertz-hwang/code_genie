use rand::prelude::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

// =========================================================================
// 🔧 配置区域 (可调参数)
// =========================================================================
mod config {
    // [文件路径]
    pub const FILE_FIXED: &str = "input-fixed.txt";           // 格式: 字根1 字根2 ... [tab] 键位(单个或空格分隔多个)
    pub const FILE_DYNAMIC: &str = "input-roots.txt";         // 格式: 字根1 字根2 ... (同行字根同编码)
    pub const FILE_SPLITS: &str = "input-division.txt";       // 格式: 汉字 [tab] 字根1 字根2 ... [tab] 字频
    pub const FILE_PAIR_EQUIV: &str = "pair_equivalence.txt"; // 格式: 按键对 [tab] 当量值
    pub const FILE_KEY_DIST: &str = "key_distribution.txt";   // 格式: 键位 [tab] 目标频率 [tab] 低频惩罚 [tab] 高频惩罚

    // [全局允许键位] - 动态字根只能分配到这些键上
    pub const ALLOWED_KEYS: &str = "qwertyuopasdfghjklzxcbnm";

    // [目标函数权重]
    pub const WEIGHT_COLLISION_COUNT: f64 = 0.5;
    pub const WEIGHT_COLLISION_RATE: f64 = 1.5;
    pub const WEIGHT_EQUIVALENCE: f64 = 0.25;
    pub const WEIGHT_EQUIV_CV: f64 = 0.01;
    pub const WEIGHT_DISTRIBUTION: f64 = 1.5;

    // [模拟退火 - 核心参数]
    pub const NUM_THREADS: usize = 16;
    pub const TOTAL_STEPS: usize = 100_000_000;
    pub const TEMP_START: f64 = 1.0;
    pub const TEMP_END: f64 = 0.2;
    pub const DECAY_RATE: f64 = 0.9998;

    // [变异策略]
    pub const SWAP_PROBABILITY: f64 = 0.6;

    // [自适应降温参数]
    pub const MIN_IMPROVE_STEPS: usize = TOTAL_STEPS / 500;
    pub const PERTURB_INTERVAL: usize = TOTAL_STEPS / 200;
    pub const PERTURB_STRENGTH: f64 = 0.15;
    pub const REHEAT_FACTOR: f64 = 1.5;
    pub const ACCEPTANCE_TARGET: f64 = 0.6;
}

// =========================================================================
// 🚀 高性能数据结构 & 预处理
// =========================================================================

const MAX_CODE_VAL: usize = 31 * 31 * 31;
const KEY_SPACE: usize = 26;
const MAX_PARTS: usize = 3;

fn char_to_key_index(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some((c as u8 - b'a') as usize),
        '_' => Some(KEY_SPACE),
        ';' => Some(27),
        ',' => Some(28),
        '.' => Some(29),
        '/' => Some(30),
        _ => None,
    }
}

const EQUIV_TABLE_SIZE: usize = 31;

#[derive(Clone, Copy, Default)]
struct KeyDistConfig {
    target_rate: f64,
    low_penalty: f64,
    high_penalty: f64,
}

#[derive(Clone, Copy)]
struct CharPackedInfo {
    parts: [u16; MAX_PARTS],
    num_parts: u8,
    frequency: u64,
}

impl Default for CharPackedInfo {
    #[inline]
    fn default() -> Self {
        Self {
            parts: [0; MAX_PARTS],
            num_parts: 0,
            frequency: 0,
        }
    }
}

/// 字根组信息
#[derive(Clone)]
struct RootGroup {
    roots: Vec<String>,      // 组内字根名称列表
    allowed_keys: Vec<u8>,   // 允许的键位列表
    is_fixed: bool,          // 是否固定（只有一个键位）
}

struct OptContext {
    num_dynamic_groups: usize,
    root_name_to_group_idx: HashMap<String, usize>,
    group_to_char_indices: Vec<Vec<usize>>,
    char_infos: Vec<CharPackedInfo>,
    raw_splits: Vec<(char, Vec<String>, u64)>,
    dynamic_groups: Vec<RootGroup>,
    fixed_roots: HashMap<String, u8>,  // 真正固定的字根
    equiv_table: [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
    key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    total_frequency: u64,
}

impl OptContext {
    fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        dynamic_groups: &[RootGroup],
        equiv_table: [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    ) -> Self {
        // 建立字根名到组索引的映射（仅动态组）
        let mut root_name_to_group_idx: HashMap<String, usize> = HashMap::new();
        for (group_idx, group) in dynamic_groups.iter().enumerate() {
            for root_name in &group.roots {
                root_name_to_group_idx.insert(root_name.clone(), group_idx);
            }
        }

        let num_dynamic_groups = dynamic_groups.len();
        let mut group_to_char_indices = vec![Vec::new(); num_dynamic_groups];
        let mut char_infos = Vec::with_capacity(splits.len());
        let mut total_frequency = 0u64;

        for (char_idx, (_, roots, freq)) in splits.iter().enumerate() {
            let mut info = CharPackedInfo::default();
            info.frequency = *freq;

            let mut used_group_indices = HashSet::new();

            for root in roots.iter().take(MAX_PARTS) {
                let i = info.num_parts as usize;
                if let Some(&key) = fixed_roots.get(root) {
                    // 固定字根直接存储键位
                    info.parts[i] = key as u16;
                    info.num_parts += 1;
                } else if let Some(&group_idx) = root_name_to_group_idx.get(root) {
                    // 动态组存储组索引 + 1000
                    info.parts[i] = (group_idx + 1000) as u16;
                    info.num_parts += 1;
                    used_group_indices.insert(group_idx);
                }
            }

            for &group_idx in &used_group_indices {
                group_to_char_indices[group_idx].push(char_idx);
            }

            total_frequency += freq;
            char_infos.push(info);
        }

        Self {
            num_dynamic_groups,
            root_name_to_group_idx,
            group_to_char_indices,
            char_infos,
            raw_splits: splits.to_vec(),
            dynamic_groups: dynamic_groups.to_vec(),
            fixed_roots: fixed_roots.clone(),
            equiv_table,
            key_dist_config,
            total_frequency,
        }
    }

    #[inline(always)]
    fn calc_code_and_keys(&self, char_idx: usize, assignment: &[u8]) -> (usize, [u8; MAX_PARTS], u8) {
        let info = &self.char_infos[char_idx];
        let n = info.num_parts as usize;
        let mut code = 0usize;
        let mut keys = [0u8; MAX_PARTS];

        match n {
            1 => {
                let p = info.parts[0];
                let key = if p >= 1000 { assignment[(p - 1000) as usize] } else { p as u8 };
                keys[0] = key;
                code = key as usize + 1;
            }
            2 => {
                let p0 = info.parts[0];
                let p1 = info.parts[1];
                let k0 = if p0 >= 1000 { assignment[(p0 - 1000) as usize] } else { p0 as u8 };
                let k1 = if p1 >= 1000 { assignment[(p1 - 1000) as usize] } else { p1 as u8 };
                keys[0] = k0;
                keys[1] = k1;
                code = (k0 as usize + 1) * 27 + (k1 as usize + 1);
            }
            3 => {
                let p0 = info.parts[0];
                let p1 = info.parts[1];
                let p2 = info.parts[2];
                let k0 = if p0 >= 1000 { assignment[(p0 - 1000) as usize] } else { p0 as u8 };
                let k1 = if p1 >= 1000 { assignment[(p1 - 1000) as usize] } else { p1 as u8 };
                let k2 = if p2 >= 1000 { assignment[(p2 - 1000) as usize] } else { p2 as u8 };
                keys[0] = k0;
                keys[1] = k1;
                keys[2] = k2;
                code = ((k0 as usize + 1) * 27 + (k1 as usize + 1)) * 27 + (k2 as usize + 1);
            }
            _ => {
                for i in 0..n {
                    let p = info.parts[i];
                    let key = if p >= 1000 { assignment[(p - 1000) as usize] } else { p as u8 };
                    keys[i] = key;
                    code = code * 27 + (key as usize + 1);
                }
            }
        }

        (code, keys, info.num_parts)
    }

    #[inline(always)]
    fn calc_key_avg_equiv_inline(&self, keys: [u8; MAX_PARTS], len: u8) -> f64 {
        match len {
            0 => 0.0,
            1 => self.equiv_table[keys[0] as usize][KEY_SPACE],
            2 => {
                let k0 = keys[0] as usize;
                let k1 = keys[1] as usize;
                (self.equiv_table[k0][k1] + self.equiv_table[k1][KEY_SPACE]) * 0.5
            }
            3 => {
                let k0 = keys[0] as usize;
                let k1 = keys[1] as usize;
                let k2 = keys[2] as usize;
                (self.equiv_table[k0][k1] + self.equiv_table[k1][k2] + self.equiv_table[k2][KEY_SPACE]) * (1.0 / 3.0)
            }
            _ => {
                let n = len as usize;
                let mut total = 0.0;
                for i in 0..n - 1 {
                    total += self.equiv_table[keys[i] as usize][keys[i + 1] as usize];
                }
                total += self.equiv_table[keys[n - 1] as usize][KEY_SPACE];
                total / n as f64
            }
        }
    }
}

// =========================================================================
// ⚡ 评估器 (状态机)
// =========================================================================

#[derive(Clone, Copy, Default)]
struct Metrics {
    collision_count: usize,
    collision_rate: f64,
    equiv_mean: f64,
    equiv_cv: f64,
    dist_deviation: f64,
}

struct Evaluator {
    current_codes: Vec<usize>,
    current_keys: Vec<([u8; MAX_PARTS], u8)>,
    current_equiv_contrib: Vec<f64>,
    current_equiv_sq_contrib: Vec<f64>,

    buckets: Vec<u16>,
    bucket_freqs: Vec<u64>,

    total_collisions: usize,
    collision_frequency: u64,
    total_equiv_weighted: f64,
    total_equiv_sq_weighted: f64,

    key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    total_key_presses: f64,
    inv_total_key_presses: f64,

    total_frequency: u64,
    inv_total_frequency: f64,
}

impl Evaluator {
    fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let mut buckets = vec![0u16; MAX_CODE_VAL];
        let mut bucket_freqs = vec![0u64; MAX_CODE_VAL];
        let mut current_codes = Vec::with_capacity(ctx.char_infos.len());
        let mut current_keys = Vec::with_capacity(ctx.char_infos.len());
        let mut current_equiv_contrib = Vec::with_capacity(ctx.char_infos.len());
        let mut current_equiv_sq_contrib = Vec::with_capacity(ctx.char_infos.len());

        let mut total_collisions = 0;
        let mut collision_frequency = 0u64;
        let mut total_equiv_weighted = 0.0;
        let mut total_equiv_sq_weighted = 0.0;

        let mut key_weighted_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for i in 0..ctx.char_infos.len() {
            let (code, keys, num_keys) = ctx.calc_code_and_keys(i, assignment);
            let freq = ctx.char_infos[i].frequency;
            let freq_f = freq as f64;

            current_codes.push(code);
            current_keys.push((keys, num_keys));

            if buckets[code] >= 1 {
                total_collisions += 1;
                collision_frequency += freq;
                if buckets[code] == 1 {
                    collision_frequency += bucket_freqs[code];
                }
            }
            buckets[code] += 1;
            bucket_freqs[code] += freq;

            let key_avg_equiv = ctx.calc_key_avg_equiv_inline(keys, num_keys);
            let contrib = key_avg_equiv * freq_f;
            let sq_contrib = key_avg_equiv * key_avg_equiv * freq_f;
            current_equiv_contrib.push(contrib);
            current_equiv_sq_contrib.push(sq_contrib);
            total_equiv_weighted += contrib;
            total_equiv_sq_weighted += sq_contrib;

            let n = num_keys as usize;
            for j in 0..n {
                key_weighted_usage[keys[j] as usize] += freq_f;
            }
            total_key_presses += freq_f * n as f64;
        }

        let inv_total_frequency = if ctx.total_frequency > 0 {
            1.0 / ctx.total_frequency as f64
        } else {
            0.0
        };

        let inv_total_key_presses = if total_key_presses > 0.0 {
            1.0 / total_key_presses
        } else {
            0.0
        };

        Self {
            current_codes,
            current_keys,
            current_equiv_contrib,
            current_equiv_sq_contrib,
            buckets,
            bucket_freqs,
            total_collisions,
            collision_frequency,
            total_equiv_weighted,
            total_equiv_sq_weighted,
            key_weighted_usage,
            total_key_presses,
            inv_total_key_presses,
            total_frequency: ctx.total_frequency,
            inv_total_frequency,
        }
    }

    #[inline(always)]
    fn calc_equiv_cv(&self) -> f64 {
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

    #[inline(always)]
    fn calc_distribution_deviation(&self, config: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
        let mut deviation = 0.0;
        
        for key in 0..EQUIV_TABLE_SIZE {
            let cfg = &config[key];
            if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
                continue;
            }
            
            let actual_pct = self.key_weighted_usage[key] * 100.0 * self.inv_total_key_presses;
            let target_pct = cfg.target_rate;
            let diff = actual_pct - target_pct;
            
            if diff < 0.0 {
                deviation += diff * diff * cfg.low_penalty;
            } else if diff > 0.0 {
                deviation += diff * diff * cfg.high_penalty;
            }
        }
        
        deviation
    }

    #[inline(always)]
    fn get_score(&self, ctx: &OptContext) -> f64 {
        let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
        let weighted_equiv = self.total_equiv_weighted * self.inv_total_frequency;
        let equiv_cv = self.calc_equiv_cv();
        let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);

        config::WEIGHT_COLLISION_COUNT * self.total_collisions as f64
            + config::WEIGHT_COLLISION_RATE * collision_rate * 10000.0
            + config::WEIGHT_EQUIVALENCE * weighted_equiv * 10000.0
            + config::WEIGHT_EQUIV_CV * equiv_cv * 100.0
            + config::WEIGHT_DISTRIBUTION * dist_deviation * 10.0
    }

    fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        let collision_rate = if self.total_frequency > 0 {
            self.collision_frequency as f64 / self.total_frequency as f64
        } else {
            0.0
        };

        let equiv_mean = if self.total_frequency > 0 {
            self.total_equiv_weighted / self.total_frequency as f64
        } else {
            0.0
        };

        let equiv_cv = self.calc_equiv_cv();
        let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);

        Metrics {
            collision_count: self.total_collisions,
            collision_rate,
            equiv_mean,
            equiv_cv,
            dist_deviation,
        }
    }

    #[inline(always)]
    fn try_swap(
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

        assignment[r1] = k2;
        assignment[r2] = k1;

        self.update_diff(ctx, assignment, r1);
        self.update_diff(ctx, assignment, r2);

        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            true
        } else {
            assignment[r1] = k1;
            assignment[r2] = k2;
            self.update_diff(ctx, assignment, r1);
            self.update_diff(ctx, assignment, r2);
            false
        }
    }

    #[inline(always)]
    fn try_move(
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

        assignment[r] = new_key;

        self.update_diff(ctx, assignment, r);

        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            true
        } else {
            assignment[r] = old_key;
            self.update_diff(ctx, assignment, r);
            false
        }
    }

    #[inline(always)]
    fn update_diff(&mut self, ctx: &OptContext, assignment: &[u8], group_idx: usize) {
        let affected = &ctx.group_to_char_indices[group_idx];
        for &char_idx in affected {
            let old_code = self.current_codes[char_idx];
            let (old_keys, old_num_keys) = self.current_keys[char_idx];
            
            let (new_code, new_keys, new_num_keys) = ctx.calc_code_and_keys(char_idx, assignment);

            if old_code == new_code {
                continue;
            }

            let freq = ctx.char_infos[char_idx].frequency;
            let freq_f = freq as f64;

            let old_count = self.buckets[old_code];
            if old_count > 1 {
                self.total_collisions -= 1;
                self.collision_frequency -= freq;
                if old_count == 2 {
                    self.collision_frequency -= self.bucket_freqs[old_code] - freq;
                }
            }
            self.buckets[old_code] -= 1;
            self.bucket_freqs[old_code] -= freq;

            let new_count = self.buckets[new_code];
            if new_count >= 1 {
                self.total_collisions += 1;
                self.collision_frequency += freq;
                if new_count == 1 {
                    self.collision_frequency += self.bucket_freqs[new_code];
                }
            }
            self.buckets[new_code] += 1;
            self.bucket_freqs[new_code] += freq;

            let old_contrib = self.current_equiv_contrib[char_idx];
            let old_sq_contrib = self.current_equiv_sq_contrib[char_idx];
            
            let new_key_avg_equiv = ctx.calc_key_avg_equiv_inline(new_keys, new_num_keys);
            let new_contrib = new_key_avg_equiv * freq_f;
            let new_sq_contrib = new_key_avg_equiv * new_key_avg_equiv * freq_f;

            self.total_equiv_weighted -= old_contrib;
            self.total_equiv_weighted += new_contrib;
            self.total_equiv_sq_weighted -= old_sq_contrib;
            self.total_equiv_sq_weighted += new_sq_contrib;
            
            self.current_equiv_contrib[char_idx] = new_contrib;
            self.current_equiv_sq_contrib[char_idx] = new_sq_contrib;

            let old_n = old_num_keys as usize;
            for j in 0..old_n {
                self.key_weighted_usage[old_keys[j] as usize] -= freq_f;
            }
            let new_n = new_num_keys as usize;
            for j in 0..new_n {
                self.key_weighted_usage[new_keys[j] as usize] += freq_f;
            }

            self.current_codes[char_idx] = new_code;
            self.current_keys[char_idx] = (new_keys, new_num_keys);
        }
    }
}

// =========================================================================
// 🧠 算法实现
// =========================================================================

fn smart_init(ctx: &OptContext) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_dynamic_groups];
    let mut rng = thread_rng();

    // 按组涉及的汉字数量排序
    let mut group_freq: Vec<(usize, usize)> = ctx
        .group_to_char_indices
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    group_freq.sort_by(|a, b| b.1.cmp(&a.1));

    let max_key_index = config::ALLOWED_KEYS
        .chars()
        .filter_map(char_to_key_index)
        .max()
        .unwrap_or(25) as usize;
    let mut key_counts = vec![0usize; max_key_index + 1];

    for (group_idx, _) in group_freq {
        let allowed = &ctx.dynamic_groups[group_idx].allowed_keys;
        
        let min_count = allowed.iter()
            .map(|&k| key_counts.get(k as usize).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);
        
        let candidates: Vec<u8> = allowed.iter()
            .filter(|&&k| key_counts.get(k as usize).copied().unwrap_or(0) == min_count)
            .copied()
            .collect();

        let best_key = if candidates.is_empty() {
            allowed[0]
        } else {
            candidates[rng.gen_range(0..candidates.len())]
        };
        
        assignment[group_idx] = best_key;
        if (best_key as usize) < key_counts.len() {
            key_counts[best_key as usize] += 1;
        }
    }
    assignment
}

fn simulated_annealing(ctx: &OptContext, thread_id: usize) -> (Vec<u8>, f64, Metrics) {
    let mut rng = thread_rng();

    let mut assignment = smart_init(ctx);
    if rng.gen_bool(0.5) {
        for (i, val) in assignment.iter_mut().enumerate() {
            if rng.gen_bool(0.1) {
                let allowed = &ctx.dynamic_groups[i].allowed_keys;
                *val = allowed[rng.gen_range(0..allowed.len())];
            }
        }
    }

    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);

    let steps = config::TOTAL_STEPS;
    let t_start = config::TEMP_START;
    let t_end = config::TEMP_END;
    let decay_rate = config::DECAY_RATE;
    let mut temp = t_start;

    let mut steps_since_improve = 0;
    let mut last_best_score = best_score;
    let mut accepted_moves = 0;
    let mut total_moves = 0;

    let n_groups = assignment.len();
    let report_interval = steps / 20;

    for step in 0..steps {
        let mut accepted = false;

        if rng.gen::<f64>() < config::SWAP_PROBABILITY {
            let r1 = rng.gen_range(0..n_groups);
            let r2 = rng.gen_range(0..n_groups);
            if r1 != r2 {
                let k1 = assignment[r1];
                let k2 = assignment[r2];
                let can_swap = ctx.dynamic_groups[r1].allowed_keys.contains(&k2) 
                            && ctx.dynamic_groups[r2].allowed_keys.contains(&k1);
                if can_swap {
                    accepted = evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
                }
            }
        } else {
            let r = rng.gen_range(0..n_groups);
            let allowed = &ctx.dynamic_groups[r].allowed_keys;
            let new_k = allowed[rng.gen_range(0..allowed.len())];
            accepted = evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
        }

        total_moves += 1;
        if accepted {
            accepted_moves += 1;
        }

        let current_score = evaluator.get_score(ctx);
        if current_score < best_score {
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            steps_since_improve = 0;

            if thread_id == 0 && best_score <= last_best_score - 0.1 {
                let m = best_metrics;
                println!(
                    "   [T0] 步数 {}/{} | 温度 {:.9} | (重码:{}, 重码率:{:.4}%, 当量:{:.4}, CV:{:.4}, 分布偏差:{:.4}) | 当前得分: {:.4}",
                    step, steps, temp, m.collision_count, m.collision_rate * 100.0, m.equiv_mean, m.equiv_cv, m.dist_deviation, best_score
                );
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        if step > 0 && step % 10000 == 0 {
            let acceptance_rate = accepted_moves as f64 / total_moves as f64;

            if acceptance_rate < config::ACCEPTANCE_TARGET * 0.5 && temp < t_start * 0.1 {
                temp *= 1.05;
            } else if acceptance_rate > config::ACCEPTANCE_TARGET * 1.5 && temp > t_end * 10.0 {
                temp *= 0.95;
            } else {
                temp *= decay_rate;
            }

            accepted_moves = 0;
            total_moves = 0;
        }

        if steps_since_improve > config::MIN_IMPROVE_STEPS && temp < t_start * 0.200001 {
            temp = (temp * config::REHEAT_FACTOR).min(t_start);
            steps_since_improve = 0;
        }

        if step > 0 && step % config::PERTURB_INTERVAL == 0 && temp < 0.200001 {
            let n_perturb = (n_groups as f64 * config::PERTURB_STRENGTH) as usize;
            for _ in 0..n_perturb {
                let r1 = rng.gen_range(0..n_groups);
                let r2 = rng.gen_range(0..n_groups);
                if r1 != r2 {
                    let k1 = assignment[r1];
                    let k2 = assignment[r2];
                    let can_swap = ctx.dynamic_groups[r1].allowed_keys.contains(&k2) 
                                && ctx.dynamic_groups[r2].allowed_keys.contains(&k1);
                    if can_swap {
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, temp * 3.0, &mut rng);
                    }
                }
            }

            if thread_id == 0 {
                let m = evaluator.get_metrics(ctx);
                println!(
                    "   [T0] 步数 {}: 扰动 | (重码={}, 重码率={:.4}%, 当量={:.4}, CV={:.4}, 分布偏差={:.4}) | 当前得分: {:.4} ",
                    step, m.collision_count, m.collision_rate * 100.0, m.equiv_mean, m.equiv_cv, m.dist_deviation, current_score
                );
            }
        }

        temp = temp.max(t_end);

        if thread_id == 0 && step % report_interval == 0 && step > 0 {
            let progress = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            println!(
                "   [T0] 进度: {}% | 温度: {:.9} | (重码={}, 重码率={:.4}%, 当量={:.4}, CV={:.4}, 分布={:.4}) | 当前得分: {:.4} 🏆最优: {:.4}",
                progress, temp, m.collision_count, m.collision_rate * 100.0, m.equiv_mean, m.equiv_cv, m.dist_deviation, current_score, best_score
            );
        }
    }

    (best_assignment, best_score, best_metrics)
}

// =========================================================================
// 📂 文件加载
// =========================================================================

/// 加载固定字根文件
/// 格式: 字根1 字根2 ... [tab] 键位(单个或空格分隔多个)
/// 
/// 示例:
///   - 单键位固定: "传	a" 或 "左 右	a" (字根固定到单个键位)
///   - 多键位受限: "左 右	a d h" (字根组可在指定键位间移动)
fn load_fixed(path: &str) -> (HashMap<String, u8>, Vec<RootGroup>) {
    let content = fs::read_to_string(path).expect("无法读取固定字根文件");
    let mut truly_fixed: HashMap<String, u8> = HashMap::new();
    let mut constrained_groups: Vec<RootGroup> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            // 解析字根组（空格分隔）
            let roots: Vec<String> = parts[0]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            
            if roots.is_empty() {
                continue;
            }

            // 解析键位（空格分隔）
            let keys: Vec<u8> = parts[1]
                .split_whitespace()
                .filter_map(|s| {
                    let c = s.chars().next()?;
                    char_to_key_index(c).map(|idx| idx as u8)
                })
                .collect();

            if keys.len() == 1 {
                // 单键位：所有字根都固定到这个键位
                for root in roots {
                    truly_fixed.insert(root, keys[0]);
                }
            } else if keys.len() > 1 {
                // 多键位：创建一个受限字根组
                constrained_groups.push(RootGroup {
                    roots,
                    allowed_keys: keys,
                    is_fixed: false,
                });
            }
        }
    }

    (truly_fixed, constrained_groups)
}

/// 加载动态字根，返回字根组列表
/// 格式: 每行一组，同组字根用空格分隔，表示同编码
/// 例如: "禾 禾框 余字底" 表示这三个字根共享同一个键位
fn load_dynamic(path: &str, constrained_groups: &[RootGroup]) -> Vec<RootGroup> {
    // 解析全局允许键位
    let global_allowed: Vec<u8> = config::ALLOWED_KEYS
        .chars()
        .filter_map(char_to_key_index)
        .map(|idx| idx as u8)
        .collect();

    let content = fs::read_to_string(path)
        .expect("无法读取动态字根文件");
    
    // 收集受限组中已有的字根
    let mut existing_roots: HashSet<String> = HashSet::new();
    for group in constrained_groups {
        for root in &group.roots {
            existing_roots.insert(root.clone());
        }
    }
    
    let mut groups: Vec<RootGroup> = Vec::new();
    
    // 先添加受限组
    for group in constrained_groups {
        groups.push(group.clone());
    }
    
    // 解析动态字根文件
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        
        let roots: Vec<String> = line
            .split_whitespace()
            .map(|s| s.to_string())
            .filter(|s| !existing_roots.contains(s))  // 过滤掉已在受限组中的字根
            .collect();
        
        if !roots.is_empty() {
            // 检查是否有部分字根已存在于某个组中
            let mut found_in_existing = false;
            for group in &mut groups {
                let has_overlap = roots.iter().any(|r| group.roots.contains(r));
                if has_overlap {
                    // 合并到现有组
                    for root in &roots {
                        if !group.roots.contains(root) && !existing_roots.contains(root) {
                            group.roots.push(root.clone());
                            existing_roots.insert(root.clone());
                        }
                    }
                    found_in_existing = true;
                    break;
                }
            }
            
            if !found_in_existing {
                for root in &roots {
                    existing_roots.insert(root.clone());
                }
                groups.push(RootGroup {
                    roots,
                    allowed_keys: global_allowed.clone(),
                    is_fixed: false,
                });
            }
        }
    }
    
    groups
}

/// 加载拆分表
/// 格式: 汉字 [tab] 字根1 字根2 字根3 [tab] 字频
fn load_splits(path: &str) -> Vec<(char, Vec<String>, u64)> {
    let content = fs::read_to_string(path).expect("无法读取拆分表");
    let mut res = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let ch = parts[0].chars().next().unwrap();
            
            let roots: Vec<String> = parts[1]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();

            let freq: u64 = if parts.len() >= 3 {
                parts[2].trim().parse().unwrap_or(1)
            } else {
                1
            };

            res.push((ch, roots, freq));
        }
    }
    res
}

fn load_pair_equivalence(path: &str) -> [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE] {
    let mut table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("警告: 无法读取当量文件 {}，将使用默认值0", path);
            return table;
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let pair_str = parts[0];
            let chars: Vec<char> = pair_str.chars().collect();

            if chars.len() == 2 {
                if let (Some(k1), Some(k2)) = (char_to_key_index(chars[0]), char_to_key_index(chars[1]))
                {
                    if let Ok(equiv) = parts[1].trim().parse::<f64>() {
                        if k1 < EQUIV_TABLE_SIZE && k2 < EQUIV_TABLE_SIZE {
                            table[k1][k2] = equiv;
                        }
                    }
                }
            }
        }
    }

    table
}

fn load_key_distribution(path: &str) -> [KeyDistConfig; EQUIV_TABLE_SIZE] {
    let mut config = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("警告: 无法读取用指分布文件 {}，将使用默认值", path);
            return config;
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            let key_char = parts[0].chars().next().unwrap();
            if let Some(key_idx) = char_to_key_index(key_char) {
                if key_idx < EQUIV_TABLE_SIZE {
                    config[key_idx] = KeyDistConfig {
                        target_rate: parts[1].trim().parse().unwrap_or(0.0),
                        low_penalty: parts[2].trim().parse().unwrap_or(0.0),
                        high_penalty: parts[3].trim().parse().unwrap_or(0.0),
                    };
                }
            }
        }
    }

    config
}

fn key_to_char(key: u8) -> char {
    match key {
        0..=25 => (key + b'a') as char,
        26 => '_',
        27 => ';',
        28 => ',',
        29 => '.',
        30 => '/',
        _ => '?',
    }
}

// =========================================================================
// 🔍 数据校验
// =========================================================================

/// 校验拆分表中的字根是否都已定义
fn validate_roots(
    splits: &[(char, Vec<String>, u64)],
    fixed_roots: &HashMap<String, u8>,
    dynamic_groups: &[RootGroup],
) -> (bool, Vec<String>, HashMap<String, Vec<char>>) {
    // 收集所有已定义的字根
    let mut defined_roots: HashSet<String> = HashSet::new();
    
    // 固定字根
    for root in fixed_roots.keys() {
        defined_roots.insert(root.clone());
    }
    
    // 动态字根组中的所有字根
    for group in dynamic_groups {
        for root in &group.roots {
            defined_roots.insert(root.clone());
        }
    }
    
    // 收集拆分表中使用的所有字根
    let mut used_roots: HashMap<String, Vec<char>> = HashMap::new();
    for (ch, roots, _) in splits {
        for root in roots {
            used_roots.entry(root.clone())
                .or_insert_with(Vec::new)
                .push(*ch);
        }
    }
    
    // 找出缺失的字根
    let mut missing_roots: Vec<String> = Vec::new();
    let mut missing_examples: HashMap<String, Vec<char>> = HashMap::new();
    
    for (root, chars) in &used_roots {
        if !defined_roots.contains(root) {
            missing_roots.push(root.clone());
            let examples: Vec<char> = chars.iter().take(10).copied().collect();
            missing_examples.insert(root.clone(), examples);
        }
    }
    
    missing_roots.sort();
    
    let is_valid = missing_roots.is_empty();
    (is_valid, missing_roots, missing_examples)
}

/// 打印校验结果
fn check_and_report_validation(
    splits: &[(char, Vec<String>, u64)],
    fixed_roots: &HashMap<String, u8>,
    dynamic_groups: &[RootGroup],
) -> bool {
    println!("\n🔍 正在校验字根定义...");
    
    let (is_valid, missing_roots, missing_examples) = validate_roots(
        splits, fixed_roots, dynamic_groups
    );
    
    if is_valid {
        println!("✅ 校验通过：拆分表中的所有字根都已定义");
        return true;
    }
    
    let separator = "=".repeat(60);
    println!("❌ 校验失败：发现 {} 个未定义的字根！", missing_roots.len());
    println!("{}", separator);
    println!("{:<15} {}", "缺失字根", "使用该字根的汉字示例");
    println!("{}", "-".repeat(60));
    
    for root in &missing_roots {
        let examples = missing_examples.get(root).unwrap();
        let examples_str: String = examples.iter().collect();
        let more = if examples.len() >= 10 { " ..." } else { "" };
        println!("{:<15} {}{}", root, examples_str, more);
    }
    
    println!("{}", separator);
    println!("\n请在 {} 或 {} 中添加以上缺失的字根后重试。",
        config::FILE_FIXED, config::FILE_DYNAMIC);
    
    // 输出到文件
    let mut report = String::new();
    report.push_str("# 缺失字根报告\n");
    report.push_str(&format!("# 共发现 {} 个未定义的字根\n\n", missing_roots.len()));
    report.push_str("# 格式: 字根 [tab] 使用该字根的汉字示例\n");
    
    for root in &missing_roots {
        let examples = missing_examples.get(root).unwrap();
        let examples_str: String = examples.iter().collect();
        report.push_str(&format!("{}\t{}\n", root, examples_str));
    }
    
    fs::write("missing-roots.txt", report).unwrap();
    println!("缺失字根列表已保存至 missing-roots.txt");
    
    false
}

// =========================================================================
// 🏁 主函数
// =========================================================================

fn main() {
    let start_time = Instant::now();
    println!("=== H3退火优化器 v3 (支持同编码字根组) ===");
    println!("线程数: {}, 总步数: {}", config::NUM_THREADS, config::TOTAL_STEPS);
    println!("初始温度: {}, 结束温度: {}", config::TEMP_START, config::TEMP_END);
    println!("全局允许键位: {}", config::ALLOWED_KEYS);
    println!(
        "目标权重: 重码数={}, 重码率={}, 当量={}, 当量CV={}, 用指分布={}",
        config::WEIGHT_COLLISION_COUNT,
        config::WEIGHT_COLLISION_RATE,
        config::WEIGHT_EQUIVALENCE,
        config::WEIGHT_EQUIV_CV,
        config::WEIGHT_DISTRIBUTION
    );

    // 1. 加载数据
    let (fixed_roots, constrained_groups) = load_fixed(config::FILE_FIXED);
    let dynamic_groups = load_dynamic(config::FILE_DYNAMIC, &constrained_groups);
    let splits = load_splits(config::FILE_SPLITS);
    let equiv_table = load_pair_equivalence(config::FILE_PAIR_EQUIV);
    let key_dist_config = load_key_distribution(config::FILE_KEY_DIST);

    // 统计信息
    let total_roots_in_groups: usize = dynamic_groups.iter().map(|g| g.roots.len()).sum();
    let constrained_count = constrained_groups.len();
    let total_freq: u64 = splits.iter().map(|(_, _, f)| f).sum();
    
    println!("\n数据加载完毕:");
    println!("  - 固定字根(单键): {}", fixed_roots.len());
    println!("  - 受限字根组(多键): {} 组", constrained_count);
    println!("  - 动态字根组总数: {} 组 (共 {} 个字根)", dynamic_groups.len(), total_roots_in_groups);
    println!("  - 汉字数量: {}", splits.len());
    println!("  - 总字频: {}", total_freq);

    // 2. 校验字根定义
    if !check_and_report_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }

    // 3. 构建上下文
    let ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
    );

    // 4. 并行执行 SA
    println!("\n开始优化...");

    let results: Vec<(Vec<u8>, f64, Metrics)> = (0..config::NUM_THREADS)
        .into_par_iter()
        .map(|i| simulated_annealing(&ctx, i))
        .collect();

    // 5. 汇总结果
    let (best_assignment, best_score, best_metrics) = results
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap();

    let m = best_metrics;
    println!("\n=================================");
    println!("🏆 最优结果:");
    println!("   综合得分: {:.4}", best_score);
    println!("   重码数: {}", m.collision_count);
    println!("   重码率: {:.8} ({:.6}%)", m.collision_rate, m.collision_rate * 100.0);
    println!("   加权键均当量: {:.4}", m.equiv_mean);
    println!("   当量变异系数(CV): {:.4}", m.equiv_cv);
    println!("   用指分布偏差(L2): {:.4}", m.dist_deviation);
    println!("⏱️ 总耗时: {:?}", start_time.elapsed());
    println!("=================================");

    // 6. 保存结果
    save_results(&ctx, &best_assignment, best_score, &best_metrics);
}

fn save_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
) {
    // 1. 保存字根键位 (每组一行，同组字根用空格分隔)
    let mut root_out = String::new();
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!("# 重码率: {:.6}%\n", metrics.collision_rate * 100.0));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!("# 用指分布偏差(L2): {:.4}\n", metrics.dist_deviation));
    root_out.push_str("#\n");
    root_out.push_str("# === 固定字根 ===\n");
    root_out.push_str("# 格式: 字根1 字根2 ... [tab] 键位\n");

    // 按键位分组输出固定字根
    let mut fixed_by_key: HashMap<u8, Vec<String>> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        fixed_by_key.entry(key).or_insert_with(Vec::new).push(root.clone());
    }
    let mut fixed_keys: Vec<u8> = fixed_by_key.keys().copied().collect();
    fixed_keys.sort();
    for key in fixed_keys {
        let roots = fixed_by_key.get(&key).unwrap();
        let roots_str = roots.join(" ");
        root_out.push_str(&format!("{}\t{}\n", roots_str, key_to_char(key)));
    }

    root_out.push_str("#\n");
    root_out.push_str("# === 动态字根组 ===\n");
    root_out.push_str("# 格式: 字根1 字根2 ... [tab] 键位 [tab] 允许键位\n");

    for (group_idx, group) in ctx.dynamic_groups.iter().enumerate() {
        let key = key_to_char(assignment[group_idx]);
        let roots_str = group.roots.join(" ");
        let allowed_str: String = group.allowed_keys.iter()
            .map(|&k| key_to_char(k))
            .collect();
        root_out.push_str(&format!("{}\t{}\t[{}]\n", roots_str, key, allowed_str));
    }
    fs::write("output-keymap.txt", root_out).unwrap();

    // 2. 保存汉字编码
    let mut code_out = String::new();

    // 建立字根名到键位的映射
    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root_str, &key) in &ctx.fixed_roots {
        root_to_key.insert(root_str.clone(), key);
    }
    for (group_idx, group) in ctx.dynamic_groups.iter().enumerate() {
        let key = assignment[group_idx];
        for root_str in &group.roots {
            root_to_key.insert(root_str.clone(), key);
        }
    }

    for (ch, roots, freq) in &ctx.raw_splits {
        let mut code_parts = Vec::new();
        for root in roots.iter().take(MAX_PARTS) {
            if let Some(&key) = root_to_key.get(root) {
                let key_char = key_to_char(key);
                code_parts.push(key_char);
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
    }

    fs::write("output-encode.txt", code_out).unwrap();
    
    // 3. 保存用指分布统计
    save_key_distribution(ctx, assignment);
    
    // 4. 保存当量分布统计
    save_equiv_distribution(ctx, assignment);
    
    println!("结果已保存至 output-keymap.txt, output-encode.txt, output-distribution.txt, output-equiv-dist.txt");
}

fn save_key_distribution(ctx: &OptContext, assignment: &[u8]) {
    let evaluator = Evaluator::new(ctx, assignment);
    
    let mut dist_out = String::new();
    dist_out.push_str("# 用指分布统计 (L2损失优化)\n");
    dist_out.push_str("# 键位\t实际频率%\t目标频率%\t偏差\t偏差²\n");
    
    let key_chars = [
        'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
        'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
        '_', ';', ',', '.', '/'
    ];
    
    for (key_idx, &key_char) in key_chars.iter().enumerate() {
        if key_idx >= EQUIV_TABLE_SIZE {
            break;
        }
        
        let actual_pct = evaluator.key_weighted_usage[key_idx] * 100.0 * evaluator.inv_total_key_presses;
        let target_pct = ctx.key_dist_config[key_idx].target_rate;
        let deviation = actual_pct - target_pct;
        let deviation_sq = deviation * deviation;
        
        dist_out.push_str(&format!(
            "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
            key_char, actual_pct, target_pct, deviation, deviation_sq
        ));
    }
    
    fs::write("output-distribution.txt", dist_out).unwrap();
}

fn save_equiv_distribution(ctx: &OptContext, assignment: &[u8]) {
    let evaluator = Evaluator::new(ctx, assignment);
    
    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        let (_, keys, num_keys) = ctx.calc_code_and_keys(i, assignment);
        let equiv = ctx.calc_key_avg_equiv_inline(keys, num_keys);
        let freq = ctx.char_infos[i].frequency;
        char_equivs.push((*ch, equiv, freq));
    }
    
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    
    let mut dist_out = String::new();
    let m = evaluator.get_metrics(ctx);
    
    dist_out.push_str("# 当量分布统计\n");
    dist_out.push_str(&format!("# 平均当量: {:.4}\n", m.equiv_mean));
    dist_out.push_str(&format!("# 变异系数(CV): {:.4}\n", m.equiv_cv));
    dist_out.push_str(&format!("# 标准差: {:.4}\n", m.equiv_cv * m.equiv_mean));
    dist_out.push_str("#\n");
    dist_out.push_str("# 当量最高的20个高频字 (字频>1000000):\n");
    dist_out.push_str("# 汉字\t当量\t字频\n");
    
    let mut count = 0;
    for (ch, equiv, freq) in &char_equivs {
        if *freq > 1000000 && count < 20 {
            dist_out.push_str(&format!("{}\t{:.4}\t{}\n", ch, equiv, freq));
            count += 1;
        }
    }
    
    dist_out.push_str("#\n");
    dist_out.push_str("# 当量最低的20个高频字 (字频>1000000):\n");
    
    let high_freq_chars: Vec<_> = char_equivs.iter().filter(|(_, _, f)| *f > 1000000).collect();
    let start = if high_freq_chars.len() > 20 { high_freq_chars.len() - 20 } else { 0 };
    for (ch, equiv, freq) in high_freq_chars.iter().skip(start) {
        dist_out.push_str(&format!("{}\t{:.4}\t{}\n", ch, equiv, freq));
    }
    
    fs::write("output-equiv-dist.txt", dist_out).unwrap();
}