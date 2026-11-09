# NeoCogi Platform documentation

## Current contracts and architecture

- [Component ownership](architecture/COMPONENTS.md)
- [PCM playback, capture and typed capabilities](contracts/AUDIO.md)
- [GPU resources, commands and execution](contracts/GPU_RENDERING.md)
- [Shader compilation and reflection](contracts/SHADER_COMPILATION.md)
- [Drawing targets and resource groups](contracts/RENDER_SURFACES.md)
- [Native artifact and resource staging](contracts/NATIVE_LAYOUT.md)

## Examples and integration

- [UI workspace integration](contracts/UI_INTEGRATION.md)
- [Native example static/dynamic assembly](../examples/native/README.md)
- [Native glTF examples and arcball controls](../examples/native/gltf/README.md)

## Development and release

- [Rust repository tools and local-source overlays](../scripts/README.md)
- [Browser regression execution](../tests/browser/README.md)
- [Release preparation and publication](project/RELEASING.md)
- [Alpha upgrade reference, dependency compatibility and history](project/HISTORY.md)

The [COM SDK and IDL specification](https://github.com/NeoCogi/neocogi-com)
define the shared object, ownership and declaration mechanisms. UI-specific
layout, widgets, documents, appearance and composition contracts are maintained
with [NeoCogi UI](https://github.com/NeoCogi/neocogi-ui/tree/master/docs).
