// =========================================================================
// 📂 文件加载模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::types::{char_to_key_index, KeyDistConfig, RootGroup, WordInfo, EQUIV_TABLE_SIZE, GROUP_MARKER, MAX_PARTS};

/// 加载固定字根和受限字根组
/// 
/// # 返回值
/// - (固定字根映射, 受限字根组)
pub fn load_fixed(path: &str) -> (HashMap<String, u8>, Vec<RootGroup>) {
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

/// 加载动态字根组
pub fn load_dynamic(path: &str, constrained: &[RootGroup], allowed_keys: &str) -> Vec<RootGroup> {
    let global_allowed: Vec<u8> = allowed_keys
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

/// 加载拆分表
/// 
/// # 返回值
/// - Vec<(字符, 根名列表, 频率)>
pub fn load_splits(path: &str) -> Vec<(char, Vec<String>, u64)> {
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

/// 加载字根对当量表
/// 
/// # 返回值
/// - 31x31 当量矩阵
pub fn load_pair_equivalence(path: &str) -> [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE] {
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

/// 从 keymap 文件加载字根到键位的映射
///
/// keymap 文件格式: 字根名\t编码\t使用次数
/// 编码格式: 首字母大写的键位字符串，如 Wko -> [w, k, o]
///
/// 需要同时传入 division 文件路径，以确定每个基础字根的实际子字根后缀列表。
/// 例如 keymap 中 `口	Wko` 表示口的编码为 [w, k, o]，
/// 而 division 中口的子字根为 口、口.1、口.2，
/// 因此映射为: 口=w, 口.1=k, 口.2=o
///
/// # 返回值
/// - HashMap<String, u8>: 子字根名 -> 键位索引
pub fn load_keymap(keymap_path: &str, division_path: &str) -> HashMap<String, u8> {
    use crate::types::{extract_base_name, extract_suffix_num};

    // 第一步：从 division 文件中提取每个基础字根的子字根后缀列表
    let splits = load_splits(division_path);
    let mut base_to_suffixes: HashMap<String, Vec<i32>> = HashMap::new();

    for (_, roots, _) in &splits {
        for root in roots {
            let base = extract_base_name(root);
            let suffix = extract_suffix_num(root);
            let entry = base_to_suffixes.entry(base).or_default();
            if !entry.contains(&suffix) {
                entry.push(suffix);
            }
        }
    }

    // 确保每个基础字根的后缀列表包含 -1（即 keymap 中的无后缀条目对应第1码）
    for suffixes in base_to_suffixes.values_mut() {
        if !suffixes.contains(&-1) {
            suffixes.push(-1);
        }
        suffixes.sort();
    }

    // 第二步：解析 keymap 文件
    let content = fs::read_to_string(keymap_path).expect("无法读取 keymap 文件");
    let mut root_to_key: HashMap<String, u8> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let base_name = parts[0].trim();
        let encoding = parts[1].trim().to_lowercase();
        let keys: Vec<u8> = encoding
            .chars()
            .filter_map(|c| char_to_key_index(c).map(|i| i as u8))
            .collect();

        if keys.is_empty() {
            continue;
        }

        // 获取该基础字根的后缀列表
        let suffixes = base_to_suffixes
            .get(base_name)
            .cloned()
            .unwrap_or_else(|| {
                // 如果 division 中没有该字根，使用默认后缀 [-1, 1, 2, ...]
                let mut default_suffixes = vec![-1];
                for i in 1..keys.len() as i32 {
                    default_suffixes.push(i);
                }
                default_suffixes
            });

        // 将键位按后缀顺序映射到子字根名
        for (i, &key) in keys.iter().enumerate() {
            if i < suffixes.len() {
                let suffix = suffixes[i];
                let sub_name = if suffix < 0 {
                    base_name.to_string()
                } else {
                    format!("{}.{}", base_name, suffix)
                };
                root_to_key.insert(sub_name, key);
            }
        }
    }

    root_to_key
}

/// 加载键位分布配置
/// 
/// # 返回值
/// - 31 个键位的分布配置
pub fn load_key_distribution(path: &str) -> [KeyDistConfig; EQUIV_TABLE_SIZE] {
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

/// 加载多字词拆分表，返回按词频降序排列的 WordInfo 列表
///
/// 格式：词\t字根1 字根2 ...\t词频
/// 字根序列直接用于编码（与单字全码相同逻辑，取前 max_parts 个字根）
pub fn load_word_divisions(
    path: &str,
    fixed_roots: &HashMap<String, u8>,
    root_to_group: &HashMap<String, usize>,
    max_parts: usize,
) -> Vec<WordInfo> {
    use crate::types::extract_base_name;

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("⚠️ 无法读取词码拆分文件 {}: {}", path, e);
            return Vec::new();
        }
    };

    let mut raw: Vec<(Vec<String>, u64)> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let roots: Vec<String> = parts[1].split_whitespace().map(|s| s.to_string()).collect();
        let freq: u64 = if parts.len() >= 3 {
            parts[2].trim().parse().unwrap_or(1)
        } else {
            1
        };
        if !roots.is_empty() {
            raw.push((roots, freq));
        }
    }

    // 按词频降序排列，用于标记 top-2000 / top-10000
    raw.sort_by(|a, b| b.1.cmp(&a.1));

    let mut result = Vec::with_capacity(raw.len());
    for (rank, (roots, freq)) in raw.into_iter().enumerate() {
        let mut info = WordInfo {
            parts: [0u16; MAX_PARTS],
            parts_len: 0,
            frequency: freq,
            current_code: 0,
            current_key_indices: [0u16; MAX_PARTS],
            is_top2000: rank < 2000,
            is_top10000: rank < 10000,
        };

        for root in &roots {
            let idx = info.parts_len as usize;
            if idx >= max_parts {
                break;
            }
            let base = extract_base_name(root);
            if let Some(&key) = fixed_roots.get(root).or_else(|| fixed_roots.get(&base)) {
                info.parts[idx] = key as u16;
                info.current_key_indices[idx] = key as u16;
                info.parts_len += 1;
            } else if let Some(&gi) = root_to_group.get(root).or_else(|| root_to_group.get(&base)) {
                info.parts[idx] = gi as u16 + GROUP_MARKER;
                info.current_key_indices[idx] = gi as u16 + GROUP_MARKER;
                info.parts_len += 1;
            }
            // 未知字根跳过（不增加 parts_len）
        }

        if info.parts_len > 0 {
            result.push(info);
        }
    }

    result
}
