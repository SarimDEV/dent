//! Spike 04 as a library, so the GPUI spike can render through the same
//! streaming layer and engines.

pub mod corpus;
pub mod math;
pub mod mermaid;
pub mod stream;
pub mod visible;

pub fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.;
    }
    v.sort_by(f64::total_cmp);
    v[(((v.len() - 1) as f64) * p / 100.).round() as usize]
}
