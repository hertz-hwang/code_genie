// =========================================================================
// 📤 输出模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::context::OptContext;
use crate::config::Config;
use crate::evaluator::SimpleEvaluator;
use crate::types::{
    extract_base_name, extract_suffix_num, key_to_char, Metrics, SimpleMetrics, SimpleCodeConfig,
    try_resolve_rule, LogicalRoot,
};

/// 统计字根使用频率
pub fn count_root_usage(ctx: &OptContext) -> HashMap<String, u64> {
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

/// 构建排序后的根编码列表
pub fn build_root_encodings_sorted(
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
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

    // 按使用频率降序排序
    result.sort_by(|a, b| {
        let ua = root_usage.get(&a.0).copied().unwrap_or(0);
        let ub = root_usage.get(&b.0).copied().unwrap_or(0);
        ub.cmp(&ua).then_with(|| a.0.cmp(&b.0))
    });

    result
}

/// 格式化编码为可读字符串
pub fn format_encoding(keys: &[u8]) -> String {
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

/// 写入键位映射输出
pub fn write_keymap_output(
    root_out: &mut String,
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
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

/// 检查简码码位是否被全码占用（与 SimpleEvaluator::is_code_occupied_by_full 逻辑一致）
fn is_code_occupied_by_full(
    full_code_to_chars: &[Vec<usize>],
    code: usize,
    assigned: &HashSet<usize>,
) -> bool {
    if code >= full_code_to_chars.len() {
        return false;
    }
    let chars = &full_code_to_chars[code];
    for &ci in chars {
        if !assigned.contains(&ci) {
            return true; // 该码位上有未出简的全码字
        }
    }
    false
}

/// 保存简码+全码编码文件
pub fn save_combined_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    // 构建根名到键位的映射
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

    // 构建全码桶
    let n_chars = ctx.char_infos.len();
    let cs = ctx.code_space;
    let mut full_code_to_chars: Vec<Vec<usize>> = vec![Vec::new(); cs];
    for ci in 0..n_chars {
        let code = ctx.calc_code_only(ci, assignment);
        full_code_to_chars[code].push(ci);
    }

    let mut out = String::new();
    let mut simple_assigned: HashSet<usize> = HashSet::new();

    // 按字频排序
    let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
    sorted_chars.sort_by(|&a, &b| {
        ctx.char_infos[b]
            .frequency
            .cmp(&ctx.char_infos[a].frequency)
    });

    // 简码部分 - 与 save_simple_code_output 完全相同的逻辑
    for (li, level_cfg) in ctx.simple_config.levels.iter().enumerate() {
        let mut code_candidates: HashMap<usize, Vec<(usize, u64)>> = HashMap::new();
        let allowed_len = level_cfg.allowed_orig_length;

        for &ci in &sorted_chars {
            if simple_assigned.contains(&ci) {
                continue;
            }
            if allowed_len != 0 && ctx.char_infos[ci].parts_len as usize != allowed_len {
                continue;
            }
            if let Some(code) = ctx.calc_simple_code(ci, li, assignment) {
                code_candidates
                    .entry(code)
                    .or_default()
                    .push((ci, ctx.char_infos[ci].frequency));
            }
        }

        // 收集该级别所有获胜者
        let mut level_winners: Vec<(usize, u64, String)> = Vec::new();

        for (&code, candidates) in &code_candidates {
            // 全码占位检查：如果该码位上有未出简的全码字，跳过
            if is_code_occupied_by_full(&full_code_to_chars, code, &simple_assigned) {
                continue;
            }

            let mut count = 0;
            for &(ci, freq) in candidates {
                if count >= level_cfg.code_num {
                    break;
                }
                if simple_assigned.contains(&ci) {
                    continue;
                }

                let ch = ctx.raw_splits[ci].0;
                let code_str: String = ctx
                    .get_simple_keys(ci, li, assignment)
                    .map(|keys| keys.iter().map(|&k| key_to_char(k)).collect())
                    .unwrap_or_else(|| String::from("?"));

                level_winners.push((ci, freq, format!("{}\t{}", ch, code_str)));
                count += 1;
            }
        }

        // 按字频排序输出
        level_winners.sort_by(|a, b| b.1.cmp(&a.1));

        for (ci, _, line) in &level_winners {
            out.push_str(line);
            out.push('\n');
            simple_assigned.insert(*ci);
        }
    }

    // 全码部分 - 所有字都输出全码
    for &ci in &sorted_chars {
        let ch = ctx.raw_splits[ci].0;
        let (_, roots, _) = &ctx.raw_splits[ci];
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        out.push_str(&format!("{}\t{}\n", ch, code_str));
    }

    fs::write(format!("{}/output-combined.txt", dir), out).unwrap();
}

/// 保存简码输出
pub fn save_simple_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    if !ctx.enable_simple_code || ctx.simple_config.levels.is_empty() {
        return;
    }

    // 构建全码到汉字的映射
    let n = ctx.char_infos.len();
    let cs = ctx.code_space;
    let mut full_code_to_chars: Vec<Vec<usize>> = vec![Vec::new(); cs];
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        full_code_to_chars[code].push(ci);
    }

    // 构建非空桶索引
    let mut populated_codes = Vec::new();
    for code in 0..cs {
        if !full_code_to_chars[code].is_empty() {
            populated_codes.push(code);
        }
    }

    let se = SimpleEvaluator::new(ctx, assignment, &populated_codes, &full_code_to_chars);
    let sm = se.get_simple_metrics(ctx);

    let mut out = String::new();
    out.push_str("# 简码分配结果\n");
    out.push_str(&format!("# 简码加权码长: {:.4}\n", sm.weighted_key_length));
    out.push_str(&format!("# 简码加权当量: {:.4}\n", sm.equiv_mean));
    out.push_str(&format!("# 简码分布偏差: {:.4}\n", sm.dist_deviation));
    out.push_str(&format!("# 简码重码数: {}\n", sm.collision_count));
    out.push_str(&format!("# 简码重码率: {:.6}%\n", sm.collision_rate * 100.0));
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
        let allowed_len = level_cfg.allowed_orig_length;

        for &ci in &sorted_chars {
            if globally_assigned.contains(&ci) {
                continue;
            }
            if allowed_len != 0 && ctx.char_infos[ci].parts_len as usize != allowed_len {
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

        for (&code, candidates) in &code_candidates {
            // 全码占位检查：如果该码位上有未出简的全码字，跳过
            if is_code_occupied_by_full(&full_code_to_chars, code, &globally_assigned) {
                continue;
            }

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

/// 保存线程结果
pub fn save_thread_results(
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
    root_out.push_str(&format!("# 用指分布偏差(L2): {:.4}\n", metrics.dist_deviation));
    if ctx.enable_simple_code {
        root_out.push_str(&format!("# 简码加权码长: {:.4}\n", simple_metrics.weighted_key_length));
        root_out.push_str(&format!("# 简码加权当量: {:.4}\n", simple_metrics.equiv_mean));
        root_out.push_str(&format!("# 简码分布偏差: {:.4}\n", simple_metrics.dist_deviation));
        root_out.push_str(&format!("# 简码重码数: {}\n", simple_metrics.collision_count));
        root_out.push_str(&format!("# 简码重码率: {:.6}%\n", simple_metrics.collision_rate * 100.0));
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

    // 保存编码结果
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
    save_combined_code_output(ctx, assignment, &thread_dir);
}

/// 保存键位分布到目录
pub fn save_key_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let _display_order = &ctx.weights; // 从 ctx 获取显示顺序需要额外存储，暂时使用默认
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
    let mut out = String::new();
    out.push_str("# 用指分布统计\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    // 使用固定的显示顺序
    let display_order = "qwertyuiopasdfghjklzxcvbnm";
    for kc in display_order.chars() {
        if let Some(ki) = crate::types::char_to_key_index(kc) {
            if ki >= 31 {
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

    let order_set: HashSet<char> = display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
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

/// 保存当量分布到目录
pub fn save_equiv_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
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

/// 保存结果到输出目录
pub fn save_results(
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
    root_out.push_str(&format!("# 用指分布偏差(L2): {:.4}\n", metrics.dist_deviation));
    if ctx.enable_simple_code {
        root_out.push_str(&format!("# 简码加权码长: {:.4}\n", simple_metrics.weighted_key_length));
        root_out.push_str(&format!("# 简码加权当量: {:.4}\n", simple_metrics.equiv_mean));
        root_out.push_str(&format!("# 简码分布偏差: {:.4}\n", simple_metrics.dist_deviation));
        root_out.push_str(&format!("# 简码重码数: {}\n", simple_metrics.collision_count));
        root_out.push_str(&format!("# 简码重码率: {:.6}%\n", simple_metrics.collision_rate * 100.0));
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

    // 保存编码结果
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
    save_combined_code_output(ctx, assignment, output_dir);

    println!(
        "结果已保存至 {}/output-keymap.txt, output-encode.txt, output-simple-codes.txt, output-combined.txt 等",
        output_dir
    );
}

/// 保存汇总信息
pub fn save_summary(
    cfg: &Config,
    results: &[(usize, Vec<u8>, f64, Metrics, SimpleMetrics, crate::types::WordMetrics)],
    best_thread: usize,
    output_dir: &str,
    elapsed: std::time::Duration,
) {
    let mut summary = String::new();
    summary.push_str("# 优化结果汇总\n");
    summary.push_str(&format!("# 输出目录: {}\n", output_dir));
    summary.push_str(&format!("# 线程数: {}\n", cfg.annealing.threads));
    summary.push_str(&format!("# 总步数: {}\n", cfg.annealing.total_steps));
    summary.push_str(&format!("# 总耗时: {:?}\n", elapsed));
    summary.push_str(&format!("# 最优线程: {}\n", best_thread));
    summary.push_str(&format!("# 简码优化: {}\n", cfg.weights.simple_code.enabled));
    summary.push_str("#\n");

    summary.push_str(&format!(
        "{:<8} {:<12} {:<10} {:<12} {:<10} {:<12} {:<12} {:<10} {:<10} {:<12}\n",
        "线程", "得分", "前N重码", "重码数", "重码率%", "当量", "分布偏差",
        "简码码长", "简码当量", "简码重码率%"
    ));
    summary.push_str(&format!("{}\n", "-".repeat(120)));

    let mut sorted: Vec<&(usize, Vec<u8>, f64, Metrics, SimpleMetrics, crate::types::WordMetrics)> = results.iter().collect();
    sorted.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    for (tid, _, score, m, sm, _wm) in &sorted {
        let marker = if *tid == best_thread { " 🏆" } else { "" };
        summary.push_str(&format!(
            "T{:<7} {:<12.4} {:<10} {:<12} {:<12.6} {:<12.4} {:<12.4} {:<10.4} {:<10.4} {:<12.6}{}\n",
            tid, score, m.top_n_collision_count, m.collision_count, m.collision_rate * 100.0,
            m.equiv_mean, m.dist_deviation,
            sm.weighted_key_length, sm.equiv_mean, sm.collision_rate * 100.0,
            marker
        ));
    }

    fs::write(format!("{}/summary.txt", output_dir), summary).unwrap();
}

// =========================================================================
// encode 子命令简码前缀生成
// =========================================================================

/// 为 encode 子命令生成简码前缀
///
/// 根据 `simple_config` 中的规则，从 `root_to_key` 和 `splits` 计算各级简码，
/// 返回 (简码文本, 简码条目数)。简码文本格式与全码相同：`汉字\t编码\t字频\n`，
/// 按字频降序排列，各级简码依次前置。
pub fn build_simple_prefix_for_encode(
    root_to_key: &HashMap<String, u8>,
    splits: &[(char, Vec<String>, u64)],
    simple_config: &SimpleCodeConfig,
) -> (String, usize) {
    if simple_config.levels.is_empty() {
        return (String::new(), 0);
    }

    // 按字频降序排序字符索引
    let mut sorted_indices: Vec<usize> = (0..splits.len()).collect();
    sorted_indices.sort_by(|&a, &b| splits[b].2.cmp(&splits[a].2));

    // 构建全码映射：字符索引 → 全码字符串（用于碰撞检测）
    // 全码 = 各根键位字符拼接
    let full_codes: Vec<Option<String>> = splits
        .iter()
        .map(|(_, roots, _)| {
            let mut parts = Vec::new();
            for root in roots {
                if let Some(&key) = root_to_key.get(root) {
                    parts.push(key_to_char(key));
                } else {
                    return None; // 有缺失字根，跳过
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.into_iter().collect::<String>())
            }
        })
        .collect();

    // 构建全码到字符索引的映射（用于全码占位检查）
    let mut full_code_to_chars: HashMap<String, Vec<usize>> = HashMap::new();
    for (ci, fc) in full_codes.iter().enumerate() {
        if let Some(code) = fc {
            full_code_to_chars.entry(code.clone()).or_default().push(ci);
        }
    }

    // 为每个字构建逻辑根列表（用于简码规则解析）
    // 逻辑根：按 extract_base_name 分组，同名多变体合并
    let char_logical_roots: Vec<Vec<LogicalRoot>> = splits
        .iter()
        .map(|(_, roots, _)| build_logical_roots_for_encode(roots, root_to_key))
        .collect();

    let mut out = String::new();
    let mut total_count = 0usize;
    let mut globally_assigned: HashSet<usize> = HashSet::new();

    for level_cfg in &simple_config.levels {
        // 对每个字，尝试用该级规则计算简码键位序列
        // 简码字符串 → Vec<(字符索引, 字频)>
        let mut code_to_candidates: HashMap<String, Vec<(usize, u64)>> = HashMap::new();

        for &ci in &sorted_indices {
            if globally_assigned.contains(&ci) {
                continue;
            }
            // allowed_orig_length 约束：0 = 不限制
            if level_cfg.allowed_orig_length != 0 {
                let n_roots = splits[ci].1.len();
                if n_roots != level_cfg.allowed_orig_length {
                    continue;
                }
            }
            let lr = &char_logical_roots[ci];
            let n_roots = lr.len();
            // 尝试每个候选规则，取第一个成功的
            let simple_keys: Option<Vec<char>> = level_cfg.rule_candidates.iter().find_map(|rule| {
                let instructions = try_resolve_rule(rule, lr, n_roots)?;
                let keys: Vec<char> = instructions
                    .iter()
                    .map(|&(root_idx, code_idx)| {
                        let lroot = &lr[root_idx];
                        let key = lroot.full_code_parts.get(code_idx).copied()?;
                        Some(key_to_char(key as u8))
                    })
                    .collect::<Option<Vec<char>>>()?;
                Some(keys)
            });

            if let Some(keys) = simple_keys {
                let code_str: String = keys.into_iter().collect();
                code_to_candidates
                    .entry(code_str)
                    .or_default()
                    .push((ci, splits[ci].2));
            }
        }

        // 对每个简码，按字频取前 code_num 个字（全码占位检查）
        let mut level_winners: Vec<(usize, u64, char, String)> = Vec::new();

        for (code_str, candidates) in &code_to_candidates {
            // 全码占位检查：若该码位上有未出简的全码字，跳过
            if is_full_code_occupied(&full_code_to_chars, code_str, &globally_assigned) {
                continue;
            }

            let mut count = 0usize;
            for &(ci, freq) in candidates {
                if level_cfg.code_num > 0 && count >= level_cfg.code_num {
                    break;
                }
                if globally_assigned.contains(&ci) {
                    continue;
                }
                let ch = splits[ci].0;
                level_winners.push((ci, freq, ch, code_str.clone()));
                count += 1;
            }
        }

        // 按字频降序排序后输出
        level_winners.sort_by(|a, b| b.1.cmp(&a.1));

        for (ci, freq, ch, code_str) in level_winners {
            out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            globally_assigned.insert(ci);
            total_count += 1;
        }
    }

    (out, total_count)
}

/// 检查某个简码字符串是否被全码占用
/// （即该码位上存在尚未出简的全码字）
fn is_full_code_occupied(
    full_code_to_chars: &HashMap<String, Vec<usize>>,
    code: &str,
    assigned: &HashSet<usize>,
) -> bool {
    if let Some(chars) = full_code_to_chars.get(code) {
        chars.iter().any(|ci| !assigned.contains(ci))
    } else {
        false
    }
}

/// 为 encode 子命令构建逻辑根列表
///
/// 根据根名列表和 root_to_key 映射，按 extract_base_name 分组，
/// 构建 LogicalRoot 列表（full_code_parts 为键位索引序列）。
fn build_logical_roots_for_encode(
    roots: &[String],
    root_to_key: &HashMap<String, u8>,
) -> Vec<LogicalRoot> {
    let mut logical_roots: Vec<LogicalRoot> = Vec::new();

    for (idx, name) in roots.iter().enumerate() {
        let base = extract_base_name(name);
        let suffix = extract_suffix_num(name);

        if suffix <= 0 {
            // 新逻辑根：收集同 base 的所有变体键位
            // 在 encode 场景下，root_to_key 是扁平的，每个根名对应一个键位
            // 逻辑根的 full_code_parts 就是该根名对应的键位（单元素）
            let key = root_to_key.get(name).copied();
            let full_code_parts: Vec<u16> = key.map(|k| vec![k as u16]).unwrap_or_default();

            logical_roots.push(LogicalRoot {
                base_name: base,
                split_part_indices: vec![idx],
                full_code_parts,
            });
        } else {
            // 带数字后缀：附加到同名逻辑根
            let mut attached = false;
            for lr in logical_roots.iter_mut().rev() {
                if lr.base_name == base {
                    lr.split_part_indices.push(idx);
                    // 追加该变体的键位
                    if let Some(&key) = root_to_key.get(name) {
                        lr.full_code_parts.push(key as u16);
                    }
                    attached = true;
                    break;
                }
            }
            if !attached {
                let key = root_to_key.get(name).copied();
                let full_code_parts: Vec<u16> = key.map(|k| vec![k as u16]).unwrap_or_default();
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