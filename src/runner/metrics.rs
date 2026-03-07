use prometheus::{Gauge, GaugeVec, Histogram, HistogramOpts, IntCounterVec, Opts, Registry};

use crate::operator::types::RunnerError;

pub struct RunnerMetrics {
    pub injection_active: GaugeVec,
    pub injection_total: IntCounterVec,
    pub targets_affected: Gauge,
    pub injection_duration: Histogram,
}

impl RunnerMetrics {
    pub fn new(registry: &Registry) -> Result<Self, RunnerError> {
        let injection_active = GaugeVec::new(
            Opts::new("chaos_injection_active", "1 if injection is active"),
            &["experiment_id", "scenario"],
        )
        .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;

        let injection_total = IntCounterVec::new(
            Opts::new("chaos_injection_total", "Total injections performed"),
            &["scenario", "result"],
        )
        .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;

        let targets_affected = Gauge::with_opts(Opts::new(
            "chaos_targets_affected",
            "Number of affected targets",
        ))
        .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;

        let injection_duration = Histogram::with_opts(
            HistogramOpts::new(
                "chaos_injection_duration_seconds",
                "Duration of chaos injection",
            )
            .buckets(vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]),
        )
        .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;

        registry
            .register(Box::new(injection_active.clone()))
            .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;
        registry
            .register(Box::new(injection_total.clone()))
            .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;
        registry
            .register(Box::new(targets_affected.clone()))
            .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;
        registry
            .register(Box::new(injection_duration.clone()))
            .map_err(|e| RunnerError::ExecutionFailed(e.to_string()))?;

        Ok(Self {
            injection_active,
            injection_total,
            targets_affected,
            injection_duration,
        })
    }

    pub fn set_active(&self, experiment_id: &str, scenario: &str) {
        self.injection_active
            .with_label_values(&[experiment_id, scenario])
            .set(1.0);
    }

    pub fn set_inactive(&self, experiment_id: &str, scenario: &str) {
        self.injection_active
            .with_label_values(&[experiment_id, scenario])
            .set(0.0);
    }

    pub fn record_success(&self, scenario: &str) {
        self.injection_total
            .with_label_values(&[scenario, "success"])
            .inc();
    }

    pub fn record_failure(&self, scenario: &str) {
        self.injection_total
            .with_label_values(&[scenario, "failure"])
            .inc();
    }

    pub fn set_targets(&self, count: u32) {
        self.targets_affected.set(f64::from(count));
    }

    pub fn observe_duration(&self, seconds: f64) {
        self.injection_duration.observe(seconds);
    }
}
