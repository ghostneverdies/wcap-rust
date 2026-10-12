#[derive(Clone, Copy, Debug, Default)]
pub struct FramePacer {
    pub last_seen: u64,
    next_encode: u128,
}

impl FramePacer {
    pub fn accept(&mut self, time: u64, limit_fps: u32, tick_freq: u64) -> bool {
        if tick_freq == 0 || time <= self.last_seen {
            return false;
        }
        self.last_seen = time;
        if limit_fps == 0 {
            return true;
        }
        let scaled = u128::from(time) * u128::from(limit_fps);
        if scaled < self.next_encode {
            return false;
        }
        if self.next_encode == 0 {
            self.next_encode = scaled;
        }
        let period = u128::from(tick_freq.max(1));
        self.next_encode += ((scaled.saturating_sub(self.next_encode) / period) + 1) * period;
        true
    }
}

pub fn trim_audio_prefix(
    time: u64,
    start: u64,
    count: usize,
    sample_rate: u32,
    tick_freq: u64,
) -> (usize, u64) {
    if time >= start {
        return (0, time);
    }
    if sample_rate == 0 || tick_freq == 0 {
        return (count, time);
    }
    let delta = u128::from(start - time);
    let skip = ((delta * u128::from(sample_rate)).div_ceil(u128::from(tick_freq)))
        .min(count as u128) as usize;
    let offset = (skip as u128 * u128::from(tick_freq)).div_ceil(u128::from(sample_rate));
    (
        skip,
        time.saturating_add(offset.min(u64::MAX as u128) as u64),
    )
}

pub fn multiply_divide_round_up(value: u64, numerator: u64, denominator: u64) -> u64 {
    assert_ne!(denominator, 0, "time denominator must be nonzero");
    (u128::from(value) * u128::from(numerator))
        .div_ceil(u128::from(denominator))
        .min(u128::from(u64::MAX)) as u64
}

pub fn idle_tick_due(last_activity: u64, now: u64, tick_frequency: u64) -> bool {
    tick_frequency != 0 && now >= last_activity && now - last_activity >= tick_frequency
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_and_old_frames_do_not_rewind() {
        let mut p = FramePacer::default();
        assert!(p.accept(100, 0, 10_000_000));
        assert!(!p.accept(100, 0, 10_000_000));
        assert!(!p.accept(90, 0, 10_000_000));
        assert!(p.accept(101, 0, 10_000_000));
        assert_eq!(p.last_seen, 101);
    }

    #[test]
    fn limiter_skips_without_synthesizing_frames() {
        let mut p = FramePacer::default();
        let f = 10_000_000;
        assert!(p.accept(f, 30, f));
        assert!(!p.accept(f + 100_000, 30, f));
        assert!(!p.accept(f, 30, f));
        assert!(p.accept(f + 333_334, 30, f));
        assert!(!p.accept(f + 333_333, 30, f));
        assert!(p.accept(f + 666_667, 30, f));
    }

    #[test]
    fn unlimited_keeps_all_unique_monotonic_frames() {
        let mut p = FramePacer::default();
        for t in [1, 2, 3, 4, 1000] {
            assert!(p.accept(t, 0, 10_000_000));
        }
    }

    #[test]
    fn multiplication_cannot_overflow() {
        let mut p = FramePacer::default();
        assert!(p.accept(u64::MAX - 1, 240, 10_000_000));
        assert!(!p.accept(u64::MAX, 240, 10_000_000));
    }

    #[test]
    fn late_frames_do_not_create_a_catch_up_burst() {
        let mut p = FramePacer::default();
        assert!(p.accept(1_000_000, 30, 1_000_000));
        assert!(p.accept(11_000_000, 30, 1_000_000));
        assert!(!p.accept(11_000_001, 30, 1_000_000));
        assert!(!p.accept(11_020_000, 30, 1_000_000));
        assert!(p.accept(11_033_334, 30, 1_000_000));
    }

    #[test]
    fn high_refresh_rate_sources_respect_the_limiter_without_drift() {
        let freq = 10_000_000u64;
        for (source_num, source_den) in [(144u64, 1u64), (120, 1), (60_000, 1001)] {
            let mut p = FramePacer::default();
            let mut accepted = Vec::new();
            let count = 60 * source_num / source_den;
            for i in 0..count {
                let t = freq + (i * freq * source_den).div_ceil(source_num);
                if p.accept(t, 30, freq) {
                    accepted.push(t);
                }
            }
            assert!(
                (accepted.len() as i64 - 1800).abs() <= 1,
                "{source_num}/{source_den}: {}",
                accepted.len()
            );
            assert!(accepted.windows(2).all(|w| w[0] < w[1]));
        }
    }

    #[test]
    fn zero_clock_is_rejected_and_new_sessions_reset_the_schedule() {
        let mut p = FramePacer::default();
        assert!(!p.accept(100, 30, 0));
        assert_eq!(p.last_seen, 0);
        assert!(p.accept(100, 30, 1000));
        p = FramePacer::default();
        assert!(p.accept(1, 30, 1000));
    }

    #[test]
    fn audio_trimming_uses_qpc_units_and_releases_full_packets() {
        assert_eq!(
            trim_audio_prefix(1_000_000, 1_005_000, 480, 48_000, 1_000_000),
            (240, 1_005_000)
        );
        assert_eq!(
            trim_audio_prefix(1_000_000, 1_010_001, 480, 48_000, 1_000_000).0,
            480
        );
        let (skip, time) = trim_audio_prefix(1_000_000, 1_000_001, 480, 48_000, 3_579_545);
        assert_eq!(skip, 1);
        assert!(time >= 1_000_001);
        assert_eq!(trim_audio_prefix(50, 50, 480, 48_000, 1_000_000), (0, 50));
        assert_eq!(
            trim_audio_prefix(0, u64::MAX, usize::MAX, u32::MAX, 1).0,
            usize::MAX
        );
    }

    #[test]
    fn timestamps_start_at_zero_and_large_products_do_not_wrap() {
        assert_eq!(multiply_divide_round_up(0, 10_000_000, 3_579_545), 0);
        assert_eq!(
            multiply_divide_round_up(3_579_545, 10_000_000, 3_579_545),
            10_000_000
        );
        assert_eq!(
            multiply_divide_round_up(u64::MAX, u64::MAX, u64::MAX),
            u64::MAX
        );
        assert_eq!(multiply_divide_round_up(1, 10, 3), 4);
    }

    #[test]
    fn idle_gap_requires_one_second_and_a_monotonic_clock() {
        assert!(!idle_tick_due(100, 99, 1000));
        assert!(!idle_tick_due(100, 1099, 1000));
        assert!(idle_tick_due(100, 1100, 1000));
        assert!(!idle_tick_due(1100, 1100, 1000));
        assert!(!idle_tick_due(100, 1100, 0));
        assert!(!idle_tick_due(u64::MAX - 10, u64::MAX, 1000));
    }
}
