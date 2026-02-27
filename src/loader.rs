// =========================================================================
// 📂 文件加载模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::types::{char_to_key_index, KeyDistConfig, RootGroup, KEY_SPACE};

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
pub fn load_pair_equivalence(path: &str) -> [[f64; 31]; 31] {
    let mut table = [[0.0f64; 31]; 31];
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
                        if k1 < 31 && k2 < 31 {
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

    // 对每个基础字根的后缀列表排序
    for suffixes in base_to_suffixes.values_mut() {
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
pub fn load_key_distribution(path: &str) -> [KeyDistConfig; 31] {
    let mut cfg = [KeyDistConfig::default(); 31];
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
                if ki < 31 {
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
