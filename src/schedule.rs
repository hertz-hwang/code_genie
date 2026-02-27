// =========================================================================
// 🌡️ 温度调度器
// =========================================================================

/// 查找表大小
const SCHEDULE_LUT_SIZE: usize = 100_000;

/// 温度调度器 - 使用高斯舒适区实现自适应降温
pub struct TemperatureSchedule {
    /// 温度查找表
    lut: Vec<f64>,
    /// 舒适区进度位置
    comfort_progress: f64,
    /// 配置参数（用于打印）
    t_start: f64,
    t_end: f64,
    comfort_temp: f64,
    comfort_width: f64,
    comfort_slowdown: f64,
}

impl TemperatureSchedule {
    /// 构建温度调度器
    /// 
    /// # 参数
    /// - `t_start`: 初始温度
    /// - `t_end`: 结束温度
    /// - `comfort_temp`: 舒适温度
    /// - `width`: 舒适区宽度
    /// - `slowdown`: 舒适区减速深度
    pub fn build(t_start: f64, t_end: f64, comfort_temp: f64, width: f64, slowdown: f64) -> Self {
        let comfort_p = if t_start <= t_end || comfort_temp >= t_start {
            0.0
        } else if comfort_temp <= t_end {
            1.0
        } else {
            (comfort_temp / t_start).ln() / (t_end / t_start).ln()
        };

        let n = SCHEDULE_LUT_SIZE;
        let mut cumulative = vec![0.0f64; n + 1];
        for i in 1..=n {
            let p = i as f64 / n as f64;
            let dp = p - comfort_p;
            let gaussian = (-dp * dp / (2.0 * width * width)).exp();
            let speed = 1.0 - slowdown * gaussian;
            cumulative[i] = cumulative[i - 1] + speed;
        }

        let total = cumulative[n];
        let mut lut = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let q = if total > 0.0 {
                cumulative[i] / total
            } else {
                i as f64 / n as f64
            };
            let temp = t_start * (t_end / t_start).powf(q);
            lut.push(temp);
        }

        Self {
            lut,
            comfort_progress: comfort_p,
            t_start,
            t_end,
            comfort_temp,
            comfort_width: width,
            comfort_slowdown: slowdown,
        }
    }

    /// 获取指定步骤的温度
    #[inline(always)]
    pub fn get(&self, step: usize, total_steps: usize) -> f64 {
        if total_steps == 0 {
            return self.lut[0];
        }
        let idx_f = step as f64 / total_steps as f64 * SCHEDULE_LUT_SIZE as f64;
        let idx = idx_f.floor() as usize;
        if idx >= SCHEDULE_LUT_SIZE {
            return self.lut[SCHEDULE_LUT_SIZE];
        }
        let frac = idx_f - idx as f64;
        self.lut[idx] + (self.lut[idx + 1] - self.lut[idx]) * frac
    }

    /// 打印降温曲线预览
    pub fn print_preview(&self, total_steps: usize) {
        println!("   🌡️ 降温曲线预览:");
        println!(
            "   舒适温度: {:.6} (进度 {:.1}% 处)",
            self.comfort_temp,
            self.comfort_progress * 100.0
        );
        println!(
            "   舒适区宽度: {:.2}, 减速深度: {:.0}%",
            self.comfort_width,
            self.comfort_slowdown * 100.0
        );
        println!("   ┌──────────────────────────────────────────────────────");

        let rows = 20;
        let bar_width = 50;
        let log_start = self.t_start.ln();
        let log_end = self.t_end.ln();
        let log_range = log_start - log_end;

        for i in 0..=rows {
            let step = total_steps * i / rows;
            let temp = self.get(step, total_steps);
            let log_pos = if log_range > 0.0 {
                ((temp.ln() - log_end) / log_range * bar_width as f64) as usize
            } else {
                0
            };
            let bar_len = log_pos.min(bar_width);
            let bar: String = "█".repeat(bar_len);
            let marker = if (i as f64 / rows as f64 - self.comfort_progress).abs() < 0.5 / rows as f64 {
                " ◄ 舒适区"
            } else {
                ""
            };
            println!(
                "   │{:>3}% T={:.2e} │{}{}",
                i * 100 / rows,
                temp,
                bar,
                marker
            );
        }
        println!("   └──────────────────────────────────────────────────────");
    }
}