// =========================================================================
// 🚀 字根编码优化器 - 主入口
// =========================================================================

use std::time::Instant;

use chrono::Local;
use rayon::prelude::*;

mod annealing;
mod calibrate;
mod config;
mod context;
mod evaluator;
mod loader;
mod output;
mod schedule;
mod simple;
mod types;
mod validate;

use crate::annealing::simulated_annealing;
use crate::calibrate::calibrate_scales;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::output::{save_results, save_summary, save_thread_results};
use crate::simple::parse_simple_code_config;
use crate::types::{SimpleCodeConfig, SimpleMetrics};

fn main() {
    let start_time = Instant::now();
    println!("=== CodeGenie 字劫算法优化器 v9 (Auto-Scaling + Simple Code Collision) ===");

    // 验证配置
    config::validate_weights();

    // 打印配置信息
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

    // 创建输出目录
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let output_dir = format!("output-{}", timestamp);
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);

    // ==================== 加载数据 ====================
    let (fixed_roots, constrained) = loader::load_fixed(config::FILE_FIXED);
    let dynamic_groups = loader::load_dynamic(config::FILE_DYNAMIC, &constrained);
    let splits = loader::load_splits(config::FILE_SPLITS);
    let equiv_table = loader::load_pair_equivalence(config::FILE_PAIR_EQUIV);
    let key_dist_config = loader::load_key_distribution(config::FILE_KEY_DIST);

    // 加载简码配置
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

    // 打印数据统计
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

    // 警告：最大码长超过配置
    if max_parts_in_data > config::MAX_PARTS {
        println!(
            "⚠️ 拆分表中最大码长({})超过 MAX_PARTS({}), 请调大 config::MAX_PARTS",
            max_parts_in_data,
            config::MAX_PARTS
        );
    }

    // ==================== 校验 ====================
    if !validate::check_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }

    // ==================== 初始校准 ====================
    println!("\n📐 正在进行初始尺度校准...");
    let temp_scale = types::ScaleConfig::default();
    let temp_ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
        temp_scale,
        simple_config.clone(),
    );

    let initial_assignment = annealing::smart_init(&temp_ctx);
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

    // 逻辑根验证（仅在启用简码时）
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
                    .map(|&p| types::key_to_char(temp_ctx.resolve_key(p, &initial_assignment)))
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
                            types::key_to_char(temp_ctx.resolve_key(part, &initial_assignment))
                        })
                        .collect();
                    let level_cfg = &temp_ctx.simple_config.levels[li];
                    let mut matched_rule_idx = 0;
                    for (ri, rule) in level_cfg.rule_candidates.iter().enumerate() {
                        if types::try_resolve_rule(rule, &si.logical_roots, si.logical_roots.len())
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

    // ==================== 正式优化 ====================
    let equiv_table_2 = loader::load_pair_equivalence(config::FILE_PAIR_EQUIV);
    let key_dist_config_2 = loader::load_key_distribution(config::FILE_KEY_DIST);

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

    let root_usage = output::count_root_usage(&ctx);

    // 并行执行模拟退火
    println!("\n🚀 开始优化...");
    let results: Vec<(Vec<u8>, f64, types::Metrics, SimpleMetrics)> = (0..config::NUM_THREADS)
        .into_par_iter()
        .map(|i| simulated_annealing(&ctx, i))
        .collect();

    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics, SimpleMetrics)> = results
        .into_iter()
        .enumerate()
        .map(|(i, (a, s, m, sm))| (i, a, s, m, sm))
        .collect();

    // 找出最优结果
    let (best_thread, best_assignment, best_score, best_metrics, best_simple_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m, sm)| (*tid, a.clone(), *s, *m, *sm))
        .unwrap();

    let elapsed = start_time.elapsed();

    // 打印最优结果
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

    // ==================== 保存结果 ====================
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
