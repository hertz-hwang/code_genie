// =========================================================================
// 📝 简码规则解析
// =========================================================================

use std::collections::HashMap;
use std::fs;

use crate::types::{SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep};

/// 解析简码配置文件
pub fn parse_simple_code_config(path: &str) -> SimpleCodeConfig {
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
            allowed_orig_length: 0,
        });
    }

    levels.sort_by_key(|l| l.level);
    SimpleCodeConfig { levels }
}
