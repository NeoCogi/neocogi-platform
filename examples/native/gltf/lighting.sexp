; Shared lighting uniforms and region counts come from environment.rs.
(struct Environment (regions (array vec4 @REGION_COUNT@)) (settings vec4))

(uniform environment Environment fragment 1)

(texture environment_map fragment 1)

; Decode only color maps from sRGB; radiance calculations remain scene-linear.
(fn linear ((color vec3)) (returns vec3)
  (return
    (select (pow (/ (+ color (vec3 0.055)) 1.055) (vec3 2.4)) (/ color 12.92) (<= color (vec3 0.04045)))))

(fn srgb ((color vec3)) (returns vec3)
  (return
    (select
      (- (* 1.055 (pow color (vec3 (/ 1.0 2.4)))) (vec3 0.055))
      (* color 12.92)
      (<= color (vec3 0.0031308)))))

; Khronos PBR Neutral tone mapping, applied once to the summed HDR radiance.
; https://github.com/KhronosGroup/ToneMapping/tree/main/PBR_Neutral
(fn display_color ((radiance vec3)) (returns vec3)
  (var color (max (* radiance (. (. environment settings) x)) (vec3 0.0)))
  (let darkest (min (. color r) (min (. color g) (. color b))))
  (set color (- color (select 0.04 (- darkest (* (* 6.25 darkest) darkest)) (< darkest 0.08))))
  (let peak (max (. color r) (max (. color g) (. color b))))
  (if
    (> peak 0.76)
    (then
      (let compressed (- 1.0 (/ 0.0576 (- peak 0.52))))
      (set color (* color (/ compressed peak)))
      (let whiten (- 1.0 (/ 1.0 (+ 1.0 (* 0.15 (- peak compressed))))))
      (set color (mix color (vec3 compressed) (vec3 whiten)))))
  (return (srgb (clamp color (vec3 0.0) (vec3 1.0)))))

; Decode each RGBE texel before interpolation so highlights retain HDR range.
(fn environment_texel ((region vec4) (coordinate vec2) (repeat_x bool) (hdr bool)) (returns vec3)
  (var pixel (clamp coordinate (vec2 0.0) (- (. region zw) (vec2 1.0))))
  (if
    repeat_x
    (then (set (. pixel x) (- (. coordinate x) (* (floor (/ (. coordinate x) (. region z))) (. region z))))))
  (let
    sample
    (sample-level
      environment_map
      (/ (+ (+ (. region xy) pixel) (vec2 0.5)) (vec2 (dimensions environment_map)))
      0.0))
  (if (! hdr) (then (return (. sample rgb))))
  (if (== (. sample a) 0.0) (then (return (vec3 0.0))))
  (return (* (* (. sample rgb) 255.0) (exp2 (- (round (* (. sample a) 255.0)) 136.0)))))

; Nearest samples at exact pixel centers implement interpolation inside one tile.
(fn environment_tile ((index u32) (uv vec2) (repeat_x bool) (hdr bool)) (returns vec3)
  (let region (index (. environment regions) index))
  (let pixel (- (* uv (. region zw)) (vec2 0.5)))
  (let first (floor pixel))
  (let weight (- pixel first))
  (let a (environment_texel region first repeat_x hdr))
  (let b (environment_texel region (+ first (vec2 1.0 0.0)) repeat_x hdr))
  (let c (environment_texel region (+ first (vec2 0.0 1.0)) repeat_x hdr))
  (let d (environment_texel region (+ first (vec2 1.0 1.0)) repeat_x hdr))
  (return (mix (mix a b (vec3 (. weight x))) (mix c d (vec3 (. weight x))) (vec3 (. weight y)))))

; The environment faces +Z after a 90-degree rotation about glTF +Y; north is row zero.
(fn environment_uv ((direction vec3)) (returns vec2)
  (let rotated (vec3 (. direction z) (. direction y) (- (. direction x))))
  (return
    (vec2
      (+ 0.5 (/ (atan2 (. rotated z) (. rotated x)) 6.28318530718))
      (/ (acos (clamp (. rotated y) (- 1.0) 1.0)) 3.14159265359))))

(fn specular_radiance ((direction vec3) (roughness f32)) (returns vec3)
  (let level (* (clamp roughness 0.0 1.0) (f32 @LAST_SPECULAR@)))
  (let first (u32 (floor level)))
  (let second (min (+ first 1u) @LAST_SPECULAR@u))
  (let uv (environment_uv direction))
  (return
    (mix (environment_tile first uv true true) (environment_tile second uv true true) (vec3 (fract level)))))

(fn diffuse_radiance ((direction vec3)) (returns vec3)
  (return (environment_tile @DIFFUSE_REGION@u (environment_uv direction) true true)))

(fn brdf_response ((nv f32) (roughness f32)) (returns vec2)
  (return
    (. (environment_tile @BRDF_REGION@u (clamp (vec2 nv roughness) (vec2 0.0) (vec2 1.0)) false false) rg)))
