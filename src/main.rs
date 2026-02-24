use chrono::Local;
use rand::prelude::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

// =========================================================================
// 🔧 配置区域
// =========================================================================
mod config {
    pub const FILE_FIXED: &str = "input-fixed.txt";
    pub const FILE_DYNAMIC: &str = "input-roots.txt";
    pub const FILE_SPLITS: &str = "input-division.txt";
    pub const FILE_PAIR_EQUIV: &str = "pair_equivalence.txt";
    pub const FILE_KEY_DIST: &str = "key_distribution.txt";
    pub const FILE_SIMPLE: &str = "input-simple.txt";

    pub const ALLOWED_KEYS: &str = "qwrtypsdfghjklzxcvbnm";
    pub const KEY_DISPLAY_ORDER: &str = "qwertyuiopasdfghjklzxcvbnm";

    // =========================================================================
    // 🎚️ 归一化权重配置 — 全码部分 (总和 = 1.0)
    // =========================================================================
    pub const WEIGHT_COLLISION_COUNT: f64 = 0.07;
    pub const WEIGHT_COLLISION_RATE: f64 = 0.62;
    pub const WEIGHT_EQUIVALENCE: f64 = 0.2;
    pub const WEIGHT_EQUIV_CV: f64 = 0.01;
    pub const WEIGHT_DISTRIBUTION: f64 = 0.1;

    // =========================================================================
    // 🎚️ 简码优化开关与权重
    // =========================================================================
    pub const ENABLE_SIMPLE_CODE: bool = true;

    /// 全码目标总权重 vs 简码目标总权重
    pub const WEIGHT_FULL_CODE: f64 = 0.7;
    pub const WEIGHT_SIMPLE_CODE: f64 = 0.3;

    /// 简码内部子权重（总和 = 1.0）
    pub const SIMPLE_WEIGHT_FREQ: f64 = 0.5;
    pub const SIMPLE_WEIGHT_EQUIV: f64 = 0.15;
    pub const SIMPLE_WEIGHT_DIST: f64 = 0.05;
    pub const SIMPLE_WEIGHT_COLLISION_COUNT: f64 = 0.05;
    pub const SIMPLE_WEIGHT_COLLISION_RATE: f64 = 0.25;

    pub fn validate_weights() {
        let total_full = WEIGHT_COLLISION_COUNT
            + WEIGHT_COLLISION_RATE
            + WEIGHT_EQUIVALENCE
            + WEIGHT_EQUIV_CV
            + WEIGHT_DISTRIBUTION;
        if (total_full - 1.0).abs() > 0.001 {
            eprintln!("⚠️ 警告：全码权重总和不为 1.0 (当前: {:.3})", total_full);
        }
        let total_simple = SIMPLE_WEIGHT_FREQ
            + SIMPLE_WEIGHT_EQUIV
            + SIMPLE_WEIGHT_DIST
            + SIMPLE_WEIGHT_COLLISION_COUNT
            + SIMPLE_WEIGHT_COLLISION_RATE;
        if ENABLE_SIMPLE_CODE && (total_simple - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：简码子权重总和不为 1.0 (当前: {:.3})",
                total_simple
            );
        }
        if ENABLE_SIMPLE_CODE && (WEIGHT_FULL_CODE + WEIGHT_SIMPLE_CODE - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：全码/简码总权重不为 1.0 (当前: {:.3})",
                WEIGHT_FULL_CODE + WEIGHT_SIMPLE_CODE
            );
        }
    }

    pub const NUM_THREADS: usize = 16;
    pub const TOTAL_STEPS: usize = 100_000;
    pub const TEMP_START: f64 = 100.0;
    pub const TEMP_END: f64 = 0.000001;
    pub const COMFORT_TEMP: f64 = 0.2;
    pub const COMFORT_WIDTH: f64 = 0.15;
    pub const COMFORT_SLOWDOWN: f64 = 0.8;

    pub const SWAP_PROBABILITY: f64 = 0.3;

    pub const MIN_IMPROVE_STEPS: usize = TOTAL_STEPS / 10;
    pub const PERTURB_INTERVAL: usize = TOTAL_STEPS / 20;
    pub const PERTURB_STRENGTH: f64 = 0.15;
    pub const REHEAT_FACTOR: f64 = 1.25;

    pub const MAX_PARTS: usize = 5;
}

// =========================================================================
// 🚀 基础数据结构
// =========================================================================

const KEY_SPACE: usize = 26;
const EQUIV_TABLE_SIZE: usize = 31;

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

fn pow_base(base: usize, exp: usize) -> usize {
    let mut result = 1;
    for _ in 0..exp {
        result *= base;
    }
    result
}

#[derive(Clone, Copy)]
struct ScaleConfig {
    collision_count: f64,
    collision_rate: f64,
    equivalence: f64,
    equiv_cv: f64,
    distribution: f64,
    simple_freq: f64,
    simple_equiv: f64,
    simple_dist: f64,
    simple_collision_count: f64,
    simple_collision_rate: f64,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            collision_count: 1.0,
            collision_rate: 1.0,
            equivalence: 1.0,
            equiv_cv: 1.0,
            distribution: 1.0,
            simple_freq: 1.0,
            simple_equiv: 1.0,
            simple_dist: 1.0,
            simple_collision_count: 1.0,
            simple_collision_rate: 1.0,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct KeyDistConfig {
    target_rate: f64,
    low_penalty: f64,
    high_penalty: f64,
}

#[derive(Clone)]
struct CharInfo {
    parts: Vec<u16>,
    frequency: u64,
}

const GROUP_MARKER: u16 = 1000;

#[derive(Clone)]
struct RootGroup {
    roots: Vec<String>,
    allowed_keys: Vec<u8>,
}

// =========================================================================
// 📝 简码规则
// =========================================================================

#[derive(Clone, Debug)]
struct SimpleCodeStep {
    root_selector: char,
    code_selector: char,
}

#[derive(Clone, Debug)]
struct SimpleCodeLevel {
    level: usize,
    code_num: usize,
    rule_candidates: Vec<Vec<SimpleCodeStep>>,
}

#[derive(Clone, Debug)]
struct SimpleCodeConfig {
    levels: Vec<SimpleCodeLevel>,
}

fn resolve_code_index(code_selector: char, total_codes: usize) -> Option<usize> {
    if total_codes == 0 {
        return None;
    }
    match code_selector {
        'a' => Some(0),
        'z' => {
            if total_codes >= 1 {
                Some(total_codes - 1)
            } else {
                None
            }
        }
        'b'..='y' => {
            let mid_offset = (code_selector as u8 - b'b') as usize;
            let actual_index = 1 + mid_offset;
            if total_codes >= 3 && actual_index <= total_codes - 2 {
                Some(actual_index)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn try_resolve_rule(
    rule: &[SimpleCodeStep],
    logical_roots: &[LogicalRoot],
    n_roots: usize,
) -> Option<Vec<(usize, usize)>> {
    let mut instructions: Vec<(usize, usize)> = Vec::with_capacity(rule.len());

    for step in rule {
        let root_idx = match step.root_selector {
            'A'..='Y' => {
                let idx = (step.root_selector as u8 - b'A') as usize;
                if idx >= n_roots {
                    return None;
                }
                idx
            }
            'Z' => {
                if n_roots == 0 {
                    return None;
                }
                n_roots - 1
            }
            _ => return None,
        };

        let lr = &logical_roots[root_idx];
        let n_codes = lr.full_code_parts.len();

        let code_idx = resolve_code_index(step.code_selector, n_codes)?;

        instructions.push((root_idx, code_idx));
    }

    Some(instructions)
}

#[derive(Clone, Debug)]
struct LogicalRoot {
    base_name: String,
    split_part_indices: Vec<usize>,
    full_code_parts: Vec<u16>,
}

#[derive(Clone, Debug)]
struct CharSimpleInfo {
    logical_roots: Vec<LogicalRoot>,
    level_instructions: Vec<Option<Vec<(usize, usize)>>>,
}

// =========================================================================
// 简码规则解析
// =========================================================================

fn parse_simple_code_config(path: &str) -> SimpleCodeConfig {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("⚠️ 无法读取简码配置文件 {}，简码优化将跳过", path);
            return SimpleCodeConfig { levels: vec![] };
        }
    };

    let mut num_map: HashMap<usize, usize> = HashMap::new();
    let mut rule_map: HashMap<usize, String> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.trim_end_matches(';').trim();
        if let Some(pos) = line.find(':') {
            let key = line[..pos].trim();
            let val = line[pos + 1..].trim();

            if key.starts_with("simple_") && key.ends_with("_code_num") {
                let mid = &key[7..key.len() - 9];
                if let Ok(level) = mid.parse::<usize>() {
                    if let Ok(num) = val.parse::<usize>() {
                        num_map.insert(level, num);
                    }
                }
            } else if key.starts_with("simple_") && key.ends_with("_code_rule") {
                let mid = &key[7..key.len() - 10];
                if let Ok(level) = mid.parse::<usize>() {
                    rule_map.insert(level, val.to_string());
                }
            }
        }
    }

    let mut levels = Vec::new();
    let mut all_levels: Vec<usize> = num_map.keys().copied().collect();
    all_levels.sort();

    for level in all_levels {
        let code_num = num_map[&level];
        if code_num == 0 {
            continue;
        }
        let rule_str = match rule_map.get(&level) {
            Some(s) => s.clone(),
            None => {
                eprintln!("⚠️ 简码级别 {} 缺少 rule 定义，跳过", level);
                continue;
            }
        };

        let mut rule_candidates: Vec<Vec<SimpleCodeStep>> = Vec::new();

        for candidate_str in rule_str.split(',') {
            let candidate_str = candidate_str.trim();
            if candidate_str.is_empty() {
                continue;
            }
            let chars: Vec<char> = candidate_str.chars().collect();
            if chars.len() % 2 != 0 {
                eprintln!(
                    "⚠️ 简码级别 {} 的候选规则长度不是偶数: '{}'，跳过该候选",
                    level, candidate_str
                );
                continue;
            }

            let mut rule = Vec::new();
            for chunk in chars.chunks(2) {
                rule.push(SimpleCodeStep {
                    root_selector: chunk[0],
                    code_selector: chunk[1],
                });
            }
            rule_candidates.push(rule);
        }

        if rule_candidates.is_empty() {
            eprintln!("⚠️ 简码级别 {} 没有有效的候选规则，跳过", level);
            continue;
        }

        levels.push(SimpleCodeLevel {
            level,
            code_num,
            rule_candidates,
        });
    }

    levels.sort_by_key(|l| l.level);
    SimpleCodeConfig { levels }
}

fn extract_base_name(name: &str) -> String {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..dot_pos].to_string();
        }
    }
    name.to_string()
}

fn extract_suffix_num(name: &str) -> i32 {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if let Ok(n) = suffix.parse::<i32>() {
            return n;
        }
    }
    -1
}

fn build_root_full_codes(
    fixed_roots: &HashMap<String, u8>,
    groups: &[RootGroup],
) -> HashMap<String, Vec<String>> {
    let mut all_names: Vec<String> = Vec::new();
    for name in fixed_roots.keys() {
        all_names.push(name.clone());
    }
    for g in groups {
        for name in &g.roots {
            all_names.push(name.clone());
        }
    }

    let mut grouped: HashMap<String, Vec<(i32, String)>> = HashMap::new();
    for name in &all_names {
        let base = extract_base_name(name);
        let suffix = extract_suffix_num(name);
        grouped
            .entry(base)
            .or_default()
            .push((suffix, name.clone()));
    }

    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for (base, mut entries) in grouped {
        entries.sort_by_key(|(s, _)| *s);
        let names: Vec<String> = entries.into_iter().map(|(_, n)| n).collect();
        result.insert(base, names);
    }

    result
}

fn extract_logical_roots_full(
    root_names: &[String],
    _parts: &[u16],
    root_full_codes: &HashMap<String, Vec<String>>,
    fixed_roots: &HashMap<String, u8>,
    root_to_group: &HashMap<String, usize>,
) -> Vec<LogicalRoot> {
    let mut logical_roots: Vec<LogicalRoot> = Vec::new();

    for (idx, name) in root_names.iter().enumerate() {
        let base = extract_base_name(name);
        let suffix = extract_suffix_num(name);

        if suffix <= 0 {
            let full_names = root_full_codes
                .get(&base)
                .cloned()
                .unwrap_or_else(|| vec![name.clone()]);

            let full_code_parts: Vec<u16> = full_names
                .iter()
                .map(|n| {
                    if let Some(&key) = fixed_roots.get(n) {
                        key as u16
                    } else if let Some(&gi) = root_to_group.get(n) {
                        gi as u16 + GROUP_MARKER
                    } else {
                        0u16
                    }
                })
                .collect();

            logical_roots.push(LogicalRoot {
                base_name: base,
                split_part_indices: vec![idx],
                full_code_parts,
            });
        } else {
            let mut attached = false;
            for lr in logical_roots.iter_mut().rev() {
                if lr.base_name == base {
                    lr.split_part_indices.push(idx);
                    attached = true;
                    break;
                }
            }
            if !attached {
                let full_names = root_full_codes
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| vec![name.clone()]);
                let full_code_parts: Vec<u16> = full_names
                    .iter()
                    .map(|n| {
                        if let Some(&key) = fixed_roots.get(n) {
                            key as u16
                        } else if let Some(&gi) = root_to_group.get(n) {
                            gi as u16 + GROUP_MARKER
                        } else {
                            0u16
                        }
                    })
                    .collect();

                logical_roots.push(LogicalRoot {
                    base_name: base,
                    split_part_indices: vec![idx],
                    full_code_parts,
                });
            }
        }
    }

    logical_roots
}

fn compute_level_instructions(
    logical_roots: &[LogicalRoot],
    levels: &[SimpleCodeLevel],
) -> Vec<Option<Vec<(usize, usize)>>> {
    let n_roots = logical_roots.len();

    levels
        .iter()
        .map(|level| {
            for rule in &level.rule_candidates {
                if let Some(instructions) = try_resolve_rule(rule, logical_roots, n_roots) {
                    return Some(instructions);
                }
            }
            None
        })
        .collect()
}

// =========================================================================
// 主上下文
// =========================================================================

struct OptContext {
    num_groups: usize,
    root_to_group: HashMap<String, usize>,
    group_to_chars: Vec<Vec<usize>>,
    char_infos: Vec<CharInfo>,
    raw_splits: Vec<(char, Vec<String>, u64)>,
    groups: Vec<RootGroup>,
    fixed_roots: HashMap<String, u8>,
    equiv_table: [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
    key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    total_frequency: u64,
    code_base: usize,
    max_parts: usize,
    code_space: usize,
    scale_config: ScaleConfig,
    simple_config: SimpleCodeConfig,
    char_simple_infos: Vec<CharSimpleInfo>,
    group_to_simple_affected: Vec<HashSet<usize>>,
    root_full_codes: HashMap<String, Vec<String>>,
}

impl OptContext {
    fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        simple_config: SimpleCodeConfig,
    ) -> Self {
        let mut root_to_group: HashMap<String, usize> = HashMap::new();
        for (gi, g) in groups.iter().enumerate() {
            for r in &g.roots {
                root_to_group.insert(r.clone(), gi);
            }
        }

        let root_full_codes = build_root_full_codes(fixed_roots, groups);

        let num_groups = groups.len();
        let mut group_to_chars = vec![Vec::new(); num_groups];
        let mut char_infos = Vec::with_capacity(splits.len());
        let mut total_frequency = 0u64;
        let mut max_parts = 0usize;

        let mut char_simple_infos = Vec::with_capacity(splits.len());
        let mut group_to_simple_affected: Vec<HashSet<usize>> = vec![HashSet::new(); num_groups];

        for (ci, (_, roots, freq)) in splits.iter().enumerate() {
            let mut info = CharInfo {
                parts: Vec::with_capacity(roots.len()),
                frequency: *freq,
            };

            let mut seen_groups = HashSet::new();

            for root in roots {
                if let Some(&key) = fixed_roots.get(root) {
                    info.parts.push(key as u16);
                } else if let Some(&gi) = root_to_group.get(root) {
                    info.parts.push(gi as u16 + GROUP_MARKER);
                    seen_groups.insert(gi);
                }
            }

            if info.parts.len() > max_parts {
                max_parts = info.parts.len();
            }

            for &gi in &seen_groups {
                group_to_chars[gi].push(ci);
            }

            total_frequency += freq;

            let logical_roots = extract_logical_roots_full(
                roots,
                &info.parts,
                &root_full_codes,
                fixed_roots,
                &root_to_group,
            );
            let level_instructions =
                compute_level_instructions(&logical_roots, &simple_config.levels);

            if config::ENABLE_SIMPLE_CODE {
                for level_cfg in &simple_config.levels {
                    for rule in &level_cfg.rule_candidates {
                        if let Some(instructions) =
                            try_resolve_rule(rule, &logical_roots, logical_roots.len())
                        {
                            for &(root_idx, code_idx) in &instructions {
                                let lr = &logical_roots[root_idx];
                                if code_idx < lr.full_code_parts.len() {
                                    let part = lr.full_code_parts[code_idx];
                                    if part >= GROUP_MARKER {
                                        let gi = (part - GROUP_MARKER) as usize;
                                        group_to_simple_affected[gi].insert(ci);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            char_simple_infos.push(CharSimpleInfo {
                logical_roots,
                level_instructions,
            });

            char_infos.push(info);
        }

        let code_base = EQUIV_TABLE_SIZE + 1;
        let code_space = pow_base(code_base, max_parts);

        Self {
            num_groups,
            root_to_group,
            group_to_chars,
            char_infos,
            raw_splits: splits.to_vec(),
            groups: groups.to_vec(),
            fixed_roots: fixed_roots.clone(),
            equiv_table,
            key_dist_config,
            total_frequency,
            code_base,
            max_parts,
            code_space,
            scale_config,
            simple_config,
            char_simple_infos,
            group_to_simple_affected,
            root_full_codes,
        }
    }

    #[inline(always)]
    fn resolve_key(&self, part: u16, assignment: &[u8]) -> u8 {
        if part >= GROUP_MARKER {
            assignment[(part - GROUP_MARKER) as usize]
        } else {
            part as u8
        }
    }

    #[inline(always)]
    fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in &info.parts {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    #[inline(always)]
    fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> f64 {
        let info = &self.char_infos[ci];
        let n = info.parts.len();
        if n == 0 {
            return 0.0;
        }

        let mut prev_key = self.resolve_key(info.parts[0], assignment) as usize;
        let mut total = 0.0;

        for i in 1..n {
            let cur_key = self.resolve_key(info.parts[i], assignment) as usize;
            total += self.equiv_table[prev_key][cur_key];
            prev_key = cur_key;
        }
        total += self.equiv_table[prev_key][KEY_SPACE];
        total / n as f64
    }

    #[inline]
    fn calc_simple_code(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<usize> {
        let si = &self.char_simple_infos[ci];
        let instr = si.level_instructions.get(level_idx)?.as_ref()?;

        let mut code = 0usize;
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                return None;
            }
            let part = lr.full_code_parts[code_idx];
            let k = self.resolve_key(part, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        Some(code)
    }

    fn get_simple_keys(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<Vec<u8>> {
        let si = &self.char_simple_infos[ci];
        let instr = si.level_instructions.get(level_idx)?.as_ref()?;

        let mut keys = Vec::with_capacity(instr.len());
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                return None;
            }
            let part = lr.full_code_parts[code_idx];
            keys.push(self.resolve_key(part, assignment));
        }
        Some(keys)
    }

    #[inline]
    fn calc_simple_equiv(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> f64 {
        let si = &self.char_simple_infos[ci];
        let instr = match si.level_instructions.get(level_idx) {
            Some(Some(ref v)) => v,
            _ => return 0.0,
        };
        let n = instr.len();
        if n == 0 {
            return 0.0;
        }

        let (root_idx0, code_idx0) = instr[0];
        let lr0 = &si.logical_roots[root_idx0];
        if code_idx0 >= lr0.full_code_parts.len() {
            return 0.0;
        }
        let mut prev_key = self.resolve_key(lr0.full_code_parts[code_idx0], assignment) as usize;
        let mut total = 0.0;

        for i in 1..n {
            let (ri, ci_code) = instr[i];
            let lr = &si.logical_roots[ri];
            if ci_code >= lr.full_code_parts.len() {
                return 0.0;
            }
            let cur_key = self.resolve_key(lr.full_code_parts[ci_code], assignment) as usize;
            total += self.equiv_table[prev_key][cur_key];
            prev_key = cur_key;
        }
        total += self.equiv_table[prev_key][KEY_SPACE];
        total / n as f64
    }
}

// =========================================================================
// ⚡ 评估器
// =========================================================================

#[derive(Clone, Copy, Default)]
struct Metrics {
    collision_count: usize,
    collision_rate: f64,
    equiv_mean: f64,
    equiv_cv: f64,
    dist_deviation: f64,
}

#[derive(Clone, Copy, Default)]
struct SimpleMetrics {
    weighted_freq_coverage: f64,
    equiv_mean: f64,
    dist_deviation: f64,
    collision_count: usize,
    collision_rate: f64,
}

// ─── 简码评估器 ───

struct SimpleLevelTracker {
    code_num: usize,
    code_to_candidates: HashMap<usize, Vec<(usize, u64)>>,
    covered_freq: u64,
    equiv_weighted: f64,
    equiv_freq_sum: u64,
    key_usage: [f64; EQUIV_TABLE_SIZE],
    key_presses: f64,
    assigned_chars: HashSet<usize>,
}

struct SimpleEvaluator {
    levels: Vec<SimpleLevelTracker>,
    /// 所有出简的汉字集合（跨级别）
    all_assigned_chars: HashSet<usize>,
    /// 简码重码数：全码桶去掉出简字后仍有重码的数量
    simple_collision_count: usize,
    /// 简码重码率：全码桶去掉出简字后仍被重码的字频 / 总频
    simple_collision_rate: f64,
    cached_simple_score: f64,
    simple_score_dirty: bool,
}

impl SimpleEvaluator {
    fn new(
        ctx: &OptContext,
        assignment: &[u8],
        full_code_to_chars: &HashMap<usize, Vec<usize>>,
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
                assigned_chars: HashSet::new(),
            })
            .collect();

        let n_chars = ctx.char_infos.len();
        let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
        sorted_chars.sort_by(|&a, &b| {
            ctx.char_infos[b]
                .frequency
                .cmp(&ctx.char_infos[a].frequency)
        });

        let mut globally_assigned: HashSet<usize> = HashSet::new();

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
                globally_assigned.insert(ci);
            }
        }

        // 计算简码重码
        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, full_code_to_chars, &globally_assigned);

        let mut se = Self {
            levels,
            all_assigned_chars: globally_assigned,
            simple_collision_count: sc_count,
            simple_collision_rate: sc_rate,
            cached_simple_score: 0.0,
            simple_score_dirty: true,
        };
        se.cached_simple_score = se.compute_simple_score(ctx);
        se.simple_score_dirty = false;
        se
    }

    fn build_level(
        ctx: &OptContext,
        assignment: &[u8],
        level: &mut SimpleLevelTracker,
        li: usize,
        sorted_chars: &[usize],
        excluded: &HashSet<usize>,
    ) {
        level.code_to_candidates.clear();
        level.covered_freq = 0;
        level.equiv_weighted = 0.0;
        level.equiv_freq_sum = 0;
        level.key_usage = [0.0; EQUIV_TABLE_SIZE];
        level.key_presses = 0.0;
        level.assigned_chars.clear();

        for &ci in sorted_chars {
            if excluded.contains(&ci) {
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
                    .filter(|(ci, _)| !excluded.contains(ci))
                    .map(|&(ci, _)| ci)
            })
            .collect();

        for ci in &all_assigned {
            let ci = *ci;
            let freq = ctx.char_infos[ci].frequency;
            level.covered_freq += freq;
            level.assigned_chars.insert(ci);

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
        full_code_to_chars: &HashMap<usize, Vec<usize>>,
        assigned: &HashSet<usize>,
    ) -> (usize, f64) {
        let mut total_collision_count: usize = 0;
        let mut total_collision_freq: u64 = 0;

        for chars in full_code_to_chars.values() {
            // 过滤掉已出简的字
            let remaining: Vec<usize> = chars
                .iter()
                .filter(|ci| !assigned.contains(ci))
                .copied()
                .collect();

            let n = remaining.len();
            if n >= 2 {
                // 重码数 = n - 1
                total_collision_count += n - 1;
                // 重码率：桶中频率总和 - 最高频率的那个字
                let mut max_freq = 0u64;
                let mut sum_freq = 0u64;
                for &ci in &remaining {
                    let f = ctx.char_infos[ci].frequency;
                    sum_freq += f;
                    if f > max_freq {
                        max_freq = f;
                    }
                }
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

    fn full_rebuild(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_code_to_chars: &HashMap<usize, Vec<usize>>,
    ) {
        let n_chars = ctx.char_infos.len();
        let n_levels = ctx.simple_config.levels.len();

        let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
        sorted_chars.sort_by(|&a, &b| {
            ctx.char_infos[b]
                .frequency
                .cmp(&ctx.char_infos[a].frequency)
        });

        let mut globally_assigned: HashSet<usize> = HashSet::new();

        for li in 0..n_levels {
            Self::build_level(
                ctx,
                assignment,
                &mut self.levels[li],
                li,
                &sorted_chars,
                &globally_assigned,
            );
            for &ci in &self.levels[li].assigned_chars {
                globally_assigned.insert(ci);
            }
        }

        let (sc_count, sc_rate) =
            Self::compute_simple_collisions(ctx, full_code_to_chars, &globally_assigned);
        self.all_assigned_chars = globally_assigned;
        self.simple_collision_count = sc_count;
        self.simple_collision_rate = sc_rate;

        self.simple_score_dirty = true;
    }

    fn compute_simple_score(&self, ctx: &OptContext) -> f64 {
        let sm = self.get_simple_metrics(ctx);

        let freq_loss = (1.0 - sm.weighted_freq_coverage) * ctx.scale_config.simple_freq;
        let equiv_loss = sm.equiv_mean * ctx.scale_config.simple_equiv;
        let dist_loss = sm.dist_deviation * ctx.scale_config.simple_dist;
        let collision_count_loss =
            sm.collision_count as f64 * ctx.scale_config.simple_collision_count;
        let collision_rate_loss = sm.collision_rate * ctx.scale_config.simple_collision_rate;

        config::SIMPLE_WEIGHT_FREQ * freq_loss
            + config::SIMPLE_WEIGHT_EQUIV * equiv_loss
            + config::SIMPLE_WEIGHT_DIST * dist_loss
            + config::SIMPLE_WEIGHT_COLLISION_COUNT * collision_count_loss
            + config::SIMPLE_WEIGHT_COLLISION_RATE * collision_rate_loss
    }

    fn get_simple_score(&mut self, ctx: &OptContext) -> f64 {
        if self.simple_score_dirty {
            self.cached_simple_score = self.compute_simple_score(ctx);
            self.simple_score_dirty = false;
        }
        self.cached_simple_score
    }

    fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
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

// ─── 主评估器 ───

struct Evaluator {
    current_codes: Vec<usize>,
    current_equiv_val: Vec<f64>,
    code_to_chars: HashMap<usize, Vec<usize>>,

    total_collisions: usize,
    collision_frequency: u64,

    total_equiv_weighted: f64,
    total_equiv_sq_weighted: f64,

    key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    total_key_presses: f64,

    total_frequency: u64,
    inv_total_frequency: f64,
    inv_total_key_presses: f64,

    cached_score: f64,
    score_dirty: bool,

    simple_eval: Option<SimpleEvaluator>,
}

impl Evaluator {
    fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let n = ctx.char_infos.len();
        let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
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
            code_to_chars.entry(code).or_default().push(ci);

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
        for chars in code_to_chars.values() {
            let cnt = chars.len();
            if cnt >= 2 {
                total_collisions += cnt - 1;
                collision_frequency += Self::bucket_cf(ctx, chars);
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

        let simple_eval = if config::ENABLE_SIMPLE_CODE && !ctx.simple_config.levels.is_empty() {
            Some(SimpleEvaluator::new(ctx, assignment, &code_to_chars))
        } else {
            None
        };

        let mut e = Self {
            current_codes,
            current_equiv_val,
            code_to_chars,
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

    #[inline]
    fn bucket_cf(ctx: &OptContext, chars: &[usize]) -> u64 {
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

    #[inline]
    fn update_char(&mut self, ctx: &OptContext, assignment: &[u8], ci: usize) {
        let old_code = self.current_codes[ci];
        let new_code = ctx.calc_code_only(ci, assignment);
        if old_code == new_code {
            return;
        }

        let freq_f = ctx.char_infos[ci].frequency as f64;

        let old_eq = self.current_equiv_val[ci];
        let new_eq = ctx.calc_equiv_from_parts(ci, assignment);
        self.total_equiv_weighted += (new_eq - old_eq) * freq_f;
        self.total_equiv_sq_weighted += (new_eq * new_eq - old_eq * old_eq) * freq_f;
        self.current_equiv_val[ci] = new_eq;

        let (dcc1, dcf1, old_empty) = {
            let b = self.code_to_chars.get_mut(&old_code).unwrap();
            let n = b.len();
            let cc0 = n.saturating_sub(1);
            let cf0 = if n >= 2 { Self::bucket_cf(ctx, b) } else { 0 };
            if let Some(pos) = b.iter().position(|&c| c == ci) {
                b.swap_remove(pos);
            }
            let m = b.len();
            let cc1 = m.saturating_sub(1);
            let cf1 = if m >= 2 { Self::bucket_cf(ctx, b) } else { 0 };
            (
                cc1 as isize - cc0 as isize,
                cf1 as i64 - cf0 as i64,
                b.is_empty(),
            )
        };
        if old_empty {
            self.code_to_chars.remove(&old_code);
        }

        let (dcc2, dcf2) = {
            let b = self.code_to_chars.entry(new_code).or_default();
            let n = b.len();
            let cc0 = n.saturating_sub(1);
            let cf0 = if n >= 2 { Self::bucket_cf(ctx, b) } else { 0 };
            b.push(ci);
            let m = b.len();
            let cc1 = m.saturating_sub(1);
            let cf1 = if m >= 2 { Self::bucket_cf(ctx, b) } else { 0 };
            (cc1 as isize - cc0 as isize, cf1 as i64 - cf0 as i64)
        };

        self.total_collisions = (self.total_collisions as isize + dcc1 + dcc2) as usize;
        self.collision_frequency = (self.collision_frequency as i64 + dcf1 + dcf2) as u64;
        self.current_codes[ci] = new_code;
    }

    #[inline(always)]
    fn compute_full_score(&self, ctx: &OptContext) -> f64 {
        let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
        let weighted_equiv = self.total_equiv_weighted * self.inv_total_frequency;
        let equiv_cv = self.calc_equiv_cv();
        let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);

        let scaled = (
            self.total_collisions as f64 * ctx.scale_config.collision_count,
            collision_rate * ctx.scale_config.collision_rate,
            weighted_equiv * ctx.scale_config.equivalence,
            equiv_cv * ctx.scale_config.equiv_cv,
            dist_deviation * ctx.scale_config.distribution,
        );

        config::WEIGHT_COLLISION_COUNT * scaled.0
            + config::WEIGHT_COLLISION_RATE * scaled.1
            + config::WEIGHT_EQUIVALENCE * scaled.2
            + config::WEIGHT_EQUIV_CV * scaled.3
            + config::WEIGHT_DISTRIBUTION * scaled.4
    }

    #[inline(always)]
    fn compute_score(&self, ctx: &OptContext) -> f64 {
        let full_score = self.compute_full_score(ctx);

        if config::ENABLE_SIMPLE_CODE {
            if let Some(ref se) = self.simple_eval {
                let simple_score = se.cached_simple_score;
                config::WEIGHT_FULL_CODE * full_score + config::WEIGHT_SIMPLE_CODE * simple_score
            } else {
                full_score
            }
        } else {
            full_score
        }
    }

    #[inline(always)]
    fn get_score(&mut self, ctx: &OptContext) -> f64 {
        if self.score_dirty {
            self.cached_score = self.compute_score(ctx);
            self.score_dirty = false;
        }
        self.cached_score
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
    fn calc_distribution_deviation(&self, kdc: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
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

    fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        Metrics {
            collision_count: self.total_collisions,
            collision_rate: self.collision_frequency as f64 * self.inv_total_frequency,
            equiv_mean: self.total_equiv_weighted * self.inv_total_frequency,
            equiv_cv: self.calc_equiv_cv(),
            dist_deviation: self.calc_distribution_deviation(&ctx.key_dist_config),
        }
    }

    fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        if let Some(ref se) = self.simple_eval {
            se.get_simple_metrics(ctx)
        } else {
            SimpleMetrics::default()
        }
    }

    fn has_simple_impact(&self, ctx: &OptContext, group: usize) -> bool {
        if !config::ENABLE_SIMPLE_CODE || self.simple_eval.is_none() {
            return false;
        }
        !ctx.group_to_simple_affected[group].is_empty()
    }

    fn rebuild_simple(&mut self, ctx: &OptContext, assignment: &[u8]) {
        if let Some(ref mut se) = self.simple_eval {
            se.full_rebuild(ctx, assignment, &self.code_to_chars);
            se.cached_simple_score = se.compute_simple_score(ctx);
            se.simple_score_dirty = false;
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
        let needs_simple = self.has_simple_impact(ctx, r);

        for &ci in &ctx.group_to_chars[r] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r {
                    self.key_weighted_usage[old_key as usize] -= freq_f;
                    self.key_weighted_usage[new_key as usize] += freq_f;
                }
            }
        }

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
            for &ci in &ctx.group_to_chars[r] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in &ctx.char_infos[ci].parts {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r {
                        self.key_weighted_usage[new_key as usize] -= freq_f;
                        self.key_weighted_usage[old_key as usize] += freq_f;
                    }
                }
            }

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
        let needs_simple = self.has_simple_impact(ctx, r1) || self.has_simple_impact(ctx, r2);

        for &ci in &ctx.group_to_chars[r1] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r1 {
                    self.key_weighted_usage[k1 as usize] -= freq_f;
                    self.key_weighted_usage[k2 as usize] += freq_f;
                }
            }
        }
        for &ci in &ctx.group_to_chars[r2] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r2 {
                    self.key_weighted_usage[k2 as usize] -= freq_f;
                    self.key_weighted_usage[k1 as usize] += freq_f;
                }
            }
        }

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
            for &ci in &ctx.group_to_chars[r1] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in &ctx.char_infos[ci].parts {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r1 {
                        self.key_weighted_usage[k2 as usize] -= freq_f;
                        self.key_weighted_usage[k1 as usize] += freq_f;
                    }
                }
            }
            for &ci in &ctx.group_to_chars[r2] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in &ctx.char_infos[ci].parts {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == r2 {
                        self.key_weighted_usage[k1 as usize] -= freq_f;
                        self.key_weighted_usage[k2 as usize] += freq_f;
                    }
                }
            }

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

// =========================================================================
// 🧠 模拟退火
// =========================================================================

fn smart_init(ctx: &OptContext) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_groups];
    let mut rng = thread_rng();

    let mut group_freq: Vec<(usize, usize)> = ctx
        .group_to_chars
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    group_freq.sort_by(|a, b| b.1.cmp(&a.1));

    let max_ki = config::ALLOWED_KEYS
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

fn simulated_annealing(
    ctx: &OptContext,
    thread_id: usize,
) -> (Vec<u8>, f64, Metrics, SimpleMetrics) {
    let mut rng = thread_rng();

    let mut assignment = smart_init(ctx);
    if rng.gen_bool(0.5) {
        for i in 0..assignment.len() {
            if rng.gen_bool(0.1) {
                let allowed = &ctx.groups[i].allowed_keys;
                assignment[i] = allowed[rng.gen_range(0..allowed.len())];
            }
        }
    }

    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);
    let mut best_simple_metrics = evaluator.get_simple_metrics(ctx);

    let steps = config::TOTAL_STEPS;

    let schedule = TemperatureSchedule::build(
        config::TEMP_START,
        config::TEMP_END,
        config::COMFORT_TEMP,
        config::COMFORT_WIDTH,
        config::COMFORT_SLOWDOWN,
    );

    if thread_id == 0 {
        schedule.print_preview(steps);
    }

    let mut temp_multiplier = 1.0f64;
    let reheat_decay = if config::MIN_IMPROVE_STEPS > 0 {
        (0.01f64).powf(1.0 / config::MIN_IMPROVE_STEPS as f64)
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

    for step in 0..steps {
        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        if rng.gen::<f64>() < config::SWAP_PROBABILITY && n_groups >= 2 {
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

        let current_score = evaluator.get_score(ctx);
        if current_score < best_score {
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            best_simple_metrics = evaluator.get_simple_metrics(ctx);
            steps_since_improve = 0;

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

        if steps_since_improve > config::MIN_IMPROVE_STEPS {
            temp_multiplier = config::REHEAT_FACTOR;
            steps_since_improve = 0;

            if thread_id == 0 {
                println!(
                    "   [T0] 步数 {}: Reheat ×{:.1} (基温 {:.6})",
                    step,
                    config::REHEAT_FACTOR,
                    base_temp
                );
            }
        }

        if step > 0
            && step % config::PERTURB_INTERVAL == 0
            && base_temp < config::COMFORT_TEMP * 0.01
        {
            let n_perturb = (n_groups as f64 * config::PERTURB_STRENGTH) as usize;
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

// =========================================================================
// 🌡️ 降温曲线
// =========================================================================

const SCHEDULE_LUT_SIZE: usize = 100_000;

struct TemperatureSchedule {
    lut: Vec<f64>,
    comfort_progress: f64,
}

impl TemperatureSchedule {
    fn build(t_start: f64, t_end: f64, comfort_temp: f64, width: f64, slowdown: f64) -> Self {
        let comfort_p = if t_start <= t_end || comfort_temp >= t_start {
            0.0
        } else if comfort_temp <= t_end {
            1.0
        } else {
            (comfort_temp / t_start).ln() / (t_end / t_start).ln()
        };

        let n = SCHEDULE_LUT_SIZE;
        let mut cumulative = vec![0.0f64; n + 1];
        for i in 1..=n {
            let p = i as f64 / n as f64;
            let dp = p - comfort_p;
            let gaussian = (-dp * dp / (2.0 * width * width)).exp();
            let speed = 1.0 - slowdown * gaussian;
            cumulative[i] = cumulative[i - 1] + speed;
        }

        let total = cumulative[n];
        let mut lut = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let q = if total > 0.0 {
                cumulative[i] / total
            } else {
                i as f64 / n as f64
            };
            let temp = t_start * (t_end / t_start).powf(q);
            lut.push(temp);
        }

        Self {
            lut,
            comfort_progress: comfort_p,
        }
    }

    #[inline(always)]
    fn get(&self, step: usize, total_steps: usize) -> f64 {
        if total_steps == 0 {
            return self.lut[0];
        }
        let idx_f = step as f64 / total_steps as f64 * SCHEDULE_LUT_SIZE as f64;
        let idx = idx_f.floor() as usize;
        if idx >= SCHEDULE_LUT_SIZE {
            return self.lut[SCHEDULE_LUT_SIZE];
        }
        let frac = idx_f - idx as f64;
        self.lut[idx] + (self.lut[idx + 1] - self.lut[idx]) * frac
    }

    fn print_preview(&self, total_steps: usize) {
        println!("   🌡️ 降温曲线预览:");
        println!(
            "   舒适温度: {:.6} (进度 {:.1}% 处)",
            config::COMFORT_TEMP,
            self.comfort_progress * 100.0
        );
        println!(
            "   舒适区宽度: {:.2}, 减速深度: {:.0}%",
            config::COMFORT_WIDTH,
            config::COMFORT_SLOWDOWN * 100.0
        );
        println!("   ┌──────────────────────────────────────────────────────");

        let rows = 20;
        let bar_width = 50;
        let log_start = config::TEMP_START.ln();
        let log_end = config::TEMP_END.ln();
        let log_range = log_start - log_end;

        for i in 0..=rows {
            let step = total_steps * i / rows;
            let temp = self.get(step, total_steps);
            let log_pos = if log_range > 0.0 {
                ((temp.ln() - log_end) / log_range * bar_width as f64) as usize
            } else {
                0
            };
            let bar_len = log_pos.min(bar_width);
            let bar: String = "█".repeat(bar_len);
            let marker =
                if (i as f64 / rows as f64 - self.comfort_progress).abs() < 0.5 / rows as f64 {
                    " ◄ 舒适区"
                } else {
                    ""
                };
            println!(
                "   │{:>3}% T={:.2e} │{}{}",
                i * 100 / rows,
                temp,
                bar,
                marker
            );
        }
        println!("   └──────────────────────────────────────────────────────");
    }
}

// =========================================================================
// 📂 文件加载
// =========================================================================

fn load_fixed(path: &str) -> (HashMap<String, u8>, Vec<RootGroup>) {
    let content = fs::read_to_string(path).expect("无法读取固定字根文件");
    let mut truly_fixed: HashMap<String, u8> = HashMap::new();
    let mut constrained: Vec<RootGroup> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let roots: Vec<String> = parts[0].split_whitespace().map(|s| s.to_string()).collect();
            if roots.is_empty() {
                continue;
            }
            let keys: Vec<u8> = parts[1]
                .split_whitespace()
                .filter_map(|s| {
                    s.chars()
                        .next()
                        .and_then(char_to_key_index)
                        .map(|i| i as u8)
                })
                .collect();

            if keys.len() == 1 {
                for root in roots {
                    truly_fixed.insert(root, keys[0]);
                }
            } else if keys.len() > 1 {
                constrained.push(RootGroup {
                    roots,
                    allowed_keys: keys,
                });
            }
        }
    }
    (truly_fixed, constrained)
}

fn load_dynamic(path: &str, constrained: &[RootGroup]) -> Vec<RootGroup> {
    let global_allowed: Vec<u8> = config::ALLOWED_KEYS
        .chars()
        .filter_map(char_to_key_index)
        .map(|i| i as u8)
        .collect();

    let content = fs::read_to_string(path).expect("无法读取动态字根文件");

    let mut existing: HashSet<String> = HashSet::new();
    for g in constrained {
        for r in &g.roots {
            existing.insert(r.clone());
        }
    }

    let mut groups: Vec<RootGroup> = constrained.to_vec();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let roots: Vec<String> = line
            .split_whitespace()
            .map(|s| s.to_string())
            .filter(|s| !existing.contains(s))
            .collect();

        if roots.is_empty() {
            continue;
        }

        let mut merged = false;
        for g in &mut groups {
            if roots.iter().any(|r| g.roots.contains(r)) {
                for r in &roots {
                    if !g.roots.contains(r) && !existing.contains(r) {
                        g.roots.push(r.clone());
                        existing.insert(r.clone());
                    }
                }
                merged = true;
                break;
            }
        }

        if !merged {
            for r in &roots {
                existing.insert(r.clone());
            }
            groups.push(RootGroup {
                roots,
                allowed_keys: global_allowed.clone(),
            });
        }
    }

    groups
}

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
            let roots: Vec<String> = parts[1].split_whitespace().map(|s| s.to_string()).collect();
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
            println!("警告: 无法读取当量文件 {}，使用默认值0", path);
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
            let chars: Vec<char> = parts[0].chars().collect();
            if chars.len() == 2 {
                if let (Some(k1), Some(k2)) =
                    (char_to_key_index(chars[0]), char_to_key_index(chars[1]))
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
    let mut cfg = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("警告: 无法读取用指分布文件 {}，使用默认值", path);
            return cfg;
        }
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            if let Some(ki) = parts[0].chars().next().and_then(char_to_key_index) {
                if ki < EQUIV_TABLE_SIZE {
                    cfg[ki] = KeyDistConfig {
                        target_rate: parts[1].trim().parse().unwrap_or(0.0),
                        low_penalty: parts[2].trim().parse().unwrap_or(0.0),
                        high_penalty: parts[3].trim().parse().unwrap_or(0.0),
                    };
                }
            }
        }
    }
    cfg
}

// =========================================================================
// 📐 自动校准
// =========================================================================

fn calibrate_scales(initial_metrics: &Metrics, initial_simple: &SimpleMetrics) -> ScaleConfig {
    let eps = 1e-9;

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

    let base = if active_count <= 1 {
        ScaleConfig::default()
    } else {
        ScaleConfig {
            collision_count: 1.0 / (initial_metrics.collision_count as f64 + eps),
            collision_rate: 1.0 / (initial_metrics.collision_rate + eps),
            equivalence: 1.0 / (initial_metrics.equiv_mean + eps),
            equiv_cv: 1.0 / (initial_metrics.equiv_cv + eps),
            distribution: 1.0 / (initial_metrics.dist_deviation + eps),
            ..ScaleConfig::default()
        }
    };

    if !config::ENABLE_SIMPLE_CODE {
        return base;
    }

    let freq_coverage_loss = 1.0 - initial_simple.weighted_freq_coverage;
    ScaleConfig {
        simple_freq: 1.0 / (freq_coverage_loss + eps),
        simple_equiv: 1.0 / (initial_simple.equiv_mean + eps),
        simple_dist: 1.0 / (initial_simple.dist_deviation + eps),
        simple_collision_count: 1.0 / (initial_simple.collision_count as f64 + eps),
        simple_collision_rate: 1.0 / (initial_simple.collision_rate + eps),
        ..base
    }
}

// =========================================================================
// 🔍 校验
// =========================================================================

fn validate_roots(
    splits: &[(char, Vec<String>, u64)],
    fixed: &HashMap<String, u8>,
    groups: &[RootGroup],
) -> (bool, Vec<String>, HashMap<String, Vec<char>>) {
    let mut defined: HashSet<String> = HashSet::new();
    for r in fixed.keys() {
        defined.insert(r.clone());
    }
    for g in groups {
        for r in &g.roots {
            defined.insert(r.clone());
        }
    }

    let mut used: HashMap<String, Vec<char>> = HashMap::new();
    for (ch, roots, _) in splits {
        for r in roots {
            used.entry(r.clone()).or_default().push(*ch);
        }
    }

    let mut missing = Vec::new();
    let mut examples: HashMap<String, Vec<char>> = HashMap::new();
    for (root, chars) in &used {
        if !defined.contains(root) {
            missing.push(root.clone());
            examples.insert(root.clone(), chars.iter().take(10).copied().collect());
        }
    }
    missing.sort();
    (missing.is_empty(), missing, examples)
}

fn check_validation(
    splits: &[(char, Vec<String>, u64)],
    fixed: &HashMap<String, u8>,
    groups: &[RootGroup],
) -> bool {
    println!("\n🔍 正在校验字根定义...");
    let (valid, missing, examples) = validate_roots(splits, fixed, groups);
    if valid {
        println!("✅ 校验通过");
        return true;
    }
    println!("❌ 校验失败：发现 {} 个未定义字根！", missing.len());
    let sep = "=".repeat(60);
    println!("{}", sep);
    println!("{:<15} {}", "缺失字根", "使用示例");
    println!("{}", "-".repeat(60));
    for root in &missing {
        let ex = examples.get(root).unwrap();
        let s: String = ex.iter().collect();
        let more = if ex.len() >= 10 { " ..." } else { "" };
        println!("{:<15} {}{}", root, s, more);
    }
    println!("{}", sep);

    let mut report = format!("# 缺失字根报告 ({} 个)\n", missing.len());
    for root in &missing {
        let ex = examples.get(root).unwrap();
        let s: String = ex.iter().collect();
        report.push_str(&format!("{}\t{}\n", root, s));
    }
    fs::write("missing-roots.txt", report).unwrap();
    println!("缺失字根列表已保存至 missing-roots.txt");
    false
}

// =========================================================================
// 📤 输出
// =========================================================================

fn count_root_usage(ctx: &OptContext) -> HashMap<String, u64> {
    let mut usage: HashMap<String, u64> = HashMap::new();
    for (_, roots, freq) in &ctx.raw_splits {
        let mut seen_bases: HashSet<String> = HashSet::new();
        for root in roots {
            let base = extract_base_name(root);
            if seen_bases.insert(base.clone()) {
                *usage.entry(base).or_default() += freq;
            }
        }
    }
    usage
}

fn build_root_encodings_sorted(
    fixed: &HashMap<String, u8>,
    groups: &[RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) -> Vec<(String, Vec<u8>)> {
    let mut all_roots: Vec<(String, u8)> = Vec::new();

    for (root, &key) in fixed {
        all_roots.push((root.clone(), key));
    }
    for (gi, group) in groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            all_roots.push((root.clone(), key));
        }
    }

    let mut grouped: HashMap<String, Vec<(String, u8)>> = HashMap::new();
    for (name, key) in all_roots {
        let base = extract_base_name(&name);
        grouped.entry(base).or_default().push((name, key));
    }

    let mut result: Vec<(String, Vec<u8>)> = Vec::new();
    for (base, mut entries) in grouped {
        entries.sort_by(|a, b| {
            let sa = extract_suffix_num(&a.0);
            let sb = extract_suffix_num(&b.0);
            sa.cmp(&sb)
        });
        let keys: Vec<u8> = entries.iter().map(|(_, k)| *k).collect();
        result.push((base, keys));
    }

    result.sort_by(|a, b| {
        let ua = root_usage.get(&a.0).copied().unwrap_or(0);
        let ub = root_usage.get(&b.0).copied().unwrap_or(0);
        ub.cmp(&ua).then_with(|| a.0.cmp(&b.0))
    });

    result
}

fn format_encoding(keys: &[u8]) -> String {
    let mut s = String::with_capacity(keys.len());
    for (i, &k) in keys.iter().enumerate() {
        let c = key_to_char(k);
        if i == 0 {
            s.extend(c.to_uppercase());
        } else {
            s.push(c);
        }
    }
    s
}

fn write_keymap_output(
    root_out: &mut String,
    fixed: &HashMap<String, u8>,
    groups: &[RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) {
    let encodings = build_root_encodings_sorted(fixed, groups, assignment, root_usage);
    for (base_name, keys) in &encodings {
        let enc = format_encoding(keys);
        let usage = root_usage.get(base_name).copied().unwrap_or(0);
        root_out.push_str(&format!("{}\t{}\t{}\n", base_name, enc, usage));
    }
}

fn save_simple_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    if !config::ENABLE_SIMPLE_CODE || ctx.simple_config.levels.is_empty() {
        return;
    }

    // Build full code_to_chars for collision computation
    let n = ctx.char_infos.len();
    let mut full_code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        full_code_to_chars.entry(code).or_default().push(ci);
    }

    let se = SimpleEvaluator::new(ctx, assignment, &full_code_to_chars);
    let sm = se.get_simple_metrics(ctx);

    let mut out = String::new();
    out.push_str("# 简码分配结果\n");
    out.push_str(&format!(
        "# 简码覆盖频率: {:.4}%\n",
        sm.weighted_freq_coverage * 100.0
    ));
    out.push_str(&format!("# 简码加权当量: {:.4}\n", sm.equiv_mean));
    out.push_str(&format!("# 简码分布偏差: {:.4}\n", sm.dist_deviation));
    out.push_str(&format!("# 简码重码数: {}\n", sm.collision_count));
    out.push_str(&format!(
        "# 简码重码率: {:.6}%\n",
        sm.collision_rate * 100.0
    ));
    out.push_str("#\n");

    let n_chars = ctx.char_infos.len();
    let mut globally_assigned: HashSet<usize> = HashSet::new();

    let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
    sorted_chars.sort_by(|&a, &b| {
        ctx.char_infos[b]
            .frequency
            .cmp(&ctx.char_infos[a].frequency)
    });

    for (li, level_cfg) in ctx.simple_config.levels.iter().enumerate() {
        let rules_str: String = level_cfg
            .rule_candidates
            .iter()
            .map(|rule| {
                rule.iter()
                    .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "\n# === {}级简码 (每位{}字, 规则:{}) ===\n",
            level_cfg.level, level_cfg.code_num, rules_str
        ));
        out.push_str("# 汉字\t简码\t字频\n");

        let mut code_candidates: HashMap<usize, Vec<(usize, u64)>> = HashMap::new();

        for &ci in &sorted_chars {
            if globally_assigned.contains(&ci) {
                continue;
            }
            if let Some(code) = ctx.calc_simple_code(ci, li, assignment) {
                code_candidates
                    .entry(code)
                    .or_default()
                    .push((ci, ctx.char_infos[ci].frequency));
            }
        }

        let mut level_winners: Vec<(usize, u64, String)> = Vec::new();

        for (_code, candidates) in &code_candidates {
            let mut count = 0;
            for &(ci, freq) in candidates {
                if count >= level_cfg.code_num {
                    break;
                }
                if globally_assigned.contains(&ci) {
                    continue;
                }

                let ch = ctx.raw_splits[ci].0;
                let code_str: String = ctx
                    .get_simple_keys(ci, li, assignment)
                    .map(|keys| keys.iter().map(|&k| key_to_char(k)).collect())
                    .unwrap_or_else(|| String::from("?"));

                level_winners.push((ci, freq, format!("{}\t{}\t{}", ch, code_str, freq)));
                count += 1;
            }
        }

        level_winners.sort_by(|a, b| b.1.cmp(&a.1));

        for (ci, _, line) in &level_winners {
            out.push_str(line);
            out.push('\n');
            globally_assigned.insert(*ci);
        }

        out.push_str(&format!("# 该级简码覆盖 {} 字\n", level_winners.len()));
    }

    fs::write(format!("{}/output-simple-codes.txt", dir), out).unwrap();
}

fn save_thread_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    simple_metrics: &SimpleMetrics,
    thread_id: usize,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let thread_dir = format!("{}/thread-{:02}", output_dir, thread_id);
    fs::create_dir_all(&thread_dir).expect("无法创建线程输出目录");

    let mut root_out = String::new();
    root_out.push_str(&format!("# 线程: {}\n", thread_id));
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if config::ENABLE_SIMPLE_CODE {
        root_out.push_str(&format!(
            "# 简码覆盖频率: {:.4}%\n",
            simple_metrics.weighted_freq_coverage * 100.0
        ));
        root_out.push_str(&format!(
            "# 简码加权当量: {:.4}\n",
            simple_metrics.equiv_mean
        ));
        root_out.push_str(&format!(
            "# 简码分布偏差: {:.4}\n",
            simple_metrics.dist_deviation
        ));
        root_out.push_str(&format!(
            "# 简码重码数: {}\n",
            simple_metrics.collision_count
        ));
        root_out.push_str(&format!(
            "# 简码重码率: {:.6}%\n",
            simple_metrics.collision_rate * 100.0
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    fs::write(format!("{}/output-keymap.txt", thread_dir), &root_out).unwrap();

    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        root_to_key.insert(root.clone(), key);
    }
    for (gi, group) in ctx.groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            root_to_key.insert(root.clone(), key);
        }
    }

    let mut code_out = String::new();
    for (ch, roots, freq) in &ctx.raw_splits {
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
    }
    fs::write(format!("{}/output-encode.txt", thread_dir), code_out).unwrap();

    save_key_distribution_to_dir(ctx, assignment, &thread_dir);
    save_equiv_distribution_to_dir(ctx, assignment, &thread_dir);
    save_simple_code_output(ctx, assignment, &thread_dir);
}

fn save_key_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = Evaluator::new(ctx, assignment);
    let mut out = String::new();
    out.push_str("# 用指分布统计\n");
    out.push_str(&format!("# 排列顺序: {}\n", config::KEY_DISPLAY_ORDER));
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    for kc in config::KEY_DISPLAY_ORDER.chars() {
        if let Some(ki) = char_to_key_index(kc) {
            if ki >= EQUIV_TABLE_SIZE {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * evaluator.inv_total_key_presses;
            let target = ctx.key_dist_config[ki].target_rate;
            let diff = actual - target;
            out.push_str(&format!(
                "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                kc,
                actual,
                target,
                diff,
                diff * diff
            ));
        }
    }

    let order_set: HashSet<char> = config::KEY_DISPLAY_ORDER.chars().collect();
    let special_keys = [('_', KEY_SPACE), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < EQUIV_TABLE_SIZE {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * evaluator.inv_total_key_presses;
                let target = ctx.key_dist_config[*ki].target_rate;
                let diff = actual - target;
                out.push_str(&format!(
                    "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                    kc,
                    actual,
                    target,
                    diff,
                    diff * diff
                ));
            }
        }
    }

    fs::write(format!("{}/output-distribution.txt", dir), out).unwrap();
}

fn save_equiv_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = Evaluator::new(ctx, assignment);
    let m = evaluator.get_metrics(ctx);

    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        let equiv = ctx.calc_equiv_from_parts(i, assignment);
        char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut out = String::new();
    out.push_str("# 当量分布统计\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", m.equiv_mean));
    out.push_str(&format!("# 变异系数(CV): {:.4}\n", m.equiv_cv));
    out.push_str(&format!("# 标准差: {:.4}\n", m.equiv_cv * m.equiv_mean));
    out.push_str("#\n# 当量最高的20个高频字 (字频>1000000):\n# 汉字\t当量\t字频\n");

    let mut count = 0;
    for (ch, eq, freq) in &char_equivs {
        if *freq > 1_000_000 && count < 20 {
            out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
            count += 1;
        }
    }

    out.push_str("#\n# 当量最低的20个高频字 (字频>1000000):\n");
    let high: Vec<_> = char_equivs
        .iter()
        .filter(|(_, _, f)| *f > 1_000_000)
        .collect();
    let start = high.len().saturating_sub(20);
    for (ch, eq, freq) in high.iter().skip(start) {
        out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
    }

    fs::write(format!("{}/output-equiv-dist.txt", dir), out).unwrap();
}

fn save_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    simple_metrics: &SimpleMetrics,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let mut root_out = String::new();
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if config::ENABLE_SIMPLE_CODE {
        root_out.push_str(&format!(
            "# 简码覆盖频率: {:.4}%\n",
            simple_metrics.weighted_freq_coverage * 100.0
        ));
        root_out.push_str(&format!(
            "# 简码加权当量: {:.4}\n",
            simple_metrics.equiv_mean
        ));
        root_out.push_str(&format!(
            "# 简码分布偏差: {:.4}\n",
            simple_metrics.dist_deviation
        ));
        root_out.push_str(&format!(
            "# 简码重码数: {}\n",
            simple_metrics.collision_count
        ));
        root_out.push_str(&format!(
            "# 简码重码率: {:.6}%\n",
            simple_metrics.collision_rate * 100.0
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    fs::write(format!("{}/output-keymap.txt", output_dir), &root_out).unwrap();

    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        root_to_key.insert(root.clone(), key);
    }
    for (gi, group) in ctx.groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            root_to_key.insert(root.clone(), key);
        }
    }

    let mut code_out = String::new();
    for (ch, roots, freq) in &ctx.raw_splits {
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
    }
    fs::write(format!("{}/output-encode.txt", output_dir), code_out).unwrap();

    save_key_distribution_to_dir(ctx, assignment, output_dir);
    save_equiv_distribution_to_dir(ctx, assignment, output_dir);
    save_simple_code_output(ctx, assignment, output_dir);

    println!(
        "结果已保存至 {}/output-keymap.txt, output-encode.txt, output-simple-codes.txt 等",
        output_dir
    );
}

fn save_summary(
    results: &[(usize, Vec<u8>, f64, Metrics, SimpleMetrics)],
    best_thread: usize,
    output_dir: &str,
    elapsed: std::time::Duration,
) {
    let mut summary = String::new();
    summary.push_str("# 优化结果汇总\n");
    summary.push_str(&format!("# 输出目录: {}\n", output_dir));
    summary.push_str(&format!("# 线程数: {}\n", config::NUM_THREADS));
    summary.push_str(&format!("# 总步数: {}\n", config::TOTAL_STEPS));
    summary.push_str(&format!("# 总耗时: {:?}\n", elapsed));
    summary.push_str(&format!("# 最优线程: {}\n", best_thread));
    summary.push_str(&format!("# 简码优化: {}\n", config::ENABLE_SIMPLE_CODE));
    summary.push_str("#\n");

    if config::ENABLE_SIMPLE_CODE {
        summary.push_str(&format!(
            "{:<8} {:<12} {:<10} {:<12} {:<10} {:<10} {:<12} {:<12} {:<10} {:<10} {:<10} {:<12}\n",
            "线程",
            "得分",
            "重码数",
            "重码率%",
            "当量",
            "CV",
            "分布偏差",
            "简码覆盖%",
            "简码当量",
            "简码分布",
            "简码重码",
            "简码重码率%"
        ));
        summary.push_str(&format!("{}\n", "-".repeat(140)));
    } else {
        summary.push_str(&format!(
            "{:<8} {:<12} {:<10} {:<12} {:<10} {:<10} {:<12}\n",
            "线程", "得分", "重码数", "重码率%", "当量", "CV", "分布偏差"
        ));
        summary.push_str(&format!("{}\n", "-".repeat(80)));
    }

    let mut sorted: Vec<&(usize, Vec<u8>, f64, Metrics, SimpleMetrics)> = results.iter().collect();
    sorted.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    for (tid, _, score, m, sm) in &sorted {
        let marker = if *tid == best_thread { " 🏆" } else { "" };
        if config::ENABLE_SIMPLE_CODE {
            summary.push_str(&format!(
                "T{:<7} {:<12.4} {:<10} {:<12.6} {:<10.4} {:<10.4} {:<12.4} {:<12.4} {:<10.4} {:<10.4} {:<10} {:<12.6}{}\n",
                tid, score, m.collision_count, m.collision_rate * 100.0,
                m.equiv_mean, m.equiv_cv, m.dist_deviation,
                sm.weighted_freq_coverage * 100.0, sm.equiv_mean, sm.dist_deviation,
                sm.collision_count, sm.collision_rate * 100.0,
                marker
            ));
        } else {
            summary.push_str(&format!(
                "T{:<7} {:<12.4} {:<10} {:<12.6} {:<10.4} {:<10.4} {:<12.4}{}\n",
                tid,
                score,
                m.collision_count,
                m.collision_rate * 100.0,
                m.equiv_mean,
                m.equiv_cv,
                m.dist_deviation,
                marker
            ));
        }
    }

    fs::write(format!("{}/summary.txt", output_dir), summary).unwrap();
}

// =========================================================================
// 🏁 主函数
// =========================================================================

fn main() {
    let start_time = Instant::now();
    println!("=== 字劫算法优化器 v9 (Auto-Scaling + Simple Code Collision) ===");

    config::validate_weights();

    println!(
        "线程数: {}, 总步数: {}",
        config::NUM_THREADS,
        config::TOTAL_STEPS
    );
    println!(
        "初始温度: {}, 结束温度: {}",
        config::TEMP_START,
        config::TEMP_END
    );
    println!("全局允许键位: {}", config::ALLOWED_KEYS);
    println!(
        "全码权重: 重码数={:.2}, 重码率={:.2}, 当量={:.2}, CV={:.2}, 分布={:.2}",
        config::WEIGHT_COLLISION_COUNT,
        config::WEIGHT_COLLISION_RATE,
        config::WEIGHT_EQUIVALENCE,
        config::WEIGHT_EQUIV_CV,
        config::WEIGHT_DISTRIBUTION
    );
    println!(
        "简码优化: {} (全码占比={:.0}%, 简码占比={:.0}%)",
        if config::ENABLE_SIMPLE_CODE {
            "开启"
        } else {
            "关闭"
        },
        config::WEIGHT_FULL_CODE * 100.0,
        config::WEIGHT_SIMPLE_CODE * 100.0
    );
    if config::ENABLE_SIMPLE_CODE {
        println!(
            "简码子权重: 频率覆盖={:.2}, 当量={:.2}, 分布={:.2}, 重码数={:.2}, 重码率={:.2}",
            config::SIMPLE_WEIGHT_FREQ,
            config::SIMPLE_WEIGHT_EQUIV,
            config::SIMPLE_WEIGHT_DIST,
            config::SIMPLE_WEIGHT_COLLISION_COUNT,
            config::SIMPLE_WEIGHT_COLLISION_RATE
        );
    }
    println!("用指分布输出顺序: {}", config::KEY_DISPLAY_ORDER);

    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let output_dir = format!("output-{}", timestamp);
    fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);

    let (fixed_roots, constrained) = load_fixed(config::FILE_FIXED);
    let dynamic_groups = load_dynamic(config::FILE_DYNAMIC, &constrained);
    let splits = load_splits(config::FILE_SPLITS);
    let equiv_table = load_pair_equivalence(config::FILE_PAIR_EQUIV);
    let key_dist_config = load_key_distribution(config::FILE_KEY_DIST);

    let simple_config = if config::ENABLE_SIMPLE_CODE {
        let cfg = parse_simple_code_config(config::FILE_SIMPLE);
        println!("\n📋 简码配置:");
        for level in &cfg.levels {
            let rules_str: String = level
                .rule_candidates
                .iter()
                .map(|rule| {
                    rule.iter()
                        .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join(" | ");
            println!(
                "  - {}级简码: 每位{}字, 规则: {}",
                level.level, level.code_num, rules_str
            );
        }
        cfg
    } else {
        SimpleCodeConfig { levels: vec![] }
    };

    let max_parts_in_data = splits.iter().map(|(_, r, _)| r.len()).max().unwrap_or(0);
    let total_roots: usize = dynamic_groups.iter().map(|g| g.roots.len()).sum();
    let total_freq: u64 = splits.iter().map(|(_, _, f)| f).sum();

    println!("\n数据加载完毕:");
    println!("  - 固定字根(单键): {}", fixed_roots.len());
    println!("  - 受限字根组(多键): {} 组", constrained.len());
    println!(
        "  - 动态字根组: {} 组 (共 {} 字根)",
        dynamic_groups.len(),
        total_roots
    );
    println!("  - 汉字数量: {}", splits.len());
    println!("  - 总字频: {}", total_freq);
    println!("  - 最大码长: {}", max_parts_in_data);

    if max_parts_in_data > config::MAX_PARTS {
        println!(
            "⚠️ 拆分表中最大码长({})超过 MAX_PARTS({}), 请调大 config::MAX_PARTS",
            max_parts_in_data,
            config::MAX_PARTS
        );
    }

    if !check_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }

    // 校准
    println!("\n📐 正在进行初始尺度校准...");
    let temp_scale = ScaleConfig::default();
    let temp_ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
        temp_scale,
        simple_config.clone(),
    );

    let initial_assignment = smart_init(&temp_ctx);
    let initial_eval = Evaluator::new(&temp_ctx, &initial_assignment);
    let initial_metrics = initial_eval.get_metrics(&temp_ctx);
    let initial_simple_metrics = initial_eval.get_simple_metrics(&temp_ctx);

    let scale_config = calibrate_scales(&initial_metrics, &initial_simple_metrics);

    println!("  初始状态观测:");
    println!(
        "    重码数: {},  重码率: {:.6}",
        initial_metrics.collision_count, initial_metrics.collision_rate
    );
    println!(
        "    当量: {:.4},  CV: {:.4}",
        initial_metrics.equiv_mean, initial_metrics.equiv_cv
    );
    if config::ENABLE_SIMPLE_CODE {
        println!(
            "    简码覆盖: {:.4}%,  简码当量: {:.4},  简码分布: {:.4}",
            initial_simple_metrics.weighted_freq_coverage * 100.0,
            initial_simple_metrics.equiv_mean,
            initial_simple_metrics.dist_deviation
        );
        println!(
            "    简码重码数: {},  简码重码率: {:.6}%",
            initial_simple_metrics.collision_count,
            initial_simple_metrics.collision_rate * 100.0
        );
    }
    println!("  校准尺度 (Scale):");
    println!("    CollisionCount: {:.6}", scale_config.collision_count);
    println!("    CollisionRate:  {:.6}", scale_config.collision_rate);
    println!("    Equivalence:    {:.6}", scale_config.equivalence);
    if config::ENABLE_SIMPLE_CODE {
        println!("    SimpleFreq:     {:.6}", scale_config.simple_freq);
        println!("    SimpleEquiv:    {:.6}", scale_config.simple_equiv);
        println!("    SimpleDist:     {:.6}", scale_config.simple_dist);
        println!(
            "    SimpleCollCnt:  {:.6}",
            scale_config.simple_collision_count
        );
        println!(
            "    SimpleCollRate: {:.6}",
            scale_config.simple_collision_rate
        );
    }

    // 逻辑根验证
    if config::ENABLE_SIMPLE_CODE {
        println!("\n  📝 逻辑根解析验证 (前3字):");
        for ci in 0..3.min(temp_ctx.raw_splits.len()) {
            let (ch, roots, _) = &temp_ctx.raw_splits[ci];
            let si = &temp_ctx.char_simple_infos[ci];
            println!("    '{}' 拆分: {:?}", ch, roots);
            for (ri, lr) in si.logical_roots.iter().enumerate() {
                let full_keys: Vec<char> = lr
                    .full_code_parts
                    .iter()
                    .map(|&p| key_to_char(temp_ctx.resolve_key(p, &initial_assignment)))
                    .collect();
                println!(
                    "      逻辑根[{}] '{}': 拆分中占位={:?}, 完整编码={:?}",
                    ri, lr.base_name, lr.split_part_indices, full_keys
                );
            }
            for (li, instr) in si.level_instructions.iter().enumerate() {
                if let Some(ref steps) = instr {
                    let keys: Vec<char> = steps
                        .iter()
                        .map(|&(root_idx, code_idx)| {
                            let lr = &si.logical_roots[root_idx];
                            let part = lr.full_code_parts[code_idx];
                            key_to_char(temp_ctx.resolve_key(part, &initial_assignment))
                        })
                        .collect();
                    let level_cfg = &temp_ctx.simple_config.levels[li];
                    let mut matched_rule_idx = 0;
                    for (ri, rule) in level_cfg.rule_candidates.iter().enumerate() {
                        if try_resolve_rule(rule, &si.logical_roots, si.logical_roots.len())
                            .is_some()
                        {
                            matched_rule_idx = ri;
                            break;
                        }
                    }
                    let rule_str: String = level_cfg.rule_candidates[matched_rule_idx]
                        .iter()
                        .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                        .collect();
                    let all_rules_str: String = level_cfg
                        .rule_candidates
                        .iter()
                        .enumerate()
                        .map(|(i, rule)| {
                            let s: String = rule
                                .iter()
                                .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                                .collect();
                            if i == matched_rule_idx {
                                format!("[{}]", s)
                            } else {
                                s
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "      {}级简码(规则: {} 命中: {}): {:?}",
                        level_cfg.level, all_rules_str, rule_str, keys
                    );
                } else {
                    let level_cfg = &temp_ctx.simple_config.levels[li];
                    let all_rules_str: String = level_cfg
                        .rule_candidates
                        .iter()
                        .map(|rule| {
                            rule.iter()
                                .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "      {}级简码(规则: {}): 无合规候选",
                        level_cfg.level, all_rules_str
                    );
                }
            }
        }
    }

    // 正式上下文
    let equiv_table_2 = load_pair_equivalence(config::FILE_PAIR_EQUIV);
    let key_dist_config_2 = load_key_distribution(config::FILE_KEY_DIST);

    let ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table_2,
        key_dist_config_2,
        scale_config,
        simple_config,
    );

    println!("\n  - 编码基数: {}", ctx.code_base);
    println!("  - 编码空间: {}", ctx.code_space);

    let root_usage = count_root_usage(&ctx);

    println!("\n🚀 开始优化...");
    let results: Vec<(Vec<u8>, f64, Metrics, SimpleMetrics)> = (0..config::NUM_THREADS)
        .into_par_iter()
        .map(|i| simulated_annealing(&ctx, i))
        .collect();

    let all_results: Vec<(usize, Vec<u8>, f64, Metrics, SimpleMetrics)> = results
        .into_iter()
        .enumerate()
        .map(|(i, (a, s, m, sm))| (i, a, s, m, sm))
        .collect();

    let (best_thread, best_assignment, best_score, best_metrics, best_simple_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m, sm)| (*tid, a.clone(), *s, *m, *sm))
        .unwrap();

    let elapsed = start_time.elapsed();

    let m = best_metrics;
    let sm = best_simple_metrics;
    println!("\n=================================");
    println!("🏆 最优结果 (线程 {}):", best_thread);
    println!("   综合得分: {:.4}", best_score);
    println!("   「全码」重码数: {}", m.collision_count);
    println!("   「全码」重码率: {:.6}%", m.collision_rate * 100.0);
    println!("   「全码」加权键均当量: {:.4}", m.equiv_mean);
    println!("   「全码」当量变异系数(CV): {:.4}", m.equiv_cv);
    println!("   「全码」用指分布偏差(L2): {:.4}", m.dist_deviation);
    if config::ENABLE_SIMPLE_CODE {
        println!("---------------------------------");
        println!("   「简码」重码数: {}", sm.collision_count);
        println!("   「简码」重码率: {:.6}%", sm.collision_rate * 100.0);
        println!(
            "   「简码」覆盖率: {:.4}%",
            sm.weighted_freq_coverage * 100.0
        );
        println!("   「简码」加权当量: {:.4}", sm.equiv_mean);
        println!("   「简码」分布偏差: {:.4}", sm.dist_deviation);
    }
    println!("⏱️ 总耗时: {:?}", elapsed);
    println!("=================================");

    println!("\n📁 保存所有线程结果...");
    for (tid, assignment, score, metrics, smetrics) in &all_results {
        save_thread_results(
            &ctx,
            assignment,
            *score,
            metrics,
            smetrics,
            *tid,
            &output_dir,
            &root_usage,
        );
    }

    save_results(
        &ctx,
        &best_assignment,
        best_score,
        &best_metrics,
        &best_simple_metrics,
        &output_dir,
        &root_usage,
    );
    save_summary(&all_results, best_thread, &output_dir, elapsed);

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - summary.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    println!("  - output-simple-codes.txt  简码分配");
    println!("  - thread-XX/               各线程结果");
}
