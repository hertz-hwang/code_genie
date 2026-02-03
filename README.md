# 三码优化器 (Three Code Optimizer)

基于模拟退火算法的高性能汉字输入法编码优化工具，支持同编码字根组的多目标优化。

## 核心功能

- **模拟退火优化**: 自动优化字根到键位的分配方案
- **多目标优化**: 同时最小化重码率、最大化键位当量、均衡用指分布
- **同编码字根组**: 支持多个字根共享同一键位
- **多线程并行**: 16线程并行退火，取最优结果
- **增量评估**: 高效的局部更新机制，支持1亿步快速迭代

## 输入文件格式

### input-fixed.txt (固定字根/受限字根组)

```
# 单键位固定：所有字根固定到同一键位
传	a

# 多键位受限：字根组可在指定键位间移动
左 右	a d h
```

格式: `字根1 字根2 ... [tab] 键位(单个或空格分隔多个)`

### input-roots.txt (动态字根组)

```
禾 禾框 余字底
口 囗
```

每行一组字根，组内字根共享同一编码，可分配到全局允许键位。

### input-division.txt (汉字拆分表)

```
明	日月	1000000
好	女子	800000
```

格式: `汉字 [tab] 字根1 字根2 ... [tab] 字频`

### pair_equivalence.txt (按键对当量)

```
aq	0.8
wr	0.9
```

格式: `按键对 [tab] 当量值`

### key_distribution.txt (键位目标分布)

```
a	3.5	1.0	1.0
s	3.5	1.0	1.0
```

格式: `键位 [tab] 目标频率% [tab] 低频惩罚 [tab] 高频惩罚`

## 输出文件

- `output-keymap.txt` - 字根键位分配表
- `output-encode.txt` - 汉字编码表
- `output-distribution.txt` - 用指分布统计
- `output-equiv-dist.txt` - 当量分布统计

## 配置参数 (src/main.rs config模块)

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `ALLOWED_KEYS` | qwertyuopasdfghjklzxcbnm | 允许的键位 |
| `NUM_THREADS` | 16 | 并行线程数 |
| `TOTAL_STEPS` | 100,000,000 | 每线程迭代步数 |
| `TEMP_START` | 1.0 | 初始温度 |
| `TEMP_END` | 0.2 | 结束温度 |
| `DECAY_RATE` | 0.9998 | 温度衰减率 |
| `SWAP_PROBABILITY` | 0.6 | 交换操作概率 |

### 目标函数权重

| 权重参数 | 默认值 | 优化目标 |
|----------|--------|----------|
| `WEIGHT_COLLISION_COUNT` | 0.5 | 重码数量 |
| `WEIGHT_COLLISION_RATE` | 1.5 | 重码率 |
| `WEIGHT_EQUIVALENCE` | 0.25 | 键位当量均值 |
| `WEIGHT_EQUIV_CV` | 0.01 | 当量变异系数 |
| `WEIGHT_DISTRIBUTION` | 1.5 | 键位分布偏差 |

## 算法特性

1. **智能初始化**: 按字频排序组，高频组优先选空闲键
2. **自适应降温**: 根据接受率动态调整温度
3. **扰动机制**: 长期无改善时进行批量扰动跳出局部最优
4. **再加热**: 陷入停滞时临时升温

## 构建与运行

```bash
# 构建
cargo build --release

# 运行
cargo run --release 2>&1
```
