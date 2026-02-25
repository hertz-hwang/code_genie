// =========================================================================
// 🔍 校验模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::types::{extract_base_name, RootGroup};

/// 校验字根定义是否完整
/// 
/// # 参数
/// - `splits`: 拆分表数据
/// - `fixed`: 固定字根映射  
/// - `groups`: 字根组列表
/// 
/// # 返回值
/// - (是否有效, 缺失字根列表, 缺失字根的使用示例)
pub fn validate_roots(
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

    // 收集使用到的字根
    let mut used: HashMap<String, Vec<char>> = HashMap::new();
    for (ch, roots, _) in splits {
        for r in roots {
            used.entry(r.clone()).or_default().push(*ch);
        }
    }

    // 找出缺失的字根
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

/// 检查字根定义并报告结果
pub fn check_validation(
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
    
    // 打印详细的缺失信息
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

    // 保存报告到文件
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
