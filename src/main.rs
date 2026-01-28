use rand::prelude::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::time::Instant;
use chrono::Local;

// =========================================================================
// 🔧 配置区域 (可调参数)
// =========================================================================
mod config {
    // [文件路径]
    pub const FILE_FIXED: &str = "input-fixed.txt";      // 格式: 字根 [tab] 键位(a-z)
    pub const FILE_DYNAMIC: &str = "input-roots.txt";    // 格式: 字根 (每行一个)
    pub const FILE_SPLITS: &str = "input-division.txt";  // 格式: 汉字 [tab] 字根1字根2...

    // [模拟退火 - 核心参数]
    // 线程数：建议设为 CPU 核心数。每个线程都会跑一遍完整的 SA。
    pub const NUM_THREADS: usize =8;

    // 单线程迭代步数：越多效果越好，但时间越长。
    // 因为增量计算极快，建议 2000万 ~ 5000万 起步。
    pub const TOTAL_STEPS: usize = 10_000_000_000;

    // 初始温度：决定了算法开始时的"胡乱探索"程度。
    pub const TEMP_START: f64 = 3000.0;
    pub const TEMP_END: f64 = 0.001;
    pub const DECAY_RATE: f64 = 0.999995;

    // [变异策略]
    // 交换概率：由"交换两个字根"而不是"移动一个字根"的概率。
    pub const SWAP_PROBABILITY: f64 = 0.6;  // 交换概率

    // [自适应降温参数]
    pub const T_START_RATE: f64 = 0.00012;               // 高温温控系数
    pub const T_END_RATE: f64 = 300.0;                   // 低温温控系数
    pub const T_PERTURBATION_RATE: f64 = 1.03;           // 扰动温控系数
    // 修改 reheat 和 perturbation 的温度判定
    pub const REHEAT_THRESHOLD: f64 = 0.1;               // 加温阈值
    pub const REHEAT_FACTOR: f64 = 2.0;                  // 温度回升因子
    pub const MIN_IMPROVE_STEPS: usize = 1_000_000_000;  // 每10亿步无改进后考虑升温
    pub const PERTURB_INTERVAL: usize = 2_000_000_000;   // 每20亿步无改进后扰动一次
    pub const PERTURBATION_THRESHOLD: f64 = 0.3;         // 扰动阈值
    pub const PERTURB_STRENGTH: f64 = 0.15;              // 扰动15%的字根
    pub const ACCEPTANCE_TARGET: f64 = 0.2;              // 目标接受率
}

// =========================================================================
// 🚀 高性能数据结构 & 预处理
// =========================================================================

// 将3个键位 (0-25) 压缩为一个整数。
// 采用 27 进制：0 表示空，1-26 表示 a-z。
// Max Code = 26 * 27^2 + 26 * 27 + 26 = 19682，可以用 u16 存储，但用 usize 索引更快。
const MAX_CODE_VAL: usize = 27 * 27 * 27 + 100;

#[derive(Clone)]
struct CharPackedInfo {
    // 存储汉字的结构。
    // 数组元素含义：
    // 0..26   : 固定键位 (0=a, 25=z)
    // >= 1000 : 动态字根索引 (值 - 1000 = dynamic_root_index)
    parts: Vec<u16>,
}

struct OptContext {
    num_dynamic_roots: usize,
    // 倒排索引：dynamic_root_index -> [char_index_1, char_index_2, ...]
    root_to_char_indices: Vec<Vec<usize>>,
    // 所有汉字的预处理结构
    char_infos: Vec<CharPackedInfo>,
    // 原始数据（用于最后输出）
    raw_splits: Vec<(char, Vec<String>)>,
    raw_dynamic_roots: Vec<String>,
    // 固定字根映射表（用于输出）
    fixed_map: HashMap<String, u8>,
}

impl OptContext {
    fn new(
        splits: &[(char, Vec<String>)],
        fixed_map: &HashMap<String, u8>,
        dynamic_roots: &[String],
    ) -> Self {
        let mut root_to_idx: HashMap<&str, usize> = HashMap::new();
        for (i, r) in dynamic_roots.iter().enumerate() {
            root_to_idx.insert(r.as_str(), i);
        }

        let num_dynamic_roots = dynamic_roots.len();
        let mut root_to_char_indices = vec![Vec::new(); num_dynamic_roots];
        let mut char_infos = Vec::with_capacity(splits.len());

        for (char_idx, (_, roots)) in splits.iter().enumerate() {
            let mut parts = Vec::new();
            let mut used_dynamic_indices = HashSet::new(); // 避免同一个字里同一个字根重复记录倒排

            for root in roots.iter().take(3) { // 只取前三码
                if let Some(&key) = fixed_map.get(root) {
                    parts.push(key as u16);
                } else if let Some(&dyn_idx) = root_to_idx.get(root.as_str()) {
                    parts.push((dyn_idx + 1000) as u16);
                    used_dynamic_indices.insert(dyn_idx);
                }
            }

            // 填充倒排索引
            for &dyn_idx in &used_dynamic_indices {
                root_to_char_indices[dyn_idx].push(char_idx);
            }

            char_infos.push(CharPackedInfo { parts });
        }

        Self {
            num_dynamic_roots,
            root_to_char_indices,
            char_infos,
            raw_splits: splits.to_vec(),
            raw_dynamic_roots: dynamic_roots.to_vec(),
            fixed_map: fixed_map.clone(),
        }
    }

    // 计算单个汉字的压缩编码
    #[inline(always)]
    fn calc_code(&self, char_idx: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[char_idx];
        let mut code = 0usize;
        for &p in &info.parts {
            let key = if p >= 1000 {
                assignment[(p - 1000) as usize]
            } else {
                p as u8
            };
            // 1-based indexing for base 27 conversion (0 is reserved/unused in this logic but safe)
            code = code * 27 + (key as usize + 1);
        }
        code
    }
}

// =========================================================================
// ⚡ 评估器 (状态机)
// =========================================================================

struct Evaluator {
    // 当前每个汉字的编码值
    current_codes: Vec<usize>,
    // 桶计数：buckets[code] = 该编码出现的次数
    buckets: Vec<u16>,
    // 当前的总重码数 (目标函数值)
    total_collisions: usize,
}

impl Evaluator {
    fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let mut buckets = vec![0u16; MAX_CODE_VAL];
        let mut current_codes = Vec::with_capacity(ctx.char_infos.len());
        let mut total_collisions = 0;

        for i in 0..ctx.char_infos.len() {
            let code = ctx.calc_code(i, assignment);
            current_codes.push(code);

            // 如果桶里已经有人了，说明发生碰撞
            if buckets[code] >= 1 {
                total_collisions += 1;
            }
            buckets[code] += 1;
        }

        Self {
            current_codes,
            buckets,
            total_collisions,
        }
    }

    // 尝试交换两个字根的键位 (Swap)
    // 如果接受，返回 true；如果拒绝，自动回滚并返回 false
    #[inline]
    fn try_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 { return false; }

        let old_score = self.total_collisions;

        // 1. 执行修改
        assignment[r1] = k2;
        assignment[r2] = k1;

        // 2. 计算 Delta (利用 dirty flag 或两遍遍历)
        // 为了极致性能，我们直接遍历受影响的汉字并更新 buckets
        // 注意：如果一个字同时包含 r1 和 r2，会被处理两次，但这在桶计数逻辑中是安全的（先减后加，最终平）
        self.update_diff(ctx, assignment, r1);
        self.update_diff(ctx, assignment, r2);

        let new_score = self.total_collisions;
        let delta = new_score as i64 - old_score as i64;

        // 3. 接受准则
        if delta <= 0 || rng.gen::<f64>() < (-delta as f64 / temp).exp() {
            true // Accept
        } else {
            // Reject: 回滚
            assignment[r1] = k1;
            assignment[r2] = k2;
            self.update_diff(ctx, assignment, r1);
            self.update_diff(ctx, assignment, r2);
            false
        }
    }

    // 尝试移动一个字根到新键位 (Move)
    #[inline]
    fn try_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let old_key = assignment[r];
        if old_key == new_key { return false; }

        let old_score = self.total_collisions;

        // 1. 执行修改
        assignment[r] = new_key;

        // 2. 更新受影响的汉字
        self.update_diff(ctx, assignment, r);

        let new_score = self.total_collisions;
        let delta = new_score as i64 - old_score as i64;

        if delta <= 0 || rng.gen::<f64>() < (-delta as f64 / temp).exp() {
            true
        } else {
            // Reject: 回滚
            assignment[r] = old_key;
            self.update_diff(ctx, assignment, r);
            false
        }
    }

    // 核心增量更新逻辑
    // 根据 assignment 重新计算 root_idx 相关汉字的编码，并更新 buckets 和 collision
    #[inline(always)]
    fn update_diff(&mut self, ctx: &OptContext, assignment: &[u8], root_idx: usize) {
        let affected = &ctx.root_to_char_indices[root_idx];
        for &char_idx in affected {
            let old_code = self.current_codes[char_idx];
            let new_code = ctx.calc_code(char_idx, assignment);

            if old_code == new_code { continue; }

            // 移除旧编码贡献
            self.buckets[old_code] -= 1;
            // 如果原本数量 > 1，现在减少了一个，那么碰撞数 -1
            // 比如 2->1 (coll 1->0), 3->2 (coll 2->1)
            if self.buckets[old_code] >= 1 {
                self.total_collisions -= 1;
            }

            // 添加新编码贡献
            // 如果原本数量 >= 1，现在增加了一个，碰撞数 +1
            // 比如 1->2 (coll 0->1), 2->3 (coll 1->2)
            if self.buckets[new_code] >= 1 {
                self.total_collisions += 1;
            }
            self.buckets[new_code] += 1;

            self.current_codes[char_idx] = new_code;
        }
    }
}

// =========================================================================
// 🧠 算法实现
// =========================================================================

// 智能初始化：基于频率的负载均衡
fn smart_init(ctx: &OptContext) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_dynamic_roots];
    let mut rng = thread_rng();

    // 计算每个字根的使用频率
    let mut root_freq: Vec<(usize, usize)> = ctx.root_to_char_indices
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    // 频率从高到低排序
    root_freq.sort_by(|a, b| b.1.cmp(&a.1));

    let mut key_counts = [0; 26];

    for (root_idx, _) in root_freq {
        // 寻找当前分配字根最少的键位集合
        let min_count = *key_counts.iter().min().unwrap();
        let candidates: Vec<usize> = (0..26)
            .filter(|&k| key_counts[k] == min_count)
            .collect();

        let best_key = candidates[rng.gen_range(0..candidates.len())];

        assignment[root_idx] = best_key as u8;
        key_counts[best_key] += 1;
    }
    assignment
}

fn simulated_annealing(ctx: &OptContext, thread_id: usize, thread_dir: &str) -> (Vec<u8>, usize) {
    let mut rng = thread_rng();

    // 创建日志文件
    let log_path = format!("{}/log.txt", thread_dir);
    let log_file = File::create(&log_path).expect("无法创建日志文件");
    let mut log_writer = BufWriter::new(log_file);

    // 写入初始信息
    writeln!(log_writer, "=== 线程 {} 开始运行 ===", thread_id).unwrap();
    writeln!(log_writer, "总步数: {}", config::TOTAL_STEPS).unwrap();
    writeln!(log_writer, "初始温度: {}, 结束温度: {}", config::TEMP_START, config::TEMP_END).unwrap();

    // 初始化：50%概率用智能初始化，50%概率随机打乱智能初始化的结果
    let mut assignment = smart_init(ctx);
    if rng.gen_bool(0.5) {
        for val in assignment.iter_mut() {
            if rng.gen_bool(0.1) { *val = rng.gen_range(0..26); }
        }
    }

    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.total_collisions;

    writeln!(log_writer, "初始重码数: {}", best_score).unwrap();

    let steps = config::TOTAL_STEPS;
    let t_start = config::TEMP_START;
    let t_end = config::TEMP_END;
    let decay_rate = config::DECAY_RATE;
    let mut temp = t_start;

    // 自适应参数
    let mut steps_since_improve = 0;
    let mut last_best_score = best_score;
    let mut accepted_moves = 0;
    let mut total_moves = 0;

    let n_roots = assignment.len();

    // 进度打印控制
    let report_interval = steps / 20;  // 每5%报告一次

    for step in 0..steps {
        // 记录是否接受移动
        let mut accepted = false;

        // 策略选择
        if rng.gen::<f64>() < config::SWAP_PROBABILITY {
            // Swap
            let r1 = rng.gen_range(0..n_roots);
            let r2 = rng.gen_range(0..n_roots);
            if r1 != r2 {
                accepted = evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
            }
        } else {
            // Move
            let r = rng.gen_range(0..n_roots);
            let new_k = rng.gen_range(0..26);
            accepted = evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
        }

        // 统计
        total_moves += 1;
        if accepted {
            accepted_moves += 1;
        }

        // 更新全局最优
        if evaluator.total_collisions < best_score {
            best_score = evaluator.total_collisions;
            best_assignment = assignment.clone();
            steps_since_improve = 0;

            // 写入日志
            if best_score <= last_best_score - 1 {
                let msg = format!("   [T{}] Step {}/{} | Temp {:.9} | New Best: {}",
                        thread_id, step, steps, temp, best_score);
                writeln!(log_writer, "{}", msg).unwrap();
                // 线程0同时打印到控制台
                if thread_id == 0 {
                    println!("{}", msg);
                }
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        // 自适应温度调整
        if step > 0 && step % 10000 == 0 {
            let acceptance_rate = accepted_moves as f64 / total_moves as f64;

            // 调整温度：如果接受率太低，需要升温；如果接受率合适，正常降温
            if acceptance_rate < config::ACCEPTANCE_TARGET * 0.5 && temp < t_start * config::T_START_RATE {
                // 接受率太低，稍微升温
                temp *= 1.005;
            } else if acceptance_rate > config::ACCEPTANCE_TARGET * 1.5 && temp > t_end * config::T_END_RATE {
                // 接受率太高，稍微降温
                temp *= 0.95;
            } else {
                // 正常降温
                temp *= decay_rate;
            }

            // 重置统计
            accepted_moves = 0;
            total_moves = 0;
        }

        // 长时间无改进，重新加热
        if steps_since_improve > config::MIN_IMPROVE_STEPS && temp < config::REHEAT_THRESHOLD {
            temp = (temp * config::REHEAT_FACTOR).min(t_start);
            steps_since_improve = 0;

            if rng.gen_bool(0.01) {
                let msg = format!("   [T{}] Step {}: 重新加热到 {:.9}", thread_id, step, temp);
                writeln!(log_writer, "{}", msg).unwrap();
                if thread_id == 0 {
                    println!("{}", msg);
                }
            }
        }

        // 周期性强力扰动
        if steps_since_improve > config::MIN_IMPROVE_STEPS && step % config::PERTURB_INTERVAL == 0 && temp < config::PERTURBATION_THRESHOLD {
            let n_perturb = (n_roots as f64 * config::PERTURB_STRENGTH) as usize;
            for _ in 0..n_perturb {
                let r1 = rng.gen_range(0..n_roots);
                let r2 = rng.gen_range(0..n_roots);
                if r1 != r2 {
                    evaluator.try_swap(ctx, &mut assignment, r1, r2, temp * config::T_PERTURBATION_RATE, &mut rng);
                }
            }

            let msg = format!("   [T{}] Step {}: 强力扰动，当前温度 {:.9}, 当前重码: {}",
                    thread_id, step, temp, evaluator.total_collisions);
            writeln!(log_writer, "{}", msg).unwrap();
            if thread_id == 0 {
                println!("{}", msg);
            }
        }

        // 确保温度不低于最小值
        temp = temp.max(t_end);

        // 进度报告
        if step % report_interval == 0 && step > 0 {
            let progress = step * 100 / steps;
            let current_collisions = evaluator.total_collisions;
            let msg = format!("   [T{}] Progress: {}% | Temp: {:.9} | Curr: {} | Best: {}",
                     thread_id, progress, temp, current_collisions, best_score);
            writeln!(log_writer, "{}", msg).unwrap();
            if thread_id == 0 {
                println!("{}", msg);
            }
        }
    }

    writeln!(log_writer, "\n=== 线程 {} 完成 ===", thread_id).unwrap();
    writeln!(log_writer, "最终最优重码数: {}", best_score).unwrap();
    log_writer.flush().unwrap();

    (best_assignment, best_score)
}

// =========================================================================
// 📂 文件加载
// =========================================================================

fn get_path(fname: &str) -> String {
    // 简单实现：只看当前目录
    fname.to_string()
}

fn load_fixed(path: &str) -> HashMap<String, u8> {
    let content = fs::read_to_string(path).expect("无法读取固定字根文件");
    let mut map = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let key_char = parts[1].trim().chars().next().unwrap();
            let key_code = if key_char >= 'a' && key_char <= 'z' {
                key_char as u8 - b'a'
            } else {
                continue;
            };
            map.insert(parts[0].trim().to_string(), key_code);
        }
    }
    map
}

fn load_dynamic(path: &str) -> Vec<String> {
    fs::read_to_string(path)
        .expect("无法读取动态字根文件")
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn load_splits(path: &str) -> Vec<(char, Vec<String>)> {
    let content = fs::read_to_string(path).expect("无法读取拆分表");
    let mut res = Vec::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let ch = parts[0].chars().next().unwrap();
            // 这里假设格式是：汉字\t字根1字根2... (无空格)
            // 或者是汉字\t字根1 字根2 (有空格)
            // 根据你的旧代码 `filter(|c| !c.is_whitespace())`，似乎是一串字符？
            // 你的旧代码逻辑：roots = parts[1].chars()...map()...
            // 如果拆分表是 "明\t日月"，则 roots=["日", "月"]
            // 必须确保这与你的数据格式匹配。这里保留通用逻辑：
            // 如果文件里字根是连在一起的单字符，用 chars()。如果是空格分隔的字符串，用 split_whitespace()。
            // 假设是单字符字根：
            let roots: Vec<String> = parts[1]
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| c.to_string())
                .collect();

            res.push((ch, roots));
        }
    }
    res
}

// =========================================================================
// 🏁 主函数
// =========================================================================

fn main() {
    let start_time = Instant::now();

    // 生成带时间戳的输出目录
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let output_base_dir = format!("output-{}", timestamp);
    fs::create_dir_all(&output_base_dir).expect("无法创建输出目录");

    println!("=== 三码增量退火算法 (Simulated Annealing High-Performance) ===");
    println!("输出目录: {}", output_base_dir);
    println!("线程数: {}, 总步数: {}", config::NUM_THREADS, config::TOTAL_STEPS);
    println!("初始温度: {}, 结束温度: {}", config::TEMP_START, config::TEMP_END);
    println!("交换概率: {}, 扰动强度: {}", config::SWAP_PROBABILITY, config::PERTURB_STRENGTH);

    // 1. 加载数据
    let fixed_map = load_fixed(&get_path(config::FILE_FIXED));
    let dynamic_roots = load_dynamic(&get_path(config::FILE_DYNAMIC));
    let splits = load_splits(&get_path(config::FILE_SPLITS));

    println!("数据加载完毕:");
    println!("  - 固定字根: {}", fixed_map.len());
    println!("  - 动态字根: {}", dynamic_roots.len());
    println!("  - 汉字数量: {}", splits.len());

    // 2. 构建上下文
    let ctx = OptContext::new(&splits, &fixed_map, &dynamic_roots);

    // 3. 为每个线程创建子目录
    for i in 0..config::NUM_THREADS {
        let thread_dir = format!("{}/{}", output_base_dir, i);
        fs::create_dir_all(&thread_dir).expect("无法创建线程目录");
    }

    // 4. 并行执行 SA
    println!("\n开始优化...");
    println!("每个线程 {} 步，共 {} 步",
             config::TOTAL_STEPS,
             config::NUM_THREADS * config::TOTAL_STEPS);

    // 使用 Rayon 并行迭代器
    let results: Vec<(usize, Vec<u8>, usize)> = (0..config::NUM_THREADS)
        .into_par_iter()
        .map(|i| {
            let thread_dir = format!("{}/{}", output_base_dir, i);
            let (assignment, score) = simulated_annealing(&ctx, i, &thread_dir);
            // 保存每个线程的结果
            save_thread_results(&ctx, &assignment, score, &thread_dir);
            (i, assignment, score)
        })
        .collect();

    // 5. 汇总结果
    let (best_thread, best_assignment, best_score) = results.iter()
        .min_by_key(|r| r.2)
        .map(|(t, a, s)| (*t, a.clone(), *s))
        .unwrap();

    println!("\n=================================");
    println!("🏆 最优结果: {} 重码 (来自线程 {})", best_score, best_thread);
    println!("⏱️ 总耗时: {:?}", start_time.elapsed());
    println!("=================================");

    // 6. 生成总结文件
    generate_summary(&output_base_dir, &results, best_thread, start_time.elapsed());

    // 7. 在根目录也保存最优结果
    save_thread_results(&ctx, &best_assignment, best_score, &output_base_dir);
    println!("结果已保存至 {}/", output_base_dir);
}

fn save_thread_results(ctx: &OptContext, assignment: &[u8], score: usize, output_dir: &str) {
    // 1. 保存字根键位
    let mut root_out = String::new();
    root_out.push_str(&format!("总重码数: {}\n", score));
    root_out.push_str("动态字根键位分配表:\n");
    for (i, root_str) in ctx.raw_dynamic_roots.iter().enumerate() {
        let key = (assignment[i] + b'a') as char;
        root_out.push_str(&format!("{}\t{}\n", root_str, key));
    }
    fs::write(format!("{}/output-字根.txt", output_dir), root_out).unwrap();

    // 2. 保存汉字编码
    let mut code_out = String::new();

    // 为所有字根（固定+动态）建立查找表
    let mut root_to_key: HashMap<String, u8> = HashMap::new();

    // 添加固定字根
    for (root_str, &key) in &ctx.fixed_map {
        root_to_key.insert(root_str.clone(), key);
    }

    // 添加动态字根
    for (i, root_str) in ctx.raw_dynamic_roots.iter().enumerate() {
        root_to_key.insert(root_str.clone(), assignment[i]);
    }

    // 计算每个汉字的编码
    for (ch, roots) in &ctx.raw_splits {
        let mut code_parts = Vec::new();

        // 只取前三个字根
        for root in roots.iter().take(3) {
            if let Some(&key) = root_to_key.get(root) {
                let key_char = (key + b'a') as char;
                code_parts.push(key_char);
            }
        }

        // 将字根编码连接成字符串
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\n", ch, code_str));
    }

    fs::write(format!("{}/output-编码.txt", output_dir), code_out).unwrap();
}

fn generate_summary(output_dir: &str, results: &[(usize, Vec<u8>, usize)], best_thread: usize, elapsed: std::time::Duration) {
    let mut summary = String::new();

    summary.push_str("=== 模拟退火优化总结 ===\n\n");
    summary.push_str(&format!("总耗时: {:?}\n", elapsed));
    summary.push_str(&format!("线程数: {}\n", config::NUM_THREADS));
    summary.push_str(&format!("每线程步数: {}\n", config::TOTAL_STEPS));
    summary.push_str(&format!("初始温度: {}\n", config::TEMP_START));
    summary.push_str(&format!("结束温度: {}\n", config::TEMP_END));
    summary.push_str(&format!("温度衰减系数: {}\n", config::DECAY_RATE));
    summary.push_str(&format!("交换概率: {}\n", config::SWAP_PROBABILITY));
    summary.push_str(&format!("高温温控系数: {}\n", config::T_START_RATE));
    summary.push_str(&format!("低温温控系数: {}\n", config::T_END_RATE));
    summary.push_str(&format!("扰动温控系数: {}\n", config::T_PERTURBATION_RATE));
    summary.push_str(&format!("加温阈值: {}\n", config::REHEAT_THRESHOLD));
    summary.push_str(&format!("温度回升因子: {}\n", config::REHEAT_FACTOR));
    summary.push_str(&format!("无改进升温周期: {}\n", config::MIN_IMPROVE_STEPS));
    summary.push_str(&format!("扰动周期步数: {}\n", config::PERTURB_INTERVAL));
    summary.push_str(&format!("扰动阈值: {}\n", config::PERTURBATION_THRESHOLD));
    summary.push_str(&format!("扰动强度: {}\n", config::PERTURB_STRENGTH));
    summary.push_str(&format!("目标接受率: {}\n\n", config::ACCEPTANCE_TARGET));

    summary.push_str("=== 各线程成绩 ===\n\n");

    // 按重码数排序
    let mut sorted_results: Vec<_> = results.iter().collect();
    sorted_results.sort_by_key(|(_, _, score)| *score);

    for (rank, (thread_id, _, score)) in sorted_results.iter().enumerate() {
        let marker = if *thread_id == best_thread { " 🏆" } else { "" };
        summary.push_str(&format!("#{} 线程 {}: {} 重码{}\n", rank + 1, thread_id, score, marker));
    }

    summary.push_str(&format!("\n=== 最优结果 ===\n"));
    summary.push_str(&format!("最优线程: {}\n", best_thread));
    summary.push_str(&format!("最优重码数: {}\n", results.iter().min_by_key(|r| r.2).unwrap().2));
    summary.push_str(&format!("详细结果见: {}/{}/\n", output_dir, best_thread));

    fs::write(format!("{}/总结.txt", output_dir), summary).unwrap();
}