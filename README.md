# CodeGenie

一个基于模拟退火算法的**字根编码优化器**，用于优化汉字输入法的字根到键位映射方案。

## 功能特性

- **模拟退火优化**: 使用自适应温度调度，智能寻找最优字根键位映射
- **多目标优化**: 同时最小化重码数、当量值和键位分布偏差
- **并行计算**: 支持多线程并行退火，加速优化过程
- **增量评估**: Evaluator 支持增量更新，避免全量重算
- **简码系统**: 支持简码规则定义和自动分配

## 安装

确保已安装 Rust 工具链，然后克隆仓库并编译：

```bash
git clone https://github.com/your-repo/code_genie.git
cd code_genie
cargo build --release
```

## 使用方法

### 优化模式

运行字根编码优化：

```bash
# 使用默认配置运行
cargo run --release

# 使用指定配置文件
cargo run --release -- -c config.toml
```

### 编码模式

根据已有的字根映射表为汉字生成编码：

```bash
cargo run --release -- encode -k output-keymap.txt -d input-division.txt
```

### 评估模式

评估现有编码方案的质量：

```bash
cargo run --release -- evaluate -k output-keymap.txt
```

## 输入文件

程序需要以下输入文件：

| 文件 | 说明 | 格式 |
|------|------|------|
| `input-fixed.txt` | 固定/受限字根定义 | `字根名 [tab] 键位` |
| `input-roots.txt` | 动态字根组定义 | `字根名1 字根名2 ...` (同组字根同行) |
| `input-division.txt` | 汉字拆分表 | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `pair_equivalence.txt` | 键对当量表 | `键对 [tab] 当量值` |
| `key_distribution.txt` | 键位分布目标 | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件

通过 `config.toml` 配置优化参数：

```toml
[annealing]
threads = 16           # 并行线程数
total_steps = 10_000   # 总优化步数
temp_start = 1.0       # 初始温度
temp_end = 0.000001    # 结束温度

[weights.full_code]
collision_count = 0.07  # 重码数权重
collision_rate = 0.62   # 重码率权重
equivalence = 0.2       # 当量权重
```

## 输出文件

优化完成后在 `output-{timestamp}/` 目录生成：

- `output-keymap.txt` - 字根到键位映射
- `output-encode.txt` - 汉字编码结果
- `output-simple-codes.txt` - 简码分配
- `output-combined.txt` - 简码与全码合并输出
- `summary.txt` - 各线程优化结果汇总

## 项目结构

```
src/
├── main.rs        # CLI 入口
├── annealing.rs   # 模拟退火算法
├── calibrate.rs   # 参数自动校准
├── config.rs      # 配置解析
├── context.rs     # 优化上下文
├── evaluator.rs   # 编码评估器
├── loader.rs      # 数据加载
├── output.rs      # 结果输出
├── schedule.rs    # 温度调度
├── simple.rs      # 简码处理
├── types.rs       # 类型定义
└── validate.rs    # 数据校验
```

## 技术细节

### 字根分类

1. **固定字根**: 预定义的单键映射
2. **受限字根**: 在限定键位集中选择
3. **动态字根**: 需要优化分配的字根

### 简码规则

简码规则格式为 `AaBaCa`：
- 大写字母 `A-Z`: 根选择器（A=第0个根）
- 小写字母 `a-z`: 编码选择器（a=第0个编码）

### 性能优化

- 使用 `rayon` 实现多线程并行退火
- Evaluator 支持增量更新 `try_move()` / `try_swap()`
- 温度调度使用预计算查找表 (LUT)
- Release 模式启用 LTO 和单代码单元优化

## 许可证

MIT License