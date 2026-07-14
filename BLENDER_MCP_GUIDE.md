# Blender MCP and Headless Review Workflow

Purpose: future Roady player-car shape iteration. Blender artifacts are prototypes/reference only; Roady continues to ship procedural Bevy geometry unless that policy is deliberately changed.

Research verified from primary sources on 2026-07-13. No Blender binary, add-on, MCP server, or downloaded asset was installed or executed while preparing this guide.

## Decision: MCP is interactive, rendering can be headless

The popular Blender MCP project is [`ahujasid/blender-mcp`](https://github.com/ahujasid/blender-mcp). Pin both server and add-on to commit:

```text
6e99eb5a442b83766a5796975ec7bb5bfc791341
```

Its add-on explicitly refuses true background Blender:

```python
if bpy.app.background:
    print("BlenderMCP: cannot start server in background mode (blender -b) ...")
    return
```

Therefore use two separate paths:

| Task | Supported path |
|---|---|
| LLM-assisted interactive editing | GUI Blender + BlenderMCP add-on + MCP stdio server |
| Deterministic unattended review renders | `blender.exe --background --python render_roady_turntable.py` without MCP |
| BlenderMCP add-on under `--background` | Unsupported |
| Linux `xvfb-run -a blender` | Experimental virtual GUI, not truly headless; not a Windows solution |
| Patched add-on bypassing the guard | Unsupported; command dispatch still depends on Blender timers |

## Roady constraints

Current project toolchain:

- Rust 1.95.0, edition 2024
- Bevy 0.19
- Native and `wasm32-unknown-unknown`/WebGL2
- Car nose/front is local **Roady `-Z`**
- Negative Roady Z wheels are front wheels
- Current visible/collision footprint is 1.12 x 2.00 Roady units

Do not commit or ship `.blend`, `.fbx`, `.obj`, `.glb`, `.gltf`, generated textures, or downloaded model assets. Keep prototypes outside the repository, e.g. `C:\RoadyBlender`. Port accepted proportions back into `src/car.rs` using Bevy primitives or deterministic generated meshes.

## Reproducibility manifest

Before actual use, select one exact official Blender portable ZIP and record:

```text
Blender exact patch: <fill>
Archive SHA-256: <fill>
blender.exe --version: <fill>
uv exact version: <fill>
Python for MCP server: 3.11
BlenderMCP commit: 6e99eb5a442b83766a5796975ec7bb5bfc791341
Socket: localhost:9876
Telemetry: disabled
```

BlenderMCP has no tagged matched server/add-on release at the research date. GitHub `pyproject.toml` reports 1.6.0 while PyPI reports 1.6.4, and the PyPI wheel does not include root-level `addon.py`. Prefer the pinned Git commit for both components.

`uvx --from git+...@commit` pins BlenderMCP source but does not consume the repository's `uv.lock`; transitive dependencies may still resolve differently. For stronger reproduction, use a checkout at the exact commit and `uv run --frozen` after validating the Windows client command.

## Windows setup

### Blender

1. Download an exact portable Windows ZIP from Blender's official archive.
2. Extract to `C:\RoadyBlender\blender-<exact-version>`.
3. Record its hash:

```powershell
Get-FileHash C:\Downloads\blender-<exact-version>-windows-x64.zip -Algorithm SHA256
$Blender = "C:\RoadyBlender\blender-<exact-version>\blender.exe"
& $Blender --version
```

### uv/uvx

Official installer:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
```

Verify from a new terminal:

```powershell
$Uv  = "$env:USERPROFILE\.local\bin\uv.exe"
$Uvx = "$env:USERPROFILE\.local\bin\uvx.exe"
& $Uv --version
& $Uvx --version
```

Use absolute executable paths in GUI MCP clients; they often do not inherit terminal `PATH` changes.

### Add-on

Download `addon.py` from the pinned commit:

```text
https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/addon.py
```

In GUI Blender:

1. Edit -> Preferences -> Add-ons -> Install.
2. Select pinned `addon.py` and enable **Interface: Blender MCP**.
3. In a 3D View press `N`, open **BlenderMCP**, confirm port 9876, and start its listener.

## MCP client configuration

Use an absolute `uvx.exe` path and source pin:

```json
{
  "mcpServers": {
    "blender": {
      "command": "C:\\Users\\YOU\\.local\\bin\\uvx.exe",
      "args": [
        "--python",
        "3.11",
        "--from",
        "git+https://github.com/ahujasid/blender-mcp@6e99eb5a442b83766a5796975ec7bb5bfc791341",
        "blender-mcp"
      ],
      "env": {
        "BLENDER_HOST": "localhost",
        "BLENDER_PORT": "9876",
        "DISABLE_TELEMETRY": "true",
        "UV_PYTHON_PREFERENCE": "only-managed"
      }
    }
  }
}
```

Claude Desktop Windows config:

```text
%APPDATA%\Claude\claude_desktop_config.json
```

After changes, fully quit the tray process and restart. Logs:

```text
%APPDATA%\Claude\logs\mcp.log
%APPDATA%\Claude\logs\mcp-server-blender.log
```

Use equivalent stdio MCP configuration in Cursor/VS Code. Run only one BlenderMCP server/client instance at a time.

## Startup and health checks

Reliable order:

1. Start GUI Blender with a disposable/versioned prototype.
2. Enable the add-on and start/confirm its localhost listener.
3. Verify port 9876.
4. Start/restart the MCP client.
5. Confirm Blender tools appear.
6. Invoke a read-only scene-information tool before mutations.
7. Save a separate copy before allowing generated Python execution.

Socket check:

```powershell
Test-NetConnection 127.0.0.1 -Port 9876
Get-NetTCPConnection -LocalPort 9876 -State Listen |
  Format-Table LocalAddress, LocalPort, OwningProcess
```

A successful TCP test proves only that a listener exists. End-to-end health requires a successful read-only MCP scene query.

Common failures:

- `spawn uvx ENOENT`: use absolute `uvx.exe`, restart client fully.
- MCP tools but no Blender connection: start GUI Blender/add-on listener.
- Listener absent under `--background`: expected; unsupported.
- `No 3D viewport found`: viewport screenshots require GUI context; use render output in background mode.
- 180-second timeout: simplify the request and inspect Blender/client logs.
- Wrong Blender instance: inspect listener PID and close duplicates.
- Address in use: close stale instance or set the same alternate port on both sides.

`BLENDER_HOST` changes only the MCP server's outbound destination. The stock add-on binds localhost, so remote operation is unsupported without unreviewed changes.

## Deterministic headless review loop

This route does not use MCP:

```powershell
$Blender = "C:\RoadyBlender\blender-<exact-version>\blender.exe"
$Blend   = "C:\RoadyBlender\prototypes\roady-car-iteration.blend"
$Script  = "C:\RoadyBlender\scripts\render_roady_turntable.py"
$Output  = "C:\RoadyBlender\renders\roady-car-iteration"

& $Blender `
  $Blend `
  --background `
  --python-exit-code 1 `
  --python $Script `
  -- `
  --output-dir $Output `
  --collection RoadyCar `
  --resolution 512 `
  --samples 64

if ($LASTEXITCODE -ne 0) { throw "Blender review render failed" }
```

The render script should:

- use a collection named `RoadyCar`;
- set CPU Cycles, fixed samples/seed, one thread, adaptive sampling off, denoising off;
- create an orthographic camera and fixed lights;
- create the ground directly in Blender XY (horizontal because Blender Z is up), not by rotating ambiguously;
- render eight named views and write `manifest.json` with Blender version, `.blend` hash, bounds, settings, and camera locations.

### Coordinate convention

For a Blender review scene:

```text
Blender (x, y, z) = Roady (x, -z, y)
```

Thus Roady nose `-Z` becomes Blender `+Y`. A front view camera is on Blender `+Y` looking toward the origin/`-Y`. In native Roady coordinates, a front camera would be at `-Z` looking `+Z`.

Eight views clockwise from front:

```text
000_front.png
045_front_right.png
090_right.png
135_rear_right.png
180_rear.png
225_rear_left.png
270_left.png
315_front_left.png
manifest.json
```

Verify:

```powershell
$Images = Get-ChildItem $Output -Filter "*.png" | Sort-Object Name
if ($Images.Count -ne 8) { throw "Expected 8 renders" }
Get-FileHash $Images.FullName -Algorithm SHA256
Get-Content (Join-Path $Output "manifest.json")
```

The script and exact Blender enum/property names must receive a one-time execution check against the selected patch release. Static review cannot promise byte-identical output across Blender versions, OSes, CPUs, drivers, or color-management changes.

## Review criteria

Review all eight views at gameplay thumbnail size:

- unmistakable front/rear silhouette;
- connected greenhouse/body/fascia with no floating pieces;
- readable wheel placement inside the 1.12 x 2.00 footprint;
- no running-gear/body clipping under steering, pitch, or roll;
- body upper hierarchy can pitch/roll while chassis/wheels/shadows remain root-level;
- negative Roady Z wheels remain front/steering wheels;
- shape remains legible in Roady's fixed-isometric camera.

Current reference geometry:

```text
Body half-extents: (0.5, 0.25, 1.0)
Body center Y: 0.35
Chassis: 0.82 x 0.16 x 1.55
Wheel radius/width: 0.15 / 0.18
Wheel X: +/-0.47
Wheel Z: +/-0.66
Bumper width/depth/Z: 0.94 / 0.08 / +/-0.90
```

Wheel roles:

```text
(+0.47, +0.66) right-rear
(-0.47, +0.66) left-rear
(+0.47, -0.66) right-front
(-0.47, -0.66) left-front
```

## Porting accepted shapes back to Rust

Do not import the Blender model. Measure accepted changes and convert:

```text
Roady x = Blender x
Roady y = Blender z
Roady z = -Blender y
```

Prefer `Cuboid`, `Cylinder`, `Plane3d`, indexed spheres, explicit transforms, and shared materials. For the smooth body, preserve deterministic topology where possible and bake dimensions into vertices rather than non-uniform transform scale. Recompute analytic or deterministic normals.

Preserve component ownership:

- `CarBody`/`BodyMotion` on painted body child;
- cabin/glass/fascia/lights under body child;
- chassis/rockers/axles/wheels/shadows under car root;
- `FrontWheel` on negative Roady Z pair;
- collision footprint aligned with visible geometry.

After porting:

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo check --locked --target wasm32-unknown-unknown
trunk build --release
```

Then review native and WebGL2 at eight comparable directions, steering lock, wheel spin, pitch/roll, brake lights, clipping, shadow, and collision footprint.

## Security boundaries

BlenderMCP exposes arbitrary Python execution via `exec(code, {"bpy": bpy})` with the permissions of the user running Blender. It can access files, subprocesses, and networks.

- Treat the MCP client and prompts as trusted code input.
- Keep port 9876 on localhost; no tunnel, LAN bind, or port forward.
- Use disposable `.blend` copies.
- Keep prototypes outside the repository.
- Disable telemetry.
- Do not put API keys in repository/config examples or prompts.
- Do not enable Poly Haven, Sketchfab, Hyper3D, or Hunyuan asset services for Roady shipping geometry.
- Prefer direct background rendering for unattended review; it avoids the open MCP socket/tool surface.

## Primary sources

- BlenderMCP repository: https://github.com/ahujasid/blender-mcp
- Pinned README: https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/README.md
- Pinned add-on: https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/addon.py
- Pinned server: https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/src/blender_mcp/server.py
- Pinned package metadata: https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/pyproject.toml
- Pinned lockfile: https://raw.githubusercontent.com/ahujasid/blender-mcp/6e99eb5a442b83766a5796975ec7bb5bfc791341/uv.lock
- PyPI metadata: https://pypi.org/pypi/blender-mcp/json
- Blender CLI: https://docs.blender.org/manual/en/latest/advanced/command_line/arguments.html
- Blender `bpy.app.background`: https://docs.blender.org/api/current/bpy.app.html
- Blender timers: https://docs.blender.org/api/current/bpy.app.timers.html
- Blender render operators: https://docs.blender.org/api/current/bpy.ops.render.html
- uv installation: https://docs.astral.sh/uv/getting-started/installation/
- uv tool/source pinning: https://docs.astral.sh/uv/guides/tools/
- MCP local server/client documentation: https://modelcontextprotocol.io/docs/develop/connect-local-servers
- MCP Python SDK: https://github.com/modelcontextprotocol/python-sdk

## Known uncertainties before first use

- Select and hash an exact Blender patch release.
- Execute and validate the render script against that patch.
- Recheck BlenderMCP commit/PyPI/add-on compatibility before upgrades.
- Confirm client-specific config schema against the installed client version.
- Linux Xvfb and remote-host paths were not tested and are not recommended for the Windows workflow.
