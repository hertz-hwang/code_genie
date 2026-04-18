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

# 使用 AMHB 算法优化
cargo run --release -- optimize --amhb

# 使用键魂当量模型（替代 pair_equivalence.txt）
cargo run --release -- optimize --keysoul

# 从检查点恢复优化（断点续算）
cargo run --release -- resume -f checkpoint.json

# 编码模式：根据 keymap 为汉字编码
cargo run --release -- encode -k output-keymap.txt -d input-division.txt -o output-encode.txt

# 评估模式：评估现有编码方案
cargo run --release -- evaluate -k output-keymap.txt
cargo run --release -- evaluate -k output-keymap.txt -d input-division.txt --keydist key_distribution.txt --equiv pair_equivalence.txt --simple simple.txt -o output-evaluate.txt
cargo run --release -- evaluate -k output-keymap.txt --keysoul

# 键魂模式：分析击键序列当量（--debug 输出逐键时间分解）
cargo run --release -- keysoul [--debug]

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
| `main.rs` | CLI 入口（clap），定义五个子命令：optimize/encode/evaluate/resume/keysoul，编排整个流程 |
| `annealing.rs` | 混合优化算法核心：4 种初始化策略、multi-start + hill climb 热身、冲突导向邻域搜索、SA 主循环；`simulated_annealing_resumable` 支持断点续算 |
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
| `checkpoint.rs` | 断点续算：`Checkpoint`/`ThreadCheckpoint` 序列化为 JSON，`save_checkpoint`/`load_checkpoint`；原子写（先写 `.tmp` 再 rename）防止崩溃损坏 |
| `keysoul.rs` | 键魂当量模型 v2.3：基于 Fitts 定律 + 神经延迟 + 肌腱联动的物理击键时间计算（ms）；`KeySoulModel` 预构建键盘模型，`calc_keysoul_from_indices` 为全局单例接口 |
| `amhb/` | AMHB（Adaptive Multi-candidate Heat Bath）算法：`optimizer.rs` 主循环（零锁裸指针多线程）、`operator_pool.rs` EXP3 自适应算子选择、`operators.rs` 点移/交换算子、`worker.rs` 工作线程 |

### 关键数据结构

- **OptContext**: 优化上下文，包含 `groups`（字根组）、`chars`（汉字信息）、`group_to_chars`（反向索引）、`equiv_table`（当量矩阵）、`simple_config`、`char_simple_info`（逻辑根+级别指令）、`group_to_simple_affected`、`group_freq_sum`（预计算的组加权频率，用于 O(1) 键位使用量更新）、`group_char_masks`（每组每字的位置掩码，用于 O(1) 增量编码更新：`new_code = old_code + mask * (new_key - old_key)`）等
- **Evaluator**: 维护 `code_to_chars` 碰撞桶（Vec 直接索引，大小 = code_space）、`char_bucket_pos`（桶内位置索引）、`bucket_freq_sum`/`bucket_max_freq`（桶频率统计）、`key_weighted_usage` 键位使用统计，支持增量 move/swap 操作和 Boltzmann 接受判定
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

配置中 `code_num` 表示该级别可分配的简码数量（0 表示不限制，按频率贪心分配）。每个级别可配置多个候选规则（`rules` 数组），系统选择能成功解析的第一个。简码评估器仅在权重配置启用时构建，避免全码优化场景的额外开销。

### 模拟退火算法

**初始化阶段** (`multi_start_init`):
- 生成 12-32 个候选解，轮流使用 4 种策略：均衡贪心、频率贪心、分散贪心、随机
- 每个候选经 `enhanced_hill_climb` 精炼（混合操作：40% 冲突解决、30% 交换、10% 三路交换、10% 键位重组、10% 随机移动）
- 小规模问题额外执行坐标下降

**温度校准** (`calibrate_temperature`):
- 采样随机 move/swap 的 Δ 分布，取**中位数**（对异常值鲁棒）反解初始温度：`T = -Δ_median / ln(target_accept_rate)`

**SA 主循环** (`simulated_annealing_resumable`):
- 按 `swap_probability`（随时间线性增长）选择 swap 或 move 操作
- `Evaluator::try_move/try_swap` 内部完成增量评估 + Boltzmann 接受（`exp(-delta/T)`）/回滚
- 每 10,000 步原子写入检查点（先写 `.tmp` 再 rename）
- 重热机制：连续 `min_improve_steps` 步无改进时乘以 `reheat_factor`
- 智能扰动：低温时周期性扰动碰撞相关组
- 最终精炼：对最优解执行 hill climb + 坐标下降
- 支持断点续算：`resume` 子命令可从 `checkpoint.json` 恢复

### AMHB 算法（`--amhb`）

AMHB（Adaptive Multi-candidate Heat Bath）是 SA 的替代算法，通过 EXP3 多臂老虎机自适应选择算子：
- `AmhbOperatorPool`：维护点移/交换算子的权重，根据历史收益用 EXP3 更新
- `worker_loop`：零锁多线程（裸指针 + `AtomicI32` 工作窃取），每个 worker 独立采样候选解
- 温度固定（Heat Bath 接受准则），不使用退火调度
- AMHB 快速路径：`probe_move()`/`probe_swap()` 使用掩码增量哈希，仅在 collision_count 权重非零时生效

### 键魂当量模型（`--keysoul`）

`keysoul.rs` 实现基于物理模型的击键时间计算，替代 `pair_equivalence.txt` 中的静态当量表：
- 考虑 Fitts 定律移动时间、神经延迟、肌腱联动、滚动奖励、小指惩罚等
- `KeySoulModel` 预构建键盘物理坐标，`calc_keysoul_from_indices` 为全局单例接口
- 输出单位为毫秒（ms），值越小表示击键越流畅

## 输入文件格式

| 文件 | 格式 |
|------|------|
| `input-fixed.txt` | `字根名 [tab] 键位`（单键=固定，多键=约束） |
| `input-roots.txt` | `字根名1 字根名2 ...`（同行为一组，空格分隔） |
| `input-division.txt` | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `pair_equivalence.txt` | `键对 [tab] 当量值` |
| `key_distribution.txt` | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件 (config.toml)

从 `config.toml.example` 复制为 `config.toml` 使用。文件内每个参数均有详细注释，以下仅列出关键项：

```toml
[annealing]
threads = 12              # 并行线程数
total_steps = 2000000     # 总迭代步数
temp_start = 100.0        # 初始温度
temp_end = 0.000001       # 结束温度
comfort_temp = 0.4        # 舒适温度（高斯中心）
comfort_width = 0.15      # 舒适区宽度（高斯 sigma）
comfort_slowdown = 0.7    # 舒适区减速因子 (0-1)
swap_probability = 0.35   # 交换操作概率
reheat_factor = 1.0       # 重热倍率（1.0=关闭）
max_parts = 3             # 最大码长

[weights.full_code]
collision_count = 0.19
collision_rate = 0.3
equivalence = 0.3
equiv_cv = 0.01
distribution = 0.2

[weights.simple_code]
enabled = true
full_code_weight = 0.6
simple_code_weight = 0.4
```

## 性能优化要点

- 使用 `rayon` 并行执行多线程模拟退火（每线程独立 SA 实例）
- `Evaluator` 支持增量更新（move/swap 只重算受影响的汉字），避免全量重算
- 碰撞桶使用 `Vec<Vec<usize>>` 直接索引替代 `HashMap`，配合 `char_bucket_pos` 实现 O(1) swap_remove
- `bucket_freq_sum`/`bucket_max_freq` 实现碰撞频率的增量维护，避免每次遍历桶
- `group_freq_sum` 预计算组加权频率，`key_weighted_usage` 更新从 O(chars) 降为 O(1)
- `group_char_masks` 预计算每组每字的位置掩码，AMHB 快速路径中 O(1) 增量更新编码
- `try_triple_swap` 和 `coordinate_descent` 改为增量评估+回滚，不再重建 Evaluator
- SA 主循环输出实时速度统计（万步/分钟）
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
