// =========================================================================
// 🎰 AMHB EXP3 算子池
// 对齐 V5 实现：正确的二项分布抽样、task_buffer 填充、worker 任务分配
// =========================================================================

use rand::prelude::*;
use rand::distributions::Uniform;
use std::sync::atomic::{AtomicI32, Ordering};
use crate::amhb::operators::AmhbOperator;

/// EXP3 算子池
pub struct AmhbOperatorPool {
    operators: Vec<AmhbOperator>,
    /// task_buffer[pos] = operator_idx，workers 通过此表查找算子
    pub task_buffer: Vec<usize>,
    temp_coef: Vec<f64>,
    weight: Vec<f64>,              // softmax 概率
    var: Vec<f64>,                 // 在线 VaR 估计
    exp3_discount: Vec<f64>,       // intra-round 衰减
    exp3_weight: Vec<f64>,         // EXP3 log-weight
    task_count: Vec<i32>,          // 每算子分配的任务数

    // 超参数
    gamma: f64,        // exploration rate
    lambda: f64,       // intra-round decay
    eta_exp3: f64,     // learning rate
    eta_var: f64,      // VaR learning rate

    total_neighbors: usize,
}

impl AmhbOperatorPool {
    pub fn new(total_neighbors: usize) -> Self {
        Self {
            operators: Vec::new(),
            task_buffer: vec![0; total_neighbors],
            temp_coef: Vec::new(),
            weight: Vec::new(),
            var: Vec::new(),
            exp3_discount: vec![0.0; total_neighbors],
            exp3_weight: Vec::new(),
            task_count: Vec::new(),
            gamma: 0.05,
            lambda: 0.9999,
            eta_exp3: 0.02,
            eta_var: 0.01,
            total_neighbors,
        }
    }

    /// 添加算子
    pub fn add_operator(&mut self, op: AmhbOperator) {
        self.operators.push(op);
        self.weight.push(1.0);
        self.var.push(1.0);
        self.exp3_weight.push(1.0);  // V5 用 1.0 而非 0.0
        self.temp_coef.push(1.0);
        self.task_count.push(0);
    }

    /// EXP3 计算权重、分配任务到 task_buffer，并将任务范围分配给各 worker。
    /// 对齐 V5 的 cal_refs 实现。
    pub fn cal_refs(
        &mut self,
        worker_lefts: &[&AtomicI32],
        worker_rights: &[&AtomicI32],
        rng: &mut impl Rng,
    ) -> usize {
        let num_ops = self.operators.len();

        // Step 1: LogSumExp → softmax → mixed probability
        let max_l = self
            .exp3_weight
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let mut sum_exp = 0.0f64;
        for i in 0..num_ops {
            self.weight[i] = (self.exp3_weight[i] - max_l).exp();
            sum_exp += self.weight[i];
        }
        let mut total_weight = 0.0f64;
        for i in 0..num_ops {
            self.weight[i] =
                (1.0 - self.gamma) * (self.weight[i] / sum_exp) + self.gamma / num_ops as f64;
            total_weight += self.weight[i];
        }

        // Step 2: Multinomial sampling (conditional-binomial decomposition)
        let mut remaining_weight = total_weight;
        let mut remaining_samples = self.total_neighbors as i32;
        for i in 0..num_ops {
            if remaining_samples <= 0 {
                self.task_count[i] = 0;
                continue;
            }
            if i == num_ops - 1 {
                self.task_count[i] = remaining_samples;
            } else {
                let p = (self.weight[i] / remaining_weight).clamp(0.0, 1.0);
                self.task_count[i] = binomial_sample(remaining_samples as u32, p, rng) as i32;
            }
            remaining_samples -= self.task_count[i];
            remaining_weight -= self.weight[i];
        }

        // Step 3: 填充 task_buffer + intra-round discount
        let mut pos = 0usize;
        for i in 0..num_ops {
            let mut cur_discount = 1.0f64;
            for _ in 0..self.task_count[i] as usize {
                self.task_buffer[pos] = i;
                self.exp3_discount[pos] = cur_discount;
                pos += 1;
                cur_discount *= self.lambda;
            }
            // Apply cumulative decay to this operator's weight
            self.exp3_weight[i] *= cur_discount;
        }

        // Step 4: 将任务范围均匀分配给 workers
        let total_tasks = self.total_neighbors;
        let num_workers = worker_lefts.len();
        let base_count = total_tasks / num_workers;
        let extra = total_tasks % num_workers;
        let mut offset = 0i32;
        for i in 0..num_workers {
            let assigned = base_count as i32 + if i < extra { 1 } else { 0 };
            worker_lefts[i].store(offset, Ordering::Relaxed);
            worker_rights[i].store(offset + assigned, Ordering::Relaxed);
            offset += assigned;
        }

        total_tasks
    }

    /// EXP3 状态更新（每个候选结果调用一次）
    pub fn update_stats(&mut self, index: usize, delta_e: f64) {
        if index >= self.task_buffer.len() {
            return;
        }
        let opt_idx = self.task_buffer[index];

        // VaR 更新（仅 uphill）
        if delta_e > 0.0 {
            if delta_e > self.var[opt_idx] {
                self.var[opt_idx] += self.eta_var;
            } else {
                self.var[opt_idx] -= self.eta_var;
            }
            self.var[opt_idx] = self.var[opt_idx].max(f64::EPSILON);
        }

        // EXP3 penalty（注意：始终用 var[0] 归一化，与 V5 一致）
        let de_f = delta_e;
        let penalty = if de_f < 0.0 {
            de_f.exp()
        } else {
            (de_f / self.var[0]).exp()
        };
        let r = 1.0 / (1.0 + penalty) / self.weight[opt_idx];
        self.exp3_weight[opt_idx] += self.eta_exp3 * r * self.exp3_discount[index];
    }

    /// 获取算子引用
    pub fn operators(&self) -> &[AmhbOperator] {
        &self.operators
    }

    /// 获取温度系数引用
    pub fn temp_coef(&self) -> &[f64] {
        &self.temp_coef
    }

    /// 获取 weight 引用
    pub fn weight(&self) -> &[f64] {
        &self.weight
    }

    /// 获取 var 引用
    pub fn var(&self) -> &[f64] {
        &self.var
    }
}

/// Binomial(n, p) 抽样（直接模拟法，n ≤ 256 时足够快）
fn binomial_sample(n: u32, p: f64, rng: &mut impl Rng) -> u32 {
    if n == 0 || p <= 0.0 {
        return 0;
    }
    if p >= 1.0 {
        return n;
    }
    let uniform = Uniform::new(0.0f64, 1.0);
    let mut count = 0u32;
    for _ in 0..n {
        if rng.sample(uniform) < p {
            count += 1;
        }
    }
    count
}
