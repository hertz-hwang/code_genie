// =========================================================================
// 🚀 AMHB 优化器主循环
// 移植自 V5，适配多目标 Evaluator
// 使用 thread::scope + 裸指针（零锁）替代 Arc<Mutex>
// =========================================================================

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::amhb::operators::{PointwiseOperator, ExchangeOperator, AmhbOperator, OperatorResult};
use crate::amhb::operator_pool::AmhbOperatorPool;
use crate::amhb::worker::{AtomicF64, WorkerShared, WorkerResult, cpu_pause, worker_loop};
use rand::Rng;
use rand_pcg::Pcg32;

/// 裸指针 Send+Sync 包装（对齐 V5）
struct SendPtr<T>(*mut T);
impl<T> Clone for SendPtr<T> { fn clone(&self) -> Self { Self(self.0) } }
impl<T> Copy for SendPtr<T> {}
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}
impl<T> SendPtr<T> {
    #[inline] fn ptr(self) -> *mut T { self.0 }
}

struct SendConstPtr<T>(*const T);
impl<T> Clone for SendConstPtr<T> { fn clone(&self) -> Self { Self(self.0) } }
impl<T> Copy for SendConstPtr<T> {}
unsafe impl<T> Send for SendConstPtr<T> {}
unsafe impl<T> Sync for SendConstPtr<T> {}
impl<T> SendConstPtr<T> {
    #[inline] fn ptr(self) -> *const T { self.0 }
}
/// AMHB 优化器
pub struct AmhbOptimizer {
    print: bool,
    temperature: f64,
    pub operator_pool: AmhbOperatorPool,
    num_workers: usize,
    steal_threshold: i32,
    pub best_assignment: Vec<u8>,
    pub best_score: f64,
}

impl AmhbOptimizer {
    pub fn new(ctx: &OptContext, num_workers: usize, print: bool, total_neighbors: usize, steal_threshold: i32) -> Self {
        let mut rng = rand::thread_rng();
        let assignment: Vec<u8> = (0..ctx.num_groups)
            .map(|gi| {
                let keys = &ctx.groups[gi].allowed_keys;
                keys[rng.gen_range(0..keys.len())]
            })
            .collect();
        let mut evaluator = Evaluator::new(ctx, &assignment);
        let best_score = evaluator.get_score(ctx);

        // 创建算子并初始化采样分布
        let num_groups = ctx.num_groups;

        let mut pw_op = PointwiseOperator::new(0);
        pw_op.init_distributions(num_groups);

        let mut ex_op = ExchangeOperator::new(0);
        ex_op.init_distributions(num_groups);

        let mut operator_pool = AmhbOperatorPool::new(total_neighbors);
        operator_pool.add_operator(AmhbOperator::Pointwise(pw_op));
        operator_pool.add_operator(AmhbOperator::Exchange(ex_op));

        Self {
            print,
            temperature: 0.0,
            operator_pool,
            num_workers,
            steal_threshold,
            best_assignment: assignment,
            best_score,
        }
    }

    /// 温度调度函数
    pub fn solve<F>(&mut self, ctx: &OptContext, param: AmhbParameters, next_temp: F, stop_flag: &AtomicBool)
    where
        F: Fn(f64, usize, f64) -> f64,
    {
        // --- 准备 worker 数据 ---
        // 每个 worker 有自己的 Evaluator + assignment 副本
        let mut worker_evaluators: Vec<Evaluator> = (0..self.num_workers)
            .map(|_| Evaluator::new(ctx, &self.best_assignment))
            .collect();
        let mut worker_assignments: Vec<Vec<u8>> = (0..self.num_workers)
            .map(|_| self.best_assignment.clone())
            .collect();

        if self.print {
            println!("\n CODEGENIE | AMHB Optimization Start. Initial Score: {:.6}", self.best_score);
        }

        // --- 校准 score_scale：采样随机 move 的 |delta_score| 中位数 ---
        let score_scale = {
            let cal_eval = &mut worker_evaluators[0];
            let cal_assign = &mut worker_assignments[0];
            let mut cal_rng = Pcg32::new(42, 1);
            let num_samples = 500usize;
            let mut deltas: Vec<f64> = Vec::with_capacity(num_samples);
            for _ in 0..num_samples {
                let gi = cal_rng.gen_range(0..ctx.num_groups);
                let keys = &ctx.groups[gi].allowed_keys;
                if keys.len() <= 1 { continue; }
                let old_key = cal_assign[gi];
                let new_key = loop {
                    let k = keys[cal_rng.gen_range(0..keys.len())];
                    if k != old_key { break k; }
                };
                let d = cal_eval.probe_move(ctx, cal_assign, gi, new_key).abs();
                if d > 0.0 { deltas.push(d); }
            }
            if deltas.is_empty() {
                1.0
            } else {
                deltas.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
                let median = deltas[deltas.len() / 2];
                if median > 0.0 { median } else { 1.0 }
            }
        };
        if self.print {
            println!("  Score scale (median |delta|): {:.6e}", score_scale);
        }

        let start_time = std::time::Instant::now();
        self.temperature = param.temp_start;
        let mut energy_cur = self.best_score;

        // --- 共享同步状态 ---
        let worker_shared: Vec<WorkerShared> =
            (0..self.num_workers).map(|_| WorkerShared::new()).collect();

        let global_allow_run = AtomicBool::new(false);
        let global_all_task_complete = AtomicBool::new(false);
        let global_total_task = AtomicI32::new(0);
        let global_used_worker = AtomicI32::new(0);
        let global_worker_done = AtomicI32::new(0);

        let shared_temperature = AtomicF64::new(self.temperature);

        let mut worker_results: Vec<WorkerResult> =
            (0..self.num_workers).map(|_| WorkerResult::new()).collect();

        let all_shared_refs: Vec<&WorkerShared> = worker_shared.iter().collect();

        // --- 使用 scoped threads ---
        thread::scope(|scope| {
            // 裸指针用于跨线程不相交访问
            let task_buffer_ptr = SendConstPtr(self.operator_pool.task_buffer.as_ptr());
            let task_buffer_len = self.operator_pool.task_buffer.len();
            let operators_ptr = SendConstPtr(self.operator_pool.operators().as_ptr());
            let operators_len = self.operator_pool.operators().len();
            let temp_coef_ptr = SendConstPtr(self.operator_pool.temp_coef().as_ptr());
            let temp_coef_len = self.operator_pool.temp_coef().len();
            let shared_temp_ref = &shared_temperature;
            let steal_threshold = self.steal_threshold;

            let evaluators_ptr = SendPtr(worker_evaluators.as_mut_ptr());
            let assignments_ptr = SendPtr(worker_assignments.as_mut_ptr());
            let results_ptr = SendPtr(worker_results.as_mut_ptr());

            let ctx_ref = ctx;

            for worker_id in 0..self.num_workers {
                let ws = &worker_shared[worker_id];
                let all_refs = &all_shared_refs;
                let g_allow_run = &global_allow_run;
                let g_all_complete = &global_all_task_complete;
                let g_total_task = &global_total_task;
                let g_used_worker = &global_used_worker;
                let g_worker_done = &global_worker_done;

                let my_eval_ptr = evaluators_ptr;
                let my_assign_ptr = assignments_ptr;
                let my_result_ptr = results_ptr;
                let my_tb_ptr = task_buffer_ptr;
                let my_op_ptr = operators_ptr;
                let my_tc_ptr = temp_coef_ptr;

                scope.spawn(move || {
                    // SAFETY: 每个线程只访问自己的索引，互不重叠。
                    // 这些数据在 worker 活跃时不被主线程修改。
                    let evaluator = unsafe { &mut *my_eval_ptr.ptr().add(worker_id) };
                    let assignment = unsafe { &mut *my_assign_ptr.ptr().add(worker_id) };
                    let result = unsafe { &mut *my_result_ptr.ptr().add(worker_id) };

                    let task_buffer = unsafe {
                        std::slice::from_raw_parts(my_tb_ptr.ptr(), task_buffer_len)
                    };
                    let operators = unsafe {
                        std::slice::from_raw_parts(my_op_ptr.ptr(), operators_len)
                    };
                    let temp_coef = unsafe {
                        std::slice::from_raw_parts(my_tc_ptr.ptr(), temp_coef_len)
                    };

                    let mut rng = Pcg32::new(
                        rand::random::<u64>(),
                        rand::random::<u64>() | 1,
                    );

                    worker_loop(
                        worker_id,
                        ctx_ref,
                        evaluator,
                        assignment.as_mut_slice(),
                        ws,
                        all_refs,
                        &mut rng,
                        task_buffer,
                        operators,
                        temp_coef,
                        score_scale,
                        shared_temp_ref,
                        steal_threshold,
                        g_allow_run,
                        g_all_complete,
                        g_total_task,
                        g_used_worker,
                        g_worker_done,
                        result,
                    );
                });
            }

            // --- 主循环 ---
            let worker_lefts: Vec<&AtomicI32> =
                worker_shared.iter().map(|ws| &ws.left.0).collect();
            let worker_rights: Vec<&AtomicI32> =
                worker_shared.iter().map(|ws| &ws.right.0).collect();

            let mut pool_rng = Pcg32::new(
                rand::random::<u64>(),
                rand::random::<u64>() | 1,
            );

            // 简码重建计数器
            let simple_rebuild_interval: usize = 10000;

            for iter in 1..=param.max_iterations as usize {
                // 1. EXP3 调度 + 任务分配给 workers
                let total_generated = self.operator_pool.cal_refs(
                    &worker_lefts,
                    &worker_rights,
                    &mut pool_rng,
                );

                global_total_task.store(total_generated as i32, Ordering::Release);
                global_used_worker.store(0, Ordering::Relaxed);
                global_worker_done.store(0, Ordering::Relaxed);

                // 2. 唤醒 workers
                global_allow_run.store(true, Ordering::Release);

                // 3. 等待完成
                let mut empty_spins: u32 = 0;
                loop {
                    if !global_allow_run.load(Ordering::Acquire) {
                        break;
                    }
                    if global_total_task.load(Ordering::Acquire) <= 0 {
                        global_allow_run.store(false, Ordering::Release);
                        break;
                    }
                    // 安全网：检查是否所有 worker 的任务队列已清空
                    // 如果连续 1024 次 spin 都看到队列为空，说明任务计数有竞态泄漏
                    let mut all_empty = true;
                    for ws in worker_shared.iter() {
                        let l = ws.left.0.load(Ordering::Relaxed);
                        let r = ws.right.0.load(Ordering::Relaxed);
                        if r > l {
                            all_empty = false;
                            break;
                        }
                    }
                    if all_empty {
                        empty_spins += 1;
                        if empty_spins >= 1024 {
                            global_allow_run.store(false, Ordering::Release);
                            global_total_task.store(0, Ordering::Release);
                            break;
                        }
                    } else {
                        empty_spins = 0;
                    }
                    cpu_pause();
                }
                while global_used_worker.load(Ordering::Acquire)
                    != global_worker_done.load(Ordering::Acquire)
                {
                    cpu_pause();
                }

                // 4. 全局 Gumbel-Max 归约 + EXP3 更新
                let mut global_best: Option<OperatorResult> = None;
                let mut global_max_score = f64::NEG_INFINITY;

                for thread_id in 0..self.num_workers {
                    let wr = unsafe { &mut *results_ptr.ptr().add(thread_id) };
                    if wr.best_candidate.is_some() {
                        if wr.max_gumbel_score > global_max_score {
                            global_max_score = wr.max_gumbel_score;
                            global_best = wr.best_candidate.take();
                        }
                        // 更新 EXP3 统计（所有候选，不只是 best）
                        for r in &wr.results {
                            self.operator_pool.update_stats(r.task_index(), r.delta_score() / score_scale);
                        }
                    }
                }

                // 5. 将最佳候选增量应用到所有 worker 的 evaluator + assignment
                if let Some(ref best) = global_best {
                    for wid in 0..self.num_workers {
                        let eval = unsafe { &mut *evaluators_ptr.ptr().add(wid) };
                        let assign = unsafe { &mut *assignments_ptr.ptr().add(wid) };
                        best.apply(ctx_ref, eval, assign.as_mut_slice());
                    }
                }

                // 6. 更新最佳解
                let eval0 = unsafe { &mut *evaluators_ptr.ptr() };
                energy_cur = eval0.get_score(ctx_ref);
                if energy_cur < self.best_score {
                    self.best_score = energy_cur;
                    let assign0 = unsafe { &*assignments_ptr.ptr() };
                    self.best_assignment = assign0.clone();
                }

                // 周期性重建简码（与 SA 模式一致，每 10000 步一次）
                if ctx.enable_simple_code && iter % simple_rebuild_interval == 0 {
                    for wid in 0..self.num_workers {
                        let eval = unsafe { &mut *evaluators_ptr.ptr().add(wid) };
                        let assign = unsafe { &*assignments_ptr.ptr().add(wid) };
                        eval.rebuild_simple(ctx_ref, assign);
                        eval.score_dirty = true;
                    }
                    // 更新当前得分
                    let eval0 = unsafe { &mut *evaluators_ptr.ptr() };
                    energy_cur = eval0.get_score(ctx_ref);
                    if energy_cur < self.best_score {
                        self.best_score = energy_cur;
                        let assign0 = unsafe { &*assignments_ptr.ptr() };
                        self.best_assignment = assign0.clone();
                    }
                }

                // 7. 打印进度
                if self.print && iter % 10000 == 0 {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed = iter as f64 / elapsed / 10000.0;
                    println!(
                        "[Iter: {} | Temp: {:.6} | Best: {:.6} | Current: {:.6} | Speed: {:.1}万步/分 | Time: {:.1}s]",
                        iter, self.temperature, self.best_score, energy_cur, speed * 60.0, elapsed
                    );
                    let weights = self.operator_pool.weight();
                    let vars = self.operator_pool.var();
                    for i in 0..weights.len() {
                        print!(
                            "  [Op{}: weight={:.4} VaR={:.4}]",
                            i, weights[i], vars[i]
                        );
                    }
                    println!();
                }

                // 8. 降温
                self.temperature = next_temp(self.temperature, iter, energy_cur);
                shared_temperature.store(self.temperature, Ordering::Release);
                if self.temperature < 0.0 {
                    break;
                }
                // 检查外部停止信号（每 10000 步一次）
                if iter % 10000 == 0 && stop_flag.load(Ordering::Relaxed) {
                    if self.print {
                        println!("\n   [AMHB] 收到停止信号，正在退出...");
                    }
                    break;
                }
            }

            // 9. 终止 workers
            global_all_task_complete.store(true, Ordering::Release);

        }); // scope 结束 — 所有 worker 线程在此 join

        if self.print {
            println!(
                "\n CODEGENIE | AMHB Optimization Complete. Best: {:.6}",
                self.best_score
            );
        }
    }
}

/// AMHB 参数
#[derive(Clone)]
#[allow(dead_code)]
pub struct AmhbParameters {
    pub max_iterations: u64,
    pub temp_start: f64,
    pub total_neighbors: usize,
    pub steal_threshold: i32,
}
