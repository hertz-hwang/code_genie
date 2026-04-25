// =========================================================================
// 🚀 基础数据类型
// =========================================================================

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// 键位空间大小（a-z 中 _ 对应空格键的索引）
pub const KEY_SPACE: usize = 26;
/// 当量表大小（全键盘 47 键）
/// 索引分配：
///   0-25:  a-z
///   26:    _ (空格)
///   27:    ;
///   28:    ,
///   29:    .
///   30:    /
///   31:    1
///   32:    2
///   33:    3
///   34:    4
///   35:    5
///   36:    6
///   37:    7
///   38:    8
///   39:    9
///   40:    0
///   41:    -
///   42:    =
///   43:    [
///   44:    ]
///   45:    \
///   46:    '
pub const EQUIV_TABLE_SIZE: usize = 47;
/// 分组标记起始值
pub const GROUP_MARKER: u16 = 1000;
/// CharInfo.parts 的最大长度（对应 config 中 max_parts 上限）
pub const MAX_PARTS: usize = 4;

/// 将字符转换为键位索引
pub fn char_to_key_index(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some((c as u8 - b'a') as usize),
        '_' => Some(KEY_SPACE),
        ';' => Some(27),
        ',' => Some(28),
        '.' => Some(29),
        '/' => Some(30),
        '1' => Some(31),
        '2' => Some(32),
        '3' => Some(33),
        '4' => Some(34),
        '5' => Some(35),
        '6' => Some(36),
        '7' => Some(37),
        '8' => Some(38),
        '9' => Some(39),
        '0' => Some(40),
        '-' => Some(41),
        '=' => Some(42),
        '[' => Some(43),
        ']' => Some(44),
        '\\' => Some(45),
        '\'' => Some(46),
        _ => None,
    }
}

/// 将键位索引转换为字符
pub fn key_to_char(key: u8) -> char {
    match key {
        0..=25 => (key + b'a') as char,
        26 => '_',
        27 => ';',
        28 => ',',
        29 => '.',
        30 => '/',
        31 => '1',
        32 => '2',
        33 => '3',
        34 => '4',
        35 => '5',
        36 => '6',
        37 => '7',
        38 => '8',
        39 => '9',
        40 => '0',
        41 => '-',
        42 => '=',
        43 => '[',
        44 => ']',
        45 => '\\',
        46 => '\'',
        _ => '?',
    }
}

/// 计算 base 的 exp 次方
pub fn pow_base(base: usize, exp: usize) -> usize {
    let mut result = 1;
    for _ in 0..exp {
        result *= base;
    }
    result
}

/// 权重配置 - 用于得分计算
/// 三组目标：单字全码 / 单字简码 / 多字词全码，顶层线性组合
#[derive(Clone, Copy, Debug)]
pub struct WeightConfig {
    // ── 顶层权重（三组目标的相对重要性，建议总和为 1.0）──
    pub weight_full_code: f64,
    pub weight_simple_code: f64,
    pub weight_word_code: f64,

    // ── 单字全码子权重 ──
    /// 字频前 N 重码数的 N 值
    pub full_top_n: usize,
    pub full_top_n_collision: f64,
    pub full_collision_count: f64,
    pub full_collision_rate: f64,
    pub full_equivalence: f64,
    pub full_distribution: f64,

    // ── 单字简码开关与子权重 ──
    pub enable_simple_code: bool,
    /// 加权码长（全字符加权平均码长）
    pub simple_weighted_key_length: f64,
    pub simple_collision_count: f64,
    pub simple_collision_rate: f64,
    pub simple_equivalence: f64,
    pub simple_distribution: f64,

    // ── 多字词全码开关与子权重 ──
    pub enable_word_code: bool,
    pub word_top2000_collision: f64,
    pub word_top10000_collision: f64,
    pub word_collision_count: f64,
    pub word_collision_rate: f64,
    pub word_equivalence: f64,
    pub word_distribution: f64,
}

impl Default for WeightConfig {
    fn default() -> Self {
        Self {
            weight_full_code: 0.5,
            weight_simple_code: 0.3,
            weight_word_code: 0.2,
            full_top_n: 1500,
            full_top_n_collision: 0.1,
            full_collision_count: 0.1,
            full_collision_rate: 0.3,
            full_equivalence: 0.3,
            full_distribution: 0.2,
            enable_simple_code: true,
            simple_weighted_key_length: 0.3,
            simple_collision_count: 0.1,
            simple_collision_rate: 0.2,
            simple_equivalence: 0.2,
            simple_distribution: 0.2,
            enable_word_code: false,
            word_top2000_collision: 0.2,
            word_top10000_collision: 0.1,
            word_collision_count: 0.1,
            word_collision_rate: 0.3,
            word_equivalence: 0.2,
            word_distribution: 0.1,
        }
    }
}

/// 缩放配置 - 用于将不同量纲的指标归一化（由 calibrate_scales 自动设置）
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ScaleConfig {
    // 单字全码
    pub full_top_n_collision: f64,
    pub full_collision_count: f64,
    pub full_collision_rate: f64,
    pub full_equivalence: f64,
    pub full_distribution: f64,
    // 单字简码
    pub simple_weighted_key_length: f64,
    pub simple_collision_count: f64,
    pub simple_collision_rate: f64,
    pub simple_equivalence: f64,
    pub simple_distribution: f64,
    // 多字词全码
    pub word_top2000_collision: f64,
    pub word_top10000_collision: f64,
    pub word_collision_count: f64,
    pub word_collision_rate: f64,
    pub word_equivalence: f64,
    pub word_distribution: f64,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            full_top_n_collision: 1.0,
            full_collision_count: 1.0,
            full_collision_rate: 1.0,
            full_equivalence: 1.0,
            full_distribution: 1.0,
            simple_weighted_key_length: 1.0,
            simple_collision_count: 1.0,
            simple_collision_rate: 1.0,
            simple_equivalence: 1.0,
            simple_distribution: 1.0,
            word_top2000_collision: 1.0,
            word_top10000_collision: 1.0,
            word_collision_count: 1.0,
            word_collision_rate: 1.0,
            word_equivalence: 1.0,
            word_distribution: 1.0,
        }
    }
}

/// 键位分布配置
#[derive(Clone, Copy, Default)]
pub struct KeyDistConfig {
    /// 目标使用率 (%)
    pub target_rate: f64,
    /// 低于目标时的惩罚系数
    pub low_penalty: f64,
    /// 高于目标时的惩罚系数
    pub high_penalty: f64,
}

/// 汉字拆分信息
#[derive(Clone, Copy)]
pub struct CharInfo {
    /// 拆分后的字根列表（键位索引或分组标记），固定大小避免堆分配
    pub parts: [u16; MAX_PARTS],
    /// parts 的实际长度
    pub parts_len: u8,
    /// 使用频率
    pub frequency: u64,
    /// 当前编码值（用于增量更新）
    #[allow(dead_code)]
    pub current_code: usize,
    /// 预计算的键位索引（分配时从 assignment 解析），用于快速增量更新
    pub current_key_indices: [u16; MAX_PARTS],
}

impl CharInfo {
    #[inline]
    pub fn parts_slice(&self) -> &[u16] {
        &self.parts[..self.parts_len as usize]
    }
}

/// 字根组 - 用于需要优化的动态字根
#[derive(Clone)]
pub struct RootGroup {
    /// 字根列表
    pub roots: Vec<String>,
    /// 允许分配的键位
    pub allowed_keys: Vec<u8>,
}

/// 单字全码评估指标
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// 字频前 N 重码数
    pub top_n_collision_count: usize,
    /// 总重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
    /// 加权平均当量
    pub equiv_mean: f64,
    /// 键位分布偏差
    pub dist_deviation: f64,
}

/// 单字简码评估指标
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct SimpleMetrics {
    /// 全字符加权平均码长（出简字取简码长，否则取全码长）
    pub weighted_key_length: f64,
    /// 加权平均当量
    pub equiv_mean: f64,
    /// 键位分布偏差
    pub dist_deviation: f64,
    /// 重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
}

/// 多字词全码评估指标
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct WordMetrics {
    /// 词频前 2000 重码数
    pub top2000_collision_count: usize,
    /// 词频前 10000 重码数
    pub top10000_collision_count: usize,
    /// 总重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
    /// 加权平均当量
    pub equiv_mean: f64,
    /// 键位分布偏差
    pub dist_deviation: f64,
}

/// 多字词拆分信息（与 CharInfo 结构相同，额外标记词频排名）
#[derive(Clone, Copy)]
pub struct WordInfo {
    /// 拆分后的字根列表（键位索引或分组标记）
    pub parts: [u16; MAX_PARTS],
    /// parts 的实际长度
    pub parts_len: u8,
    /// 词频
    pub frequency: u64,
    /// 当前编码值
    #[allow(dead_code)]
    pub current_code: usize,
    /// 预计算的键位索引
    pub current_key_indices: [u16; MAX_PARTS],
    /// 是否在词频前 2000
    pub is_top2000: bool,
    /// 是否在词频前 10000
    pub is_top10000: bool,
}

impl WordInfo {
    #[inline]
    pub fn parts_slice(&self) -> &[u16] {
        &self.parts[..self.parts_len as usize]
    }
}

/// 简码步骤 - 选择哪个逻辑根的哪个编码
#[derive(Clone, Debug)]
pub struct SimpleCodeStep {
    /// 根选择器 (A-Y, Z)
    pub root_selector: char,
    /// 编码选择器 (a-z)
    pub code_selector: char,
}

/// 简码级别配置
#[derive(Clone, Debug)]
pub struct SimpleCodeLevel {
    /// 简码级别
    pub level: usize,
    /// 该级别可分配的汉字数
    pub code_num: usize,
    /// 候选规则列表
    pub rule_candidates: Vec<Vec<SimpleCodeStep>>,
    /// 允许出简码的全码码长（0 = 不限制）
    pub allowed_orig_length: usize,
}

/// 简码配置
#[derive(Clone, Debug)]
pub struct SimpleCodeConfig {
    /// 简码级别列表
    pub levels: Vec<SimpleCodeLevel>,
}

/// 逻辑根 - 同一基础字的不同拆分变体
#[derive(Clone, Debug)]
pub struct LogicalRoot {
    /// 基础名称
    pub base_name: String,
    /// 在拆分中的位置索引
    pub split_part_indices: Vec<usize>,
    /// 完整编码的各部分（键位索引或分组标记）
    pub full_code_parts: Vec<u16>,
}

/// 汉字的简码信息
#[derive(Clone, Debug)]
pub struct CharSimpleInfo {
    /// 逻辑根列表
    pub logical_roots: Vec<LogicalRoot>,
    /// 每级简码的指令（root_idx, code_idx）
    pub level_instructions: Vec<Option<Vec<(usize, usize)>>>,
}

/// 解析编码选择器
/// - a: 第0个编码
/// - b-y: 中间编码
/// - z: 最后一个编码
pub fn resolve_code_index(code_selector: char, total_codes: usize) -> Option<usize> {
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

/// 尝试解析简码规则
/// 返回 (root_idx, code_idx) 列表
pub fn try_resolve_rule(
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

/// 从名称中提取基础名（去掉数字后缀）
pub fn extract_base_name(name: &str) -> String {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..dot_pos].to_string();
        }
    }
    name.to_string()
}

/// 从名称中提取数字后缀
pub fn extract_suffix_num(name: &str) -> i32 {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if let Ok(n) = suffix.parse::<i32>() {
            return n;
        }
    }
    -1
}

/// 构建根名的完整编码映射
pub fn build_root_full_codes(
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

/// 从根名列表提取逻辑根
pub fn extract_logical_roots_full(
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

/// 计算每级简码的指令
pub fn compute_level_instructions(
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