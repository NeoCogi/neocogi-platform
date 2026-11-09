//! Example-owned virtual trackball. Pointer coordinates are physical drawable pixels.
use neocogi_platform_examples::sdk::render::{
    abi::Size,
    platform::{Point, UiInput},
};
use rs_math3d::{CrossProduct, Mat4f, Quatf, RealVector, Vec3f, Vector, basis_from_unit_rh, perspective, translate};

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
            center: Vec3f::from_array(center),
            radius,
            size,
            rotation: Quatf::identity(),
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
        self.rotation = Quatf::identity();
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
                    let dot = Vec3f::dot(&previous, &next).clamp(-1., 1.);
                    let axis = Vec3f::cross(&previous, &next);
                    // At opposite rim points choose a stable orthogonal half-turn axis.
                    let delta = if dot < -0.99999 {
                        let [_, axis, _] = basis_from_unit_rh(&previous);
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
            Vec3f::new(x, y, 0.).normalize()
        }
    }

    /// Returns right-handed perspective * view with renderer-canonical depth in [0,1].
    pub fn view_projection(&self) -> Mat4f {
        let aspect = self.size.width.max(1) as f32 / self.size.height.max(1) as f32;
        let near = self.radius * 0.001;
        let far = self.radius * 250.;
        let mut projection = perspective(FOV, aspect, near, far);
        // rs-math3d projects to OpenGL depth [-1,1]. The platform contract uses [0,1]:
        // z' = (z + w) / 2 changes only these two coefficients of this perspective matrix.
        projection.col[2].z = (projection.col[2].z - 1.) * 0.5;
        projection.col[3].z *= 0.5;
        projection * translate(Vec3f::new(self.pan[0], self.pan[1], -self.distance)) * self.rotation.mat4() * translate(-self.center)
    }

    /// Camera position in scene coordinates for view-dependent fragment lighting.
    pub fn eye(&self) -> [f32; 3] {
        let offset = Quatf::conjugate(&self.rotation).mat3() * Vec3f::new(-self.pan[0], -self.pan[1], self.distance);
        (self.center + offset).to_array()
    }

    /// Perspective ray basis for an infinitely distant skybox. Only camera rotation and
    /// aspect ratio apply: panning and distance changes cannot translate the environment.
    pub fn sky_matrix(&self) -> Mat4f {
        let tangent = (FOV * 0.5).tan();
        let aspect = self.size.width.max(1) as f32 / self.size.height.max(1) as f32;
        let rays = Mat4f::new(tangent * aspect, 0., 0., 0., 0., tangent, 0., 0., 0., 0., -1., 0., 0., 0., 0., 0.);
        Quatf::conjugate(&self.rotation).mat4() * rays
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_math3d::Vec4f;

    /// Test viewport centered on the virtual sphere and a translated world-space pivot.
    fn camera() -> Camera {
        Camera::new([1., 2., 3.], 2., Size { width: 800, height: 600 })
    }
    /// Projects a point to normalized device coordinates to test visible camera behavior.
    fn project(camera: &Camera, point: [f32; 3]) -> [f32; 3] {
        let p = camera.view_projection() * Vec4f::new(point[0], point[1], point[2], 1.);
        [p.x / p.w, p.y / p.w, p.z / p.w]
    }

    /// Library perspective retains canonical near/far depth and both screen edges across aspect ratios.
    #[test]
    fn perspective_depth_and_field_of_view_remain_canonical() {
        for size in [
            Size { width: 800, height: 600 },
            Size { width: 300, height: 800 },
            Size { width: 800, height: 800 },
        ] {
            let c = Camera::new([0.; 3], 1., size);
            let near = c.radius * 0.001;
            let far = c.radius * 250.;
            assert!(project(&c, [0., 0., c.distance - near])[2].abs() < 5e-4);
            assert!((project(&c, [0., 0., c.distance - far])[2] - 1.).abs() < 1e-6);
            let y = far * (FOV * 0.5).tan();
            let x = y * size.width as f32 / size.height as f32;
            for sign in [-1., 1.] {
                let edge = project(&c, [sign * x, sign * y, c.distance - far]);
                assert!((edge[0] - sign).abs() < 1e-6 && (edge[1] - sign).abs() < 1e-6);
            }
        }
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

    /// A skybox moves under orbit rotation, remains fixed under pan/zoom, and follows aspect.
    #[test]
    fn sky_rays_follow_rotation_without_camera_translation() {
        let mut c = camera();
        let ray = |c: &Camera| c.sky_matrix() * Vec4f::new(0.5, 0.3, 1., 0.);
        let initial = ray(&c);
        c.input(UiInput::Down {
            position: Point { x: 400, y: 300 },
            buttons: 2,
        });
        c.input(UiInput::Pointer(Point { x: 500, y: 400 }));
        c.input(UiInput::Scroll(Point { x: 0, y: -120 }));
        let same = ray(&c);
        assert_eq!([initial.x, initial.y, initial.z], [same.x, same.y, same.z]);
        c.input(UiInput::Down {
            position: Point { x: 400, y: 300 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 500, y: 400 }));
        let rotated = ray(&c);
        assert_ne!([same.x, same.y, same.z], [rotated.x, rotated.y, rotated.z]);
        c.reset();
        c.resize(Size { width: 300, height: 800 });
        assert_ne!(initial.x, ray(&c).x);
    }
}
