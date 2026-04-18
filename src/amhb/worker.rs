// =========================================================================
// 🏗️ AMHB Worker 与工作窃取
// 移植自 V5，适配多目标 Evaluator
// =========================================================================

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use rand::prelude::*;
use rand::distributions::Uniform;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::amhb::operators::OperatorResult;

/// Pcg32 随机数生成器类型
use rand_pcg::Pcg32;

// =========================================================================
// 平台抽象
// =========================================================================

/// Atomically-shared f64 via bit-casting to u64
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(val: f64) -> Self {
        Self(AtomicU64::new(val.to_bits()))
    }
    pub fn store(&self, val: f64, ordering: Ordering) {
        self.0.store(val.to_bits(), ordering);
    }
    pub fn load(&self, ordering: Ordering) -> f64 {
        f64::from_bits(self.0.load(ordering))
    }
}

/// CPU spin-wait hint
#[inline(always)]
pub fn cpu_pause() {
    std::hint::spin_loop();
}

// =========================================================================
// Cache-line aligned 原子类型（防止伪共享）
// =========================================================================

#[repr(align(64))]
pub struct AlignedAtomicI32(pub AtomicI32);

impl AlignedAtomicI32 {
    pub fn new(val: i32) -> Self {
        Self(AtomicI32::new(val))
    }
}

#[repr(align(64))]
pub struct AlignedAtomicBool(pub AtomicBool);

impl AlignedAtomicBool {
    pub fn new(val: bool) -> Self {
        Self(AtomicBool::new(val))
    }
}

#[repr(align(64))]
pub struct AlignedAtomicFlag(pub AtomicBool);

impl AlignedAtomicFlag {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    #[inline]
    pub fn test_and_set(&self, order: Ordering) -> bool {
        self.0.swap(true, order)
    }

    #[inline]
    pub fn clear(&self, order: Ordering) {
        self.0.store(false, order);
    }
}

// =========================================================================
// Worker 共享状态
// =========================================================================

/// Worker 共享状态（cache-line aligned 防止伪共享）
pub struct WorkerShared {
    pub left: AlignedAtomicI32,
    pub right: AlignedAtomicI32,
    pub allow_steal: AlignedAtomicBool,
    pub steal_lock: AlignedAtomicFlag,
}

impl WorkerShared {
    pub fn new() -> Self {
        Self {
            left: AlignedAtomicI32::new(0),
            right: AlignedAtomicI32::new(0),
            allow_steal: AlignedAtomicBool::new(false),
            steal_lock: AlignedAtomicFlag::new(),
        }
    }
}

/// Worker 结果
pub struct WorkerResult {
    pub max_gumbel_score: f64,
    pub best_candidate: Option<OperatorResult>,
    pub results: Vec<OperatorResult>,
}

impl WorkerResult {
    pub fn new() -> Self {
        Self {
            max_gumbel_score: f64::NEG_INFINITY,
            best_candidate: None,
            results: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.max_gumbel_score = f64::NEG_INFINITY;
        self.best_candidate = None;
        self.results.clear();
    }
}

// =========================================================================
// Worker 主循环
// =========================================================================

/// Worker 主循环
#[allow(clippy::too_many_arguments)]
pub fn worker_loop(
    _worker_id: usize,
    ctx: &OptContext,
    evaluator: &mut Evaluator,
    assignment: &mut [u8],
    shared: &WorkerShared,
    all_shared: &[&WorkerShared],
    rng: &mut Pcg32,
    task_buffer: &[usize],
    operators: &[crate::amhb::operators::AmhbOperator],
    temp_coef: &[f64],
    score_scale: f64,
    temperature: &AtomicF64,
    steal_threshold: i32,
    global_allow_run: &AtomicBool,
    global_all_task_complete: &AtomicBool,
    global_total_task: &AtomicI32,
    global_used_worker: &AtomicI32,
    global_worker_done: &AtomicI32,
    result: &mut WorkerResult,
) {
    let gumbel_uniform = Uniform::new(0.0f64, 1.0);
    let steal_dist = if all_shared.len() > 1 {
        Some(Uniform::new(0usize, all_shared.len()))
    } else {
        None
    };

    loop {
        // 1. 等待开始指令
        while !global_allow_run.load(Ordering::Acquire) {
            if global_all_task_complete.load(Ordering::Acquire) {
                return;
            }
            cpu_pause();
        }

        // 2. 检查是否有任务
        let current_left = shared.left.0.load(Ordering::SeqCst);
        let current_right = shared.right.0.load(Ordering::SeqCst);
        if current_right - current_left <= 0 {
            // 没有任务，等待本轮结束
            while global_allow_run.load(Ordering::Acquire) {
                cpu_pause();
            }
            continue;
        }

        // 3. 标记已使用
        global_used_worker.fetch_add(1, Ordering::Relaxed);
        result.reset();

        // 4. 运行本地任务 + 窃取循环
        let temp = temperature.load(Ordering::Relaxed);
        let mut check_allow_run = true;
        while check_allow_run {
            run_local_tasks(
                ctx,
                evaluator,
                assignment,
                shared,
                rng,
                task_buffer,
                operators,
                temp_coef,
                score_scale,
                temp,
                steal_threshold,
                global_allow_run,
                global_total_task,
                result,
                &gumbel_uniform,
            );
            // 尝试窃取
            loop {
                if let Some(ref sd) = steal_dist {
                    if run_steal(shared, all_shared, rng, sd, steal_threshold) {
                        break; // 成功窃取，回去执行
                    }
                }
                if !global_allow_run.load(Ordering::Acquire) {
                    check_allow_run = false;
                    break;
                }
                cpu_pause();
            }
        }

        // 5. 完成报告
        global_worker_done.fetch_add(1, Ordering::Release);
    }
}

/// 锁定最终批次
fn lock_final_batch(
    shared: &WorkerShared,
    pos: i32,
    local_left: &mut i32,
    local_allow_steal: &mut bool,
) -> bool {
    shared.allow_steal.0.store(false, Ordering::Release);
    *local_allow_steal = false;

    let new_right = shared.right.0.load(Ordering::SeqCst);
    if pos < new_right {
        *local_left = new_right;
        shared.left.0.store(new_right, Ordering::SeqCst);
        true
    } else {
        false
    }
}

/// 执行本地任务
#[allow(clippy::too_many_arguments)]
fn run_local_tasks(
    ctx: &OptContext,
    evaluator: &mut Evaluator,
    assignment: &mut [u8],
    shared: &WorkerShared,
    rng: &mut Pcg32,
    task_buffer: &[usize],
    operators: &[crate::amhb::operators::AmhbOperator],
    temp_coef: &[f64],
    score_scale: f64,
    temperature: f64,
    steal_threshold: i32,
    global_allow_run: &AtomicBool,
    global_total_task: &AtomicI32,
    result: &mut WorkerResult,
    gumbel_uniform: &Uniform<f64>,
) {
    let mut local_completed: i32 = 0;
    let mut pos = shared.left.0.load(Ordering::SeqCst);
    let mut local_right = shared.right.0.load(Ordering::SeqCst);
    let mut step_size = get_step_size(pos, local_right);

    let mut local_left;
    let mut local_allow_steal;

    if local_right - pos >= step_size + steal_threshold {
        local_left = (pos + step_size).min(local_right);
        shared.left.0.store(local_left, Ordering::SeqCst);
        local_allow_steal = true;
        shared.allow_steal.0.store(true, Ordering::Release);
    } else {
        local_left = local_right;
        shared.left.0.store(local_left, Ordering::SeqCst);
        local_allow_steal = false;
        shared.allow_steal.0.store(false, Ordering::Release);
    }

    loop {
        if pos >= local_right {
            break;
        }

        // 查找 task_buffer 中的算子索引
        let task_pos = pos as usize;
        if task_pos >= task_buffer.len() {
            break;
        }
        let op_idx = task_buffer[task_pos];
        let op = &operators[op_idx];

        // 调用算子进行增量探测
        let op_result = op.explore(ctx, evaluator, assignment, task_pos, rng);
        if let Some((delta_e, result_op)) = op_result {
            // Gumbel-Max trick
            let u: f64 = rng.sample(gumbel_uniform).clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON);
            let gumbel_noise = -(-u.ln()).ln();
            let score = -(delta_e / (score_scale * temp_coef[op_idx] * temperature)) + gumbel_noise;

            if score > result.max_gumbel_score {
                result.max_gumbel_score = score;
                result.best_candidate = Some(result_op.clone());
            }
            result.results.push(result_op);
        }

        local_completed += 1;
        pos += 1;

        // 动态调整步进边界
        if pos >= local_left {
            local_right = shared.right.0.load(Ordering::SeqCst);
            step_size = get_step_size(pos, local_right);

            if local_right - local_left >= step_size + steal_threshold {
                local_left += step_size;
                shared.left.0.store(local_left, Ordering::SeqCst);
                // 检查是否还有足够的任务供窃取
                if shared.right.0.load(Ordering::SeqCst) - local_left < steal_threshold {
                    if !lock_final_batch(shared, pos, &mut local_left, &mut local_allow_steal) {
                        break;
                    }
                }
            } else if local_allow_steal {
                if !lock_final_batch(shared, pos, &mut local_left, &mut local_allow_steal) {
                    break;
                }
            } else {
                break;
            }
        }
    }

    // 通知完成的任务数 + 放弃的任务数
    // 关键修复：当 worker 因边界竞态提前退出时，它持有范围内未处理的任务
    // 也必须从 global_total_task 中扣除，否则计数永远无法归零导致死锁。
    // 放弃的任务 = 当前 right 中还未被窃走的部分（从 pos 到 right）。
    //
    // 必须在持有 steal_lock 的情况下禁止窃取并读取 right，
    // 防止窃取者在 allow_steal=false 之前已经通过双重检查并正在修改 right。
    while shared.steal_lock.test_and_set(Ordering::Acquire) {
        cpu_pause();
    }
    shared.allow_steal.0.store(false, Ordering::Release);
    let final_right = shared.right.0.load(Ordering::SeqCst);
    // 将 left 推到 right，表示本 worker 范围已清空（不会被窃取）
    shared.left.0.store(final_right, Ordering::SeqCst);
    shared.steal_lock.clear(Ordering::Release);

    let abandoned = (final_right - pos).max(0);

    let total_to_sub = local_completed + abandoned;
    if total_to_sub > 0 {
        let prev_total = global_total_task.fetch_sub(total_to_sub, Ordering::AcqRel);
        if prev_total <= total_to_sub {
            // 计数已归零（或下溢，理论上不应该），通知主线程
            global_allow_run.store(false, Ordering::Release);
        }
    }
}

/// 工作窃取
fn run_steal(
    self_shared: &WorkerShared,
    all_shared: &[&WorkerShared],
    rng: &mut Pcg32,
    steal_dist: &Uniform<usize>,
    steal_threshold: i32,
) -> bool {
    // 随机选择受害者
    let victim_idx = rng.sample(steal_dist);
    let victim = all_shared[victim_idx];

    // 检查是否允许窃取
    if !victim.allow_steal.0.load(Ordering::Relaxed) {
        return false;
    }

    // 尝试获取锁
    if victim.steal_lock.test_and_set(Ordering::Acquire) {
        return false; // 锁已被持有
    }

    // 双重检查
    if !victim.allow_steal.0.load(Ordering::Relaxed) {
        victim.steal_lock.clear(Ordering::Release);
        return false;
    }

    let v_left = victim.left.0.load(Ordering::Relaxed);
    let v_right = victim.right.0.load(Ordering::Relaxed);

    // 检查是否有足够任务窃取
    if v_right - v_left < 2 * steal_threshold {
        victim.steal_lock.clear(Ordering::Release);
        return false;
    }

    // 窃取上半部分
    let middle = ((v_left + v_right + 1) / 2).min(v_right - 1);
    victim.right.0.store(middle, Ordering::SeqCst);

    // 回滚检查
    let new_left = victim.left.0.load(Ordering::SeqCst);
    if new_left > middle - steal_threshold {
        victim.right.0.store(v_right, Ordering::Relaxed);
        victim.steal_lock.clear(Ordering::Release);
        return false;
    }

    // 成功窃取 [middle, v_right)
    self_shared.left.0.store(middle, Ordering::SeqCst);
    self_shared.right.0.store(v_right, Ordering::Release);
    victim.steal_lock.clear(Ordering::Release);
    true
}

/// 获取步长：min(32, floor(sqrt(n)))，至少 1
#[inline]
fn get_step_size(l: i32, r: i32) -> i32 {
    let n = r - l;
    1.max(32.min((n as f64).sqrt().floor() as i32))
}
