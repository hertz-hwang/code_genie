// =========================================================================
// 📦 主上下文
// =========================================================================

use std::collections::{HashMap, HashSet};

use crate::config;
use crate::types::{
    build_root_full_codes, CharInfo, CharSimpleInfo, compute_level_instructions,
    extract_logical_roots_full, try_resolve_rule, KeyDistConfig,
    LogicalRoot, RootGroup, ScaleConfig, SimpleCodeConfig, KEY_SPACE, EQUIV_TABLE_SIZE,
    GROUP_MARKER,
};

/// 等价表类型别名
pub type EquivTable = [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

/// 优化上下文 - 存储所有算法需要的数据
pub struct OptContext {
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
    /// 当量表
    pub equiv_table: EquivTable,
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
}

impl OptContext {
    /// 创建新的优化上下文
    pub fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: EquivTable,
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
        let code_space = crate::types::pow_base(code_base, max_parts);

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

    /// 解析键位 - 将部分索引解析为实际键位
    #[inline(always)]
    pub fn resolve_key(&self, part: u16, assignment: &[u8]) -> u8 {
        if part >= GROUP_MARKER {
            assignment[(part - GROUP_MARKER) as usize]
        } else {
            part as u8
        }
    }

    /// 计算仅全码
    #[inline(always)]
    pub fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in &info.parts {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 从拆分计算等价值
    #[inline(always)]
    pub fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> f64 {
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

    /// 计算简码等价值
    #[inline]
    pub fn calc_simple_equiv(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> f64 {
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
