//! What a long job is doing now, for the "still at work" its client hears
//! every few seconds ([decision 0078]): "Receiving objects 42%" rather than
//! only that it's alive. A job's handler runs inside [`scoped`]; anything it
//! calls, on its own task or handed a [`Step`] on a blocking thread, says
//! where it's got to with [`Step::say`].
//!
//! [decision 0078]: ../../../docs/decisions/0078-requests-that-fail-well.md

use std::sync::{Arc, Mutex};

/// Where a job has got to, shared with the beat that reports it.
#[derive(Clone, Default)]
pub struct Step(Arc<Mutex<String>>);

impl Step {
    pub fn say(&self, s: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
    }

    pub fn now(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

tokio::task_local! {
    static STEP: Step;
}

/// Run `f` as a job whose steps go to `step`.
pub async fn scoped<F: std::future::Future>(step: Step, f: F) -> F::Output {
    STEP.scope(step, f).await
}

/// The step of the job this runs in, to hand to a blocking thread.
pub fn current() -> Option<Step> {
    STEP.try_with(Clone::clone).ok()
}

/// A line of git's progress, as a step: "Receiving objects:  42% (420/1000),
/// 1.2 MiB | 300 KiB/s" reads "Receiving objects 42%". What isn't progress
/// is left out.
pub fn git_step(line: &str) -> Option<String> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (what, rest) = line.split_once(':')?;
    let pct = rest.split_whitespace().find(|w| w.ends_with('%'))?;
    pct.trim_end_matches('%').parse::<u32>().ok()?;
    Some(format!("{} {pct}", what.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_progress_reads_as_a_step() {
        assert_eq!(
            git_step("Receiving objects:  42% (420/1000), 1.20 MiB | 300.00 KiB/s").as_deref(),
            Some("Receiving objects 42%")
        );
        assert_eq!(
            git_step("remote: Counting objects: 100% (5/5), done.").as_deref(),
            Some("Counting objects 100%")
        );
        assert_eq!(git_step("Cloning into 'dent'..."), None);
        assert_eq!(git_step("fatal: repository not found"), None);
    }

    #[tokio::test]
    async fn a_job_says_its_step_to_whoever_holds_it() {
        let step = Step::default();
        let seen = step.clone();
        scoped(step, async {
            current().expect("inside a job").say("Resolving deltas 7%");
        })
        .await;
        assert_eq!(seen.now(), "Resolving deltas 7%");
        assert!(current().is_none(), "outside a job, no step");
    }
}
