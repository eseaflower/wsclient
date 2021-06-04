/*
Check bitrates for different resolutions
Visually lossless!

Resolutions scaling = 1.0:
320x240 = 2.0 MB
640x480 = 3.5 MB
1280x720 = 5.0 MB
1920x1080 = 7 MB (?)
2560x1360 = 10 MB (?)

Resolutions (scaling):
320x240 (don't) = 2.0 MB
640x480 (don't) = 3.5 MB
1280x720 (0.75) = 4.0 MB
1920x1080 (0.75)= 5.5 MB (?)
2560x1360 (0.75)= 7 MB (?)
 */
#[derive(Debug, Copy, Clone)]
pub enum Schedule {
    Default,
    Performance,
    Quality,
}

impl Schedule {
    const BREAK_POINTS: [(u32, u32); 5] = [
        (320, 240),
        (640, 480),
        (1280, 720),
        (1920, 1080),
        (2560, 1360),
    ];
    const DEFAULT_RATES: [f32; 5] = [2.0, 3.5, 5.0, 7.0, 10.0];
    const DEFAULT_SCALING: [f32; 5] = [1.0, 1.0, 1.0, 1.0, 1.0];
    const PERFORMANCE_RATES: [f32; 5] = [2.0, 3.5, 4.0, 5.5, 7.0];
    const PERFORMANCE_SCALING: [f32; 5] = [1.0, 1.0, 0.75, 0.75, 0.75];

    fn get_bin(&self, size: (u32, u32)) -> (Option<usize>, usize) {
        let size_area = size.0 * size.1;
        let upper = Self::BREAK_POINTS
            .iter()
            .enumerate()
            .find_map(|(idx, bp)| {
                if bp.0 * bp.1 >= size_area {
                    // Return the previous bin
                    Some(idx)
                } else {
                    None
                }
            })
            .unwrap_or(Self::BREAK_POINTS.len() - 1);
        let lower = if upper > 0 { Some(upper - 1) } else { None };
        (lower, upper)
    }

    pub fn scaling(&self, size: (u32, u32)) -> f32 {
        let (lower_bin, _) = self.get_bin(size);
        let lower_bin = lower_bin.unwrap_or(0);
        match self {
            Schedule::Default => Self::DEFAULT_SCALING[lower_bin],
            Schedule::Performance => Self::PERFORMANCE_SCALING[lower_bin],
            Schedule::Quality => 1_f32,
        }
    }

    fn interpolate(rates: &[f32; 5], lower: usize, upper: usize, size: (u32, u32)) -> f32 {
        let low_size = Self::BREAK_POINTS[lower];
        let high_size = Self::BREAK_POINTS[upper];
        let low_area = low_size.0 * low_size.1;
        let high_area = high_size.0 * high_size.1;
        let area_span = (high_area - low_area) as f32;
        let size_area = size.0 * size.1;
        let factor = (size_area - low_area) as f32 / area_span;
        // Mix the rates according to the factor
        rates[lower] * (1_f32 - factor) + factor * rates[upper]
    }

    fn extrapolate(rates: &[f32; 5], upper: usize, size: (u32, u32)) -> f32 {

        let upper_size = Self::BREAK_POINTS[upper];
        let low_area = upper_size.0 * upper_size.1;
        let size_area = size.0 * size.1;
        let factor = size_area as f32 / low_area as f32;
        factor * rates[upper]
    }

    pub fn bitrate(&self, size: (u32, u32)) -> f32 {
        let rates = match self {
            Schedule::Default => &Self::DEFAULT_RATES,
            Schedule::Performance => &Self::PERFORMANCE_RATES,
            Schedule::Quality => &Self::DEFAULT_RATES,
        };

        let (lower, upper) = self.get_bin(size);
        let rate = if let Some(lower) = lower {
            Self::interpolate(rates, lower, upper, size)
        } else {
            Self::extrapolate(rates, upper, size)
        };
        if matches!(self, Schedule::Quality) {
            1.2f32 * rate
        } else {
            rate
        }
    }
}
