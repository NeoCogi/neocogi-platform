; Inverse-rotation perspective rays contain no camera translation.
(struct SkyCamera (rays mat4))

(uniform sky_camera SkyCamera vertex 0)

(struct Sky (clip vec4 (builtin position)) (ray vec3 (location 0)))

(vertex vs ((index u32 (builtin vertex_index))) (returns Sky)
  (let corners ((array vec2 3) (vec2 (- 1.0) (- 1.0)) (vec2 3.0 (- 1.0)) (vec2 (- 1.0) 3.0)))
  (let point (index corners index))
  (var output Sky)
  (set (. output clip) (vec4 point 1.0 1.0))
  (set (. output ray) (. (* (. sky_camera rays) (vec4 point 1.0 0.0)) xyz))
  (return output))

(fragment fs ((sky Sky)) (returns vec4 (location 0))
  (let radiance (* (specular_radiance (normalize (. sky ray)) 0.25) (. (. environment settings) y)))
  (return (vec4 (display_color radiance) 1.0)))
