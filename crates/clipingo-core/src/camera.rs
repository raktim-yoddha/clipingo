//! Virtual camera: turns noisy face centres into a calm crop path.
//!
//! Port of BridgeClip's `smooth_focus_path` / `simplify_path`: hold still inside a
//! dead zone, pan with a capped speed, then keep the fewest keyframes that
//! reproduce the path (they become ffmpeg crop expressions and Premiere Motion keyframes).

/// Analysis rate the path grid is built on.
pub const ANALYSIS_FPS: f64 = 4.0;
/// Subject may drift this share of the crop width before the camera moves.
pub const CAMERA_DEADZONE: f64 = 0.12;
/// Fastest pan, in crop widths per second.
pub const CAMERA_MAX_SPEED: f64 = 0.9;
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
    let max_step = CAMERA_MAX_SPEED * crop_w_frac * step_ms as f64 / 1000.0;
    let first_second = ((1000 / step_ms) as usize).max(1).min(fx.len());
    let mut cam_x = median(&fx[..first_second]);
    let mut cam_y = median(&fy[..first_second]);
    let mut path = vec![Keyframe { t_ms: 0, x: cam_x, y: cam_y }];
    for i in 1..grid.len() {
        let dx = fx[i] - cam_x;
        if dx.abs() > deadzone {
            // Move toward the subject, easing: never all the way in one step.
            let toward = (dx - deadzone * dx.signum()) * 0.5;
            cam_x += toward.clamp(-max_step, max_step);
        }
        cam_y += (fy[i] - cam_y) * 0.2;
        path.push(Keyframe { t_ms: grid[i].0, x: cam_x, y: cam_y });
    }
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
        assert!((path[0].x - path[1].x).abs() < 1e-9);
    }

    #[test]
    fn pan_speed_is_capped_and_path_simplified() {
        // Subject jumps from left to right at 2 s.
        let samples: Vec<_> = (0..40).map(|i| (i * 250, if i < 8 { 0.2 } else { 0.8 }, 0.5)).collect();
        let path = smooth_focus_path(&samples, 10_000, 0.3);
        assert!(path.len() <= MAX_PATH_KEYFRAMES);
        let max_step = CAMERA_MAX_SPEED * 0.3 * 0.25;
        for w in path.windows(2) {
            let dt_steps = (w[1].t_ms - w[0].t_ms) as f64 / 250.0;
            assert!((w[1].x - w[0].x).abs() <= max_step * dt_steps + 1e-9);
        }
        assert!(path.last().unwrap().x > 0.6, "camera must end near the subject");
    }
}
