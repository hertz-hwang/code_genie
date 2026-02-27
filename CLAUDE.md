# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

CodeGenie 是一个**字根编码优化器**，使用模拟退火算法优化汉字输入法的字根到键位映射方案。目标是最小化重码数、当量值和键位分布偏差。

## 构建与运行命令

```bash
# 编译（Release 模式，推荐用于优化）
cargo build --release

# 运行优化（默认行为）
cargo run --release

# 使用指定配置文件运行
cargo run --release -- -c config.toml

# 编码模式：根据 keymap 为汉字编码
cargo run --release -- encode -k output-keymap.txt -d input-division.txt

# 评估模式：评估现有编码方案
cargo run --release -- evaluate -k output-keymap.txt

# 检查编译错误
cargo check
```

## 核心架构

### 数据流

```
输入文件 → loader.rs → OptContext → annealing.rs → Evaluator → output.rs → 输出文件
```

### 模块职责

| 模块 | 职责 |
|------|------|
| `main.rs` | CLI 入口，定义三个子命令：optimize/encode/evaluate |
| `annealing.rs` | 模拟退火算法核心，包含 `smart_init()` 智能初始化和 `simulated_annealing()` 主循环 |
| `context.rs` | `OptContext` 存储所有优化相关数据：字根组、汉字信息、当量表、简码配置 |
| `evaluator.rs` | 评估器，计算重码数、当量、分布偏差等指标，支持增量更新 |
| `loader.rs` | 加载固定字根、动态字根、拆分表、当量表、键位分布配置 |
| `types.rs` | 核心类型定义：`CharInfo`、`RootGroup`、`Metrics`、简码相关类型 |
| `config.rs` | TOML 配置解析，包含权重、退火参数、简码级别配置 |
| `schedule.rs` | 温度调度器，使用高斯舒适区实现自适应降温 |
| `calibrate.rs` | 根据初始状态自动校准缩放因子 |
| `simple.rs` | 简码规则解析 |
| `validate.rs` | 字根定义完整性校验 |

### 关键数据结构

- **OptContext**: 优化上下文，包含所有算法需要的数据
- **Evaluator**: 评估当前编码方案，支持 `try_move()` 和 `try_swap()` 增量更新
- **RootGroup**: 字根组，包含字根列表和允许分配的键位
- **CharInfo**: 汉字拆分信息，包含 parts（键位索引或组标记）和频率

### 字根分类

1. **固定字根**: 单键映射，从 `input-fixed.txt` 加载
2. **受限字根**: 多键选择，也在 `input-fixed.txt` 中定义
3. **动态字根**: 需要优化分配，从 `input-roots.txt` 加载

### 简码系统

简码规则格式为 `AaBaCa`，其中：
- 大写字母 `A-Z`: 根选择器（A=第0个根，Z=最后一个根）
- 小写字母 `a-z`: 编码选择器（a=第0个编码，z=最后一个编码）

## 输入文件格式

| 文件 | 格式 |
|------|------|
| `input-fixed.txt` | `字根名 [tab] 键位` |
| `input-roots.txt` | `字根名1 字根名2 ...` (同组字根同行) |
| `input-division.txt` | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `pair_equivalence.txt` | `键对 [tab] 当量值` |
| `key_distribution.txt` | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件 (config.toml)

关键配置项：

```toml
[annealing]
threads = 16          # 并行线程数
total_steps = 10_000  # 总优化步数
temp_start = 1.0      # 初始温度
temp_end = 0.000001   # 结束温度

[weights.full_code]
collision_count = 0.07  # 重码数权重
collision_rate = 0.62   # 重码率权重
equivalence = 0.2       # 当量权重
```

## 性能优化要点

- 使用 `rayon` 并行执行多线程模拟退火
- `Evaluator` 支持增量更新，避免全量重算
- 温度调度使用预计算查找表 (LUT)
- Release 编译启用 LTO 和单代码单元

## 输出文件

优化完成后生成 `output-{timestamp}/` 目录：
- `output-keymap.txt`: 字根到键位映射
- `output-encode.txt`: 汉字编码结果
- `output-simple-codes.txt`: 简码分配
- `output-combined.txt`: 简码+全码合并输出
- `summary.txt`: 各线程结果汇总