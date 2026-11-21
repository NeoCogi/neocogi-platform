//! Example-owned virtual trackball. Pointer coordinates are physical drawable pixels.
use neocogi_com_sdk::NC_E_OUT_OF_RANGE;
use neocogi_platform_examples::sdk::render::{
    abi::{RenderError, Size},
    platform::{Point, UiInput},
};
use rs_math3d::{CrossProduct, Mat4d, Mat4f, Quatd, RealVector, Vec3d, Vec4f, Vector, basis_from_unit_rh, perspective, translate};

/// Vertical field of view; horizontal fitting accounts for portrait windows as well.
const FOV: f64 = std::f64::consts::FRAC_PI_4;

/// Active gesture. Focus loss, resize and matching button release terminate the gesture.
enum Drag {
    /// Pointer motion has no effect.
    Idle,
    /// Left button rotates between successive virtual-sphere positions.
    Rotate(Vec3d),
    /// Right or middle button translates in the camera plane.
    Pan(Point),
}

/// Owned arcball state, mutated only by the application after synchronous event collection.
pub struct Camera {
    /// Initial scene-space rotation pivot, fixed during pan/zoom.
    center: Vec3d,
    /// Initial bounding sphere, also defines clipping and zoom limits.
    radius: f64,
    /// Drawable size used for sphere mapping and perspective aspect ratio.
    size: Size,
    /// Normalized world-to-view rotation; incremental rotations compose on the left.
    rotation: Quatd,
    /// Camera-plane translation; independent of world rotation.
    pan: [f64; 2],
    /// Positive distance from the pivot; bounded relative to the initial sphere.
    distance: f64,
    /// Current exclusive pointer gesture.
    drag: Drag,
}

/// Camera values checked and narrowed together for one immutable frame.
#[derive(Debug)]
pub(super) struct FrameCamera {
    /// Physical viewport used by both projection and frame submission.
    size: Size,
    /// Column-major world-to-clip transform with canonical depth in [0,1].
    view_projection: Mat4f,
    /// World-space eye reused by every primitive's lighting parameters.
    eye: [f32; 3],
    /// Perspective ray basis for an infinitely distant skybox.
    sky_basis: Mat4f,
}

impl FrameCamera {
    /// Returns the exact drawable used to compute this snapshot.
    pub(super) fn drawable_size(&self) -> Size {
        self.size
    }

    /// Returns the checked matrix for the vertex uniform.
    pub(super) fn view_projection(&self) -> Mat4f {
        self.view_projection
    }

    /// Returns the checked eye for view-dependent fragment lighting.
    pub(super) fn eye(&self) -> [f32; 3] {
        self.eye
    }

    /// Returns the checked sky basis, independent of pan and zoom.
    pub(super) fn sky_basis(&self) -> Mat4f {
        self.sky_basis
    }
}

impl Camera {
    /// Fits a finite positive bounding sphere in both axes with a small margin.
    pub fn new(center: [f32; 3], radius: f32, size: Size) -> Self {
        let mut camera = Self {
            center: Vec3d::from_array(center.map(f64::from)),
            radius: f64::from(radius),
            size,
            rotation: Quatd::identity(),
            pan: [0.; 2],
            distance: f64::from(radius),
            drag: Drag::Idle,
        };
        camera.reset();
        camera
    }

    /// Resets orientation, pan and distance, retaining the current window aspect ratio.
    pub fn reset(&mut self) {
        let half_angle = ((FOV * 0.5).tan() * self.aspect().min(1.0)).atan();
        self.distance = self.radius / half_angle.sin() * 1.15;
        self.rotation = Quatd::identity();
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
                    let dot = Vec3d::dot(&previous, &next).clamp(-1., 1.);
                    let axis = Vec3d::cross(&previous, &next);
                    // At opposite rim points choose a stable orthogonal half-turn axis.
                    let delta = if dot < -0.99999 {
                        let [_, axis, _] = basis_from_unit_rh(&previous);
                        Quatd::new(axis.x, axis.y, axis.z, 0.)
                    } else {
                        Quatd::new(axis.x, axis.y, axis.z, 1. + dot)
                    };
                    self.rotation = Quatd::normalize(&(Quatd::normalize(&delta) * self.rotation));
                    self.drag = Drag::Rotate(next);
                }
                Drag::Pan(previous) => {
                    let units = 2. * self.distance * (FOV * 0.5).tan() / f64::from(self.size.height.max(1));
                    self.pan[0] += (f64::from(position.x) - f64::from(previous.x)) * units;
                    self.pan[1] -= (f64::from(position.y) - f64::from(previous.y)) * units;
                    self.drag = Drag::Pan(position);
                }
            },
            UiInput::Up { buttons, .. } => match self.drag {
                Drag::Rotate(_) if buttons & 1 != 0 => self.drag = Drag::Idle,
                Drag::Pan(_) if buttons & 6 != 0 => self.drag = Drag::Idle,
                _ => {}
            },
            UiInput::Scroll(delta) => {
                let factor = (f64::from(delta.y) * 0.002).clamp(-20., 20.).exp();
                self.distance = (self.distance / self.radius * factor).clamp(0.05, 100.) * self.radius;
            }
            UiInput::Cancel => self.drag = Drag::Idle,
            _ => {}
        }
    }

    /// Keeps input and reset well defined while the zero-size drawable is suspended.
    fn aspect(&self) -> f64 {
        f64::from(self.size.width.max(1)) / f64::from(self.size.height.max(1))
    }

    /// Maps the smaller drawable axis to a unit sphere; outside points lie on its rim.
    fn sphere(&self, position: Point) -> Vec3d {
        let extent = f64::from(self.size.width.min(self.size.height).max(1));
        let x = (2. * f64::from(position.x) - f64::from(self.size.width)) / extent;
        let y = (f64::from(self.size.height) - 2. * f64::from(position.y)) / extent;
        let squared = x * x + y * y;
        if squared <= 1. {
            Vec3d::new(x, y, (1. - squared).sqrt())
        } else {
            Vec3d::new(x, y, 0.).normalize()
        }
    }

    /// Prepares the complete upload before the caller opens any frame or writes any uniform.
    pub(super) fn frame(&self) -> Result<FrameCamera, RenderError> {
        if !self.radius.is_finite() || self.radius <= 0. {
            return Err(RenderError::new(NC_E_OUT_OF_RANGE, "glTF camera requires a finite positive scene radius"));
        }
        let aspect = self.aspect();
        let view_rotation = self.rotation.mat4();
        let world_rotation = view_rotation.transpose();
        let translation = Vec3d::new(self.pan[0], self.pan[1], -self.distance);
        let eye = (self.center + world_rotation * -translation).to_array();
        check_upload_range(&eye, "eye")?;

        // Clip planes and intermediate transforms can exceed the f32 range
        // while the final view-projection matrix remains representable.
        let near = self.radius * 0.001;
        let far = self.radius * 250.;
        let mut projection = perspective(FOV, aspect, near, far);
        // rs-math3d projects to OpenGL depth [-1,1]. The platform contract uses [0,1]:
        // z' = (z + w) / 2 changes only these two coefficients of this perspective matrix.
        projection.col[2].z = (projection.col[2].z - 1.) * 0.5;
        projection.col[3].z *= 0.5;
        let view_projection = projection * translate(translation) * view_rotation * translate(-self.center);

        // Only rotation and aspect apply to the infinitely distant environment.
        let tangent = (FOV * 0.5).tan();
        let rays = Mat4d::new(tangent * aspect, 0., 0., 0., 0., tangent, 0., 0., 0., 0., -1., 0., 0., 0., 0., 0.);
        Ok(FrameCamera {
            size: self.size,
            view_projection: upload_matrix(view_projection, "view-projection")?,
            eye: eye.map(|value| value as f32),
            sky_basis: upload_matrix(world_rotation * rays, "sky basis")?,
        })
    }
}

/// Rejects nonfinite or out-of-range values before ordinary f32 rounding.
fn check_upload_range(values: &[f64], field: &str) -> Result<(), RenderError> {
    if values.iter().all(|value| value.abs() <= f64::from(f32::MAX)) {
        Ok(())
    } else {
        Err(RenderError::new(
            NC_E_OUT_OF_RANGE,
            format!("glTF camera {field} is outside the finite f32 upload range"),
        ))
    }
}

/// Verifies and narrows the complete column-major matrix without changing its layout.
fn upload_matrix(matrix: Mat4d, field: &str) -> Result<Mat4f, RenderError> {
    check_upload_range(&matrix.to_cols_array(), field)?;
    Ok(Mat4f {
        col: matrix.col.map(|axis| Vec4f::from_array(axis.to_array().map(|value| value as f32))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test viewport centered on the virtual sphere and a translated world-space pivot.
    fn camera() -> Camera {
        Camera::new([1., 2., 3.], 2., Size { width: 800, height: 600 })
    }
    /// Projects a point to normalized device coordinates to test visible camera behavior.
    fn project(camera: &Camera, point: [f64; 3]) -> [f32; 3] {
        let [x, y, z] = point.map(|axis| axis as f32);
        let p = camera.frame().unwrap().view_projection() * Vec4f::new(x, y, z, 1.);
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
            for radius in [1., 1e36] {
                let c = Camera::new([0.; 3], radius, size);
                assert_eq!(c.frame().unwrap().drawable_size(), size);
                let near = c.radius * 0.001;
                let far = c.radius * 250.;
                // Projection subtracts world-space terms on the scale of distance,
                // so allow their f32 roundoff relative to the much smaller near plane.
                let near_tolerance = (2. * f64::from(f32::EPSILON) * c.distance / near) as f32;
                let near_depth = project(&c, [0., 0., c.distance - near])[2];
                assert!(near_depth.abs() <= near_tolerance, "{size:?} radius {radius:e}: near depth {near_depth}");
                assert!((project(&c, [0., 0., c.distance - far])[2] - 1.).abs() < 1e-6);
                let y = far * (FOV * 0.5).tan();
                let x = y * c.aspect();
                for sign in [-1., 1.] {
                    let edge = project(&c, [sign * x, sign * y, c.distance - far]);
                    assert!((edge[0] - sign as f32).abs() < 1e-6 && (edge[1] - sign as f32).abs() < 1e-6);
                }
            }
        }
    }
    /// Arcball rotates around the fitted pivot, remains normalized and stops on button release.
    #[test]
    fn rotates_about_pivot_and_releases() {
        let mut c = camera();
        let old = c.frame().unwrap().eye();
        c.input(UiInput::Down {
            position: Point { x: 400, y: 300 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 550, y: 350 }));
        assert_ne!(old, c.frame().unwrap().eye());
        let p = project(&c, [1., 2., 3.]);
        assert!(p[0].abs() < 1e-6 && p[1].abs() < 1e-6 && p[2] > 0. && p[2] < 1.);
        let rotated = c.frame().unwrap().eye();
        c.input(UiInput::Up {
            position: Point { x: 550, y: 350 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 600, y: 400 }));
        assert_eq!(rotated, c.frame().unwrap().eye());
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
            let eye = c.frame().unwrap().eye();
            c.input(UiInput::Pointer(Point { x: 700, y: 100 }));
            assert_eq!(eye, c.frame().unwrap().eye());
        }
        c.resize(Size { width: 0, height: 0 });
        c.reset();
        assert!(c.frame().unwrap().eye().iter().all(|v| v.is_finite()));
        assert!(project(&c, [1., 2., 3.]).iter().all(|v| v.is_finite()));
        c.resize(Size { width: 800, height: 600 });
        c.input(UiInput::Down {
            position: Point { x: 0, y: 300 },
            buttons: 1,
        });
        c.input(UiInput::Pointer(Point { x: 800, y: 300 }));
        assert!(c.frame().unwrap().eye().iter().all(|v| v.is_finite()));
    }

    /// A skybox moves under orbit rotation, remains fixed under pan/zoom, and follows aspect.
    #[test]
    fn sky_rays_follow_rotation_without_camera_translation() {
        let mut c = camera();
        let ray = |c: &Camera| c.frame().unwrap().sky_basis() * Vec4f::new(0.5, 0.3, 1., 0.);
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

    /// Wide pan and zoom remain reversible through unrepresentable intermediate camera states.
    #[test]
    fn large_camera_controls_can_return_to_upload_range() {
        let size = Size { width: 800, height: 800 };
        let mut c = Camera::new([0.; 3], 2e36, size);
        let initial = c.frame().unwrap();
        c.input(UiInput::Scroll(Point { x: 0, y: i32::MAX }));
        assert_eq!(c.distance, c.radius * 100.);
        c.input(UiInput::Down {
            position: Point { x: 400, y: 400 },
            buttons: 2,
        });
        c.input(UiInput::Pointer(Point { x: i32::MAX, y: 400 }));
        let failure = c.frame().unwrap_err();
        assert_eq!(failure.status, NC_E_OUT_OF_RANGE);
        assert!(failure.message.contains("eye"));
        c.input(UiInput::Pointer(Point { x: 400, y: 400 }));
        c.input(UiInput::Scroll(Point { x: 0, y: i32::MIN }));
        assert_eq!(c.distance, c.radius * 0.05);
        assert!(c.frame().is_ok());
        c.resize(Size { width: 300, height: 800 });
        c.reset();
        let portrait = c.frame().unwrap();
        assert_ne!(portrait.eye(), initial.eye());
        c.resize(size);
        c.reset();
        let reset = c.frame().unwrap();
        assert_eq!(reset.eye(), initial.eye());
        assert_eq!(reset.view_projection().to_cols_array(), initial.view_projection().to_cols_array());
        assert_eq!(reset.sky_basis().to_cols_array(), initial.sky_basis().to_cols_array());
    }

    /// A finite eye does not guarantee that the projected translation can be uploaded.
    #[test]
    fn rejects_view_projection_overflow_and_invalid_bounds() {
        let size = Size { width: 64, height: 64 };
        let failure = Camera::new([2e38, 0., 0.], 1., size).frame().unwrap_err();
        assert_eq!(failure.status, NC_E_OUT_OF_RANGE);
        assert!(failure.message.contains("view-projection"));
        for radius in [0., -1., f32::INFINITY, f32::NAN] {
            let mut c = Camera::new([0.; 3], radius, size);
            c.input(UiInput::Scroll(Point { x: 0, y: -120 }));
            assert!(c.frame().unwrap_err().message.contains("finite positive scene radius"));
        }
        let limit = f64::from(f32::MAX);
        assert!(check_upload_range(&[limit, -limit], "test").is_ok());
        assert_eq!(check_upload_range(&[limit.next_up()], "test").unwrap_err().status, NC_E_OUT_OF_RANGE);
    }
}
