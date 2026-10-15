//! How loud the speakers are right now, for the island's music bars.
//!
//! One background thread holds the system's peak meter and samples it 30 times a
//! second, but only while someone asked within the last second; otherwise it sleeps.

/// Current output peak from 0 to 1 (0 where the system has no meter).
pub fn peak() -> f32 {
    imp::peak()
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
    use std::thread::Thread;
    use std::time::{Duration, Instant};
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::System::Com::CLSCTX_ALL;

    struct Sampler {
        level: AtomicU32,
        /// Milliseconds since `start` until which someone wants samples.
        wanted_until: AtomicU64,
        start: Instant,
        thread: Thread,
    }

    static SAMPLER: OnceLock<&'static Sampler> = OnceLock::new();

    fn sampler() -> &'static Sampler {
        SAMPLER.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<&'static Sampler>();
            let handle = std::thread::Builder::new()
                .name("audio-meter".into())
                .spawn(move || run(rx.recv().expect("sampler")))
                .expect("meter thread");
            let s: &'static Sampler = Box::leak(Box::new(Sampler {
                level: AtomicU32::new(0),
                wanted_until: AtomicU64::new(0),
                start: Instant::now(),
                thread: handle.thread().clone(),
            }));
            let _ = tx.send(s);
            s
        })
    }

    pub fn peak() -> f32 {
        let s = sampler();
        s.wanted_until.store(s.start.elapsed().as_millis() as u64 + 1000, Relaxed);
        s.thread.unpark();
        f32::from_bits(s.level.load(Relaxed))
    }

    fn acquire() -> Option<IAudioMeterInformation> {
        unsafe { crate::volume::imp::device().ok()?.Activate(CLSCTX_ALL, None).ok() }
    }

    fn run(s: &'static Sampler) {
        let mut meter: Option<(IAudioMeterInformation, Instant)> = None;
        loop {
            if s.start.elapsed().as_millis() as u64 > s.wanted_until.load(Relaxed) {
                s.level.store(0, Relaxed);
                meter = None;
                std::thread::park();
                continue;
            }
            // Pick the meter up again every few seconds, so a change of speakers is followed.
            if meter.as_ref().is_none_or(|(_, at)| at.elapsed() > Duration::from_secs(3)) {
                meter = acquire().map(|m| (m, Instant::now()));
            }
            let v = meter.as_ref().and_then(|(m, _)| unsafe { m.GetPeakValue() }.ok()).unwrap_or(0.0);
            s.level.store(v.clamp(0.0, 1.0).to_bits(), Relaxed);
            std::thread::sleep(Duration::from_millis(33));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn peak() -> f32 {
        0.0
    }
}
