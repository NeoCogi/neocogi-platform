; Explicit std140-compatible blocks match scene.rs and material.rs packing.
(struct Frame (view_projection mat4))

(struct Skin (joints (array mat4 64)))

(uniform frame Frame vertex 0)

(uniform skin Skin vertex 1)

(struct Vertex
  (position vec3 (location 0))
  (normal vec3 (location 1))
  (uv vec2 (location 2))
  (joints uvec4 (location 3))
  (weights vec4 (location 4)))

(struct Surface
  (clip vec4 (builtin position))
  (position vec3 (location 0))
  (normal vec3 (location 1))
  (uv vec2 (location 2)))

(vertex vs ((vertex Vertex)) (returns Surface)
  (let
    transform
    (+
      (+
        (+
          (* (index (. skin joints) (. (. vertex joints) x)) (. (. vertex weights) x))
          (* (index (. skin joints) (. (. vertex joints) y)) (. (. vertex weights) y)))
        (* (index (. skin joints) (. (. vertex joints) z)) (. (. vertex weights) z)))
      (* (index (. skin joints) (. (. vertex joints) w)) (. (. vertex weights) w))))
  (let position (* transform (vec4 (. vertex position) 1.0)))
  ; Cofactors apply inverse-transpose normals without a native matrix inverse.
  (let a (. (index transform 0) xyz))
  (let b (. (index transform 1) xyz))
  (let c (. (index transform 2) xyz))
  (let determinant (dot a (cross b c)))
  (var normal (. vertex normal))
  (if
    (> (abs determinant) 0.0000001)
    (then (set normal (/ (* (mat3x3 (cross b c) (cross c a) (cross a b)) normal) determinant))))
  (var output Surface)
  (set (. output clip) (* (. frame view_projection) position))
  (set (. output position) (. position xyz))
  (set (. output normal) normal)
  (set (. output uv) (. vertex uv))
  (return output))

(struct Material (base vec4) (emissive vec4) (parameters vec4) (eye vec4) (tiles (array vec4 4)))

(uniform material Material fragment 0)

(texture material_map fragment 0)

; Decode sRGB before interpolating color texels; keep each sample within its atlas tile.
(fn material_texel ((tile vec4) (coordinate vec2) (color bool)) (returns vec3)
  (let pixel (- coordinate (* (floor (/ coordinate (. tile zw))) (. tile zw))))
  (let
    value
    (.
      (sample-level
        material_map
        (/ (+ (+ (. tile xy) pixel) (vec2 0.5)) (vec2 (dimensions material_map)))
        0.0)
      rgb))
  (if color (then (return (linear value))))
  (return value))

(fn material_sample ((index u32) (uv vec2) (color bool)) (returns vec3)
  (let tile (index (. material tiles) index))
  (let pixel (- (* (fract uv) (. tile zw)) (vec2 0.5)))
  (let first (floor pixel))
  (let weight (- pixel first))
  (let a (material_texel tile first color))
  (let b (material_texel tile (+ first (vec2 1.0 0.0)) color))
  (let c (material_texel tile (+ first (vec2 0.0 1.0)) color))
  (let d (material_texel tile (+ first (vec2 1.0 1.0)) color))
  (return (mix (mix a b (vec3 (. weight x))) (mix c d (vec3 (. weight x))) (vec3 (. weight y)))))

(fragment fs ((surface Surface)) (returns vec4 (location 0))
  (let base (* (material_sample 0u (. surface uv) true) (. (. material base) rgb)))
  (let mr (material_sample 1u (. surface uv) false))
  (let metallic (clamp (* (. mr b) (. (. material parameters) x)) 0.0 1.0))
  (let roughness (clamp (* (. mr g) (. (. material parameters) y)) 0.045 1.0))
  (let sample_normal (- (* (material_sample 2u (. surface uv) false) 2.0) (vec3 1.0)))
  (var n (normalize (. surface normal)))
  (let px (dpdx (. surface position)))
  (let py (dpdy (. surface position)))
  (let ux (dpdx (. surface uv)))
  (let uy (dpdy (. surface uv)))
  (let determinant (- (* (. ux x) (. uy y)) (* (. ux y) (. uy x))))
  (if
    (> (abs determinant) 0.0000001)
    (then
      (let t (normalize (/ (- (* px (. uy y)) (* py (. ux y))) determinant)))
      (let b (normalize (/ (- (* py (. ux x)) (* px (. uy x))) determinant)))
      (set
        n
        (normalize
          (+
            (+
              (* (* t (. sample_normal x)) (. (. material parameters) z))
              (* (* b (. sample_normal y)) (. (. material parameters) z)))
            (* n (. sample_normal z)))))))
  (let v (normalize (- (. (. material eye) xyz) (. surface position))))
  (let nv (max (dot n v) 0.001))
  (let response (brdf_response nv roughness))
  (let f0 (mix (vec3 0.04) base (vec3 metallic)))
  (let fresnel (+ f0 (* (- (max (vec3 (- 1.0 roughness)) f0) f0) (pow (- 1.0 nv) 5.0))))
  (let single (+ (* fresnel (. response x)) (vec3 (. response y))))
  (let missing (max (- (- 1.0 (. response x)) (. response y)) 0.0))
  (let average (+ f0 (/ (- (vec3 1.0) f0) 21.0)))
  (let multiple (/ (* (* missing single) average) (max (- (vec3 1.0) (* average missing)) (vec3 0.001))))
  (let reflected (+ single multiple))
  (let
    diffuse
    (* (* (* (diffuse_radiance n) base) (- 1.0 metallic)) (max (- (vec3 1.0) reflected) (vec3 0.0))))
  (let specular (* (specular_radiance (reflect (- v) n) roughness) reflected))
  (let emissive (* (material_sample 3u (. surface uv) true) (. (. material emissive) rgb)))
  (let color (+ (* (* (+ diffuse specular) (. mr r)) (. (. environment settings) y)) emissive))
  (return (vec4 (display_color color) 1.0)))
