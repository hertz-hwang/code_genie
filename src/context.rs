// =========================================================================
// 📦 主上下文
// =========================================================================

use std::collections::{HashMap, HashSet};

use crate::types::{
    build_root_full_codes, CharInfo, CharSimpleInfo, compute_level_instructions,
    extract_logical_roots_full, try_resolve_rule, KeyDistConfig,
    RootGroup, ScaleConfig, SimpleCodeConfig, WeightConfig, KEY_SPACE, EQUIV_TABLE_SIZE,
    GROUP_MARKER,
};
use crate::keysoul;

/// pair 当量表类型（从文件加载后传入 OptContext::new）
pub type PairEquivTable = [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

/// 优化上下文 - 存储所有算法需要的数据
pub struct OptContext {
    /// 是否启用简码优化
    pub enable_simple_code: bool,
    /// 权重配置
    pub weights: WeightConfig,
    /// 字根组数量
    pub num_groups: usize,
    /// 字根名到组索引的映射
    pub root_to_group: HashMap<String, usize>,
    /// 组索引到使用该组的汉字索引列表的映射
    pub group_to_chars: Vec<Vec<usize>>,
    /// 汉字信息列表
    pub char_infos: Vec<CharInfo>,
    /// 原始拆分数据 (字符, 根名列表, 频率)
    pub raw_splits: Vec<(char, Vec<String>, u64)>,
    /// 字根组列表
    pub groups: Vec<RootGroup>,
    /// 固定字根映射
    pub fixed_roots: HashMap<String, u8>,
    /// 全序列当量表（最长序列匹配）
    /// 索引方案：长度 n 的序列 [k0..k_{n-1}]
    ///   flat = sum(ki * KEY_COUNT^(n-1-i))
    ///   index = equiv_table_offsets[n] + flat
    /// 覆盖长度 1..=max_parts（长度 1 存单键自身当量，通常为 0）
    pub equiv_table: Vec<f64>,
    /// equiv_table 各长度的起始偏移（equiv_table_offsets[n] = 长度 n 序列的起始位置）
    pub equiv_table_offsets: Vec<usize>,
    /// 键位分布配置
    pub key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    /// 总频率
    pub total_frequency: u64,
    /// 编码基数
    pub code_base: usize,
    /// 最大码长
    pub max_parts: usize,
    /// 编码空间大小
    pub code_space: usize,
    /// 缩放配置
    pub scale_config: ScaleConfig,
    /// 简码配置
    pub simple_config: SimpleCodeConfig,
    /// 汉字简码信息
    pub char_simple_infos: Vec<CharSimpleInfo>,
    /// 每个组影响的简码汉字集合
    pub group_to_simple_affected: Vec<HashSet<usize>>,
    /// 根名的完整编码映射
    pub root_full_codes: HashMap<String, Vec<String>>,
    /// 每个组的加权频率总和（用于 key_weighted_usage 的 O(1) 更新）
    pub group_freq_sum: Vec<f64>,
    /// code_base 的幂次表（code_base_powers[i] = code_base^i），用于增量编码计算
    pub code_base_powers: Vec<usize>,

    /// 增量编码掩码：group_char_masks[group_idx] = [(char_idx, mask), ...]
    /// mask = Σ code_base^(n-1-pos) 对于该 group 在该汉字中出现的所有位置
    /// 用于 O(1) 增量编码更新：new_code = old_code + mask * (new_key - old_key)
    pub group_char_masks: Vec<Vec<(usize, usize)>>,

    // === 热路径优化标志（预计算，避免每次 update_char 查权重）===
    /// 是否需要维护等价值（weight_equivalence > 0 或 weight_equiv_cv > 0）
    pub need_equiv: bool,
    /// 是否需要维护碰撞频率（weight_collision_rate > 0）
    pub need_collision_rate: bool,
    /// 是否需要维护桶成员列表（enable_simple_code 或 need_collision_rate）
    pub need_bucket_members: bool,
    /// 是否使用键魂当量模型（仅用于日志输出，热路径不分支）
    pub use_keysoul: bool,
}

impl OptContext {
    /// 创建新的优化上下文
    pub fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        pair_table: PairEquivTable,
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        simple_config: SimpleCodeConfig,
        weights: WeightConfig,
        use_keysoul: bool,
    ) -> Self {
        let enable_simple_code = weights.enable_simple_code;
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
                current_code: 0,
                current_key_indices: Vec::with_capacity(roots.len()),
            };

            let mut seen_groups = HashSet::new();

            for root in roots {
                if let Some(&key) = fixed_roots.get(root) {
                    info.parts.push(key as u16);
                    info.current_key_indices.push(key as u16);
                } else if let Some(&gi) = root_to_group.get(root) {
                    info.parts.push(gi as u16 + GROUP_MARKER);
                    info.current_key_indices.push(gi as u16 + GROUP_MARKER);
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

            if enable_simple_code {
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

            info.current_key_indices = info.parts.clone();

            char_simple_infos.push(CharSimpleInfo {
                logical_roots,
                level_instructions,
            });

            char_infos.push(info);
        }

        let code_base = EQUIV_TABLE_SIZE + 1;
        let code_space = crate::types::pow_base(code_base, max_parts);

        let mut group_freq_sum = vec![0.0f64; num_groups];
        for ci in 0..char_infos.len() {
            let freq_f = char_infos[ci].frequency as f64;
            for &p in &char_infos[ci].parts {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    group_freq_sum[gi] += freq_f;
                }
            }
        }

        let mut code_base_powers = vec![1usize; max_parts + 1];
        for i in 1..=max_parts {
            code_base_powers[i] = code_base_powers[i - 1] * code_base;
        }

        let mut group_char_masks: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_groups];
        for (ci, info) in char_infos.iter().enumerate() {
            let n = info.parts.len();
            let mut group_mask: HashMap<usize, usize> = HashMap::new();
            for (pos, &p) in info.parts.iter().enumerate() {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    *group_mask.entry(gi).or_insert(0) += code_base_powers[n - 1 - pos];
                }
            }
            for (gi, mask) in group_mask {
                group_char_masks[gi].push((ci, mask));
            }
        }

        let need_equiv = weights.weight_equivalence > 0.0 || weights.weight_equiv_cv > 0.0;
        let need_collision_rate = weights.weight_collision_rate > 0.0;
        let need_bucket_members = enable_simple_code || need_collision_rate;

        // 预计算全序列当量表（最长序列匹配）
        // offsets[n] = 长度 n 序列在 equiv_table 中的起始位置
        // 覆盖长度 1..=max_parts
        let key_count = EQUIV_TABLE_SIZE;
        let mut offsets = vec![0usize; max_parts + 1];
        let mut acc = 0usize;
        for n in 1..=max_parts {
            offsets[n] = acc;
            acc += key_count.pow(n as u32);
        }
        let mut table = vec![0.0f64; acc];

        // 长度 1：单键当量为 0（无转移）
        // offsets[1] 已填充为 0.0，无需额外处理

        // 长度 2..=max_parts
        for n in 2..=max_parts {
            let base = offsets[n];
            let count = key_count.pow(n as u32);
            let mut indices = vec![0u8; n];
            for flat in 0..count {
                // 将 flat 解码为 indices（大端，base key_count）
                let mut tmp = flat;
                for pos in (0..n).rev() {
                    indices[pos] = (tmp % key_count) as u8;
                    tmp /= key_count;
                }

                let val = if use_keysoul {
                    let time = keysoul::calc_keysoul_from_indices(&indices);
                    if time < 0.0 { 0.0 } else { time / n as f64 }
                } else {
                    // pair 累加：sum(pair[i][i+1]) + pair[last][SPACE]，除以 n
                    let mut total = 0.0;
                    for i in 0..n - 1 {
                        total += pair_table[indices[i] as usize][indices[i + 1] as usize];
                    }
                    total += pair_table[indices[n - 1] as usize][KEY_SPACE];
                    total / n as f64
                };

                table[base + flat] = val;
            }
        }

        Self {
            enable_simple_code,
            weights,
            num_groups,
            root_to_group,
            group_to_chars,
            char_infos,
            raw_splits: splits.to_vec(),
            groups: groups.to_vec(),
            fixed_roots: fixed_roots.clone(),
            equiv_table: table,
            equiv_table_offsets: offsets,
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
            group_freq_sum,
            code_base_powers,
            group_char_masks,
            need_equiv,
            need_collision_rate,
            need_bucket_members,
            use_keysoul,
        }
    }

    /// 解析键位 - 将部分索引解析为实际键位
    #[inline(always)]
    pub fn resolve_key(&self, part: u16, assignment: &[u8]) -> u8 {
        if part >= GROUP_MARKER {
            assignment[(part - GROUP_MARKER) as usize]
        } else {
            part as u8
        }
    }

    /// 更新单个汉字的 key_indices（当其某个 radical 的分配改变时）
    #[allow(dead_code)]
    pub fn update_char_key_indices(&self, ci: usize, assignment: &[u8], key_indices: &mut Vec<u16>) {
        let info = &self.char_infos[ci];
        key_indices.clear();
        for &p in &info.parts {
            key_indices.push(self.resolve_key(p, assignment) as u16);
        }
    }

    /// 计算仅全码（使用临时缓冲区）
    #[allow(dead_code)]
    pub fn calc_code_only_fast(&self, ci: usize, key_indices: &[u16]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &ki in key_indices.iter().take(info.parts.len()) {
            code = code * self.code_base + (ki as usize + 1);
        }
        code
    }

    /// 计算仅全码
    pub fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in &info.parts {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 从拆分计算等价值（最长序列匹配查表）
    #[inline(always)]
    pub fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> f64 {
        let info = &self.char_infos[ci];
        let n = info.parts.len();
        if n == 0 || n >= self.equiv_table_offsets.len() {
            return 0.0;
        }
        let base = self.equiv_table_offsets[n];
        let key_count = EQUIV_TABLE_SIZE;
        let mut flat = 0usize;
        for &p in &info.parts {
            flat = flat * key_count + self.resolve_key(p, assignment) as usize;
        }
        self.equiv_table[base + flat]
    }

    /// 从预计算 key_indices 计算等价值
    #[inline(always)]
    #[allow(dead_code)]
    pub fn calc_equiv_from_key_indices(&self, _ci: usize, key_indices: &[u16]) -> f64 {
        let n = key_indices.len();
        if n == 0 || n >= self.equiv_table_offsets.len() {
            return 0.0;
        }
        let base = self.equiv_table_offsets[n];
        let key_count = EQUIV_TABLE_SIZE;
        let mut flat = 0usize;
        for &ki in key_indices {
            flat = flat * key_count + ki as usize;
        }
        self.equiv_table[base + flat]
    }

    /// 计算简码
    #[inline]
    pub fn calc_simple_code(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<usize> {
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

    /// 获取简码键位列表
    pub fn get_simple_keys(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<Vec<u8>> {
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

    /// 计算简码等价值（最长序列匹配查表）
    #[inline]
    pub fn calc_simple_equiv(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> f64 {
        let si = &self.char_simple_infos[ci];
        let instr = match si.level_instructions.get(level_idx) {
            Some(Some(ref v)) => v,
            _ => return 0.0,
        };
        let n = instr.len();
        if n == 0 || n >= self.equiv_table_offsets.len() {
            return 0.0;
        }
        let base = self.equiv_table_offsets[n];
        let key_count = EQUIV_TABLE_SIZE;
        let mut flat = 0usize;
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                return 0.0;
            }
            flat = flat * key_count + self.resolve_key(lr.full_code_parts[code_idx], assignment) as usize;
        }
        self.equiv_table[base + flat]
    }
}

// 实现 Clone（OptContext 只包含数据结构，可以克隆）
impl Clone for OptContext {
    fn clone(&self) -> Self {
        Self {
            enable_simple_code: self.enable_simple_code,
            weights: self.weights.clone(),
            num_groups: self.num_groups,
            root_to_group: self.root_to_group.clone(),
            group_to_chars: self.group_to_chars.clone(),
            char_infos: self.char_infos.clone(),
            raw_splits: self.raw_splits.clone(),
            groups: self.groups.clone(),
            fixed_roots: self.fixed_roots.clone(),
            equiv_table: self.equiv_table.clone(),
            equiv_table_offsets: self.equiv_table_offsets.clone(),
            key_dist_config: self.key_dist_config,
            total_frequency: self.total_frequency,
            code_base: self.code_base,
            max_parts: self.max_parts,
            code_space: self.code_space,
            scale_config: self.scale_config.clone(),
            simple_config: self.simple_config.clone(),
            char_simple_infos: self.char_simple_infos.clone(),
            group_to_simple_affected: self.group_to_simple_affected.clone(),
            root_full_codes: self.root_full_codes.clone(),
            group_freq_sum: self.group_freq_sum.clone(),
            code_base_powers: self.code_base_powers.clone(),
            group_char_masks: self.group_char_masks.clone(),
            need_equiv: self.need_equiv,
            need_collision_rate: self.need_collision_rate,
            need_bucket_members: self.need_bucket_members,
            use_keysoul: self.use_keysoul,
        }
    }
}
