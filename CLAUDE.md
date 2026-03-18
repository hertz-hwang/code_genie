# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

CodeGenie 是一个**字根编码优化器**，使用模拟退火算法优化汉字输入法的字根到键位映射方案。目标是最小化重码数、当量值和键位分布偏差，同时支持简码优化。

## 构建与运行命令

```bash
# 编译（Release 模式，推荐用于优化）
cargo build --release

# 运行优化（默认行为，等同于 optimize 子命令）
cargo run --release

# 使用指定配置文件运行
cargo run --release -- -c config.toml

# 编码模式：根据 keymap 为汉字编码
cargo run --release -- encode -k output-keymap.txt -d input-division.txt -o output-encode.txt

# 评估模式：评估现有编码方案
cargo run --release -- evaluate -k output-keymap.txt
cargo run --release -- evaluate -k output-keymap.txt -d input-division.txt --keydist key_distribution.txt --equiv pair_equivalence.txt --simple simple.txt -o output-evaluate.txt

# 检查编译错误
cargo check
```

## 核心架构

### 数据流

```
输入文件 → loader.rs → OptContext::new() → annealing.rs → Evaluator → output.rs → 输出文件
```

### 模块职责

| 模块 | 职责 |
|------|------|
| `main.rs` | CLI 入口（clap），定义三个子命令：optimize/encode/evaluate，编排整个流程 |
| `annealing.rs` | 混合优化算法核心：4 种初始化策略、multi-start + hill climb 热身、冲突导向邻域搜索、SA 主循环 |
| `context.rs` | `OptContext` 构建所有优化数据：字根组、汉字信息、当量表、简码配置、反向索引 |
| `evaluator.rs` | `Evaluator` 计算重码数/率、当量、分布偏差等指标；`SimpleEvaluator` 处理简码评估；支持 `try_move()`/`try_swap()` 增量更新 |
| `loader.rs` | 加载固定字根、动态字根、拆分表、当量表、键位分布、keymap 文件 |
| `types.rs` | 核心类型：`CharInfo`、`RootGroup`、`Metrics`、`SimpleMetrics`、`LogicalRoot`、`CharSimpleInfo`、`SimpleCodeConfig` 等 |
| `config.rs` | TOML 配置解析（`Config` 及子结构体），权重校验，简码规则转换，默认配置后备 |
| `schedule.rs` | `TemperatureSchedule` 温度调度器，高斯舒适区自适应降温，预计算 LUT |
| `calibrate.rs` | `calibrate_scales()` 根据初始指标自动校准各维度缩放因子 |
| `simple.rs` | 解析独立简码规则文件（非 TOML 格式），用于 evaluate 子命令的 `--simple` 参数 |
| `output.rs` | 输出所有结果文件：keymap、编码、简码、合并编码、键位分布、当量分布、summary |
| `validate.rs` | 校验拆分表中所有字根是否已定义，输出缺失报告 |

### 关键数据结构

- **OptContext**: 优化上下文，包含 `groups`（字根组）、`chars`（汉字信息）、`group_to_chars`（反向索引）、`equiv_table`（当量矩阵）、`simple_config`、`char_simple_info`（逻辑根+级别指令）、`group_to_simple_affected` 等
- **Evaluator**: 维护 `code_to_chars` 碰撞桶、`key_weighted_usage` 键位使用统计，支持增量 move/swap 操作和 Boltzmann 接受判定
- **SimpleEvaluator**: 按级别跟踪简码分配，计算简码重码数/率、频率覆盖、当量、分布
- **RootGroup**: 字根组，包含 `roots`（字根名列表）和 `allowed_keys`（允许分配的键位）
- **CharInfo**: 汉字拆分信息，`parts: Vec<u16>` 中值 < 1000 为直接键位索引，>= 1000 为组标记（GROUP_MARKER + group_index）
- **LogicalRoot**: 逻辑根，桥接拆分表与简码系统，包含 base_name 和有序子根列表

### 字根分类

1. **固定字根**: 单键映射，从 `input-fixed.txt` 加载，不参与优化
2. **有约束字根**: 多键选择（也在 `input-fixed.txt` 中定义），作为 `RootGroup` 参与优化但键位有约束
3. **动态字根**: 需要优化分配，从 `input-roots.txt` 加载，同行字根为一组

### 简码系统

简码规则格式为 `AaBaCa`，每两个字符为一组：
- 大写字母（根选择器）: A=第1根, B=第2根, ... Z=最后一根
- 小写字母（码位选择器）: a=第1码, b=第2码, ... z=最后一码

配置中 `code_num` 表示该级别可分配的简码数量（0 表示不限制，按频率贪心分配）。每个级别可配置多个候选规则（`rules` 数组），系统选择能成功解析的第一个。

### 模拟退火算法

**初始化阶段** (`multi_start_init`):
- 生成 12-32 个候选解，轮流使用 4 种策略：均衡贪心、频率贪心、分散贪心、随机
- 每个候选经 `enhanced_hill_climb` 精炼（混合操作：40% 冲突解决、30% 交换、10% 三路交换、10% 键位重组、10% 随机移动）
- 小规模问题额外执行坐标下降

**SA 主循环** (`simulated_annealing`):
- 按 `swap_probability`（随时间线性增长）选择 swap 或 move 操作
- `Evaluator::try_move/try_swap` 内部完成增量评估 + Boltzmann 接受/回滚
- 重热机制：连续 `min_improve_steps` 步无改进时乘以 `reheat_factor`
- 智能扰动：低温时周期性扰动碰撞相关组
- 最终精炼：对最优解执行 hill climb + 坐标下降

## 输入文件格式

| 文件 | 格式 |
|------|------|
| `input-fixed.txt` | `字根名 [tab] 键位`（单键=固定，多键=约束） |
| `input-roots.txt` | `字根名1 字根名2 ...`（同行为一组，空格分隔） |
| `input-division.txt` | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `pair_equivalence.txt` | `键对 [tab] 当量值` |
| `key_distribution.txt` | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件 (config.toml)

从 `config.toml.example` 复制为 `config.toml` 使用。

关键配置项：

```toml
[files]
fixed = "input-fixed.txt"
dynamic = "input-roots.txt"
splits = "input-division.txt"
pair_equiv = "pair_equivalence.txt"
key_dist = "key_distribution.txt"

[annealing]
threads = 12              # 并行线程数
total_steps = 2000000     # 总迭代步数
temp_start = 100.0        # 初始温度
temp_end = 0.000001       # 结束温度
comfort_temp = 0.4        # 舒适温度（高斯中心）
comfort_width = 0.15      # 舒适区宽度（高斯 sigma）
comfort_slowdown = 0.7    # 舒适区减速因子 (0-1)
swap_probability = 0.35   # 交换操作概率
min_improve_steps_ratio = 0.1  # 无改进触发重热的步数比例
perturb_interval_ratio = 0.0   # 扰动间隔比例（0=关闭）
perturb_strength = 0.0         # 扰动强度（0=关闭）
reheat_factor = 1.0            # 重热倍率（1.0=关闭）
max_parts = 3                  # 最大码长

[keys]
allowed = "qwertyuiopasdfghjklzxcvbnm"
display_order = "qwertyuiopasdfghjklzxcvbnm"

[weights.full_code]
collision_count = 0.19   # 重码数权重
collision_rate = 0.3     # 重码率权重
equivalence = 0.3        # 当量权重
equiv_cv = 0.01          # 当量变异系数权重
distribution = 0.2       # 分布偏差权重

[weights.simple_code]
enabled = true
full_code_weight = 0.6   # 全码目标总权重
simple_code_weight = 0.4 # 简码目标总权重
collision_count = 0.1    # 简码重码数权重
collision_rate = 0.3     # 简码重码率权重
freq = 0.2               # 频率覆盖权重
equiv = 0.2              # 当量权重
dist = 0.2               # 分布偏差权重

[[simple_levels]]
level = 1
code_num = 1             # 该级别分配的简码数
rules = ["Aa"]           # 取码规则

[[simple_levels]]
level = 2
code_num = 1
rules = ["AaBa"]

[[simple_levels]]
level = 3
code_num = 0             # 0 = 不分配该级别简码
rules = ["AaAbZz", "AaBaZz"]  # 多候选规则
```

## 性能优化要点

- 使用 `rayon` 并行执行多线程模拟退火（每线程独立 SA 实例）
- `Evaluator` 支持增量更新（move/swap 只重算受影响的汉字），避免全量重算
- 温度调度使用预计算查找表 (LUT, 100001 个采样点 + 线性插值)
- Release 编译启用 `opt-level=3`、LTO、单代码单元、`panic=abort`
- `calibrate_scales` 自动校准使各指标在初始状态贡献均衡

## 输出文件

优化完成后生成 `output-{YYYYMMDDHHMMSS}/` 目录：
- `output-keymap.txt`: 字根到键位映射
- `output-encode.txt`: 汉字编码结果
- `output-simple-codes.txt`: 简码分配（按级别、按频率排序）
- `output-combined.txt`: 简码+全码合并输出
- `output-distribution.txt`: 键位使用分布统计
- `output-equiv-dist.txt`: 当量分布统计
- `summary.txt`: 各线程结果排名汇总
- `thread-XX/`: 每个线程的独立输出子目录（包含上述同名文件）

## 依赖

| crate | 用途 |
|-------|------|
| `clap` (4, derive) | CLI 参数解析 |
| `rayon` (1.10) | 并行迭代 |
| `rand` (0.8, small_rng) | 随机数生成 |
| `serde` (1.0, derive) + `toml` (0.8) | TOML 配置反序列化 |
| `chrono` (0.4) | 输出目录时间戳 |
| `hashbrown` (0.14) / `rustc-hash` (1.1) | 高性能哈希容器 |
| `serde_json` (1.0) | JSON 支持 |
