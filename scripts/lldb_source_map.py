"""Restore local source lookup without embedding build paths into native binaries.

CodeLLDB imports this module after creating its target, before launching it.
The pinned Cargo emits a version-1 .trim-paths.json beside the executable; its
remaps cover workspace, generated, registry and rebuilt standard-library sources.
Those local debugging files remain outside the shipped lib/assets layout.
"""
import json
from pathlib import Path
import shlex


def source_map(executable: Path) -> list[tuple[str, str]]:
    """Read Cargo's inverse path mappings; an ordinary untrimmed target needs none."""
    sidecar = executable.with_name(executable.name + ".trim-paths.json")
    if not sidecar.is_file():
        return []
    document = json.loads(sidecar.read_text(encoding="utf-8"))
    if not isinstance(document, dict) or document.get("v") != 1 or not isinstance(document.get("remaps"), list):
        raise ValueError(f"unsupported Cargo source-map format: {sidecar}")
    mappings = []
    for mapping in document["remaps"]:
        if (not isinstance(mapping, dict)
                or not all(isinstance(mapping.get(key), str) and mapping[key] for key in ("from", "to"))):
            raise ValueError(f"invalid Cargo source mapping: {sidecar}")
        mappings.append((mapping["from"], mapping["to"]))
    return mappings


def __lldb_init_module(debugger, _internal_dict):
    """Apply the selected target's maps; preserve explicit debugger mappings first."""
    import lldb

    target = debugger.GetSelectedTarget()
    if not target.IsValid():
        raise RuntimeError("import lldb_source_map after creating the debug target")
    executable = target.GetExecutable()
    path = Path(executable.GetDirectory()) / executable.GetFilename()
    mappings = source_map(path)
    if mappings:
        # These are LLDB command arguments, never shell commands. Quoting also
        # preserves source paths containing spaces, backslashes and apostrophes.
        arguments = " ".join(shlex.quote(path) for pair in mappings for path in pair)
        result = lldb.SBCommandReturnObject()
        debugger.GetCommandInterpreter().HandleCommand("settings append target.source-map " + arguments, result)
        if not result.Succeeded():
            raise RuntimeError(result.GetError())
