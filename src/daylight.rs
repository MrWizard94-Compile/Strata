//! Pausable day/night clock and the colors it feeds to the camera uniform.
//!
//! Fraction `0` is midnight and `0.5` is noon. The clock counts fixed 60 Hz
//! simulation ticks, so a full cycle is exactly [`TICKS_PER_DAY`] and pausing
//! is a skipped tick rather than a frozen timestamp. Sky and fog colors are
//! linear (the same `pow(2.2)` the shader used to apply). Ambient and sun
//! multipliers stay in the shader's existing light space. Night ambient is
//! dark blue and deliberately above black so the world stays readable.

use std::f32::consts::TAU;

/// Real seconds in one full day. Twenty minutes keeps dawn, noon, and dusk
/// easy to see in a session without rushing the light.
pub const DAY_LENGTH_SECONDS: u32 = 20 * 60;
pub const SIM_TICKS_PER_SECOND: u32 = 60;
pub const SIM_TICK_SECONDS: f32 = 1.0 / SIM_TICKS_PER_SECOND as f32;
pub const TICKS_PER_DAY: u32 = DAY_LENGTH_SECONDS * SIM_TICKS_PER_SECOND;
/// 08:00. Schema 1 saves written before time of day existed load here.
pub const MORNING: f32 = (TICKS_PER_DAY / 3) as f32 / TICKS_PER_DAY as f32;

const QUARTERS: usize = 4;
const SUN_INTENSITY: f32 = 0.72;
const SOUTH_BIAS: f32 = -0.42;

/// Midnight, dawn (06:00), noon, dusk (18:00). Smoothstep blends each quarter.
const SKY_ZENITH: [[f32; 3]; QUARTERS] = [
    [0.04, 0.06, 0.14],
    [0.22, 0.28, 0.48],
    [0.27, 0.52, 0.70],
    [0.18, 0.18, 0.38],
];
const SKY_HORIZON: [[f32; 3]; QUARTERS] = [
    [0.09, 0.11, 0.18],
    [0.96, 0.58, 0.36],
    [0.76, 0.84, 0.86],
    [0.94, 0.42, 0.26],
];
const FOG_COLOR: [[f32; 3]; QUARTERS] = [
    [0.07, 0.09, 0.16],
    [0.78, 0.50, 0.38],
    [0.68, 0.78, 0.82],
    [0.70, 0.38, 0.30],
];
const AMBIENT_GROUND: [[f32; 3]; QUARTERS] = [
    [0.07, 0.08, 0.12],
    [0.16, 0.13, 0.12],
    [0.28, 0.32, 0.32],
    [0.16, 0.11, 0.10],
];
const AMBIENT_SKY: [[f32; 3]; QUARTERS] = [
    [0.10, 0.12, 0.18],
    [0.32, 0.28, 0.36],
    [0.62, 0.72, 0.82],
    [0.30, 0.22, 0.28],
];
const SUN_TINT: [[f32; 3]; QUARTERS] = [
    [0.55, 0.62, 0.85],
    [1.05, 0.48, 0.22],
    [1.00, 0.87, 0.65],
    [1.05, 0.42, 0.18],
];

/// Sun direction and the uniform colors for one instant of the day.
#[derive(Clone, Copy, Debug)]
pub struct Daylight {
    pub sun_direction: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub sky_zenith: [f32; 3],
    pub fog_color: [f32; 3],
    pub ambient_ground: [f32; 3],
    pub ambient_sky: [f32; 3],
    pub sun_color: [f32; 3],
    /// 0 at night, 1 when the sun is clearly above the horizon.
    pub sun_glow: f32,
    /// Wrapped fraction in `[0, 1)`.
    pub time_of_day: f32,
}

/// Simulation-tick clock. `tick` is always in `0..TICKS_PER_DAY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayClock {
    tick: u32,
}

impl DayClock {
    pub fn morning() -> Self {
        Self {
            tick: TICKS_PER_DAY / 3,
        }
    }

    pub fn from_fraction(fraction: f32) -> Result<Self, String> {
        validate_time_of_day(fraction)?;
        let rounded = (fraction * TICKS_PER_DAY as f32).round();
        let tick = if rounded >= TICKS_PER_DAY as f32 {
            0
        } else {
            rounded as u32
        };
        Ok(Self { tick })
    }

    /// Tick index in `0..TICKS_PER_DAY`. Exposed for tests and save round-trips.
    #[cfg(test)]
    pub fn tick(self) -> u32 {
        self.tick
    }

    pub fn fraction(self) -> f32 {
        self.tick as f32 / TICKS_PER_DAY as f32
    }

    /// Advances one simulation tick. A paused clock stays on the same tick.
    pub fn advance(&mut self, paused: bool) {
        if paused {
            return;
        }
        self.tick += 1;
        if self.tick >= TICKS_PER_DAY {
            self.tick = 0;
        }
    }

    /// Debug scrub. Non-finite input leaves the clock unchanged.
    pub fn skip_hours(&mut self, hours: f32) {
        let ticks_per_hour = (TICKS_PER_DAY / 24) as f32;
        let delta = (hours * ticks_per_hour).round();
        if !delta.is_finite() {
            return;
        }
        let wrapped = delta.rem_euclid(TICKS_PER_DAY as f32) as u32;
        self.tick = (self.tick + wrapped) % TICKS_PER_DAY;
    }

    pub fn sample(self) -> Daylight {
        sample_daylight(self.fraction())
    }

    pub fn phase(self) -> &'static str {
        match self.hour_minute().0 {
            0..=4 | 21..=23 => "night",
            5..=7 => "dawn",
            8..=10 => "morning",
            11..=13 => "midday",
            14..=16 => "afternoon",
            _ => "dusk",
        }
    }

    pub fn label(self) -> String {
        let (hour, minute) = self.hour_minute();
        format!("{hour:02}:{minute:02} {}", self.phase())
    }

    fn hour_minute(self) -> (u32, u32) {
        let ticks_per_hour = TICKS_PER_DAY / 24;
        let ticks_per_minute = TICKS_PER_DAY / (24 * 60);
        let hour = self.tick / ticks_per_hour;
        let minute = (self.tick % ticks_per_hour) / ticks_per_minute;
        (hour, minute)
    }
}

pub fn validate_time_of_day(value: f32) -> Result<(), String> {
    if value.is_finite() && (0.0..1.0).contains(&value) {
        Ok(())
    } else {
        Err("World contains an invalid time of day.".into())
    }
}

pub fn sample_daylight(fraction: f32) -> Daylight {
    let fraction = wrapped_fraction(fraction);
    let sun_height = ((fraction - 0.25) * TAU).sin();
    // Glow is 0.5 when the sun sits on the horizon and 0 once it is well below.
    let sun_glow = smoothstep(-0.15, 0.15, sun_height);
    let sun_tint = sample_curve(fraction, &SUN_TINT);
    Daylight {
        sun_direction: sun_direction(fraction),
        sky_horizon: sample_linear(fraction, &SKY_HORIZON),
        sky_zenith: sample_linear(fraction, &SKY_ZENITH),
        fog_color: sample_linear(fraction, &FOG_COLOR),
        ambient_ground: sample_curve(fraction, &AMBIENT_GROUND),
        ambient_sky: sample_curve(fraction, &AMBIENT_SKY),
        sun_color: scale(sun_tint, SUN_INTENSITY * sun_glow),
        sun_glow,
        time_of_day: fraction,
    }
}

fn wrapped_fraction(fraction: f32) -> f32 {
    if !fraction.is_finite() {
        return 0.0;
    }
    // f32::rem_euclid loses the remainder once the magnitude exceeds the mantissa.
    let reduced = (fraction as f64).rem_euclid(1.0);
    if !reduced.is_finite() {
        return 0.0;
    }
    let reduced = reduced as f32;
    if reduced.is_finite() && (0.0..1.0).contains(&reduced) {
        reduced
    } else {
        0.0
    }
}

fn sun_direction(fraction: f32) -> [f32; 3] {
    let theta = (fraction - 0.25) * TAU;
    normalize([theta.cos(), theta.sin(), SOUTH_BIAS])
}

fn sample_linear(fraction: f32, srgb_keys: &[[f32; 3]; QUARTERS]) -> [f32; 3] {
    let linear = srgb_keys.map(to_linear);
    sample_curve(fraction, &linear)
}

fn sample_curve(fraction: f32, keys: &[[f32; 3]; QUARTERS]) -> [f32; 3] {
    let x = fraction * QUARTERS as f32;
    let index = (x as usize).min(QUARTERS - 1);
    let blend = smoothstep(0.0, 1.0, x - index as f32);
    lerp(keys[index], keys[(index + 1) % QUARTERS], blend)
}

fn to_linear(color: [f32; 3]) -> [f32; 3] {
    color.map(|channel| channel.max(0.0).powf(2.2))
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let span = edge1 - edge0;
    let t = if span.abs() <= f32::EPSILON {
        0.0
    } else {
        ((value - edge0) / span).clamp(0.0, 1.0)
    };
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale(color: [f32; 3], factor: f32) -> [f32; 3] {
    [color[0] * factor, color[1] * factor, color[2] * factor]
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    scale(value, 1.0 / length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(color: [f32; 3]) -> f32 {
        0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn simulation_tick_advances_and_wraps_after_a_full_day() {
        let mut clock = DayClock::from_fraction(0.0).unwrap();
        assert_eq!(clock.label(), "00:00 night");
        clock.advance(false);
        assert_eq!(clock.tick(), 1);
        assert!((clock.fraction() - 1.0 / TICKS_PER_DAY as f32).abs() < 1e-7);

        for _ in 1..TICKS_PER_DAY {
            let restored = DayClock::from_fraction(clock.fraction()).unwrap();
            assert_eq!(restored.tick(), clock.tick(), "tick {}", clock.tick());
            clock.advance(false);
        }
        assert_eq!(clock.tick(), 0);
        assert_eq!(clock.fraction(), 0.0);
        assert_eq!(DayClock::morning().label(), "08:00 morning");
        assert_eq!(DayClock::morning().fraction(), MORNING);
    }

    #[test]
    fn paused_advance_does_not_move_the_clock() {
        let mut clock = DayClock::morning();
        let before = clock.fraction();
        let light = clock.sample();
        for _ in 0..TICKS_PER_DAY {
            clock.advance(true);
        }
        assert_eq!(clock.fraction(), before);
        assert_eq!(clock.sample().time_of_day, light.time_of_day);
        assert_eq!(clock.sample().sun_direction, light.sun_direction);
        clock.advance(false);
        assert!(clock.fraction() > before);
        clock.skip_hours(f32::NAN);
        let moved = clock.fraction();
        clock.skip_hours(f32::INFINITY);
        assert_eq!(clock.fraction(), moved);
    }

    #[test]
    fn skipping_forward_wraps_past_midnight() {
        let mut clock = DayClock::from_fraction(0.0).unwrap();
        clock.skip_hours(23.0);
        assert_eq!(clock.label(), "23:00 night");
        clock.skip_hours(2.0);
        assert_eq!(clock.label(), "01:00 night");
        clock.skip_hours(24.0);
        assert_eq!(clock.label(), "01:00 night");
        assert_eq!(
            DayClock::from_fraction(0.5).unwrap().label(),
            "12:00 midday"
        );
        assert_eq!(DayClock::from_fraction(0.75).unwrap().label(), "18:00 dusk");
    }

    #[test]
    fn non_finite_and_out_of_range_times_are_rejected() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.0, 1.5] {
            assert!(validate_time_of_day(bad).is_err(), "{bad}");
            assert!(DayClock::from_fraction(bad).is_err(), "{bad}");
        }
        assert!(DayClock::from_fraction(0.0).is_ok());
        assert!(DayClock::from_fraction(MORNING).is_ok());
    }

    #[test]
    fn noon_dusk_and_midnight_are_distinct_and_readable() {
        let midnight = sample_daylight(0.0);
        let noon = sample_daylight(0.5);
        let dusk = sample_daylight(0.75);

        assert!((dot(noon.sun_direction, noon.sun_direction) - 1.0).abs() < 1e-5);
        assert!((dot(dusk.sun_direction, dusk.sun_direction) - 1.0).abs() < 1e-5);
        assert!((dot(midnight.sun_direction, midnight.sun_direction) - 1.0).abs() < 1e-5);

        assert!(noon.sun_direction[1] > 0.75, "noon sun should be high");
        assert!(noon.sky_zenith[2] > noon.sky_zenith[0], "noon sky is blue");
        assert_eq!(noon.sky_zenith, to_linear([0.27, 0.52, 0.70]));
        assert_eq!(noon.sky_horizon, to_linear([0.76, 0.84, 0.86]));
        assert_eq!(noon.ambient_ground, [0.28, 0.32, 0.32]);
        assert_eq!(noon.ambient_sky, [0.62, 0.72, 0.82]);
        assert!(noon.sun_glow > 0.95);
        for (actual, expected) in noon.sun_color.into_iter().zip([
            1.0 * SUN_INTENSITY,
            0.87 * SUN_INTENSITY,
            0.65 * SUN_INTENSITY,
        ]) {
            assert!((actual - expected).abs() < 1e-5);
        }

        assert!(
            dusk.sun_direction[0] < -0.5,
            "dusk sun sets toward the west"
        );
        assert!(
            dusk.sun_direction[1].abs() < 0.25,
            "dusk sun is near the horizon"
        );
        assert!(
            dusk.sky_horizon[0] > dusk.sky_horizon[2],
            "dusk horizon is warm"
        );
        assert!(dusk.fog_color[0] > dusk.fog_color[2], "dusk fog is warm");
        assert!(dusk.sun_glow > midnight.sun_glow);
        assert!(dusk.sun_glow < noon.sun_glow);
        assert!((sample_daylight(0.25).sun_glow - 0.5).abs() < 0.02);
        assert!((dusk.sun_glow - 0.5).abs() < 0.02);

        assert!(midnight.sun_direction[1] < -0.7, "midnight sun is below");
        assert!(
            midnight.sky_zenith[2] > midnight.sky_zenith[0],
            "night sky is blue"
        );
        assert!(luma(midnight.sky_zenith) > 0.001, "night sky is not black");
        assert!(luma(midnight.sky_zenith) < 0.05, "night sky is dark");
        assert!(luma(midnight.sky_horizon) > luma(midnight.sky_zenith));
        assert!(
            luma(midnight.ambient_ground) > 0.04,
            "terrain stays readable"
        );
        assert!(
            luma(midnight.ambient_ground) < 0.2,
            "night is actually dark"
        );
        assert!(luma(midnight.ambient_sky) > luma(midnight.ambient_ground));
        assert_eq!(midnight.sun_color, [0.0, 0.0, 0.0]);
        assert!(midnight.sun_glow < 0.01);

        assert!(luma(noon.sky_zenith) > luma(midnight.sky_zenith) * 4.0);
        assert!(luma(noon.ambient_sky) > luma(midnight.ambient_sky));
        let between = sample_daylight(0.625);
        assert!(between.sky_horizon[0] > noon.sky_horizon[0]);
        assert!(between.sky_horizon[0] < dusk.sky_horizon[0]);
    }

    #[test]
    fn sun_and_sky_change_continuously_through_the_day() {
        let steps = 4_096;
        let mut previous = sample_daylight(0.0);
        for step in 1..=steps {
            let fraction = step as f32 / steps as f32;
            let next = sample_daylight(if fraction >= 1.0 { 0.0 } else { fraction });
            assert_channels_close(previous.sun_direction, next.sun_direction);
            assert_channels_close(previous.sky_horizon, next.sky_horizon);
            assert_channels_close(previous.sky_zenith, next.sky_zenith);
            assert_channels_close(previous.fog_color, next.fog_color);
            assert_channels_close(previous.ambient_ground, next.ambient_ground);
            assert_channels_close(previous.ambient_sky, next.ambient_sky);
            assert_channels_close(previous.sun_color, next.sun_color);
            assert!((previous.sun_glow - next.sun_glow).abs() < 0.02);
            assert!(
                dot(previous.sun_direction, next.sun_direction) > 0.999,
                "sun direction popped at {fraction}"
            );
            for color in [
                next.sky_horizon,
                next.sky_zenith,
                next.fog_color,
                next.ambient_ground,
                next.ambient_sky,
                next.sun_color,
            ] {
                assert!(color.iter().all(|channel| channel.is_finite()));
            }
            previous = next;
        }
    }

    #[test]
    fn extreme_inputs_stay_finite_and_sane() {
        let midnight = sample_daylight(0.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let light = sample_daylight(bad);
            assert_eq!(light.time_of_day, midnight.time_of_day);
            assert_eq!(light.sun_direction, midnight.sun_direction);
            assert_eq!(light.sun_color, midnight.sun_color);
            assert_eq!(light.sun_glow, midnight.sun_glow);
        }
        for huge in [1.0e10, -1.0e10, 1.0e20, -1.0e20, f32::MAX, f32::MIN] {
            let light = sample_daylight(huge);
            assert!(
                light.time_of_day.is_finite() && (0.0..1.0).contains(&light.time_of_day),
                "{huge} -> {}",
                light.time_of_day
            );
            let length = dot(light.sun_direction, light.sun_direction).sqrt();
            assert!(
                (length - 1.0).abs() < 1e-4,
                "sun length {length} for {huge}"
            );
            assert!((0.0..=1.0).contains(&light.sun_glow));
            for color in [
                light.sun_direction,
                light.sky_horizon,
                light.sky_zenith,
                light.fog_color,
                light.ambient_ground,
                light.ambient_sky,
                light.sun_color,
            ] {
                assert!(color.iter().all(|channel| channel.is_finite()), "{huge}");
            }
        }
    }

    fn assert_channels_close(a: [f32; 3], b: [f32; 3]) {
        for (left, right) in a.into_iter().zip(b) {
            assert!((left - right).abs() < 0.02, "{a:?} jumped to {b:?}");
        }
    }
}
