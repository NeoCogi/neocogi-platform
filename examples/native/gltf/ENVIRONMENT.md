# HDR lighting data

The example uses **Cannon Exterior**, © 2024 Greg Zaal, CC0-1.0, from Khronos
glTF-Sample-Environments revision `3a030b54c2e4f0f4d469eec2f98f47398c9a2a30`.
It is the reference viewer's default HDR environment, provided separately from
the helmet model. [Asset attribution and license](Cannon_Exterior-LICENSE.md).

`python3 scripts/fetch_gltf_examples.py` fetches the HDR alongside the two GLBs,
verifies its pinned SHA-256 and installs it atomically. No Cargo build or ordinary
test needs network access. The first load computes lighting on the CPU and caches
it as `assets/Cannon_Exterior.ibl`; subsequent loads reuse that prepared atlas.

`environment.rs` owns the preparation contract. It decodes floating-point Radiance
pixels, supports positive equirectangular sources up to 4096 × 2048 with radiance
in [0,65504], and checks atlas size against the renderer's 64 MiB image limit.
The atlas contains the original panorama, eight GGX importance-filtered levels
(roughness 1/8 through 1), a cosine-weighted diffuse convolution storing
irradiance/pi, and the integrated Smith-GGX/Schlick scale/bias lookup. Sampling
uses deterministic Hammersley sequences. Filtered maps are 256 × 128; diffuse
is 32 × 16; the BRDF table is 64 × 64. Each integral uses 128 samples. This is
finite numerical integration, rather than a claim of pixel identity with the
reference viewer's GPU filtering implementation.

Radiance tiles use RGBE8 to preserve highlights above one through the existing
RGBA8 texture contract. They must be decoded **before** interpolation. Longitude
wraps and latitude clamps at the polar rows. Source row zero is north, longitude
zero faces +X, and shader environment rotation supplies the reference +Z-facing
90-degree orientation. BRDF R/G are ordinary linear UNORM coefficients, not RGBE.
All shader-visible region rectangles and uniform byte counts derive from the
Rust preparation module. No cube, floating-point texture, compute or native mip
API is required.

The local cache carries a format version, a SHA-256 key over the source and all
quality settings, and a payload checksum. Read validates the bounded exact file
length, image dimensions and every rectangle before exposing it to a shader.
Stale, truncated, corrupt or invalid caches are rebuilt from the HDR. Complete
cache writes publish by rename; a read-only asset directory still permits use of
the prepared in-memory atlas. Source decode/preparation failures remain explicit
application errors; the viewer must not silently substitute approximate lights.

```sh
cargo test --locked -p neocogi-platform-examples --test gltf_environment
cargo test --locked -p neocogi-platform-examples --test gltf_environment -- --include-ignored
```

Offline regressions cover real HDR decoding, RGBE dynamic range, longitude seams,
polar clamping, constant-environment energy, roughness convolution, bounded BRDF
responses and raw cache corruption/bounds. The opt-in asset test prepares Cannon
Exterior and checks exact persisted atlas reuse.
