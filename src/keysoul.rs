// =========================================================================
// 键魂当量模型 v2.3 (Key-Soul Equivalence Model v2.3) — Rust 移植
//
// 基于 keySoulEquiv.ts 的完整移植。
// 计算击键序列的时间成本（当量），作为输入法编码方案评估的核心指标。
//
// 输出单位: 毫秒 (ms)
// =========================================================================

/// 手指编号: 0=小指, 1=无名指, 2=中指, 3=食指, 4=拇指
type Finger = u8;

/// 单个键的物理信息
#[derive(Clone, Copy)]
struct KeyInfo {
    /// 键面字符
    ch: char,
    /// 行号: 0=数字行, 1=QWER行, 2=ASDF行(home), 3=ZXCV行, 4=空格行
    row: i8,
    /// 物理X坐标 (mm)
    x: f64,
    /// 物理Y坐标 (mm)
    y: f64,
    /// 所属手: false=左手, true=右手
    right_hand: bool,
    /// 手指编号
    finger: Finger,
}

/// 手指动态状态
#[derive(Clone)]
struct FingerState {
    eff_x: f64,
    eff_y: f64,
    last_key: Option<KeyInfo>,
    release_time: f64,
    repeat_count: u32,
}

/// 键对分类
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PairCategory {
    SameKey,
    SameFinger,
    SameHand,
    DiffHand,
}

/// 单个键对的时间分解（用于 --debug 输出）
pub struct PairDebugInfo {
    pub prev_ch: char,
    pub curr_ch: char,
    pub category: &'static str,
    pub finger_path: String,
    pub t_neural: f64,
    pub t_move_raw: f64,
    pub move_discount: Option<f64>,
    pub t_move: f64,
    pub t_couple: f64,
    pub t_row: f64,
    pub t_sf_jump: f64,
    pub t_pinky: f64,
    pub t_stretch: f64,
    pub t_roll: f64,
    pub t_repeat: f64,
    pub repeat_count: u32,
    pub tendon_delta: f64,
    pub total: f64,
    pub note: Option<String>,
}

// ═══════════════════════════════════════════════════════════════
// 常量
// ═══════════════════════════════════════════════════════════════

const KEY_PITCH: f64 = 19.05;

/// 每行的水平偏移(stagger): 行0..3
const ROW_OFFSETS: [f64; 4] = [
    0.0,
    0.25 * KEY_PITCH,
    0.50 * KEY_PITCH,
    0.75 * KEY_PITCH,
];

/// 神经延迟 (ms): same_key, same_finger, same_hand, diff_hand
const NEURAL_SAME_KEY: f64 = 120.0;
const NEURAL_SAME_FINGER: f64 = 150.0;
const NEURAL_SAME_HAND: f64 = 90.0;
const NEURAL_DIFF_HAND: f64 = 55.0;

const FITTS_B: f64 = 55.0;
const KEY_WIDTH: f64 = 14.0;

/// 手指灵活度系数: [小指, 无名指, 中指, 食指, 拇指]
const FINGER_SPEED: [f64; 5] = [1.5, 1.3, 1.0, 1.05, 1.15];

/// 行跳跃惩罚: [0行差, 1行差, 2行差, 3行差]
const ROW_JUMP_BASE: [f64; 4] = [0.0, 8.0, 20.0, 35.0];

const SAME_FINGER_BIG_JUMP: f64 = 80.0;
const SAME_FINGER_SMALL_JUMP: f64 = 40.0;

const PINKY_BASE: f64 = 8.0;
const PINKY_PER_ROW: f64 = 20.0;
const STRETCH_PER_ROW: f64 = 20.0;

const ROLL_INWARD: f64 = -25.0;
const ROLL_OUTWARD: f64 = -15.0;
const ROLL_ROW_DECAY: f64 = 0.50;
const ROLL_MOVE_DISCOUNT: f64 = 0.40;

const FIRST_KEY_COST_RATIO: f64 = 0.60;

const RELEASE_DELAY: f64 = 40.0;
const PARALLEL_EFFICIENCY: f64 = 0.75;
const MINIMUM_INTERVAL: f64 = 45.0;

const REPEAT_BASE_PENALTY: f64 = 50.0;
const REPEAT_ESCALATION_FACTOR: f64 = 1.55;
const REPEAT_MAX_PENALTY: f64 = 250.0;

/// 手指连击困难度: [小指, 无名指, 中指, 食指, 拇指]
const FINGER_REPEAT_DIFFICULTY: [f64; 5] = [1.60, 1.35, 1.00, 1.10, 1.20];

// ═══════════════════════════════════════════════════════════════
// 耦合惩罚表 & 肌腱联动表
// ═══════════════════════════════════════════════════════════════

/// 同手异指耦合惩罚 (ms): coupling[f1][f2]
/// 仅 finger 0..3 之间有效（拇指无耦合）
fn coupling_penalty(f1: Finger, f2: Finger) -> f64 {
    match (f1, f2) {
        (0, 1) | (1, 0) => 15.0,
        (1, 2) | (2, 1) => 10.0,
        (2, 3) | (3, 2) => 5.0,
        (0, 2) | (2, 0) => 8.0,
        (0, 3) | (3, 0) => 3.0,
        (1, 3) | (3, 1) => 3.0,
        _ => 0.0,
    }
}

/// 肌腱联动 Y轴位移系数: tendon_coupling_y[acting][other]
fn tendon_coupling_y(acting: Finger, other: Finger) -> f64 {
    match (acting, other) {
        (0, 1) => 0.35,
        (1, 0) => 0.30,
        (1, 2) => 0.25,
        (2, 1) => 0.20,
        (2, 3) => 0.10,
        (3, 2) => 0.10,
        (0, 2) => 0.10,
        (2, 0) => 0.08,
        (0, 3) => 0.05,
        (3, 0) => 0.05,
        (1, 3) => 0.08,
        (3, 1) => 0.08,
        _ => 0.0,
    }
}

// ═══════════════════════════════════════════════════════════════
// 键盘模型 — 构建时预计算所有键的物理信息
// ═══════════════════════════════════════════════════════════════

/// 所有支持的键（按行排列）
const ROW_KEYS: [&[u8]; 4] = [
    b"1234567890-=",
    b"qwertyuiop[]\\",
    b"asdfghjkl;'",
    b"zxcvbnm,./",
];

/// 手指分配: (hand_right, finger)
fn key_assignment(ch: char) -> Option<(bool, Finger)> {
    Some(match ch {
        '1' => (false, 0),
        '2' => (false, 1),
        '3' => (false, 2),
        '4' | '5' => (false, 3),
        '6' | '7' => (true, 3),
        '8' => (true, 2),
        '9' => (true, 1),
        '0' | '-' | '=' => (true, 0),
        'q' => (false, 0),
        'w' => (false, 1),
        'e' => (false, 2),
        'r' | 't' => (false, 3),
        'y' | 'u' => (true, 3),
        'i' => (true, 2),
        'o' => (true, 1),
        'p' | '[' | ']' | '\\' => (true, 0),
        'a' => (false, 0),
        's' => (false, 1),
        'd' => (false, 2),
        'f' | 'g' => (false, 3),
        'h' | 'j' => (true, 3),
        'k' => (true, 2),
        'l' => (true, 1),
        ';' | '\'' => (true, 0),
        'z' => (false, 0),
        'x' => (false, 1),
        'c' => (false, 2),
        'v' | 'b' => (false, 3),
        'n' | 'm' => (true, 3),
        ',' => (true, 2),
        '.' => (true, 1),
        '/' => (true, 0),
        ' ' => (true, 4),
        _ => return None,
    })
}

/// Home 键: (hand_right, finger) -> char
fn home_key(right_hand: bool, finger: Finger) -> Option<char> {
    Some(match (right_hand, finger) {
        (false, 0) => 'a',
        (false, 1) => 's',
        (false, 2) => 'd',
        (false, 3) => 'f',
        (true, 0) => ';',
        (true, 1) => 'l',
        (true, 2) => 'k',
        (true, 3) => 'j',
        (true, 4) => ' ',
        _ => return None,
    })
}

/// 键魂当量计算器（预构建键盘模型）
pub struct KeySoulModel {
    /// 键信息查找表: 最多 128 个 ASCII 字符
    keys: [Option<KeyInfo>; 128],
}

impl KeySoulModel {
    pub fn new() -> Self {
        let mut keys = [None; 128];

        // 构建行键
        for (row, row_chars) in ROW_KEYS.iter().enumerate() {
            for (col, &byte) in row_chars.iter().enumerate() {
                let ch = byte as char;
                if let Some((right_hand, finger)) = key_assignment(ch) {
                    let x = col as f64 * KEY_PITCH + ROW_OFFSETS[row];
                    let y = row as f64 * KEY_PITCH;
                    keys[byte as usize] = Some(KeyInfo {
                        ch,
                        row: row as i8,
                        x,
                        y,
                        right_hand,
                        finger,
                    });
                }
            }
        }

        // 空格键特殊处理
        keys[b' ' as usize] = Some(KeyInfo {
            ch: ' ',
            row: 4,
            x: 5.25 * KEY_PITCH,
            y: 4.0 * KEY_PITCH,
            right_hand: true,
            finger: 4,
        });

        Self { keys }
    }

    /// 获取键信息，'_' 映射为空格，字母自动转小写
    #[inline]
    fn get_key(&self, ch: char) -> Option<KeyInfo> {
        let c = if ch == '_' { ' ' } else { ch.to_ascii_lowercase() };
        if (c as usize) < 128 {
            self.keys[c as usize]
        } else {
            None
        }
    }

    /// 从键位索引获取 KeyInfo（索引与 types::char_to_key_index 对齐）
    #[inline]
    fn get_key_by_index(&self, idx: u8) -> Option<KeyInfo> {
        let ch = match idx {
            0..=25 => (idx + b'a') as char,
            26 => ' ',   // _ -> space
            27 => ';',
            28 => ',',
            29 => '.',
            30 => '/',
            31 => '1',
            32 => '2',
            33 => '3',
            34 => '4',
            35 => '5',
            36 => '6',
            37 => '7',
            38 => '8',
            39 => '9',
            40 => '0',
            41 => '-',
            42 => '=',
            43 => '[',
            44 => ']',
            45 => '\\',
            46 => '\'',
            _ => return None,
        };
        self.get_key(ch)
    }

    /// 两键之间的欧几里得距离
    #[inline]
    fn dist(a: &KeyInfo, b: &KeyInfo) -> f64 {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// 获取手指的 home 键 KeyInfo
    #[inline]
    fn home_of(&self, right_hand: bool, finger: Finger) -> Option<KeyInfo> {
        home_key(right_hand, finger).and_then(|ch| self.get_key(ch))
    }

    // ─────────────────────────────────────────
    // 基础计算
    // ─────────────────────────────────────────

    /// Fitts 定律 (Shannon)
    #[inline]
    fn fitts(dist_mm: f64, finger: Finger) -> f64 {
        if dist_mm < 0.5 {
            return 0.0;
        }
        let id_bits = (dist_mm / KEY_WIDTH + 1.0).log2();
        FITTS_B * id_bits * FINGER_SPEED[finger as usize]
    }

    /// 分类键对关系
    #[inline]
    fn classify(a: &KeyInfo, b: &KeyInfo) -> PairCategory {
        if a.ch == b.ch {
            PairCategory::SameKey
        } else if a.right_hand != b.right_hand {
            PairCategory::DiffHand
        } else if a.finger == b.finger {
            PairCategory::SameFinger
        } else {
            PairCategory::SameHand
        }
    }

    /// 检测滚动: (is_roll, is_inward, row_diff, decay)
    #[inline]
    fn detect_roll(a: &KeyInfo, b: &KeyInfo) -> (bool, bool, i8, f64) {
        if a.right_hand != b.right_hand {
            return (false, false, 0, 0.0);
        }
        if a.finger == b.finger {
            return (false, false, 0, 0.0);
        }
        let fdiff = (a.finger as i8 - b.finger as i8).unsigned_abs();
        if fdiff != 1 {
            return (false, false, 0, 0.0);
        }
        let is_inward = b.finger > a.finger;
        let row_diff = (a.row - b.row).unsigned_abs() as i8;
        let decay = ROLL_ROW_DECAY.powi(row_diff as i32);
        (true, is_inward, row_diff, decay)
    }

    /// 滚动奖励
    #[inline]
    fn roll_bonus(a: &KeyInfo, b: &KeyInfo) -> f64 {
        let (is_roll, is_inward, _, decay) = Self::detect_roll(a, b);
        if !is_roll {
            return 0.0;
        }
        let base = if is_inward { ROLL_INWARD } else { ROLL_OUTWARD };
        base * decay
    }

    /// 滚动移动折扣
    #[inline]
    fn roll_move_discount(a: &KeyInfo, b: &KeyInfo) -> f64 {
        let (is_roll, _, _, decay) = Self::detect_roll(a, b);
        if !is_roll {
            return 1.0;
        }
        ROLL_MOVE_DISCOUNT + (1.0 - ROLL_MOVE_DISCOUNT) * (1.0 - decay)
    }

    /// 同指跨排惩罚
    #[inline]
    fn same_finger_row_penalty(a: &KeyInfo, b: &KeyInfo) -> f64 {
        let rd = (a.row - b.row).unsigned_abs();
        if rd >= 2 {
            SAME_FINGER_BIG_JUMP
        } else if rd == 1 {
            SAME_FINGER_SMALL_JUMP
        } else {
            0.0
        }
    }

    /// 小指干扰惩罚
    #[inline]
    fn pinky_interference(a: &KeyInfo, b: &KeyInfo) -> f64 {
        let pinky = if a.finger == 0 {
            Some(a)
        } else if b.finger == 0 {
            Some(b)
        } else {
            None
        };
        match pinky {
            Some(k) => PINKY_BASE + PINKY_PER_ROW * (k.row - 2).unsigned_abs() as f64,
            None => 0.0,
        }
    }

    /// 手部伸展惩罚
    #[inline]
    fn stretch_penalty(a: &KeyInfo, b: &KeyInfo) -> f64 {
        STRETCH_PER_ROW * (a.row - b.row).unsigned_abs() as f64
    }

    /// 连击递增惩罚
    #[inline]
    fn repeat_escalation_penalty(finger: Finger, repeat_count: u32) -> f64 {
        if repeat_count < 2 {
            return 0.0;
        }
        let difficulty = FINGER_REPEAT_DIFFICULTY[finger as usize];
        let exponent = repeat_count - 2;
        let escalation = REPEAT_ESCALATION_FACTOR.powi(exponent as i32);
        let penalty = REPEAT_BASE_PENALTY * difficulty * escalation;
        penalty.min(REPEAT_MAX_PENALTY)
    }

    /// 首键定位成本
    #[inline]
    fn first_key_cost(&self, key: &KeyInfo) -> f64 {
        if let Some(home) = self.home_of(key.right_hand, key.finger) {
            let d = Self::dist(&home, key);
            if d < 0.5 {
                0.0
            } else {
                Self::fitts(d, key.finger) * FIRST_KEY_COST_RATIO
            }
        } else {
            0.0
        }
    }

    // ─────────────────────────────────────────
    // 手指状态
    // ─────────────────────────────────────────

    /// 10 个手指: L0..L3, R0..R4
    fn init_all_finger_states(&self) -> [FingerState; 10] {
        let mut states: [FingerState; 10] = std::array::from_fn(|_| FingerState {
            eff_x: 0.0,
            eff_y: 0.0,
            last_key: None,
            release_time: 0.0,
            repeat_count: 0,
        });

        // L: 0..3 -> indices 0..3
        for f in 0u8..=3 {
            if let Some(home) = self.home_of(false, f) {
                states[f as usize] = FingerState {
                    eff_x: home.x,
                    eff_y: home.y,
                    last_key: Some(home),
                    release_time: 0.0,
                    repeat_count: 0,
                };
            }
        }
        // R: 0..4 -> indices 5..9
        for f in 0u8..=4 {
            if let Some(home) = self.home_of(true, f) {
                states[5 + f as usize] = FingerState {
                    eff_x: home.x,
                    eff_y: home.y,
                    last_key: Some(home),
                    release_time: 0.0,
                    repeat_count: 0,
                };
            }
        }

        states
    }

    /// 从 (right_hand, finger) 映射到状态数组索引
    #[inline]
    fn finger_idx(right_hand: bool, finger: Finger) -> usize {
        if right_hand { 5 + finger as usize } else { finger as usize }
    }

    /// 初始化单手手指状态 (用于单手子序列时间计算)
    fn init_hand_finger_states(&self, right_hand: bool) -> [FingerState; 5] {
        let mut states: [FingerState; 5] = std::array::from_fn(|_| FingerState {
            eff_x: 0.0,
            eff_y: 0.0,
            last_key: None,
            release_time: 0.0,
            repeat_count: 0,
        });

        let max_f: u8 = if right_hand { 4 } else { 3 };
        for f in 0..=max_f {
            if let Some(home) = self.home_of(right_hand, f) {
                states[f as usize] = FingerState {
                    eff_x: home.x,
                    eff_y: home.y,
                    last_key: Some(home),
                    release_time: 0.0,
                    repeat_count: 0,
                };
            }
        }
        states
    }

    /// 有效距离
    #[inline]
    fn effective_dist(state: &FingerState, target: &KeyInfo) -> f64 {
        let dx = target.x - state.eff_x;
        let dy = target.y - state.eff_y;
        (dx * dx + dy * dy).sqrt()
    }

    /// 应用肌腱联动 (全手指模式)
    fn apply_tendon_coupling(
        acting_finger: Finger,
        target: &KeyInfo,
        right_hand: bool,
        states: &mut [FingerState; 10],
    ) {
        let max_f: u8 = if right_hand { 4 } else { 3 };
        for other in 0..=max_f {
            if other == acting_finger {
                continue;
            }
            let c = tendon_coupling_y(acting_finger, other);
            if c == 0.0 {
                continue;
            }
            let idx = Self::finger_idx(right_hand, other);
            states[idx].eff_y += (target.y - states[idx].eff_y) * c;
        }
    }

    /// 应用肌腱联动 (单手模式)
    fn apply_tendon_coupling_hand(
        acting_finger: Finger,
        target: &KeyInfo,
        states: &mut [FingerState; 5],
    ) {
        for other in 0u8..5 {
            if other == acting_finger {
                continue;
            }
            let c = tendon_coupling_y(acting_finger, other);
            if c == 0.0 {
                continue;
            }
            states[other as usize].eff_y += (target.y - states[other as usize].eff_y) * c;
        }
    }

    // ─────────────────────────────────────────
    // 核心间隔计算
    // ─────────────────────────────────────────

    /// 计算两个连续击键之间的间隔时间
    fn compute_interval(
        &self,
        prev: &KeyInfo,
        curr: &KeyInfo,
        cat: PairCategory,
        states: &[FingerState; 10],
        cur_time: f64,
    ) -> f64 {
        let fk = Self::finger_idx(curr.right_hand, curr.finger);
        let t_neural = match cat {
            PairCategory::SameKey => NEURAL_SAME_KEY,
            PairCategory::SameFinger => NEURAL_SAME_FINGER,
            PairCategory::SameHand => NEURAL_SAME_HAND,
            PairCategory::DiffHand => NEURAL_DIFF_HAND,
        };

        let move_discount = if cat == PairCategory::SameHand {
            Self::roll_move_discount(prev, curr)
        } else {
            1.0
        };

        let t_move = match cat {
            PairCategory::SameKey => 0.0,
            PairCategory::DiffHand => {
                let state = &states[fk];
                let raw_move = Self::fitts(Self::effective_dist(state, curr), curr.finger);
                if raw_move > 0.0 {
                    let earliest = if state.release_time > 0.0 {
                        state.release_time
                    } else {
                        (cur_time - 200.0).max(0.0)
                    };
                    let available = (cur_time - earliest).max(0.0);
                    let effective_prep = available * PARALLEL_EFFICIENCY;
                    (raw_move - effective_prep).max(0.0)
                } else {
                    0.0
                }
            }
            PairCategory::SameFinger => {
                let state = &states[fk];
                Self::fitts(Self::effective_dist(state, curr), curr.finger)
            }
            PairCategory::SameHand => {
                let state = &states[fk];
                Self::fitts(Self::effective_dist(state, curr), curr.finger) * move_discount
            }
        };

        let t_couple = if cat == PairCategory::SameHand {
            coupling_penalty(prev.finger, curr.finger)
        } else {
            0.0
        };

        let row_diff = (prev.row - curr.row).unsigned_abs() as usize;
        let t_row = if cat == PairCategory::SameHand || cat == PairCategory::SameFinger {
            if row_diff < ROW_JUMP_BASE.len() {
                ROW_JUMP_BASE[row_diff]
            } else {
                40.0
            }
        } else {
            0.0
        };

        let t_sf_jump = if cat == PairCategory::SameFinger {
            Self::same_finger_row_penalty(prev, curr)
        } else {
            0.0
        };

        let t_pinky = if cat == PairCategory::SameHand {
            Self::pinky_interference(prev, curr)
        } else {
            0.0
        };

        let t_stretch = if cat == PairCategory::SameHand {
            Self::stretch_penalty(prev, curr)
        } else {
            0.0
        };

        let t_roll = if cat == PairCategory::SameHand {
            Self::roll_bonus(prev, curr)
        } else {
            0.0
        };

        let t_repeat = if cat == PairCategory::SameKey {
            let new_repeat = states[fk].repeat_count + 1;
            Self::repeat_escalation_penalty(curr.finger, new_repeat)
        } else {
            0.0
        };

        (t_neural + t_move + t_couple + t_row + t_sf_jump + t_pinky + t_stretch + t_roll + t_repeat)
            .max(MINIMUM_INTERVAL)
    }

    // ─────────────────────────────────────────
    // 单手子序列时间
    // ─────────────────────────────────────────

    fn compute_single_hand_time(&self, hand_keys: &[KeyInfo]) -> f64 {
        if hand_keys.is_empty() {
            return 0.0;
        }
        if hand_keys.len() == 1 {
            return self.first_key_cost(&hand_keys[0]);
        }

        let right_hand = hand_keys[0].right_hand;
        let mut states = self.init_hand_finger_states(right_hand);

        let mut total = self.first_key_cost(&hand_keys[0]);

        let first = &hand_keys[0];
        let s0 = &mut states[first.finger as usize];
        s0.eff_x = first.x;
        s0.eff_y = first.y;
        s0.last_key = Some(*first);
        s0.repeat_count = 1;
        Self::apply_tendon_coupling_hand(first.finger, first, &mut states);

        for i in 1..hand_keys.len() {
            let prev = &hand_keys[i - 1];
            let curr = &hand_keys[i];

            let cat = if prev.ch == curr.ch {
                PairCategory::SameKey
            } else if prev.finger == curr.finger {
                PairCategory::SameFinger
            } else {
                PairCategory::SameHand
            };

            let t_neural = match cat {
                PairCategory::SameKey => NEURAL_SAME_KEY,
                PairCategory::SameFinger => NEURAL_SAME_FINGER,
                PairCategory::SameHand => NEURAL_SAME_HAND,
                PairCategory::DiffHand => unreachable!(),
            };

            let move_discount = if cat == PairCategory::SameHand {
                Self::roll_move_discount(prev, curr)
            } else {
                1.0
            };

            let t_move = match cat {
                PairCategory::SameKey => 0.0,
                PairCategory::SameFinger => {
                    let state = &states[curr.finger as usize];
                    Self::fitts(Self::effective_dist(state, curr), curr.finger)
                }
                PairCategory::SameHand => {
                    let state = &states[curr.finger as usize];
                    Self::fitts(Self::effective_dist(state, curr), curr.finger) * move_discount
                }
                _ => 0.0,
            };

            let t_couple = if cat == PairCategory::SameHand {
                coupling_penalty(prev.finger, curr.finger)
            } else {
                0.0
            };

            let row_diff = (prev.row - curr.row).unsigned_abs() as usize;
            let t_row = if cat == PairCategory::SameHand || cat == PairCategory::SameFinger {
                if row_diff < ROW_JUMP_BASE.len() { ROW_JUMP_BASE[row_diff] } else { 40.0 }
            } else {
                0.0
            };

            let t_sf_jump = if cat == PairCategory::SameFinger {
                Self::same_finger_row_penalty(prev, curr)
            } else {
                0.0
            };

            let t_pinky = if cat == PairCategory::SameHand {
                Self::pinky_interference(prev, curr)
            } else {
                0.0
            };

            let t_stretch = if cat == PairCategory::SameHand {
                Self::stretch_penalty(prev, curr)
            } else {
                0.0
            };

            let t_roll = if cat == PairCategory::SameHand {
                Self::roll_bonus(prev, curr)
            } else {
                0.0
            };

            let t_repeat = if cat == PairCategory::SameKey {
                let new_repeat = states[curr.finger as usize].repeat_count + 1;
                Self::repeat_escalation_penalty(curr.finger, new_repeat)
            } else {
                0.0
            };

            let interval = (t_neural + t_move + t_couple + t_row + t_sf_jump
                + t_pinky + t_stretch + t_roll + t_repeat)
                .max(MINIMUM_INTERVAL);
            total += interval;

            // 更新手指状态
            let cf = curr.finger as usize;
            if cat == PairCategory::SameKey {
                states[cf].repeat_count += 1;
                states[cf].eff_x = curr.x;
                states[cf].eff_y = curr.y;
                states[cf].last_key = Some(*curr);
            } else {
                states[cf].eff_x = curr.x;
                states[cf].eff_y = curr.y;
                states[cf].last_key = Some(*curr);
                states[cf].repeat_count = 1;
            }
            Self::apply_tendon_coupling_hand(curr.finger, curr, &mut states);
        }
        total
    }

    // ─────────────────────────────────────────
    // 公开接口
    // ─────────────────────────────────────────

    /// 计算击键序列的总时间 (当量, ms)
    ///
    /// 返回 -1.0 表示包含未知键
    /// 返回 0.0 表示 <2 键
    #[allow(dead_code)]
    pub fn sequence_time(&self, keys: &str) -> f64 {
        if keys.len() < 2 {
            return 0.0;
        }

        let mut infos = Vec::with_capacity(keys.len());
        for ch in keys.chars() {
            match self.get_key(ch) {
                Some(k) => infos.push(k),
                None => return -1.0,
            }
        }

        self.sequence_time_from_infos(&infos)
    }

    /// 从键位索引序列计算当量 (用于优化器集成)
    ///
    /// key_indices: 每个元素是 0..30 的键位索引
    /// 返回总时间 (ms)，若索引无效返回 -1.0
    pub fn sequence_time_from_indices(&self, key_indices: &[u8]) -> f64 {
        if key_indices.len() < 2 {
            return 0.0;
        }

        let mut infos = Vec::with_capacity(key_indices.len());
        for &idx in key_indices {
            match self.get_key_by_index(idx) {
                Some(k) => infos.push(k),
                None => return -1.0,
            }
        }

        self.sequence_time_from_infos(&infos)
    }

    /// 内部：从 KeyInfo 序列计算总时间
    fn sequence_time_from_infos(&self, infos: &[KeyInfo]) -> f64 {
        if infos.len() < 2 {
            return 0.0;
        }

        let first_key_cost = self.first_key_cost(&infos[0]);

        let mut states = self.init_all_finger_states();
        let first = &infos[0];
        let fk = Self::finger_idx(first.right_hand, first.finger);

        states[fk].eff_x = first.x;
        states[fk].eff_y = first.y;
        states[fk].last_key = Some(*first);
        states[fk].release_time = first_key_cost + RELEASE_DELAY;
        states[fk].repeat_count = 1;
        Self::apply_tendon_coupling(first.finger, first, first.right_hand, &mut states);

        let mut cur_time = first_key_cost;
        let mut stepwise_total = first_key_cost;

        for i in 1..infos.len() {
            let prev = &infos[i - 1];
            let curr = &infos[i];
            let cat = Self::classify(prev, curr);
            let fk = Self::finger_idx(curr.right_hand, curr.finger);

            let interval = self.compute_interval(prev, curr, cat, &states, cur_time);
            cur_time += interval;
            stepwise_total += interval;

            // 更新手指状态
            if cat == PairCategory::SameKey {
                states[fk].repeat_count += 1;
                states[fk].release_time = cur_time + RELEASE_DELAY;
            } else {
                states[fk].eff_x = curr.x;
                states[fk].eff_y = curr.y;
                states[fk].last_key = Some(*curr);
                states[fk].release_time = cur_time + RELEASE_DELAY;
                states[fk].repeat_count = 1;
            }
            Self::apply_tendon_coupling(curr.finger, curr, curr.right_hand, &mut states);
        }

        // 左右手子序列时间
        let left_keys: Vec<KeyInfo> = infos.iter().filter(|k| !k.right_hand).copied().collect();
        let right_keys: Vec<KeyInfo> = infos.iter().filter(|k| k.right_hand).copied().collect();

        let left_time = self.compute_single_hand_time(&left_keys);
        let right_time = self.compute_single_hand_time(&right_keys);

        let total = stepwise_total.max(left_time).max(right_time);
        (total * 100.0).round() / 100.0
    }

    /// 计算击键序列时间并返回每个键对的详细调试信息
    /// 返回 None 表示包含未知键
    /// 返回 Some((total, left_time, right_time, pairs))
    pub fn sequence_time_debug(&self, keys: &str) -> Option<(f64, f64, f64, Vec<PairDebugInfo>)> {
        if keys.len() < 2 {
            return Some((0.0, 0.0, 0.0, vec![]));
        }
        let mut infos = Vec::with_capacity(keys.len());
        for ch in keys.chars() {
            match self.get_key(ch) {
                Some(k) => infos.push(k),
                None => return None,
            }
        }

        let first_key_cost = self.first_key_cost(&infos[0]);
        let mut states = self.init_all_finger_states();
        let first = &infos[0];
        let fk0 = Self::finger_idx(first.right_hand, first.finger);
        states[fk0].eff_x = first.x;
        states[fk0].eff_y = first.y;
        states[fk0].last_key = Some(*first);
        states[fk0].release_time = first_key_cost + RELEASE_DELAY;
        states[fk0].repeat_count = 1;
        Self::apply_tendon_coupling(first.finger, first, first.right_hand, &mut states);

        let mut cur_time = first_key_cost;
        let mut stepwise_total = first_key_cost;
        let mut pairs = Vec::new();

        for i in 1..infos.len() {
            let prev = &infos[i - 1];
            let curr = &infos[i];
            let cat = Self::classify(prev, curr);
            let fk = Self::finger_idx(curr.right_hand, curr.finger);

            let category_str = match cat {
                PairCategory::SameKey => "同键连击",
                PairCategory::SameFinger => "同指跨键",
                PairCategory::SameHand => "同手移动",
                PairCategory::DiffHand => "异手交替",
            };
            let finger_path = format!("{}→{}",
                finger_name_zh(prev.right_hand, prev.finger),
                finger_name_zh(curr.right_hand, curr.finger));

            let t_neural = match cat {
                PairCategory::SameKey => NEURAL_SAME_KEY,
                PairCategory::SameFinger => NEURAL_SAME_FINGER,
                PairCategory::SameHand => NEURAL_SAME_HAND,
                PairCategory::DiffHand => NEURAL_DIFF_HAND,
            };

            let move_discount = if cat == PairCategory::SameHand {
                Some(Self::roll_move_discount(prev, curr))
            } else {
                None
            };

            let (t_move_raw, t_move, note) = match cat {
                PairCategory::SameKey => (0.0, 0.0, None),
                PairCategory::DiffHand => {
                    let state = &states[fk];
                    let raw = Self::fitts(Self::effective_dist(state, curr), curr.finger);
                    if raw > 0.0 {
                        let earliest = if state.release_time > 0.0 {
                            state.release_time
                        } else {
                            (cur_time - 200.0).max(0.0)
                        };
                        let available = (cur_time - earliest).max(0.0);
                        let prep = available * PARALLEL_EFFICIENCY;
                        let eff = (raw - prep).max(0.0);
                        let note = if eff == 0.0 {
                            format!("异手移动: {} 提前到达[{}] (准备 {:.1}ms ≥ 移动 {:.1}ms)",
                                finger_name_zh(curr.right_hand, curr.finger), curr.ch, prep, raw)
                        } else {
                            format!("异手移动: {} 需移动 {:.1}ms (原始 {:.1}ms, 准备 {:.1}ms)",
                                finger_name_zh(curr.right_hand, curr.finger), eff, raw, prep)
                        };
                        (raw, eff, Some(note))
                    } else {
                        let note = format!("异手移动: {} 已在目标[{}]上",
                            finger_name_zh(curr.right_hand, curr.finger), curr.ch);
                        (0.0, 0.0, Some(note))
                    }
                }
                PairCategory::SameFinger => {
                    let state = &states[fk];
                    let raw = Self::fitts(Self::effective_dist(state, curr), curr.finger);
                    (raw, raw, None)
                }
                PairCategory::SameHand => {
                    let state = &states[fk];
                    let raw = Self::fitts(Self::effective_dist(state, curr), curr.finger);
                    let disc = move_discount.unwrap_or(1.0);
                    (raw, raw * disc, None)
                }
            };

            let t_couple = if cat == PairCategory::SameHand {
                coupling_penalty(prev.finger, curr.finger)
            } else {
                0.0
            };
            let row_diff = (prev.row - curr.row).unsigned_abs() as usize;
            let t_row = if cat == PairCategory::SameHand || cat == PairCategory::SameFinger {
                if row_diff < ROW_JUMP_BASE.len() { ROW_JUMP_BASE[row_diff] } else { 40.0 }
            } else {
                0.0
            };
            let t_sf_jump = if cat == PairCategory::SameFinger {
                Self::same_finger_row_penalty(prev, curr)
            } else {
                0.0
            };
            let t_pinky = if cat == PairCategory::SameHand {
                Self::pinky_interference(prev, curr)
            } else {
                0.0
            };
            let t_stretch = if cat == PairCategory::SameHand {
                Self::stretch_penalty(prev, curr)
            } else {
                0.0
            };
            let t_roll = if cat == PairCategory::SameHand {
                Self::roll_bonus(prev, curr)
            } else {
                0.0
            };
            let repeat_count = states[fk].repeat_count;
            let t_repeat = if cat == PairCategory::SameKey {
                Self::repeat_escalation_penalty(curr.finger, repeat_count + 1)
            } else {
                0.0
            };

            let interval = (t_neural + t_move + t_couple + t_row + t_sf_jump
                + t_pinky + t_stretch + t_roll + t_repeat)
                .max(MINIMUM_INTERVAL);

            // 肌腱联动位移量（信息性，不影响时间）
            let tendon_delta = {
                let max_f: u8 = if curr.right_hand { 4 } else { 3 };
                let mut delta = 0.0f64;
                for other in 0..=max_f {
                    if other == curr.finger { continue; }
                    let c = tendon_coupling_y(curr.finger, other);
                    if c == 0.0 { continue; }
                    let idx = Self::finger_idx(curr.right_hand, other);
                    delta += ((curr.y - states[idx].eff_y) * c).abs();
                }
                delta
            };

            cur_time += interval;
            stepwise_total += interval;

            if cat == PairCategory::SameKey {
                states[fk].repeat_count += 1;
                states[fk].release_time = cur_time + RELEASE_DELAY;
            } else {
                states[fk].eff_x = curr.x;
                states[fk].eff_y = curr.y;
                states[fk].last_key = Some(*curr);
                states[fk].release_time = cur_time + RELEASE_DELAY;
                states[fk].repeat_count = 1;
            }
            Self::apply_tendon_coupling(curr.finger, curr, curr.right_hand, &mut states);

            pairs.push(PairDebugInfo {
                prev_ch: prev.ch,
                curr_ch: curr.ch,
                category: category_str,
                finger_path,
                t_neural,
                t_move_raw,
                move_discount,
                t_move,
                t_couple,
                t_row,
                t_sf_jump,
                t_pinky,
                t_stretch,
                t_roll,
                t_repeat,
                repeat_count,
                tendon_delta,
                total: interval,
                note,
            });
        }

        let left_keys: Vec<KeyInfo> = infos.iter().filter(|k| !k.right_hand).copied().collect();
        let right_keys: Vec<KeyInfo> = infos.iter().filter(|k| k.right_hand).copied().collect();
        let left_time = self.compute_single_hand_time(&left_keys);
        let right_time = self.compute_single_hand_time(&right_keys);
        let total = stepwise_total.max(left_time).max(right_time);
        Some(((total * 100.0).round() / 100.0, left_time, right_time, pairs))
    }
}

fn finger_name_zh(right_hand: bool, finger: Finger) -> String {
    let hand = if right_hand { "右" } else { "左" };
    let f = ["小指", "无名指", "中指", "食指", "拇指"][finger.min(4) as usize];
    format!("{}{}", hand, f)
}

// ═══════════════════════════════════════════════════════════════
// 全局懒初始化单例
// ═══════════════════════════════════════════════════════════════

use std::sync::LazyLock;

static GLOBAL_MODEL: LazyLock<KeySoulModel> = LazyLock::new(KeySoulModel::new);

/// 计算编码序列的键魂当量 (全局单例)
#[allow(dead_code)]
pub fn calc_keysoul_equivalence(code: &str) -> f64 {
    GLOBAL_MODEL.sequence_time(code)
}

/// 从键位索引序列计算键魂当量 (全局单例)
pub fn calc_keysoul_from_indices(key_indices: &[u8]) -> f64 {
    GLOBAL_MODEL.sequence_time_from_indices(key_indices)
}

/// 获取全局 KeySoulModel 引用
#[allow(dead_code)]
pub fn global_model() -> &'static KeySoulModel {
    &GLOBAL_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_sequence() {
        let model = KeySoulModel::new();
        // 单键应返回 0
        assert_eq!(model.sequence_time("f"), 0.0);
        // 两键同手
        let t = model.sequence_time("fj");
        assert!(t > 0.0, "fj 应有正的时间: {}", t);
        // 同键连击
        let t_same = model.sequence_time("ff");
        assert!(t_same > t, "ff ({}) 应比 fj ({}) 更慢", t_same, t);
    }

    #[test]
    fn test_unknown_key() {
        let model = KeySoulModel::new();
        assert_eq!(model.sequence_time("f中"), -1.0);
    }

    #[test]
    fn test_index_based() {
        let model = KeySoulModel::new();
        // f=5, j=9
        let t1 = model.sequence_time("fj");
        let t2 = model.sequence_time_from_indices(&[5, 9]);
        assert!(
            (t1 - t2).abs() < 0.01,
            "字符串和索引计算应一致: {} vs {}",
            t1,
            t2
        );
    }
}
