// =========================================================================
// 📦 主上下文
// =========================================================================

use std::collections::{HashMap, HashSet};

use crate::types::{
    build_root_full_codes, CharInfo, CharSimpleInfo, compute_level_instructions,
    extract_logical_roots_full, try_resolve_rule, KeyDistConfig,
    RootGroup, ScaleConfig, SimpleCodeConfig, WeightConfig, WordInfo,
    KEY_SPACE, EQUIV_TABLE_SIZE, GROUP_MARKER, MAX_PARTS,
};
use crate::keysoul;

/// pair 当量表类型（从文件加载后传入 OptContext::new）
pub type PairEquivTable = [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

/// 优化上下文 - 存储所有算法需要的数据
pub struct OptContext {
    /// 是否启用简码优化
    pub enable_simple_code: bool,
    /// 是否启用词码优化
    pub enable_word_code: bool,
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
    pub equiv_table: Vec<f64>,
    /// equiv_table 各长度的起始偏移
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
    /// 每个组的加权频率总和
    pub group_freq_sum: Vec<f64>,
    /// code_base 的幂次表
    pub code_base_powers: Vec<usize>,
    /// 增量编码掩码（CSR 格式，单字）
    pub gcm_data: Vec<(usize, usize)>,
    /// CSR 偏移数组（单字），长度 = num_groups + 1
    pub gcm_offsets: Vec<usize>,

    // ── 词码数据 ──
    /// 多字词信息列表（按词频降序）
    pub word_infos: Vec<WordInfo>,
    /// 组索引到使用该组的词索引列表（反向索引）
    pub group_to_words: Vec<Vec<usize>>,
    /// 增量编码掩码（CSR 格式，词码）
    pub word_gcm_data: Vec<(usize, usize)>,
    /// CSR 偏移数组（词码），长度 = num_groups + 1
    pub word_gcm_offsets: Vec<usize>,
    /// 词码总频率
    pub word_total_frequency: u64,

    // ── 单字全码 top-N 标记 ──
    /// 字频前 N 的单字标记（top_n 由 weights.full_top_n 决定）
    pub top_n_char_flags: Vec<bool>,

    // ── 简码加权码长基准值 ──
    /// Σ(freq_i * full_code_len_i)，用于增量计算加权码长
    pub base_wkl: f64,

    // === 热路径优化标志 ===
    pub need_equiv: bool,
    pub need_collision_rate: bool,
    pub need_bucket_members: bool,
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
        word_infos_input: Vec<WordInfo>,
    ) -> Self {
        let enable_simple_code = weights.enable_simple_code;
        let enable_word_code = weights.enable_word_code;
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
                parts: [0u16; MAX_PARTS],
                parts_len: 0,
                frequency: *freq,
                current_code: 0,
                current_key_indices: [0u16; MAX_PARTS],
            };

            let mut seen_groups = HashSet::new();

            for root in roots {
                let idx = info.parts_len as usize;
                assert!(idx < MAX_PARTS, "parts 超出 MAX_PARTS={MAX_PARTS}，请增大该常量");
                if let Some(&key) = fixed_roots.get(root) {
                    info.parts[idx] = key as u16;
                    info.current_key_indices[idx] = key as u16;
                    info.parts_len += 1;
                } else if let Some(&gi) = root_to_group.get(root) {
                    info.parts[idx] = gi as u16 + GROUP_MARKER;
                    info.current_key_indices[idx] = gi as u16 + GROUP_MARKER;
                    info.parts_len += 1;
                    seen_groups.insert(gi);
                }
            }

            if info.parts_len as usize > max_parts {
                max_parts = info.parts_len as usize;
            }

            for &gi in &seen_groups {
                group_to_chars[gi].push(ci);
            }

            total_frequency += freq;

            let logical_roots = extract_logical_roots_full(
                roots,
                info.parts_slice(),
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

            info.current_key_indices = info.parts;

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
            for &p in char_infos[ci].parts_slice() {
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

        let mut group_char_masks_tmp: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_groups];
        for (ci, info) in char_infos.iter().enumerate() {
            let n = info.parts_len as usize;
            let mut group_mask: HashMap<usize, usize> = HashMap::new();
            for (pos, &p) in info.parts_slice().iter().enumerate() {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    *group_mask.entry(gi).or_insert(0) += code_base_powers[n - 1 - pos];
                }
            }
            for (gi, mask) in group_mask {
                group_char_masks_tmp[gi].push((ci, mask));
            }
        }

        // CSR 扁平化：消除内层 Vec 的堆分配，提升热路径缓存局部性
        let mut gcm_offsets = Vec::with_capacity(num_groups + 1);
        let mut gcm_data: Vec<(usize, usize)> = Vec::new();
        gcm_offsets.push(0usize);
        for g in &group_char_masks_tmp {
            gcm_data.extend_from_slice(g);
            gcm_offsets.push(gcm_data.len());
        }

        let need_equiv = weights.full_equivalence > 0.0;
        let need_collision_rate = weights.full_collision_rate > 0.0;
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

        // ── 计算 base_wkl（简码加权码长基准值）──
        let base_wkl: f64 = char_infos.iter().map(|ci| {
            ci.frequency as f64 * ci.parts_len as f64
        }).sum();

        // ── 计算 top-N 单字标记 ──
        let top_n = weights.full_top_n;
        let mut sorted_char_indices: Vec<usize> = (0..char_infos.len()).collect();
        sorted_char_indices.sort_by(|&a, &b| char_infos[b].frequency.cmp(&char_infos[a].frequency));
        let mut top_n_char_flags = vec![false; char_infos.len()];
        for &ci in sorted_char_indices.iter().take(top_n) {
            top_n_char_flags[ci] = true;
        }

        // ── 构建词码上下文 ──
        let mut word_infos = word_infos_input;
        let mut group_to_words = vec![Vec::new(); num_groups];
        let mut word_gcm_tmp: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_groups];
        let mut word_total_frequency = 0u64;

        for (wi, winfo) in word_infos.iter_mut().enumerate() {
            word_total_frequency += winfo.frequency;
            let n = winfo.parts_len as usize;
            let mut group_mask: HashMap<usize, usize> = HashMap::new();
            let mut seen_groups = HashSet::new();
            for (pos, &p) in winfo.parts_slice().iter().enumerate() {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    *group_mask.entry(gi).or_insert(0) += code_base_powers[n - 1 - pos];
                    seen_groups.insert(gi);
                }
            }
            for gi in seen_groups {
                group_to_words[gi].push(wi);
            }
            for (gi, mask) in group_mask {
                word_gcm_tmp[gi].push((wi, mask));
            }
        }

        let mut word_gcm_offsets = Vec::with_capacity(num_groups + 1);
        let mut word_gcm_data: Vec<(usize, usize)> = Vec::new();
        word_gcm_offsets.push(0usize);
        for g in &word_gcm_tmp {
            word_gcm_data.extend_from_slice(g);
            word_gcm_offsets.push(word_gcm_data.len());
        }

        Self {
            enable_simple_code,
            enable_word_code,
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
            gcm_data,
            gcm_offsets,
            word_infos,
            group_to_words,
            word_gcm_data,
            word_gcm_offsets,
            word_total_frequency,
            top_n_char_flags,
            base_wkl,
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
        for &p in info.parts_slice() {
            key_indices.push(self.resolve_key(p, assignment) as u16);
        }
    }

    /// 计算仅全码（使用临时缓冲区）
    #[allow(dead_code)]
    pub fn calc_code_only_fast(&self, ci: usize, key_indices: &[u16]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &ki in key_indices.iter().take(info.parts_len as usize) {
            code = code * self.code_base + (ki as usize + 1);
        }
        code
    }

    /// 计算仅全码
    pub fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in info.parts_slice() {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 计算词码
    #[inline(always)]
    pub fn calc_word_code(&self, wi: usize, assignment: &[u8]) -> usize {
        let info = &self.word_infos[wi];
        let mut code = 0usize;
        for &p in info.parts_slice() {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 从词码拆分计算当量
    #[inline(always)]
    pub fn calc_word_equiv(&self, wi: usize, assignment: &[u8]) -> f64 {
        let info = &self.word_infos[wi];
        let n = info.parts_len as usize;
        if n == 0 || n >= self.equiv_table_offsets.len() {
            return 0.0;
        }
        let base = self.equiv_table_offsets[n];
        let key_count = EQUIV_TABLE_SIZE;
        let mut flat = 0usize;
        for &p in info.parts_slice() {
            flat = flat * key_count + self.resolve_key(p, assignment) as usize;
        }
        self.equiv_table[base + flat]
    }

    /// 从拆分计算等价值（最长序列匹配查表）
    #[inline(always)]
    pub fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> f64 {
        let info = &self.char_infos[ci];
        let n = info.parts_len as usize;
        if n == 0 || n >= self.equiv_table_offsets.len() {
            return 0.0;
        }
        let base = self.equiv_table_offsets[n];
        let key_count = EQUIV_TABLE_SIZE;
        let mut flat = 0usize;
        for &p in info.parts_slice() {
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
            enable_word_code: self.enable_word_code,
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
            gcm_data: self.gcm_data.clone(),
            gcm_offsets: self.gcm_offsets.clone(),
            word_infos: self.word_infos.clone(),
            group_to_words: self.group_to_words.clone(),
            word_gcm_data: self.word_gcm_data.clone(),
            word_gcm_offsets: self.word_gcm_offsets.clone(),
            word_total_frequency: self.word_total_frequency,
            top_n_char_flags: self.top_n_char_flags.clone(),
            base_wkl: self.base_wkl,
            need_equiv: self.need_equiv,
            need_collision_rate: self.need_collision_rate,
            need_bucket_members: self.need_bucket_members,
            use_keysoul: self.use_keysoul,
        }
    }
}
