//! Example-owned virtual trackball. Pointer coordinates are physical drawable pixels.
use neocogi_render_core::{
    abi::Size,
    platform::{Point, UiInput},
};
use rs_math3d::{Mat4f, Quatf, Vec3f, Vec4f, translate};

/// Vertical field of view; horizontal fitting accounts for portrait windows as well.
const FOV: f32 = std::f32::consts::FRAC_PI_4;

/// Active gesture. Focus loss, resize and matching button release terminate the gesture.
enum Drag {
    /// Pointer motion has no effect.
    Idle,
    /// Left button rotates between successive virtual-sphere positions.
    Rotate(Vec3f),
    /// Right or middle button translates in the camera plane.
    Pan(Point),
}

/// Owned arcball state, mutated only by the application after synchronous event collection.
pub struct Camera {
    /// Initial scene-space rotation pivot, fixed during pan/zoom.
    center: Vec3f,
    /// Initial bounding sphere, also defines clipping and zoom limits.
    radius: f32,
    /// Drawable size used for sphere mapping and perspective aspect ratio.
    size: Size,
    /// Normalized world-to-view rotation; incremental rotations compose on the left.
    rotation: Quatf,
    /// Camera-plane translation; independent of world rotation.
    pan: [f32; 2],
    /// Positive distance from the pivot; bounded relative to the initial sphere.
    distance: f32,
    /// Current exclusive pointer gesture.
    drag: Drag,
}

impl Camera {
    /// Fits a finite positive bounding sphere in both axes with a small margin.
    pub fn new(center: [f32; 3], radius: f32, size: Size) -> Self {
        let mut camera = Self {
            center: Vec3f::new(center[0], center[1], center[2]),
            radius,
            size,
            rotation: Quatf::new(0., 0., 0., 1.),
            pan: [0.; 2],
            distance: radius,
            drag: Drag::Idle,
        };
        camera.reset();
        camera
    }

    /// Resets orientation, pan and distance, retaining the current window aspect ratio.
    pub fn reset(&mut self) {
        let aspect = self.size.width.max(1) as f32 / self.size.height.max(1) as f32;
        let half_angle = ((FOV * 0.5).tan() * aspect.min(1.0)).atan();
        self.distance = self.radius / half_angle.sin() * 1.15;
        self.rotation = Quatf::new(0., 0., 0., 1.);
        self.pan = [0.; 2];
        self.drag = Drag::Idle;
    }

    /// Updates projection and cancels an active gesture when its coordinate system changes.
    pub fn resize(&mut self, size: Size) {
        if self.size != size {
            self.size = size;
            self.drag = Drag::Idle;
        }
    }

    /// Applies one normalized platform event: left rotate, right/middle pan, wheel zoom.
    pub fn input(&mut self, event: UiInput) {
        match event {
            UiInput::Down { position, buttons } => {
                self.drag = if buttons & 1 != 0 {
                    Drag::Rotate(self.sphere(position))
                } else if buttons & 6 != 0 {
                    Drag::Pan(position)
                } else {
                    Drag::Idle
                };
            }
            UiInput::Pointer(position) => match self.drag {
                Drag::Idle => {}
                Drag::Rotate(previous) => {
                    let next = self.sphere(position);
                    let dot = (previous.x * next.x + previous.y * next.y + previous.z * next.z).clamp(-1., 1.);
                    let axis = Vec3f::new(
                        previous.y * next.z - previous.z * next.y,
                        previous.z * next.x - previous.x * next.z,
                        previous.x * next.y - previous.y * next.x,
                    );
                    // At opposite rim points choose a stable orthogonal half-turn axis.
                    let delta = if dot < -0.99999 {
                        let axis = if previous.x.abs() < 0.9 {
                            Vec3f::new(0., previous.z, -previous.y)
                        } else {
                            Vec3f::new(-previous.z, 0., previous.x)
                        };
                        Quatf::new(axis.x, axis.y, axis.z, 0.)
                    } else {
                        Quatf::new(axis.x, axis.y, axis.z, 1. + dot)
                    };
                    self.rotation = Quatf::normalize(&(Quatf::normalize(&delta) * self.rotation));
                    self.drag = Drag::Rotate(next);
                }
                Drag::Pan(previous) => {
                    let units = 2. * self.distance * (FOV * 0.5).tan() / self.size.height.max(1) as f32;
                    self.pan[0] += (position.x as f32 - previous.x as f32) * units;
                    self.pan[1] -= (position.y as f32 - previous.y as f32) * units;
                    self.drag = Drag::Pan(position);
                }
            },
            UiInput::Up { buttons, .. } => match self.drag {
                Drag::Rotate(_) if buttons & 1 != 0 => self.drag = Drag::Idle,
                Drag::Pan(_) if buttons & 6 != 0 => self.drag = Drag::Idle,
                _ => {}
            },
            UiInput::Scroll(delta) => {
                self.distance = (self.distance * (delta.y as f32 * 0.002).clamp(-20., 20.).exp()).clamp(self.radius * 0.05, self.radius * 100.)
            }
            UiInput::Cancel => self.drag = Drag::Idle,
            _ => {}
        }
    }

    /// Maps the smaller drawable axis to a unit sphere; outside points lie on its rim.
    fn sphere(&self, position: Point) -> Vec3f {
        let extent = self.size.width.min(self.size.height).max(1) as f32;
        let x = (2. * position.x as f32 - self.size.width as f32) / extent;
        let y = (self.size.height as f32 - 2. * position.y as f32) / extent;
        let squared = x * x + y * y;
        if squared <= 1. {
            Vec3f::new(x, y, (1. - squared).sqrt())
        } else {
            Vec3f::new(x / squared.sqrt(), y / squared.sqrt(), 0.)
        }
    }

    /// Returns right-handed perspective * view with renderer-canonical depth in [0,1].
    pub fn view_projection(&self) -> Mat4f {
        let aspect = self.size.width.max(1) as f32 / self.size.height.max(1) as f32;
        let f = 1. / (FOV * 0.5).tan();
        let near = self.radius * 0.001;
        let far = self.radius * 250.;
        let projection = Mat4f::new(
            f / aspect,
            0.,
            0.,
            0.,
            0.,
            f,
            0.,
            0.,
            0.,
            0.,
            far / (near - far),
            -1.,
            0.,
            0.,
            near * far / (near - far),
            0.,
        );
        projection * translate(Vec3f::new(self.pan[0], self.pan[1], -self.distance)) * self.rotation.mat4() * translate(-self.center)
    }

    /// Camera position in scene coordinates for view-dependent fragment lighting.
    pub fn eye(&self) -> [f32; 3] {
        let offset = Quatf::conjugate(&self.rotation).mat4() * Vec4f::new(-self.pan[0], -self.pan[1], self.distance, 0.);
        [self.center.x + offset.x, self.center.y + offset.y, self.center.z + offset.z]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test viewport centered on the virtual sphere and a translated world-space pivot.
    fn camera() -> Camera {
        Camera::new([1., 2., 3.], 2., Size { width: 800, height: 600 })
    }
    /// Projects a point to normalized device coordinates to test visible camera behavior.
    fn project(camera: &Camera, point: [f32; 3]) -> [f32; 3] {
        let p = camera.view_projection() * Vec4f::new(point[0], point[1], point[2], 1.);
        [p.x / p.w, p.y / p.w, p.z / p.w]
    }
    /// Arcball rotates around the fitted pivot, remains normalized and stops on button release.
    #[test]
    fn rotates_about_pivot_and_releases() {
        let mut c = camera();
        let old = c.eye();
        c.input(UiInput::Down {
            position: Point { x: 400, y: 300 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 550, y: 350 }));
        assert_ne!(old, c.eye());
        let p = project(&c, [1., 2., 3.]);
        assert!(p[0].abs() < 1e-6 && p[1].abs() < 1e-6 && p[2] > 0. && p[2] < 1.);
        let rotated = c.eye();
        c.input(UiInput::Up {
            position: Point { x: 550, y: 350 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 600, y: 400 }));
        assert_eq!(rotated, c.eye());
    }
    /// Both pan buttons move screen coordinates in the same direction as pointer motion.
    #[test]
    fn pan_and_zoom_are_camera_relative_and_bounded() {
        for buttons in [2, 4] {
            let mut c = camera();
            c.input(UiInput::Down {
                position: Point { x: 100, y: 100 },
                buttons,
            });
            c.input(UiInput::Pointer(Point { x: 200, y: 200 }));
            let p = project(&c, [1., 2., 3.]);
            assert!(p[0] > 0. && p[1] < 0.);
            c.input(UiInput::Scroll(Point { x: 0, y: i32::MIN }));
            assert_eq!(c.distance, c.radius * 0.05);
            c.input(UiInput::Scroll(Point { x: 0, y: i32::MAX }));
            assert_eq!(c.distance, c.radius * 100.);
            c.reset();
            assert_eq!(c.pan, [0., 0.]);
        }
    }
    /// Focus loss and resize cancel gestures; portrait and zero extents remain well defined.
    #[test]
    fn cancel_resize_and_sphere_edges() {
        let mut c = camera();
        for cancel in [true, false] {
            c.input(UiInput::Down {
                position: Point { x: 400, y: 300 },
                buttons: 1,
            });
            if cancel {
                c.input(UiInput::Cancel);
            } else {
                c.resize(Size { width: 300, height: 800 });
            }
            let eye = c.eye();
            c.input(UiInput::Pointer(Point { x: 700, y: 100 }));
            assert_eq!(eye, c.eye());
        }
        c.resize(Size { width: 0, height: 0 });
        c.reset();
        assert!(c.eye().iter().all(|v| v.is_finite()));
        assert!(project(&c, [1., 2., 3.]).iter().all(|v| v.is_finite()));
        c.resize(Size { width: 800, height: 600 });
        c.input(UiInput::Down {
            position: Point { x: 0, y: 300 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 800, y: 300 }));
        assert!(c.eye().iter().all(|v| v.is_finite()));
    }
}
