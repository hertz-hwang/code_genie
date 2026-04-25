// =========================================================================
// 🚀 字根编码优化器 - 主入口
// =========================================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use clap::{Parser, Subcommand};
use rayon::prelude::*;

mod amhb;
mod annealing;
mod calibrate;
mod checkpoint;
mod config;
mod context;
mod evaluator;
mod keysoul;
mod loader;
mod output;
mod schedule;
mod simple;
mod types;
mod validate;

use crate::amhb::optimizer::{AmhbOptimizer, AmhbParameters};
use crate::annealing::{random_init, simulated_annealing_resumable, SaResult};
use crate::calibrate::calibrate_scales;
use crate::checkpoint::{
    save_checkpoint, load_checkpoint, Checkpoint, CHECKPOINT_FILENAME, CHECKPOINT_VERSION,
};
use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::output::{save_results, save_summary, save_thread_results};
use crate::types::{
    key_to_char, SimpleCodeConfig, SimpleMetrics, WordMetrics, EQUIV_TABLE_SIZE, KeyDistConfig,
};

// =========================================================================
// CLI 定义
// =========================================================================

#[derive(Parser)]
#[command(name = "CodeGenie", about = "字根编码优化器", version)]
struct Cli {
    /// 配置文件路径
    #[arg(short = 'c', long, default_value = "config.toml")]
    config: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行模拟退火优化（默认行为）
    Optimize {
        /// 使用 AMHB 算法 instead of SA
        #[arg(short, long)]
        amhb: bool,

        /// 使用键魂当量模型替代 pair_equivalence.txt
        #[arg(long)]
        keysoul: bool,
    },

    /// 根据 keymap 为汉字编码
    Encode {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 编码输出文件
        #[arg(short = 'o', long, default_value = "output-encode.txt")]
        output: String,

        /// 配置文件路径（用于读取 simple_levels，在输出前部插入简码）
        #[arg(long)]
        config: Option<String>,
    },

    /// 全方位评估编码方案
    Evaluate {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 目标键位分布文件
        #[arg(long)]
        keydist: Option<String>,

        /// 当量数据文件
        #[arg(long)]
        equiv: Option<String>,

        /// 简码规则文件（可选）
        #[arg(long)]
        simple: Option<String>,

        /// 使用键魂当量模型替代 pair_equivalence.txt
        #[arg(long)]
        keysoul: bool,

        /// 评估输出文件
        #[arg(short = 'o', long, default_value = "output-evaluate.txt")]
        output: String,
    },

    /// 从检查点恢复优化（断点续算）
    Resume {
        /// 检查点文件路径
        #[arg(short = 'f', long, default_value = "checkpoint.json")]
        checkpoint: String,
    },

    /// 评估键魂当量（击键序列时间分析）
    Keysoul {
        /// 按键序列（如 "dkfj"）
        sequence: String,

        /// 显示每个键对的详细时间分解
        #[arg(long)]
        debug: bool,
    },
}

// =========================================================================
// 主入口
// =========================================================================

fn main() {
    let cli = Cli::parse();

    // 加载配置
    let cfg = Config::load_from_path(&cli.config);

    match cli.command {
        Some(Commands::Encode {
            division,
            keymap,
            output,
            config: encode_config,
        }) => {
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            // 若指定了 --config，加载该配置以获取 simple_levels
            let encode_cfg = encode_config.as_deref().map(Config::load_from_path);
            run_encode(division_path, &keymap, &output, encode_cfg.as_ref());
        }
        Some(Commands::Evaluate {
            division,
            keymap,
            keydist,
            equiv,
            simple,
            keysoul,
            output,
        }) => {
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            let keydist_path = keydist.as_deref().unwrap_or(&cfg.files.key_dist);
            let equiv_path = equiv.as_deref().unwrap_or(&cfg.files.pair_equiv);
            run_evaluate(
                &cfg,
                division_path,
                &keymap,
                keydist_path,
                equiv_path,
                simple.as_deref(),
                keysoul,
                &output,
            );
        }
        Some(Commands::Optimize { amhb, keysoul }) => run_optimize(&cfg, amhb, keysoul, &cli.config),
        Some(Commands::Resume { checkpoint }) => run_resume(&cfg, &checkpoint),
        Some(Commands::Keysoul { sequence, debug }) => run_keysoul(&sequence, debug),
        None => run_optimize(&cfg, false, false, &cli.config),
    }
}

// =========================================================================
// encode 子命令
// =========================================================================

fn run_encode(division_path: &str, keymap_path: &str, output_path: &str, cfg: Option<&Config>) {
    println!("=== CodeGenie 编码模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = loader::load_keymap(keymap_path, division_path);
    let splits = loader::load_splits(division_path);

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 若指定了配置文件，先生成简码前缀
    let mut simple_count = 0usize;
    let simple_prefix = if let Some(c) = cfg {
        let simple_config = c.get_simple_code_config();
        if !simple_config.levels.is_empty() {
            println!("  简码级别: {} 级", simple_config.levels.len());
            let (prefix, count) = output::build_simple_prefix_for_encode(&root_to_key, &splits, &simple_config);
            simple_count = count;
            prefix
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // 为每个汉字编码
    let mut code_out = String::new();
    let mut missing_roots: HashMap<String, usize> = HashMap::new();
    let mut encoded_count = 0usize;
    let mut failed_count = 0usize;

    for (ch, roots, freq) in &splits {
        let mut code_parts = Vec::new();
        let mut all_found = true;

        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            } else {
                all_found = false;
                *missing_roots.entry(root.clone()).or_default() += 1;
            }
        }

        if all_found && !code_parts.is_empty() {
            let code_str: String = code_parts.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            encoded_count += 1;
        } else {
            // 即使有缺失字根，也输出已有部分（用 ? 标记缺失）
            let mut partial_code = Vec::new();
            for root in roots {
                if let Some(&key) = root_to_key.get(root) {
                    partial_code.push(key_to_char(key));
                } else {
                    partial_code.push('?');
                }
            }
            let code_str: String = partial_code.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            failed_count += 1;
        }
    }

    // 写入文件（简码前缀 + 全码）
    let final_out = format!("{}{}", simple_prefix, code_out);
    std::fs::write(output_path, &final_out).expect("无法写入编码输出文件");

    println!("\n✅ 编码完成:");
    if simple_count > 0 {
        println!("  简码条目: {} 条", simple_count);
    }
    println!("  成功编码: {} 字", encoded_count);
    if failed_count > 0 {
        println!("  ⚠️ 部分编码: {} 字（存在未映射字根）", failed_count);
    }
    if !missing_roots.is_empty() {
        println!("  ⚠️ 未找到映射的字根 ({} 种):", missing_roots.len());
        let mut sorted: Vec<_> = missing_roots.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (root, count) in sorted.iter().take(20) {
            println!("    {} (出现 {} 次)", root, count);
        }
        if sorted.len() > 20 {
            println!("    ... 还有 {} 种", sorted.len() - 20);
        }
    }
    println!("  结果已保存至 {}", output_path);
}

// =========================================================================
// evaluate 子命令
// =========================================================================

fn run_evaluate(
    cfg: &Config,
    division_path: &str,
    keymap_path: &str,
    keydist_path: &str,
    equiv_path: &str,
    simple_path: Option<&str>,
    use_keysoul: bool,
    output_path: &str,
) {
    println!("=== CodeGenie 评估模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  键位分布: {}", keydist_path);
    if use_keysoul {
        println!("  当量模型: 键魂当量 (keySoul v2.3)");
    } else {
        println!("  当量数据: {}", equiv_path);
    }
    if let Some(sp) = simple_path {
        println!("  简码规则: {}", sp);
    }
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = loader::load_keymap(keymap_path, division_path);
    let splits = loader::load_splits(division_path);
    let equiv_table = loader::load_pair_equivalence(equiv_path);
    let key_dist_config = loader::load_key_distribution(keydist_path);

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 加载简码配置
    let simple_config = if let Some(sp) = simple_path {
        let scfg = simple::parse_simple_code_config(sp);
        println!("  已加载简码配置: {} 级", scfg.levels.len());
        scfg
    } else {
        cfg.get_simple_code_config()
    };

    // 将所有字根作为 fixed_roots，groups 为空
    let fixed_roots: HashMap<String, u8> = root_to_key;
    let groups: Vec<types::RootGroup> = vec![];
    let assignment: Vec<u8> = vec![];

    // 构建 OptContext
    let scale_config = types::ScaleConfig::default();
    let weights = cfg.get_weight_config();
    let ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &groups,
        equiv_table,
        key_dist_config,
        scale_config,
        simple_config,
        weights,
        use_keysoul,
        vec![],
    );

    println!("  编码基数: {}", ctx.code_base);
    println!("  编码空间: {}", ctx.code_space);
    println!("  最大码长: {}", ctx.max_parts);

    // 运行评估
    let evaluator = Evaluator::new(&ctx, &assignment);
    let metrics = evaluator.get_metrics(&ctx);
    let simple_metrics = evaluator.get_simple_metrics(&ctx);
    let word_metrics = evaluator.get_word_metrics(&ctx);

    // 打印评估结果
    println!("\n📊 评估结果:");
    println!("  ═══════════════════════════════════════");
    println!("  「全码」前{}重码数:        {}", ctx.weights.full_top_n, metrics.top_n_collision_count);
    println!("  「全码」重码数:          {}", metrics.collision_count);
    println!(
        "  「全码」重码率:          {:.6}%",
        metrics.collision_rate * 100.0
    );
    println!(
        "  「全码」加权键均当量:    {:.4}",
        metrics.equiv_mean
    );
    println!(
        "  「全码」用指分布偏差(L2): {:.4}",
        metrics.dist_deviation
    );

    if !ctx.simple_config.levels.is_empty() {
        println!("  ─────────────────────────────────────");
        println!(
            "  「简码」重码数:          {}",
            simple_metrics.collision_count
        );
        println!(
            "  「简码」重码率:          {:.6}%",
            simple_metrics.collision_rate * 100.0
        );
        println!(
            "  「简码」加权码长:        {:.4}",
            simple_metrics.weighted_key_length
        );
        println!(
            "  「简码」加权当量:        {:.4}",
            simple_metrics.equiv_mean
        );
        println!(
            "  「简码」分布偏差:        {:.4}",
            simple_metrics.dist_deviation
        );
    }
    if ctx.enable_word_code {
        println!("  ─────────────────────────────────────");
        println!("  「词码」前2000重码数:     {}", word_metrics.top2000_collision_count);
        println!("  「词码」前10000重码数:    {}", word_metrics.top10000_collision_count);
        println!("  「词码」总重码数:         {}", word_metrics.collision_count);
        println!("  「词码」重码率:           {:.6}%", word_metrics.collision_rate * 100.0);
        println!("  「词码」加权当量:         {:.4}", word_metrics.equiv_mean);
        println!("  「词码」分布偏差:         {:.4}", word_metrics.dist_deviation);
    }
    println!("  ═══════════════════════════════════════");

    // 生成详细评估报告
    let report = build_evaluate_report(
        cfg,
        &ctx,
        &assignment,
        &evaluator,
        &metrics,
        &simple_metrics,
        &word_metrics,
        &key_dist_config,
    );

    std::fs::write(output_path, &report).expect("无法写入评估输出文件");
    println!("\n✅ 详细评估报告已保存至 {}", output_path);
}

/// 构建详细评估报告
fn build_evaluate_report(
    cfg: &Config,
    ctx: &OptContext,
    assignment: &[u8],
    evaluator: &Evaluator,
    metrics: &types::Metrics,
    simple_metrics: &SimpleMetrics,
    word_metrics: &WordMetrics,
    key_dist_config: &[KeyDistConfig; EQUIV_TABLE_SIZE],
) -> String {
    let mut out = String::new();

    out.push_str("# CodeGenie 编码方案评估报告\n");
    out.push_str("#\n");
    out.push_str(&format!("# 汉字数量: {}\n", ctx.char_infos.len()));
    out.push_str(&format!("# 总字频: {}\n", ctx.total_frequency));
    out.push_str(&format!("# 编码基数: {}\n", ctx.code_base));
    out.push_str(&format!("# 最大码长: {}\n", ctx.max_parts));
    out.push_str("#\n");

    // ===== 全码指标 =====
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 全码指标\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 前{}重码数: {}\n", ctx.weights.full_top_n, metrics.top_n_collision_count));
    out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    out.push_str(&format!(
        "# 加权键均当量: {:.4}\n",
        metrics.equiv_mean
    ));
    out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    out.push_str("#\n");

    // ===== 简码指标 =====
    if !ctx.simple_config.levels.is_empty() {
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str("# 简码指标\n");
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str(&format!("# 简码重码数: {}\n", simple_metrics.collision_count));
        out.push_str(&format!("# 简码重码率: {:.6}%\n", simple_metrics.collision_rate * 100.0));
        out.push_str(&format!("# 简码加权码长: {:.4}\n", simple_metrics.weighted_key_length));
        out.push_str(&format!("# 简码加权当量: {:.4}\n", simple_metrics.equiv_mean));
        out.push_str(&format!("# 简码分布偏差: {:.4}\n", simple_metrics.dist_deviation));
        out.push_str("#\n");
    }

    // ===== 词码指标 =====
    if ctx.enable_word_code {
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str("# 词码指标\n");
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str(&format!("# 词码前2000重码数: {}\n", word_metrics.top2000_collision_count));
        out.push_str(&format!("# 词码前10000重码数: {}\n", word_metrics.top10000_collision_count));
        out.push_str(&format!("# 词码总重码数: {}\n", word_metrics.collision_count));
        out.push_str(&format!("# 词码重码率: {:.6}%\n", word_metrics.collision_rate * 100.0));
        out.push_str(&format!("# 词码加权当量: {:.4}\n", word_metrics.equiv_mean));
        out.push_str(&format!("# 词码分布偏差: {:.4}\n", word_metrics.dist_deviation));
        out.push_str("#\n");
    }

    // ===== 用指分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 用指分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    let inv_tkp = evaluator.inv_total_key_presses;
    for kc in cfg.keys.display_order.chars() {
        if let Some(ki) = types::char_to_key_index(kc) {
            if ki >= 31 {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * inv_tkp;
            let target = key_dist_config[ki].target_rate;
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

    // 特殊键位
    let order_set: std::collections::HashSet<char> = cfg.keys.display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * inv_tkp;
                let target = key_dist_config[*ki].target_rate;
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

    // ===== 当量分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 当量分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", metrics.equiv_mean));

    // 计算每个汉字的当量
    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        let equiv = ctx.calc_equiv_from_parts(i, assignment);
        char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

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

    // ===== 重码详情 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 重码详情\n");
    out.push_str("# ═══════════════════════════════════════\n");

    // 构建编码到汉字的映射
    let n = ctx.char_infos.len();
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }

    // 收集重码组
    let mut collision_groups: Vec<(usize, Vec<(char, u64)>)> = Vec::new();
    for (code, chars) in &code_to_chars {
        if chars.len() >= 2 {
            let mut group: Vec<(char, u64)> = chars
                .iter()
                .map(|&ci| (ctx.raw_splits[ci].0, ctx.char_infos[ci].frequency))
                .collect();
            group.sort_by(|a, b| b.1.cmp(&a.1));
            collision_groups.push((*code, group));
        }
    }

    // 按组内最高频率降序排序
    collision_groups.sort_by(|a, b| {
        let max_a = a.1.first().map(|x| x.1).unwrap_or(0);
        let max_b = b.1.first().map(|x| x.1).unwrap_or(0);
        max_b.cmp(&max_a)
    });

    out.push_str(&format!(
        "# 共 {} 组重码 (按最高频率降序)\n",
        collision_groups.len()
    ));
    out.push_str("# 编码\t重码字\t字频列表\n");

    for (_, group) in collision_groups.iter().take(200) {
        let chars_str: String = group.iter().map(|(ch, _)| *ch).collect();
        let freqs_str: String = group
            .iter()
            .map(|(_, f)| f.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // 获取编码字符串
        let first_ci = code_to_chars
            .values()
            .find(|v| v.len() >= 2 && ctx.raw_splits[v[0]].0 == group[0].0)
            .and_then(|v| v.first())
            .copied();

        let code_str = if let Some(ci) = first_ci {
            let info = &ctx.char_infos[ci];
            let keys: String = info
                .parts_slice()
                .iter()
                .map(|&p| key_to_char(ctx.resolve_key(p, assignment)))
                .collect();
            keys
        } else {
            "?".to_string()
        };

        out.push_str(&format!("{}\t{}\t{}\n", code_str, chars_str, freqs_str));
    }

    if collision_groups.len() > 200 {
        out.push_str(&format!(
            "# ... 还有 {} 组重码未列出\n",
            collision_groups.len() - 200
        ));
    }

    out
}

// =========================================================================
// optimize 子命令（原有优化流程）
// =========================================================================

fn run_optimize(cfg: &Config, use_amhb: bool, use_keysoul: bool, cli_config_path: &str) {
    let start_time = Instant::now();
    println!("=== CodeGenie 码灵算法优化器 v10 ===");

    // 验证配置
    cfg.validate_weights();

    // 打印配置信息
    if use_amhb {
        let steps_str = match cfg.amhb.total_steps {
            Some(s) => format!("{}", s),
            None => "无限制(由温度终止)".to_string(),
        };
        println!(
            "线程数: {}, 总步数: {}",
            cfg.annealing.threads, steps_str
        );
    } else {
        println!(
            "线程数: {}, 总步数: {}",
            cfg.annealing.threads, cfg.annealing.total_steps
        );
    }
    println!(
        "初始温度: {}, 结束温度: {}",
        if cfg.annealing.temp_start <= 0.0 {
            "自动校准".to_string()
        } else {
            format!("{}", cfg.annealing.temp_start)
        },
        cfg.annealing.temp_end
    );
    println!("全局允许键位: {}", cfg.keys.allowed);
    println!(
        "全码权重: 前N重码数={:.2}, 重码数={:.2}, 重码率={:.2}, 当量={:.2}, 分布={:.2}",
        cfg.weights.full_code.top_n_collision_count,
        cfg.weights.full_code.collision_count,
        cfg.weights.full_code.collision_rate,
        cfg.weights.full_code.equivalence,
        cfg.weights.full_code.distribution,
    );
    println!(
        "简码优化: {} (全码占比={:.0}%, 简码占比={:.0}%, 词码占比={:.0}%)",
        if cfg.weights.simple_code.enabled { "开启" } else { "关闭" },
        cfg.weights.full * 100.0,
        cfg.weights.simple * 100.0,
        cfg.weights.word * 100.0,
    );
    if cfg.weights.simple_code.enabled {
        println!(
            "简码子权重: 加权码长={:.2}, 当量={:.2}, 分布={:.2}, 重码数={:.2}, 重码率={:.2}",
            cfg.weights.simple_code.weighted_key_length,
            cfg.weights.simple_code.equivalence,
            cfg.weights.simple_code.distribution,
            cfg.weights.simple_code.collision_count,
            cfg.weights.simple_code.collision_rate,
        );
    }
    println!("用指分布输出顺序: {}", cfg.keys.display_order);
    if use_keysoul {
        println!("当量模型: 键魂当量 (keySoul v2.3)");
    } else {
        println!("当量模型: pair_equivalence.txt (陈一凡当量表)");
    }

    // 选择优化算法
    if use_amhb {
        println!("\n算法选择: AMHB (Adaptive Multi-candidate Heat Bath)");
    } else {
        println!("\n算法选择: SA (Simulated Annealing)");
    }

    // 创建输出目录
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let output_dir = format!("output-{}", timestamp);
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);
    if let Err(e) = std::fs::copy(cli_config_path, format!("{}/config.toml", output_dir)) {
        eprintln!("警告：无法复制配置文件: {}", e);
    }

    // ==================== 加载数据 ====================
    let (fixed_roots, constrained) = loader::load_fixed(&cfg.files.fixed);
    let dynamic_groups = loader::load_dynamic(&cfg.files.dynamic, &constrained, &cfg.keys.allowed);
    let splits = loader::load_splits(&cfg.files.splits);
    let equiv_table = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config = loader::load_key_distribution(&cfg.files.key_dist);

    // 加载简码配置
    let simple_config = if cfg.weights.simple_code.enabled {
        let scfg = cfg.get_simple_code_config();
        println!("\n📋 简码配置:");
        for level in &scfg.levels {
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
        scfg
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
    if max_parts_in_data > cfg.annealing.max_parts {
        println!(
            "⚠️ 拆分表中最大码长({})超过 max_parts({}), 请调大配置",
            max_parts_in_data, cfg.annealing.max_parts
        );
    }

    // ==================== 校验 ====================
    if !validate::check_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }

    // ==================== 加载词码数据 ====================
    let word_infos_for_calib = if cfg.weights.word_code.enabled {
        let word_div_path = cfg.files.word_div.as_deref().unwrap_or("input-worddiv.txt");
        let root_to_group_tmp: std::collections::HashMap<String, usize> = dynamic_groups
            .iter()
            .enumerate()
            .flat_map(|(gi, g)| g.roots.iter().map(move |r| (r.clone(), gi)))
            .collect();
        loader::load_word_divisions(word_div_path, &fixed_roots, &root_to_group_tmp, cfg.annealing.max_parts)
    } else {
        vec![]
    };

    // ==================== 初始校准 ====================
    println!("\n📐 正在进行初始尺度校准...");
    let temp_scale = types::ScaleConfig::default();
    let weights = cfg.get_weight_config();
    let temp_ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
        temp_scale,
        simple_config.clone(),
        weights,
        use_keysoul,
        word_infos_for_calib.clone(),
    );

    let initial_assignment = random_init(&temp_ctx);
    let initial_eval = Evaluator::new(&temp_ctx, &initial_assignment);
    let initial_metrics = initial_eval.get_metrics(&temp_ctx);
    let initial_simple_metrics = initial_eval.get_simple_metrics(&temp_ctx);
    let initial_word_metrics = initial_eval.get_word_metrics(&temp_ctx);

    let weights = cfg.get_weight_config();
    let scale_config = calibrate_scales(&initial_metrics, &initial_simple_metrics, &initial_word_metrics, &weights);

    println!("  初始状态观测:");
    println!(
        "    前{}重码数: {},  重码数: {},  重码率: {:.6}",
        weights.full_top_n, initial_metrics.top_n_collision_count,
        initial_metrics.collision_count, initial_metrics.collision_rate
    );
    println!("    当量: {:.4}", initial_metrics.equiv_mean);
    if cfg.weights.simple_code.enabled {
        println!(
            "    简码加权码长: {:.4},  简码当量: {:.4},  简码分布: {:.4}",
            initial_simple_metrics.weighted_key_length,
            initial_simple_metrics.equiv_mean,
            initial_simple_metrics.dist_deviation
        );
        println!(
            "    简码重码数: {},  简码重码率: {:.6}%",
            initial_simple_metrics.collision_count,
            initial_simple_metrics.collision_rate * 100.0
        );
    }
    if cfg.weights.word_code.enabled {
        println!(
            "    词码前2000重码数: {},  前10000重码数: {},  总重码数: {}",
            initial_word_metrics.top2000_collision_count,
            initial_word_metrics.top10000_collision_count,
            initial_word_metrics.collision_count,
        );
    }
    println!("  校准尺度 (Scale):");
    println!("    FullTopN:       {:.6}", scale_config.full_top_n_collision);
    println!("    FullCollCnt:    {:.6}", scale_config.full_collision_count);
    println!("    FullCollRate:   {:.6}", scale_config.full_collision_rate);
    println!("    FullEquiv:      {:.6}", scale_config.full_equivalence);
    if cfg.weights.simple_code.enabled {
        println!("    SimpleWKL:      {:.6}", scale_config.simple_weighted_key_length);
        println!("    SimpleEquiv:    {:.6}", scale_config.simple_equivalence);
        println!("    SimpleDist:     {:.6}", scale_config.simple_distribution);
        println!("    SimpleCollCnt:  {:.6}", scale_config.simple_collision_count);
        println!("    SimpleCollRate: {:.6}", scale_config.simple_collision_rate);
    }

    // 逻辑根验证（仅在启用简码时）
    if cfg.weights.simple_code.enabled {
        println!("\n  📝 逻辑根解析验证 (前3字):");
        for ci in 0..3.min(temp_ctx.raw_splits.len()) {
            let (ch, roots, _) = &temp_ctx.raw_splits[ci];
            let si = &temp_ctx.char_simple_infos[ci];
            println!("    '{}' 拆分: {:?}", ch, roots);
            for (ri, lr) in si.logical_roots.iter().enumerate() {
                let full_keys: Vec<char> = lr
                    .full_code_parts
                    .iter()
                    .map(|&p| key_to_char(temp_ctx.resolve_key(p, &initial_assignment)))
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
                            key_to_char(temp_ctx.resolve_key(part, &initial_assignment))
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
    let equiv_table_2 = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config_2 = loader::load_key_distribution(&cfg.files.key_dist);

    let ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table_2,
        key_dist_config_2,
        scale_config,
        simple_config,
        weights,
        use_keysoul,
        word_infos_for_calib,
    );

    println!("\n  - 编码基数: {}", ctx.code_base);
    println!("  - 编码空间: {}", ctx.code_space);

    let root_usage = output::count_root_usage(&ctx);

    // 并行执行优化
    println!("\n🚀 开始优化...");
    println!("💡 提示: 按 Ctrl+C 可暂停优化并保存检查点，之后使用 resume 子命令恢复");
    let num_threads = cfg.annealing.threads;

    // 设置 Ctrl+C 信号处理
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let sf = Arc::clone(&stop_flag);
        ctrlc_set_handler(move || {
            eprintln!("\n[信号] 收到 Ctrl+C，等待各线程到达检查点...");
            sf.store(true, Ordering::Relaxed);
            // 不退出进程，让各线程检测到信号后优雅退出
        });
    }

    let results: Vec<(Vec<u8>, f64, types::Metrics, SimpleMetrics, WordMetrics)> = if use_amhb {
        // AMHB 模式 — 分段指数降温（piecewise exponential cooling）
        let segments = cfg.amhb.cooling_segments.clone();
        let next_temp = move |t: f64, _iter: usize, _score: f64| -> f64 {
            for seg in &segments {
                if t > seg.threshold {
                    return t * seg.factor;
                }
            }
            -1.0 // 低于所有阈值，终止
        };

        let param = AmhbParameters {
            max_iterations: cfg.amhb.total_steps.unwrap_or(u64::MAX as usize) as u64,
            temp_start: cfg.amhb.temp_start,
            total_neighbors: cfg.amhb.total_neighbors,
            steal_threshold: cfg.amhb.steal_threshold,
        };

        let mut optimizer = AmhbOptimizer::new(
            &ctx,
            num_threads,
            true,
            cfg.amhb.total_neighbors,
            cfg.amhb.steal_threshold,
        );
        optimizer.solve(&ctx, param, next_temp, &stop_flag);

        // 从 optimizer 获取最佳结果
        let amhb_result = vec![(optimizer.best_assignment.clone(), optimizer.best_score, types::Metrics::default(), SimpleMetrics::default(), WordMetrics::default())];

        // 若被中断，直接输出当前最优并退出（AMHB 暂不支持断点续算）
        if stop_flag.load(Ordering::Relaxed) {
            let eval = Evaluator::new(&ctx, &optimizer.best_assignment);
            let m = eval.get_metrics(&ctx);
            let sm = eval.get_simple_metrics(&ctx);
            let wm = eval.get_word_metrics(&ctx);
            println!("\n⏸️  优化已暂停（AMHB 模式不支持断点续算）");
            println!("   当前最优得分: {:.4}", optimizer.best_score);
            println!("   重码数: {}", m.collision_count);
            let all_results = vec![(0usize, optimizer.best_assignment.clone(), optimizer.best_score, m, sm, wm)];
            let output_dir_ref = &output_dir;
            let root_usage_ref = &root_usage;
            save_results(&ctx, &optimizer.best_assignment, optimizer.best_score, &m, &sm, output_dir_ref, root_usage_ref);
            save_summary(cfg, &all_results, 0, output_dir_ref, start_time.elapsed());
            println!("\n当前最优结果已保存至 {}/", output_dir);
            return;
        }

        amhb_result
    } else {
        // SA 模式 — 带断点续算支持
        let sa_results: Vec<SaResult> = (0..num_threads)
            .into_par_iter()
            .map(|i| simulated_annealing_resumable(&ctx, cfg, i, &stop_flag, None, None))
            .collect();

        // 检查是否被中断
        let was_interrupted = sa_results.iter().any(|r| r.checkpoint.is_some());
        if was_interrupted {
            // 收集所有线程检查点并保存
            let thread_checkpoints: Vec<_> = sa_results
                .iter()
                .filter_map(|r| r.checkpoint.clone())
                .collect();

            // 获取自动校准的温度参数（使用线程 0 的值）
            let (at_start, at_comfort) = sa_results
                .first()
                .map(|r| (r.actual_temp_start, r.actual_comfort_temp))
                .unwrap_or((cfg.annealing.temp_start, cfg.annealing.comfort_temp));

            let ckpt = Checkpoint {
                version: CHECKPOINT_VERSION,
                timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                config_path: cli_config_path.to_string(),
                scale_config,
                actual_temp_start: at_start,
                actual_comfort_temp: at_comfort,
                total_steps: cfg.annealing.total_steps,
                num_threads,
                use_keysoul,
                threads: thread_checkpoints,
            };

            let ckpt_path = std::path::Path::new(CHECKPOINT_FILENAME);
            match save_checkpoint(&ckpt, ckpt_path) {
                Ok(()) => {
                    println!("\n✅ 检查点已保存至 {}", CHECKPOINT_FILENAME);
                    println!("   可使用以下命令恢复优化:");
                    println!("   cargo run --release -- resume -f {}", CHECKPOINT_FILENAME);
                }
                Err(e) => {
                    eprintln!("\n❌ 保存检查点失败: {}", e);
                }
            }

            // 即使被中断，也输出当前最优结果
            let best_so_far = sa_results
                .iter()
                .min_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
                .unwrap();

            println!("\n⏸️  优化已暂停");
            println!("   当前最优得分: {:.4}", best_so_far.score);
            println!("   重码数: {}", best_so_far.metrics.collision_count);
            return;
        }

        sa_results
            .into_iter()
            .map(|r| (r.assignment, r.score, r.metrics, r.simple_metrics, r.word_metrics))
            .collect()
    };

    // SA 模式需要额外处理结果
    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics, SimpleMetrics, WordMetrics)> = if use_amhb {
        let best = results.into_iter().min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        vec![(0, best.0, best.1, best.2, best.3, best.4)]
    } else {
        results
            .into_iter()
            .enumerate()
            .map(|(i, (a, s, m, sm, wm))| (i, a, s, m, sm, wm))
            .collect()
    };

    let (best_thread, best_assignment, best_score, best_metrics, best_simple_metrics, best_word_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m, sm, wm)| (*tid, a.clone(), *s, *m, *sm, *wm))
        .unwrap();

    let elapsed = start_time.elapsed();

    let m = best_metrics;
    let sm = best_simple_metrics;
    let wm = best_word_metrics;
    println!("\n=================================");
    println!("🏆 最优结果 (线程 {}):", best_thread);
    println!("   综合得分: {:.4}", best_score);
    println!("   「全码」前{}重码数: {}", ctx.weights.full_top_n, m.top_n_collision_count);
    println!("   「全码」重码数: {}", m.collision_count);
    println!("   「全码」重码率: {:.6}%", m.collision_rate * 100.0);
    println!("   「全码」加权键均当量: {:.4}", m.equiv_mean);
    println!("   「全码」用指分布偏差(L2): {:.4}", m.dist_deviation);
    if ctx.enable_simple_code {
        println!("---------------------------------");
        println!("   「简码」重码数: {}", sm.collision_count);
        println!("   「简码」重码率: {:.6}%", sm.collision_rate * 100.0);
        println!("   「简码」加权码长: {:.4}", sm.weighted_key_length);
        println!("   「简码」加权当量: {:.4}", sm.equiv_mean);
        println!("   「简码」分布偏差: {:.4}", sm.dist_deviation);
    }
    if ctx.enable_word_code {
        println!("---------------------------------");
        println!("   「词码」前2000重码数: {}", wm.top2000_collision_count);
        println!("   「词码」前10000重码数: {}", wm.top10000_collision_count);
        println!("   「词码」总重码数: {}", wm.collision_count);
        println!("   「词码」重码率: {:.6}%", wm.collision_rate * 100.0);
    }
    println!("⏱️ 总耗时: {:?}", elapsed);
    println!("=================================");

    println!("\n📁 保存所有线程结果...");
    for (tid, assignment, score, metrics, smetrics, _) in &all_results {
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
    save_summary(cfg, &all_results, best_thread, &output_dir, elapsed);

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - summary.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    println!("  - output-simple-codes.txt  简码分配");
    println!("  - thread-XX/               各线程结果");
}

/// 封装 ctrlc 信号处理设置
fn ctrlc_set_handler<F: Fn() + Send + 'static>(handler: F) {
    ctrlc::set_handler(handler).expect("无法设置 Ctrl+C 信号处理");
}
// =========================================================================
// resume 子命令 — 从检查点恢复优化
// =========================================================================

fn run_resume(cfg: &Config, checkpoint_path: &str) {
    let start_time = Instant::now();
    println!("=== CodeGenie 断点续算模式 ===");
    println!("  检查点文件: {}", checkpoint_path);

    // 加载检查点
    let ckpt = match load_checkpoint(std::path::Path::new(checkpoint_path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
    };

    println!("  保存时间: {}", ckpt.timestamp);
    println!("  原配置文件: {}", ckpt.config_path);
    println!("  线程数: {}", ckpt.num_threads);
    println!("  总步数: {}", ckpt.total_steps);
    println!(
        "  各线程进度: {}",
        ckpt.threads
            .iter()
            .map(|t| format!("T{}@{}", t.thread_id, t.current_step))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // 验证线程数匹配
    if ckpt.num_threads != cfg.annealing.threads {
        println!(
            "⚠️  注意: 配置文件线程数({})与检查点线程数({})不同，以检查点为准",
            cfg.annealing.threads, ckpt.num_threads
        );
    }

    // 验证配置
    cfg.validate_weights();

    // ==================== 加载数据 ====================
    let (fixed_roots, constrained) = loader::load_fixed(&cfg.files.fixed);
    let dynamic_groups = loader::load_dynamic(&cfg.files.dynamic, &constrained, &cfg.keys.allowed);
    let splits = loader::load_splits(&cfg.files.splits);
    let equiv_table = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config = loader::load_key_distribution(&cfg.files.key_dist);

    let simple_config = if cfg.weights.simple_code.enabled {
        cfg.get_simple_code_config()
    } else {
        SimpleCodeConfig { levels: vec![] }
    };

    let weights = cfg.get_weight_config();

    // 使用检查点中保存的 ScaleConfig（避免重新校准）
    let ctx = OptContext::new(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
        ckpt.scale_config,
        simple_config,
        weights,
        ckpt.use_keysoul,
        vec![],
    );

    println!("\n  数据加载完毕:");
    println!("  - 字根组: {}", ctx.num_groups);
    println!("  - 汉字数: {}", ctx.char_infos.len());
    println!("  - 编码空间: {}", ctx.code_space);

    // 验证 assignment 维度
    for tc in &ckpt.threads {
        if tc.assignment.len() != ctx.num_groups {
            eprintln!(
                "❌ 线程 {} 的 assignment 长度({})与当前字根组数({})不匹配，数据可能已更改",
                tc.thread_id,
                tc.assignment.len(),
                ctx.num_groups
            );
            std::process::exit(1);
        }
    }

    let root_usage = output::count_root_usage(&ctx);

    // 创建输出目录
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let output_dir = format!("output-{}", timestamp);
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("  输出目录: {}", output_dir);
    if let Err(e) = std::fs::copy(&ckpt.config_path, format!("{}/config.toml", output_dir)) {
        eprintln!("警告：无法复制配置文件: {}", e);
    }

    // 设置 Ctrl+C 信号处理
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let sf = Arc::clone(&stop_flag);
        ctrlc_set_handler(move || {
            sf.store(true, Ordering::Relaxed);
        });
    }

    // 温度调度参数
    let schedule_override = if ckpt.actual_temp_start > 0.0 {
        Some((ckpt.actual_temp_start, ckpt.actual_comfort_temp))
    } else {
        None
    };

    println!("\n🚀 恢复优化...");
    println!("💡 提示: 再次按 Ctrl+C 可暂停并保存新的检查点");

    let num_threads = ckpt.threads.len();
    let sa_results: Vec<SaResult> = ckpt
        .threads
        .par_iter()
        .map(|tc| {
            simulated_annealing_resumable(
                &ctx,
                cfg,
                tc.thread_id,
                &stop_flag,
                Some(tc),
                schedule_override,
            )
        })
        .collect();

    // 检查是否再次被中断
    let was_interrupted = sa_results.iter().any(|r| r.checkpoint.is_some());
    if was_interrupted {
        let thread_checkpoints: Vec<_> = sa_results
            .iter()
            .filter_map(|r| r.checkpoint.clone())
            .collect();

        let (at_start, at_comfort) = sa_results
            .first()
            .map(|r| (r.actual_temp_start, r.actual_comfort_temp))
            .unwrap_or((ckpt.actual_temp_start, ckpt.actual_comfort_temp));

        let new_ckpt = Checkpoint {
            version: CHECKPOINT_VERSION,
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            config_path: ckpt.config_path.clone(),
            scale_config: ckpt.scale_config,
            actual_temp_start: at_start,
            actual_comfort_temp: at_comfort,
            total_steps: ckpt.total_steps,
            num_threads,
            use_keysoul: ckpt.use_keysoul,
            threads: thread_checkpoints,
        };

        let ckpt_path = std::path::Path::new(CHECKPOINT_FILENAME);
        match save_checkpoint(&new_ckpt, ckpt_path) {
            Ok(()) => {
                println!("\n✅ 新检查点已保存至 {}", CHECKPOINT_FILENAME);
                println!("   可使用以下命令继续恢复:");
                println!("   cargo run --release -- resume -f {}", CHECKPOINT_FILENAME);
            }
            Err(e) => {
                eprintln!("\n❌ 保存检查点失败: {}", e);
            }
        }

        let best_so_far = sa_results
            .iter()
            .min_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
            .unwrap();

        println!("\n⏸️  优化已暂停");
        println!("   当前最优得分: {:.4}", best_so_far.score);
        println!("   重码数: {}", best_so_far.metrics.collision_count);
        return;
    }

    // 正常完成 — 输出结果
    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics, SimpleMetrics, WordMetrics)> = sa_results
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i, r.assignment, r.score, r.metrics, r.simple_metrics, r.word_metrics))
        .collect();

    let (best_thread, best_assignment, best_score, best_metrics, best_simple_metrics, _best_word_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m, sm, wm)| (*tid, a.clone(), *s, *m, *sm, *wm))
        .unwrap();

    let elapsed = start_time.elapsed();

    let m = best_metrics;
    let sm = best_simple_metrics;
    println!("\n=================================");
    println!("🏆 最优结果 (线程 {}):", best_thread);
    println!("   综合得分: {:.4}", best_score);
    println!("   「全码」前{}重码数: {}", ctx.weights.full_top_n, m.top_n_collision_count);
    println!("   「全码」重码数: {}", m.collision_count);
    println!("   「全码」重码率: {:.6}%", m.collision_rate * 100.0);
    println!("   「全码」加权键均当量: {:.4}", m.equiv_mean);
    println!("   「全码」用指分布偏差(L2): {:.4}", m.dist_deviation);
    if ctx.enable_simple_code {
        println!("---------------------------------");
        println!("   「简码」重码数: {}", sm.collision_count);
        println!("   「简码」重码率: {:.6}%", sm.collision_rate * 100.0);
        println!("   「简码」加权码长: {:.4}", sm.weighted_key_length);
        println!("   「简码」加权当量: {:.4}", sm.equiv_mean);
        println!("   「简码」分布偏差: {:.4}", sm.dist_deviation);
    }
    println!("⏱️ 总耗时: {:?}", elapsed);
    println!("=================================");

    println!("\n📁 保存所有线程结果...");
    for (tid, assignment, score, metrics, smetrics, _) in &all_results {
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
    save_summary(cfg, &all_results, best_thread, &output_dir, elapsed);

    // 清理检查点文件
    let ckpt_file = std::path::Path::new(checkpoint_path);
    if ckpt_file.exists() {
        let _ = std::fs::remove_file(ckpt_file);
        println!("🗑️  已清理检查点文件 {}", checkpoint_path);
    }

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - summary.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    println!("  - output-simple-codes.txt  简码分配");
    println!("  - thread-XX/               各线程结果");
}

// =========================================================================
// keysoul 子命令
// =========================================================================

fn run_keysoul(sequence: &str, debug: bool) {
    use crate::keysoul::global_model;

    // 中文字符双宽，ASCII 单宽
    fn dw(s: &str) -> usize {
        s.chars().map(|c| {
            let cp = c as u32;
            if (0x1100..=0x115F).contains(&cp)
                || (0x2E80..=0x303F).contains(&cp)
                || (0x3040..=0x33FF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0x4E00..=0x9FFF).contains(&cp)
                || (0xAC00..=0xD7AF).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFE10..=0xFE6F).contains(&cp)
                || (0xFF00..=0xFFEF).contains(&cp)
            { 2 } else { 1 }
        }).sum()
    }
    // 右填充到显示宽度 w
    let pr = |s: &str, w: usize| -> String {
        let d = dw(s);
        if d >= w { s.to_string() } else { format!("{}{}", s, " ".repeat(w - d)) }
    };
    // 左填充到显示宽度 w
    let pl = |s: &str, w: usize| -> String {
        let d = dw(s);
        if d >= w { s.to_string() } else { format!("{}{}", " ".repeat(w - d), s) }
    };

    let model = global_model();

    if !debug {
        match model.sequence_time(sequence) {
            t if t < 0.0 => eprintln!("错误：序列包含未知键"),
            t => println!("序列: {}\n总时间: {:.2} 毫秒 = {:.4} 秒", sequence, t, t / 1000.0),
        }
        return;
    }

    match model.sequence_time_debug(sequence) {
        None => eprintln!("错误：序列包含未知键"),
        Some((total, left_time, right_time, pairs)) => {
            println!("\n  序列: {}", sequence);
            println!("  总时间: {:.2} 毫秒 = {:.4} 秒\n", total, total / 1000.0);

            // 各列显示宽度（按终端列数）
            let (c0, c1, c2) = (8, 10, 17);  // 键对, 分类, 手指路径
            let (c3, c4, c5, c6) = (6, 6, 6, 6);  // 神经, 原始, 折扣, 移动
            let (c7, c8, c9) = (6, 6, 7);  // 耦合, 跨行, 同指跨
            let (c10, c11, c12, c13) = (6, 6, 6, 6);  // 小指, 伸展, 滚动, 连击
            let (c14, c15, c16) = (4, 7, 7);  // 次, 联动, 合计

            println!("{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
                pr("键对", c0), pr("分类", c1), pr("手指路径", c2),
                pl("神经", c3), pl("原始", c4), pl("折扣", c5), pl("移动", c6),
                pl("耦合", c7), pl("跨行", c8), pl("同指跨", c9),
                pl("小指", c10), pl("伸展", c11), pl("滚动", c12), pl("连击", c13),
                pl("次", c14), pl("联动", c15), pl("合计", c16));

            let total_w = c0+c1+c2+c3+c4+c5+c6+c7+c8+c9+c10+c11+c12+c13+c14+c15+c16;
            let sep = "─".repeat(total_w);
            println!("{}", sep);

            for p in &pairs {
                let pair_str = format!("{}→{}", p.prev_ch, p.curr_ch);
                let discount_str = match p.move_discount {
                    Some(d) => format!("{:.2}", d),
                    None => "—".to_string(),
                };
                println!("{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
                    pr(&pair_str, c0),
                    pr(p.category, c1),
                    pr(&p.finger_path, c2),
                    pl(&format!("{:.1}", p.t_neural), c3),
                    pl(&format!("{:.1}", p.t_move_raw), c4),
                    pl(&discount_str, c5),
                    pl(&format!("{:.1}", p.t_move), c6),
                    pl(&format!("{:.1}", p.t_couple), c7),
                    pl(&format!("{:.1}", p.t_row), c8),
                    pl(&format!("{:.1}", p.t_sf_jump), c9),
                    pl(&format!("{:.1}", p.t_pinky), c10),
                    pl(&format!("{:.1}", p.t_stretch), c11),
                    pl(&format!("{:.1}", p.t_roll), c12),
                    pl(&format!("{:.1}", p.t_repeat), c13),
                    pl(&format!("{}", p.repeat_count), c14),
                    pl(&format!("{:+.1}", p.tendon_delta), c15),
                    pl(&format!("{:.1}", p.total), c16),
                );
                if let Some(note) = &p.note {
                    println!("   └─ {}", note);
                }
            }

            println!("{}", sep);
            println!("  ⊙ 逐步累加总计:   {:.2} 毫秒", pairs.iter().map(|p| p.total).sum::<f64>());
            println!("  ⊙ 左手子序列下界: {:.2} 毫秒", left_time);
            println!("  ⊙ 右手子序列下界: {:.2} 毫秒", right_time);
            println!("  ★ 最终时间:       {:.2} 毫秒", total);
        }
    }
}