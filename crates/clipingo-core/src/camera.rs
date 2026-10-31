//! Virtual camera: turns noisy face centres into a calm crop path that stays on the face.
//!
//! Median filter (drops single-frame detector jumps), then a dead zone (a still head never moves the camera), then
//! zero-phase smoothing: run forwards and backwards, so the camera does not trail the subject the way a one-way
//! filter or a speed-capped pan does. Finally the fewest keyframes that reproduce the path (they become ffmpeg crop
//! expressions and Premiere Motion keyframes).

/// Analysis rate the path grid is built on.
pub const ANALYSIS_FPS: f64 = 4.0;
/// Subject may drift this share of the crop width before the camera moves.
pub const CAMERA_DEADZONE: f64 = 0.06;
/// Time constant of the smoothing, seconds: longer is calmer, shorter sticks closer.
pub const CAMERA_TAU_S: f64 = 0.4;
/// FFmpeg expression budget per shot.
pub const MAX_PATH_KEYFRAMES: usize = 40;

/// Camera focus at `t_ms`, normalised 0–1 of the source frame.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Keyframe {
    pub t_ms: i64,
    pub x: f64,
    pub y: f64,
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// `samples` are (t_ms, face centre x, face centre y); `crop_w_frac` is the crop width as a share of the source.
pub fn smooth_focus_path(samples: &[(i64, f64, f64)], duration_ms: i64, crop_w_frac: f64) -> Vec<Keyframe> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut samples = samples.to_vec();
    samples.sort_by_key(|s| s.0);
    let step_ms = (1000.0 / ANALYSIS_FPS) as i64;

    // Resample onto a regular grid, holding the nearest detection.
    let mut grid = Vec::new();
    let mut j = 0;
    let mut t = 0;
    while t < duration_ms.max(1) {
        while j + 1 < samples.len() && (samples[j + 1].0 - t).abs() <= (samples[j].0 - t).abs() {
            j += 1;
        }
        grid.push((t, samples[j].1, samples[j].2));
        t += step_ms;
    }

    // Median filter rejects single-frame detector jumps.
    let k = 2;
    let window = |i: usize, pick: fn(&(i64, f64, f64)) -> f64| {
        let lo = i.saturating_sub(k);
        let hi = (i + k + 1).min(grid.len());
        median(&grid[lo..hi].iter().map(pick).collect::<Vec<_>>())
    };
    let fx: Vec<f64> = (0..grid.len()).map(|i| window(i, |g| g.1)).collect();
    let fy: Vec<f64> = (0..grid.len()).map(|i| window(i, |g| g.2)).collect();

    let deadzone = CAMERA_DEADZONE * crop_w_frac;
    let hold = |v: &[f64], dz: f64| {
        let mut out = vec![v[0]];
        for &x in &v[1..] {
            let cam = out[out.len() - 1];
            let d = x - cam;
            out.push(if d.abs() > dz { x - dz * d.signum() } else { cam });
        }
        out
    };
    let alpha = 1.0 - (-(step_ms as f64 / 1000.0) / CAMERA_TAU_S).exp();
    let zero_phase = |v: &[f64]| {
        let mut f = v.to_vec();
        for i in 1..f.len() {
            f[i] = f[i - 1] + alpha * (v[i] - f[i - 1]);
        }
        let mut b = f.clone();
        for i in (0..b.len().saturating_sub(1)).rev() {
            b[i] = b[i + 1] + alpha * (f[i] - b[i + 1]);
        }
        b
    };
    let (sx, sy) = (zero_phase(&hold(&fx, deadzone)), zero_phase(&hold(&fy, deadzone * 1.5)));
    let path: Vec<Keyframe> =
        grid.iter().enumerate().map(|(i, g)| Keyframe { t_ms: g.0, x: sx[i], y: sy[i] }).collect();
    simplify_path(&path)
}

/// Fewest keyframes reproducing `path` within 0.2% of the source in both axes.
pub fn simplify_path(path: &[Keyframe]) -> Vec<Keyframe> {
    if path.len() <= 2 {
        return path.to_vec();
    }
    let mut keep = vec![0, path.len() - 1];
    while keep.len() < MAX_PATH_KEYFRAMES {
        let mut worst = (0.002, None);
        for pair in keep.windows(2) {
            let (a, b) = (path[pair[0]], path[pair[1]]);
            for (i, p) in path.iter().enumerate().take(pair[1]).skip(pair[0] + 1) {
                let f = (p.t_ms - a.t_ms) as f64 / (b.t_ms - a.t_ms) as f64;
                let err = (p.x - (a.x + (b.x - a.x) * f)).abs().max((p.y - (a.y + (b.y - a.y) * f)).abs());
                if err > worst.0 {
                    worst = (err, Some(i));
                }
            }
        }
        let Some(i) = worst.1 else { break };
        keep.push(i);
        keep.sort_unstable();
    }
    keep.into_iter().map(|i| path[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_subject_inside_dead_zone_keeps_camera_still() {
        let samples: Vec<_> = (0..40).map(|i| (i * 250, 0.5 + if i % 2 == 0 { 0.01 } else { -0.01 }, 0.4)).collect();
        let path = smooth_focus_path(&samples, 10_000, 0.3);
        assert_eq!(path.len(), 2, "jitter inside the dead zone must not create keyframes");
        assert!((path[0].x - path[1].x).abs() < 0.005);
    }

    #[test]
    fn camera_keeps_up_with_a_moving_subject_without_lag() {
        // A presenter walks steadily across the frame.
        let samples: Vec<_> = (0..40).map(|i| (i * 250, 0.2 + 0.6 * i as f64 / 40.0, 0.5)).collect();
        let path = smooth_focus_path(&samples, 10_000, 0.3);
        assert!(path.len() <= MAX_PATH_KEYFRAMES);
        let at = |t: i64| {
            let w = path.windows(2).find(|w| w[0].t_ms <= t && t <= w[1].t_ms).unwrap();
            w[0].x + (w[1].x - w[0].x) * (t - w[0].t_ms) as f64 / (w[1].t_ms - w[0].t_ms) as f64
        };
        assert!((at(5000) - 0.5).abs() < 0.03, "no lag behind the subject: {}", at(5000));
    }

    #[test]
    fn jumps_are_eased_not_snapped() {
        let samples: Vec<_> = (0..40).map(|i| (i * 250, if i < 20 { 0.3 } else { 0.7 }, 0.5)).collect();
        let path = smooth_focus_path(&samples, 10_000, 0.3);
        let last = path.last().unwrap();
        assert!((last.x - 0.7).abs() < 0.03, "ends on the subject: {last:?}");
        for w in path.windows(2) {
            let speed = (w[1].x - w[0].x).abs() / ((w[1].t_ms - w[0].t_ms) as f64 / 1000.0);
            assert!(speed < 0.6, "a calm pan: {w:?}");
        }
    }
}
