/// Dynamic burning-token pixels for the quota tray icon.
/// Token height encodes remaining quota; colour is a secondary severity cue.
pub fn ashtray_rgba(size: u32, remaining: Option<f32>) -> Vec<u8> {
    let remaining = remaining
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0));
    let mut pixels = vec![0; (size * size * 4) as usize];
    // Four samples per axis keep the curved rim crisp at tray sizes and smooth
    // at larger sizes. Average premultiplied coverage
    // so transparent edges do not acquire a black fringe on light backgrounds.
    const SAMPLES: u32 = 4;
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0u32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * 32.0 / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * 32.0 / size as f32;
                    let color = sample(px, py, remaining);
                    for channel in 0..3 {
                        sum[channel] += color[channel] as u32 * color[3] as u32;
                    }
                    sum[3] += color[3] as u32;
                }
            }
            let offset = ((y * size + x) * 4) as usize;
            for channel in 0..3 {
                pixels[offset + channel] = sum[channel].checked_div(sum[3]).unwrap_or(0) as u8;
            }
            pixels[offset + 3] = (sum[3] / (SAMPLES * SAMPLES)) as u8;
        }
    }
    pixels
}

fn ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> bool {
    ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0
}

fn polygon(x: f32, y: f32, vertices: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = vertices[vertices.len() - 1];
    for &point in vertices {
        if (point.1 > y) != (previous.1 > y)
            && x < (previous.0 - point.0) * (y - point.1) / (previous.1 - point.1) + point.0
        {
            inside = !inside;
        }
        previous = point;
    }
    inside
}

fn sample(x: f32, y: f32, remaining: Option<f32>) -> [u8; 4] {
    const INK: [u8; 4] = [25, 34, 45, 255];
    const RIM: [u8; 4] = [230, 239, 245, 255];
    const STEEL: [u8; 4] = [115, 143, 162, 255];
    let mut color = [0; 4];

    // Wide shallow bowl, with a dark exterior and a light lip: one of the two
    // boundaries stays visible against either light or dark native trays.
    if ellipse(x, y, 16.0, 24.0, 14.5, 6.0) {
        color = INK;
    }
    if ellipse(x, y, 16.0, 24.0, 13.0, 4.5) {
        color = STEEL;
    }
    if ellipse(x, y, 16.0, 22.5, 13.0, 3.5) {
        color = RIM;
    }
    if ellipse(x, y, 16.0, 22.5, 10.8, 2.0) {
        color = INK;
    }

    match remaining {
        None => {
            // A hollow token with a diagonal break, not a filled/full token.
            // No live ember: missing data cannot masquerade as remaining quota.
            if ellipse(x, y, 16.0, 14.0, 8.7, 9.0) {
                color = INK;
            }
            if ellipse(x, y, 16.0, 14.0, 7.3, 7.6) {
                color = RIM;
            }
            if ellipse(x, y, 16.0, 14.0, 5.1, 5.4) {
                color = [0; 4];
            }
            if (x + y - 30.0).abs() < 2.2 && ellipse(x, y, 16.0, 14.0, 6.6, 6.9) {
                color = INK;
                if (x + y - 30.0).abs() < 0.9 {
                    color = RIM;
                }
            }
        }
        Some(remaining) if remaining > 0.0 => {
            let (face, highlight) = if remaining > 50.0 {
                ([56, 183, 115, 255], [179, 245, 182, 255])
            } else if remaining > 20.0 {
                ([241, 168, 46, 255], [255, 229, 148, 255])
            } else {
                ([230, 88, 59, 255], [255, 188, 139, 255])
            };
            // Consumption removes the token from the top. Keep a small live
            // shard for positive fractions; only exact zero becomes cold ash.
            let burn = 6.0 + (100.0 - remaining) * 0.135;
            let edge = burn
                + if x < 13.0 {
                    0.7
                } else if x < 18.0 {
                    0.0
                } else {
                    1.0
                };
            if y >= edge && ellipse(x, y, 16.0, 14.5, 8.5, 9.0) {
                color = INK;
            }
            if y >= edge + 1.0 && ellipse(x, y, 16.0, 14.5, 7.1, 7.6) {
                color = highlight;
            }
            if y >= edge + 2.1 && ellipse(x, y, 16.0, 14.5, 5.7, 6.2) {
                color = face;
            }
            // Embossed T distinguishes a token from an ordinary flame or leaf.
            if y >= edge + 3.0
                && (((12.5..19.5).contains(&x) && (11.0..13.0).contains(&y))
                    || ((15.0..17.0).contains(&x) && (13.0..18.0).contains(&y)))
            {
                color = INK;
            }

            // A broad asymmetric flame follows the receding edge. Its pale
            // centre and dark outline survive monochrome and tray resampling.
            let fx = x - 20.0;
            let fy = y - (burn + 2.5);
            if polygon(
                fx,
                fy,
                &[
                    (-3.7, 1.7),
                    (-4.0, -1.0),
                    (-1.2, -4.0),
                    (0.0, -7.0),
                    (2.0, -4.6),
                    (2.2, -2.8),
                    (3.6, -3.5),
                    (4.0, -0.2),
                    (2.6, 2.0),
                    (0.0, 3.0),
                ],
            ) {
                color = INK;
            }
            if polygon(
                fx,
                fy,
                &[
                    (-2.2, 1.0),
                    (-2.5, -0.6),
                    (-0.3, -3.0),
                    (0.4, -4.7),
                    (1.0, -1.0),
                    (2.5, -1.7),
                    (2.4, 0.8),
                    (0.4, 1.8),
                ],
            ) {
                color = face;
            }
            if polygon(fx, fy, &[(-0.9, 0.9), (0.5, -1.7), (1.5, 0.7), (0.3, 1.5)]) {
                color = highlight;
            }
        }
        Some(_) => {
            // Two cold, separated ash fragments stay below the live token.
            if polygon(
                x,
                y,
                &[(10.0, 22.0), (12.0, 19.0), (16.0, 20.0), (17.5, 23.0)],
            ) {
                color = RIM;
            }
            if polygon(x, y, &[(18.5, 23.0), (20.0, 20.5), (23.0, 22.5)]) {
                color = STEEL;
            }
        }
    }

    // Foreground lip seats every state in the same ashtray rather than letting
    // the token float above it. Preserve the dark well on either side.
    if y >= 23.0 && ellipse(x, y, 16.0, 22.5, 13.0, 3.5) && !ellipse(x, y, 16.0, 22.5, 10.8, 2.0) {
        color = RIM;
    }
    color
}
#[cfg(test)]
mod tests {
    use super::ashtray_rgba;

    fn silhouette(size: u32, remaining: Option<f32>) -> Vec<bool> {
        ashtray_rgba(size, remaining)
            .chunks_exact(4)
            .map(|pixel| pixel[3] >= 128)
            .collect()
    }

    #[test]
    fn quota_states_have_distinct_silhouettes_at_every_tray_size() {
        let states = [Some(100.0), Some(50.0), Some(10.0), Some(0.0), None];
        for size in [16, 24, 32, 48] {
            for (index, state) in states.iter().enumerate() {
                for other in &states[index + 1..] {
                    assert_ne!(
                        silhouette(size, *state),
                        silhouette(size, *other),
                        "{size}px: {state:?} and {other:?} must differ without hue"
                    );
                }
            }
        }
    }

    #[test]
    fn burning_material_recedes_as_primary_quota_is_consumed() {
        for size in [16, 24, 32, 48] {
            let material: Vec<_> = [100.0, 50.0, 10.0, 0.0]
                .map(|remaining| {
                    silhouette(size, Some(remaining))
                        .into_iter()
                        .take((size * size * 2 / 3) as usize)
                        .filter(|filled| *filled)
                        .count()
                })
                .into();
            assert!(
                material.windows(2).all(|pair| pair[0] > pair[1]),
                "{size}px: upper silhouette must burn down, got {material:?}"
            );
        }
    }

    #[test]
    fn non_finite_measurements_are_unknown_and_finite_values_are_clamped() {
        for size in [16, 24, 32, 48, 64, 256] {
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                assert_eq!(ashtray_rgba(size, Some(invalid)), ashtray_rgba(size, None));
            }
            assert_eq!(
                ashtray_rgba(size, Some(-1.0)),
                ashtray_rgba(size, Some(0.0))
            );
            assert_eq!(
                ashtray_rgba(size, Some(101.0)),
                ashtray_rgba(size, Some(100.0))
            );
        }
    }

    #[test]
    fn a_positive_fraction_keeps_a_live_ember() {
        for size in [16, 24, 32, 48] {
            assert_ne!(
                silhouette(size, Some(0.4)),
                silhouette(size, Some(0.0)),
                "{size}px: a positive fraction must not appear exhausted"
            );
        }
    }
}
