# CodeGenie

高性能汉字输入法字根编码优化器。基于模拟退火（SA）与自适应热浴（AMHB）算法，在给定拆分方案下搜索最优字根到键位映射，最小化重码数、击键当量与键位负载偏差等及更多目标。

## 安装

需要 Rust stable 工具链（推荐 1.75+）：

```bash
cargo build --release
```

产物位于 `target/release/code_genie`。以下示例均使用 `cargo run --release --`，可替换为直接调用二进制。

---

## 快速开始

```bash
# 1. 准备输入文件（见"输入文件"一节）
# 2. 复制并编辑配置
cp config.toml.example config.toml

# 3. 运行优化
cargo run --release

# 4. 查看结果
ls output-$(date +%Y%m%d)*/summary.txt
```

---

## CLI 参考

```
code_genie [-c <config>] [COMMAND]
```

不指定子命令时默认执行 `optimize`。

### 全局选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-c, --config <FILE>` | `config.toml` | 配置文件路径 |

---

### `optimize` — 字根优化（默认）

```bash
cargo run --release -- [optimize] [--amhb] [--keysoul]
```

| 选项 | 说明 |
|------|------|
| `--amhb` | 使用 AMHB 算法（EXP3 自适应算子 + Heat Bath 接受准则） |
| `--keysoul` | 使用键魂物理当量模型，替代 `pair_equivalence.txt` |

两个标志可组合使用。结果输出到 `output-{YYYYMMDDHHMMSS}/`。

---

### `encode` — 按 keymap 生成编码

```bash
cargo run --release -- encode -k <KEYMAP> [-d <DIVISION>] [-o <OUTPUT>]
```

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-k, --keymap <FILE>` | 必需 | 字根→键位映射文件 |
| `-d, --division <FILE>` | 配置中的 `splits` | 汉字拆分表 |
| `-o, --output <FILE>` | `output-encode.txt` | 输出文件 |

输出格式：`汉字\t编码\t频率`，缺失字根用 `?` 占位。

---

### `evaluate` — 评估现有方案

```bash
cargo run --release -- evaluate -k <KEYMAP> [OPTIONS]
```

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-k, --keymap <FILE>` | 必需 | 字根→键位映射文件 |
| `-d, --division <FILE>` | 配置中的 `splits` | 汉字拆分表 |
| `--keydist <FILE>` | 配置中的 `key_dist` | 键位分布目标 |
| `--equiv <FILE>` | 配置中的 `pair_equiv` | 键对当量表 |
| `--simple <FILE>` | — | 简码规则文件（独立格式） |
| `--keysoul` | — | 使用键魂当量模型 |
| `-o, --output <FILE>` | `output-evaluate.txt` | 输出文件 |

评估指标：重码数/率、当量均值与变异系数、键位分布偏差；启用简码时额外输出简码重码数/率、频率覆盖、当量、分布。

---

### `resume` — 从检查点续算

```bash
cargo run --release -- resume [-f <CHECKPOINT>]
```

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-f, --checkpoint <FILE>` | `checkpoint.json` | 检查点文件 |

优化过程中定期原子写入检查点，中断后可无损恢复。

---

## 输入文件

| 文件 | 行格式 | 说明 |
|------|--------|------|
| `input-fixed.txt` | `字根名\t键位` | 单键=固定字根；多键（空格分隔）=受限字根 |
| `input-roots.txt` | `字根名1 字根名2 ...` | 动态字根，同行为一组，共享键位 |
| `input-division.txt` | `汉字\t字根1 字根2 ...\t频率` | 汉字拆分表与语料频率 |
| `pair_equivalence.txt` | `键对\t当量值` | 连续两键的手指移动代价 |
| `key_distribution.txt` | `键位\t目标%\t低惩罚\t高惩罚` | 键位负载目标与偏离惩罚 |

---

## 配置文件

从 `config.toml.example` 复制后按需修改，文件内每个参数均有详细注释。核心配置项：

```toml
[annealing]
threads = 12          # 并行线程数，建议等于 CPU 核心数
total_steps = 2000000 # 每线程迭代步数

[weights.full_code]
collision_count = 0.19  # 重码数权重
collision_rate  = 0.3   # 重码率权重（关注高频字）
equivalence     = 0.3   # 击键当量权重
distribution    = 0.2   # 键位负载均衡权重

[weights.simple_code]
enabled = true          # 启用简码联合优化
full_code_weight   = 0.6
simple_code_weight = 0.4
```

---

## 输出文件

优化完成后在 `output-{YYYYMMDDHHMMSS}/` 生成：

| 文件 | 说明 |
|------|------|
| `output-keymap.txt` | 字根→键位映射（可直接用于 `encode`/`evaluate`） |
| `output-encode.txt` | 汉字全码编码结果 |
| `output-simple-codes.txt` | 简码分配，按级别与频率排序 |
| `output-combined.txt` | 简码与全码合并输出 |
| `output-distribution.txt` | 键位使用分布统计 |
| `output-equiv-dist.txt` | 当量分布统计 |
| `summary.txt` | 各线程结果排名汇总 |
| `thread-XX/` | 每线程独立输出子目录 |

---

## 算法概览

### 模拟退火（默认）

多线程并行 SA，每线程独立搜索：

1. **初始化**：生成 12–32 个候选解（均衡贪心 / 频率贪心 / 分散贪心 / 随机），经 hill climb 精炼后取最优
2. **主循环**：按 `swap_probability`（随进度线性增长）选择 swap 或 move，`Evaluator::try_move/try_swap` 增量评估 + Boltzmann 接受/回滚
3. **温度调度**：高斯舒适区自适应减速，预计算 LUT 插值，支持重热与低温扰动
4. **收尾**：对最优解执行 hill climb + 坐标下降精炼

### AMHB（`--amhb`）

EXP3 多臂老虎机自适应选择算子，Heat Bath 固定温度接受准则，零锁裸指针多线程 + `AtomicI32` 工作窃取。适合需要更强探索性的场景。

### 键魂当量模型（`--keysoul`）

基于 Fitts 定律 + 神经延迟 + 肌腱联动的物理击键时间模型（单位 ms），替代静态 `pair_equivalence.txt`，无需手工标注当量数据。

---

## 许可证

MIT License
