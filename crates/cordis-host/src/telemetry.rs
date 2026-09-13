use cordis_core::Core;
use gpui_kit::{FrameEvent, FrameTimingCollector};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Sampling observes GPUI's completed draws; it never invalidates a view or calls a guest.
pub fn start(root: PathBuf, core: Arc<Mutex<Core>>) {
    std::thread::spawn(move || {
        let mut collector = FrameTimingCollector::new();
        let mut frames = Vec::<u64>::new();
        let mut total_frames = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            for event in collector.collect_unseen() {
                if let FrameEvent::Draw(frame) = event {
                    total_frames += 1;
                    frames.push(frame.draw_duration().as_micros() as u64);
                }
            }
            if frames.len() > 512 {
                frames.drain(..frames.len() - 512);
            }
            let mut sorted = frames.clone();
            sorted.sort_unstable();
            let (trace, effects, active, blocked, guest_calls) = {
                let core = core.lock().unwrap();
                (
                    core.trace.clone(),
                    core.episodes
                        .values()
                        .map(|e| e.effects.len())
                        .sum::<usize>(),
                    core.episodes
                        .values()
                        .filter(|e| e.phase == cordis_core::Phase::Active)
                        .count(),
                    core.episodes
                        .values()
                        .filter(|e| e.phase == cordis_core::Phase::RecoveryBlocked)
                        .count(),
                    core.guest_calls,
                )
            };
            let value = serde_json::json!({"host_pid":std::process::id(),"guest_calls":guest_calls,"owned_effects":effects,"active_generations":active,"blocked_generations":blocked,"draw_count":total_frames,"draw_samples":frames.len(),"draw_p50_us":sorted.get(sorted.len()/2),"draw_p95_us":sorted.get(sorted.len()*95/100),"draw_max_us":sorted.last()});
            let _ = std::fs::write(root.join("data/telemetry.tmp"), value.to_string());
            let _ = std::fs::rename(
                root.join("data/telemetry.tmp"),
                root.join("data/telemetry.json"),
            );
            let _ = std::fs::write(
                root.join("data/trace.tmp"),
                serde_json::to_vec_pretty(&trace).unwrap_or_default(),
            );
            let _ = std::fs::rename(root.join("data/trace.tmp"), root.join("data/trace.json"));
        }
    });
}
