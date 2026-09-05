//! Conversion shared by high-resolution native and browser wheel input.

/// Unit in which a host reports each wheel axis.
#[derive(Clone, Copy)]
pub enum WheelUnit {
    /// CSS pixels in the browser, logical window pixels on native hosts.
    Pixel,
    /// One host line, interpreted as thirty logical pixels by the demo.
    Line,
    /// One viewport extent, independently on each axis.
    Page,
}

/// Fractional physical pixels carried forward instead of losing slow trackpad movement.
#[derive(Default)]
pub struct WheelAccumulator {
    /// Unemitted fractions for the horizontal and vertical axes.
    remainder: [f64; 2],
}

impl WheelAccumulator {
    /// Converts positive-right/down host deltas into the core's physical pixel convention.
    pub fn translate(&mut self, delta: [f64; 2], unit: WheelUnit, scale: [f64; 2], viewport: [f64; 2]) -> [i32; 2] {
        let mut result = [0; 2];
        for axis in 0..2 {
            let multiplier = match unit {
                WheelUnit::Pixel => scale[axis],
                WheelUnit::Line => 30.0 * scale[axis],
                WheelUnit::Page => viewport[axis],
            };
            let value = delta[axis] * multiplier + self.remainder[axis];
            if value.is_finite() {
                // Truncate toward zero so opposite fractional samples cancel symmetrically.
                let bounded = value.clamp(i32::MIN as f64, i32::MAX as f64);
                result[axis] = bounded.trunc() as i32;
                self.remainder[axis] = bounded.fract();
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Covers direction, resolution, line/page conversion, cancellation, and invalid host values.
    #[test]
    fn wheel_units_preserve_physical_distance_and_fractional_motion() {
        let mut wheel = WheelAccumulator::default();
        let scale = [2.0, 2.0];
        let size = [800.0, 600.0];
        assert_eq!(wheel.translate([0.2, -0.2], WheelUnit::Pixel, scale, size), [0, 0]);
        assert_eq!(wheel.translate([0.3, -0.3], WheelUnit::Pixel, scale, size), [1, -1]);
        assert_eq!(wheel.translate([1.0, -2.0], WheelUnit::Line, scale, size), [60, -120]);
        assert_eq!(wheel.translate([0.5, -1.0], WheelUnit::Page, scale, size), [400, -600]);
        assert_eq!(wheel.translate([f64::NAN, f64::INFINITY], WheelUnit::Pixel, scale, size), [0, 0]);
        assert_eq!(wheel.translate([0.2, 0.2], WheelUnit::Pixel, scale, size), [0, 0]);
        assert_eq!(wheel.translate([-0.2, -0.2], WheelUnit::Pixel, scale, size), [0, 0]);
    }
}
