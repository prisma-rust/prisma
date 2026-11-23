# Comment migration report (2026-11-24)

Every non-rustdoc comment removed by the repository-wide comment-only sweep is
listed below with its original file, line and text. Facts that still matter should
be migrated into rustdoc (`///` / `//!`) or a dedicated document under `docs/`.
This report is the recovery record; the sweep itself changed no code tokens.

## crates/prism-anim/src/blend.rs

- 52: `out`, `a`, and `b` are guaranteed to be distinct poses (borrow checker guarantees it),
- 53: so we can compute and write in place without an intermediate buffer.
- 107: Inverse of the reference pose: rotation takes conjugate, scale takes reciprocal.
- 320: Linear scan: the sample count of a blend space is hand-filled by the author (single digits to a few tens), binary search not worth it.
- 333: Unsorted / NaN-containing input: clamp to the last sample, instead of returning an out-of-bounds index.

## crates/prism-anim/src/blend_space2d.rs

- 138: Sample all three sample poses into scratch, then blend by weight.
- 290: Weights in descending order: put the largest first, so `t = w / total` does not start from 0.
- 347: Super triangle: cover the entire bounding box, then delete triangles referencing it later.
- 350: Counter-clockwise order: the circumcircle test depends on triangle orientation.
- 371: Shared edges appear in the opposite direction in adjacent triangles and cancel out;
- 372: the remainder is the boundary of the new triangle.
- 395: Delete the super triangle's vertices and keep a deterministic order (sort by smallest index + the other two).
- 416: Translate to p to avoid loss of precision from subtracting large coordinates.
- 422: For counter-clockwise triangles, a positive determinant means inside the circumcircle;
- 423: co-circular (≈0) doesn't count, to avoid infinite recursion.

## crates/prism-anim/src/blend_space2d_tests.rs

- 69: Square split along diagonal a-c into two triangles; centre lies on this shared edge, both triangles yield
- 70: the same result: a(0) and c(20) each half.
- 99: Pull out from centre toward +x: should stop at x = 10 edge, still blend by y = 5.
- 214: Two clips have different durations; same normalized phase each samples its own time point.
- 278: Both segments reach midpoint of their own duration: 1 * 0.5 and 3 * 0.5, mixed to 1.0.

## crates/prism-anim/src/blend_stack_tests.rs

- 48: 0 -> 5 (first 50%) -> 12.5 (second 50% of 5..20)
- 75: joint 0: 1 + (2 - 0) = 3; joint 1 is masked out.
- 130: The first two are dropped; the last is clamped to 1.
- 155: Missing indices default to zero for a partial mask.
- 157: The empty mask means "all bones".

## crates/prism-anim/src/blend_tests.rs

- 41: 170° → 190° (= −170°) differ by only 20°, slerp must take the 20° path, not the 340° detour.
- 53: Midpoint of the shortest path = 180° (around Y).
- 127: Single sample: both left and right point to itself.
- 163: Intentionally out of order: the author's fill order shouldn't affect the result.
- 206: 75 = midpoint of idle(0) and walk(150).
- 224: The two clips have different durations (1s / 2s); at the same normalized phase each samples its own keyframe.
- 281: Phase 0.5: short samples 0.5s (x=0.5), long samples 1.0s (x=1.5), blended at position 0.5 → x=1.0.
- 352: Reference pose is 0°, overlay sampled to 90° → increment is 90°; then rotate another 90° on top of base pose 30°.
- 366: Translation is the difference relative to the reference pose: reference is at origin, so just add the overlay's displacement directly.
- 378: Increment = 90° − 30° = 60°, half = 30°.
- 400: When reference scale is 0 (bad data), treat as 1 instead of dividing to inf.
- 410: Reference scale 0 → increment treated as 1 (unchanged), not divided to inf / collapsed to 0.
- 474: At 0.25, A is the leading clip: B's phase is mapped from A to 0.5; A itself stays at 0.25.
- 475: A = 2.5, B = 10, blend weight 0.75 / 0.25.
- 481: After the parameter moves to B's end, leader becomes B; B's phase is no longer mapped (0.25), x = 5.

## crates/prism-anim/src/clip.rs

- 251: `to < from` can only be loop wrap-around (after `rem_euclid`), scan in two segments.
- 420: Only one keyframe covers this time point.

## crates/prism-anim/src/clip_tests.rs

- 36: 1.5 falls in the middle of the second segment ((2,0,0) → (0,4,0)).
- 40: First segment.
- 57: Endpoints hold.
- 111: Position was not written (the clip doesn't move the position channel).
- 200: 0.0 / 1.0 each appear twice → keep one each; NaN dropped.
- 250: Start is an open interval: 0.5 already fired that frame, paused on it shouldn't replay.
- 252: One frame crossing two doesn't miss either.
- 254: No events / non-finite / zero duration are all safe.
- 263: 0.9 → 1.0 → 0.1: each segment hits one.
- 265: Only cross the end.
- 267: Only cross the start (point 0 itself must also be able to trigger).

## crates/prism-anim/src/foley.rs

- 155: Suppressed within cooldown window.
- 159: Allowed once cooldown expires; opposite foot is independent.

## crates/prism-anim/src/graph_eval.rs

- 642: Per-sample speeds: this node maintains independent playheads, phase array in node.clips order.
- 886: MontagePlayer takes precedence: segments played/enqueued on the game side override
- 887: the static slot clip authored by the author.

## crates/prism-anim/src/graph_eval_tests.rs

- 137: A second evaluation on the same frame is cached (no double time advance),
- 138: and a new frame still returns the same static clip.
- 555: Compile-time guarantees exhaustive enum match; at runtime confirm Slots / CachedPose are also transparent pass-through.
- 713: Weight 0 keeps the base untouched.
- 955: run's playhead speed is 2: after 0.25s it reaches phase 0.5, linear track x = 10.
- 959: another 0.25s exactly back to 0.
- 963: back to shared phase after clearing config.

## crates/prism-anim/src/montage.rs

- 130: Looped Montages return to the end every cycle; fading out by remaining time would
- 131: flash periodically, so blend-out is applied only on single-shot Montages.
- 328: blend-in is wall-clock duration, not scaled by playback speed; paused tick returns early
- 329: so it stays frozen.
- 358: BranchIn is an explicit segment jump: no longer loop-wrapping, and not scanning
- 359: Notify in the target segment within the same frame.

## crates/prism-anim/src/montage_tests.rs

- 149: start time wraps to 0.75; sampling the constant track still gives 1.
- 219: Falls back to start_time when the segment name doesn't exist, instead of getting stuck at 0 or panicking.
- 427: Looped Montages don't flash at the end of every cycle.

## crates/prism-anim/src/pose.rs

- 52: `reserve` guarantees `capacity >= len + count` (len is already 0 here); a no-op when capacity suffices.

## crates/prism-anim/src/skeleton_tests.rs

- 64: The inverse of the bind matrix exactly cancels the world matrix: in the reference pose the palette is the identity matrix.
- 74: The parent bone rotates 90° around Y: the child's world position moves from +X to -Z (right-handed, +Y up).
- 98: The vertex is at the bind pose origin; now it should be lifted to y=5.
- 101: The child bone also translates 5m along.
- 126: Matrix round-trip.
- 143: Half of 180° is 90°.

## crates/prism-anim/src/skinning_tests.rs

- 35: 0.25 / 0.25 (sum 0.5) should be equivalent to 0.5 / 0.5.
- 73: +X rotated 90° around Z → +Y.
- 81: Bone rotated 90° around Z, vertex at (1,0,0) → should land at (0,1,0).

## crates/prism-anim/src/state_machine_tests.rs

- 36: Speed 5 satisfies both "> 0.1" and "> 4.0", but idle→walk is written first, so walk is taken first.
- 44: From walk only walk→run satisfied.
- 51: Run state speed still 5: run→walk condition (< 4) not met.
- 61: Missing parameters also don't transition (data not filled in shouldn't let character move randomly).
- 66: NaN also doesn't hold.
- 95: Re-evaluation won't retrigger.
- 116: Played to 40%: condition met but not yet earliest interruption point.
- 118: Played to end: transition allowed.
- 125: Unknown duration (0 / NaN) treated as "played to end", otherwise single clip gets stuck at end.
- 148: Self-loop: never chosen.
- 150: Only from b: not effective on a.
- 152: Out-of-bounds target: skip.
- 160: Out-of-bounds source state doesn't panic.
- 261: Points to non-existent state: cooker wouldn't write such data, but bad assets shouldn't fail the whole machine.
- 300: Idle only played 10%: min_state_fraction blocks.
- 304: Played to 30%: transition to Run, 0.2s.
- 311: In Run press Stop: trigger consumed, direction back to Idle.

## crates/prism-anim/src/sync.rs

- 105: Find the segment of the leader clip containing `phase`; `phase` before the first marker belongs
- 106: to the last segment (phase + 1 compared with the wrap-around segment).

## crates/prism-anim/src/sync_tests.rs

- 44: Leader segment: 0.75 -> 1.25 (i.e. 0.25); phase 0.1 is at 70% of this segment.

## crates/prism-anim-ik/src/foot.rs

- 158: The upper bone of each leg hangs from the pelvis; the pelvis itself is
- 159: never rotated here (both legs share it and would be applied twice).
- 183: 1. Resolve each foot against the ground and smooth its target.
- 199: Anything above `max_step_up` is a wall/ceiling, and anything
- 200: below `max_step_down` is a hole: both unplant the foot.
- 228: 2. Pelvis correction: lower it just enough for the most over-stretched
- 229: leg. Raising the pelvis would only put planted feet further out of
- 230: reach, so a foot that is above the stance is left to the IK (the knee
- 231: simply bends further).
- 232: Measure against the un-adjusted pelvis, not the pose we already moved:
- 233: otherwise the correction would be recomputed as zero as soon as the
- 234: previous frame's drop made the leg reach and the pelvis would bounce.
- 262: 3. Bend each leg onto its target.

## crates/prism-anim-ik/src/foot_tests.rs

- 292: dt = 0 snaps the smoothing but the foot is still resolved against the
- 293: empty ground, so it unplants while keeping the last target position.

## crates/prism-anim-ik/src/pbik.rs

- 25: Capture rest bone lengths.
- 35: 1. Pull effectors toward targets.
- 45: 2. Enforce bone-length distance constraints.

## crates/prism-anim-ik/src/two_bone.rs

- 116: Keep a sliver of bend at both extremes: a perfectly straight chain has an
- 117: undefined bend plane and `from_rotation_arc` would return identity anyway.
- 126: Bend plane: the pole projected perpendicular to the target direction.
- 127: A pole parallel to the direction cannot define a plane, so fall back to
- 128: the axis least aligned with it (stable and deterministic per target).
- 143: Law of cosines at the upper joint.
- 212: Local delta for a bone: `parent_world^-1 * world_delta * parent_world`,
- 213: so that `world_delta * parent_world * local` composes correctly.
- 229: The middle bone's parent is the upper bone, whose world rotation changed.
- 242: Report where the chain actually ended up: slerp on the two rotations is not
- 243: the same curve as lerping the positions.

## crates/prism-anim-ik/src/two_bone_tests.rs

- 77: Straight down: the knee sits between hip and foot on the same line.
- 84: Unequal bones so the folded chain still has a non-trivial minimum reach
- 85: (0.6 - 0.4 = 0.2); a target inside that radius cannot be reached.
- 110: Target straight down, pole straight down as well: no bend plane.
- 123: Half way: the end sits between the original position and the target.
- 132: Zero-length upper bone.
- 134: Zero-length lower bone.
- 136: Target on top of the upper joint.
- 138: Non-finite target / pole.
- 140: Zero / negative / NaN weight solves nothing.
- 163: The hip itself does not move (only rotations are written).

## crates/prism-anim-retarget/src/lib.rs

- 271: Fold the leg to 80 % of its rest length, as a crouch would.
- 289: The source rest pose already has the arm out at 90°, the target rest
- 290: pose does not, so a rest-pose frame must leave the target at rest.

## crates/prism-app/examples/hosted_png.rs

- 46: Render thread is async: per frame request_frame -> wait readback (wait up to 5s/frame).

## crates/prism-app/examples/pass_dump.rs

- 90: `host_tick(false)` skips fixed_update / update / late_update, only runs hierarchy propagation
- 91: and frame extraction: camera, lights, transforms all stay at authored values, no simulation drift between frames.

## crates/prism-app/src/android_assets.rs

- 153: Old project settings.toml still distributed with package, overwritten each time too.

## crates/prism-app/src/app/input.rs

- 31: `Other(u32::MAX)` = platform gave no physical key (winit's `Unidentified`).
- 32: Old implementation directly discarded such events, keep consistent here.
- 134: Letter key enters engine still as "raw keycode" payload, but takes ASCII value from `Key::Letter`,
- 135: backend-independent (both backends give same KeyCode::Other).
- 137: Unmapped keys keep raw virtual keycode, do not drop key.

## crates/prism-app/src/app/mod.rs

- 96: ===========================================================================
- 97: Subsystem enum
- 98: ===========================================================================
- 141: ===========================================================================
- 142: App
- 143: ===========================================================================
- 160: ---------- Startup config ----------
- 166: ---------- Engine (main thread) ----------
- 170: ---------- Render thread ----------
- 175: ---------- Window context ----------
- 179: ---------- Frame hook (injected by editor/host etc) ----------
- 187: ---------- Per-frame state ----------
- 197: ---------- Input (main thread) ----------
- 203: ---------- Audio (main thread, `audio` feature) ----------
- 222: ---------- Physics (main thread + independent step thread, `physics` feature) ----------
- 228: ---------- Window resize ----------
- 250: ---------- lifecycle ----------
- 262: ---------- timing ----------
- 268: ---------- plugins (#1165) ----------
- 327: Bring the logger up before `build`, which mounts the asset packages and
- 328: loads the startup scene; their warnings must not be dropped (#1013).
- 366: Phase 1 – Subsystem registration
- 368: Resource loading: ResourceManager held by app layer and injected into engine.
- 372: The launch config can declare that this process wants no scene at all
- 373: (editor / tool host). Skip manifest resolution entirely: a missing
- 374: `assets/scenes.json` would otherwise be recorded as "scene missing" and
- 375: `update` would paint the full-window placeholder.
- 378: Scene loading — launch config can specify scene name (editor/launcher preview);
- 379: when not specified follows default behavior "manifest first parseable scene".
- 386: Save directory follows project `[app] company/name` (AppConfig already read from settings).
- 387: Previously `scene_state_path()` used default config, all projects shared one directory (#484).
- 390: Player save (#984) and scene save same directory: both follow `[app] company/name`.
- 391: when `PRISMA_USER_DATA` set use it instead (portable / Android host).
- 403: Runtime startup callbacks
- 406: Project-level render defaults (`[render]` section).
- 469: -----------------------------------------------------------------------
- 470: ECS integration (user projects)
- 471: -----------------------------------------------------------------------
- 636: already enabled, skip
- 642: render conditionally created at resumed()
- 647: #522: physics thread integrates `Rigidbody` each fixed step, results written back to
- 648: `LocalTransform`; `Collider` continues serving raycast queries.
- 662: do not stay silent when no sound card / device is exclusive: one-shot sounds would all be discarded.
- 676: Scene/asset/UI/animation are always-on engine capabilities, no separate switch to flip.
- 713: -----------------------------------------------------------------------
- 714: Plugin system (#1165)
- 715: -----------------------------------------------------------------------
- 763: -----------------------------------------------------------------------
- 764: Host (editor) mode
- 765: -----------------------------------------------------------------------
- 786: Every entry point that starts delivering frames must drain the pending
- 787: plugin list first, or a host driving the app itself (`start_hosted` +
- 788: `host_tick`) silently loses every plugin it registered.
- 1014: -----------------------------------------------------------------------
- 1015: Launch config
- 1016: -----------------------------------------------------------------------
- 1046: Fallback: construction paths outside `App::hosted` also need logger before event loop (idempotent).
- 1149: `start_hosted` drains pending plugins itself.
- 1188: `suspended()` shuts the decode thread down; restart it so
- 1189: playback queued after resume is decoded again.
- 1200: The render thread exclusively owns the Vulkan context. If it exited
- 1201: (renderer init failure or panic), stop instead of spinning a blank
- 1202: window at 100% CPU.
- 1241: Frame hook (host UI etc) — main thread. All use field-level borrows (not borrow self as a whole).
- 1259: Reveal the window once the render thread has finished building and
- 1260: warming up. Everything expensive is behind us at that point, so the
- 1261: first frame follows within one frame interval and the window never
- 1262: appears as an empty rectangle.
- 1285: 1. Stop audio decode thread
- 1288: 2. Stop render thread
- 1290: 4. Engine pre-shutdown
- 1294: 5. Drop audio engine (stop audio stream before platform/engine)
- 1297: 6. Drop window (renderer owned by render thread, destroyed with render thread stop)
- 1299: 7. Engine post-shutdown
- 1319: Reset suspend flag + restore subsystems (corresponds to winit `resumed`).
- 1342: Render thread asynchronously builds renderer + starts frame loop (heavy init doesn't block main thread).
- 1361: 1. Message pump: convert WM_* to neutral events (including user close button click).
- 1369: First send events to frame hook (host UI etc); if consumed skip hotkeys.
- 1375: Route to InputManager; `false` = user closed window.
- 1396: 2. Simulate + extract + submit render (same as winit `about_to_wait`).
- 1403: 3. Yield CPU: return immediately if message, otherwise wait at most 1 ms.
- 1416: -----------------------------------------------------------------------
- 1417: Render thread lifecycle
- 1418: -----------------------------------------------------------------------
- 1427: Raw handles must be obtained on **window creation thread** before cross-thread transfer (winit's
- 1428: `window_handle()` fails on other threads); raw HWND is process-level value,
- 1429: cross-thread used to create Vulkan surface is safe (see `SendWindowHandles`).
- 1433: Vulkan instance extensions (depends on window display handle, cheap).
- 1442: Scene declarative lighting: pure CPU computation, no renderer needed (None → cheap empty IBL).
- 1447: External overlay factory (host UI etc) --- only provide factory, render thread built
- 1448: renderer then call once to produce overlay (GPU resources lazily created at record time).
- 1499: Notify render thread to stop.
- 1507: Wait for thread to end
- 1517: -----------------------------------------------------------------------
- 1518: Audio decode thread lifecycle (`audio` feature)
- 1519: -----------------------------------------------------------------------
- 1554: -----------------------------------------------------------------------
- 1555: Platform context (window + renderer) lifecycle
- 1556: -----------------------------------------------------------------------
- 1557: Note: window created by main thread `PlatformContext::create_window` (fast),
- 1558: renderer by `spawn_render_thread` → render thread internal construction (async, not blocking events).
- 1560: -----------------------------------------------------------------------
- 1561: Game loop (main thread)
- 1562: -----------------------------------------------------------------------
- 1570: Poll mode may call this at any event rate. Time is measured once from
- 1571: the monotonic clock; input events must never determine simulation speed.
- 1580: Advance the frame clock HERE, at the point `elapsed` was measured:
- 1581: the heavy per-frame work (extract + render submit) happens later in
- 1582: this same call, so stamping the clock at the end of `about_to_wait`
- 1583: made `dt` measure only the idle gap between iterations (≈0.5 µs).
- 1584: That froze time-driven simulation and made its speed depend on how
- 1585: often winit woke the loop (i.e. on mouse movement).
- 1588: --- Engine simulation (independent borrow self.engine, released at block end) ---
- 1589: Size must be taken before borrowing engine: `surface_size()` borrows entire `&self`.
- 1596: --- Sync screen size (ECS UI layout depends on ScreenSize resource) ---
- 1597: 
- 1598: Layout still computed by window pixels; when `internal_resolution` effective UI and scene together
- 1599: drawn in PostPass target rectangle, so tell engine again "window → layout space"
- 1600: conversion, pointer hit test won't be off (#988).
- 1614: Pointer lock has single truth (platform window), copied into InputManager each frame:
- 1615: grab rejected by system / Esc unlock / focus loss release all change window state, after alignment here
- 1616: camera (engine) and panel (project hook) read boolean values won't each say their own thing.
- 1621: --- Fixed timestep: integer nanosecond clock, capped catch-up ---
- 1626: Physics uses fixed step (#522): result in below `engine.update()` by
- 1627: `hierarchy_system` propagated to `LocalToWorld`, render same frame can see it.
- 1641: --- Variable time step update ---
- 1644: --- Late update ---
- 1647: Edit mode: does not consume fixed accumulator, avoids catching up big chunk when switching to Play.
- 1655: --- Audio update (`audio` feature) ---
- 1662: Process audio decode results: hand to scene audio cache (no longer auto-play,
- 1663: playback decided by `AudioSource` component).
- 1675: Scene audio: ECS `AudioSource` → mixer (#508).
- 1684: One-shot sounds (#987): project code queues, here actually play.
- 1692: engine borrow ends here
- 1694: Window events arrive after the previous tick, so transient input must
- 1695: be cleared only after this frame's gameplay / UI have read the
- 1696: snapshot. Otherwise a fresh Space / mouse press edge is dropped
- 1697: before the ECS sees it. Held state stays valid across frames.
- 1700: --- Dev hot reload (#495): after rebuilding `.pak` no need to exit and re-enter ---
- 1703: --- Asset resolution: main thread CPU segment prepares request → channel → render thread GPU segment ---
- 1706: --- Extract frame packet → send to render thread (render enabled only) ---
- 1708: Reuse data packet returned by render thread last frame (`Vec` capacity retained, #66).
- 1710: #613: when internal resolution effective camera projects by **internal** aspect ratio, post-process then integer
- 1711: upscale to window; otherwise 2D pixels stretched into unequal-width rectangles.
- 1713: #978: subpixel polyline converts "1 pixel" world length by scene actual render height.
- 1725: Render settings sent to render thread with frame packet (hotkey / project config changes take effect here).
- 1727: Local post-processing volume (#502 Phase 2): parse this frame PostGrade by main camera position.
- 1736: Runtime diagnostics HUD: appended to ECS UI overlay (then uniformly drawn by ui_overlay).
- 1777: Project bundled font (#583): main thread reads bytes, render thread registers it into UI atlas.
- 1785: Upload UI RGBA8 icon textures (e.g. rasterized SVGs) into bindless slots.
- 1822: Skeletal animation assets (#560 P0-2): idempotent, only read disk when new `.rskel` / `.ranim` appears.
- 1826: CPU segment: scan pending-parse entities and produce upload requests (pure data, not touching renderer).
- 1832: Invalid GPU resources (entity destroyed / hot reload / path rewrite) truly released on render thread (#495).
- 1840: GPU segment result write-back: render thread returns handles, main thread writes into components.
- 1861: First drain events then change state: `receiver()` borrows watcher, processing events borrows self.
- 1901: Also verify build stamp once after repack (#976): when package and document don't match immediately prompt,
- 1902: instead of waiting for player to find screen wrong.
- 1915: -----------------------------------------------------------------------
- 1916: PBR debug helpers
- 1917: -----------------------------------------------------------------------
- 1929: A host that owns the cursor gets no grab/release from us.
- 1937: Regaining focus re-grabs: pointer lock is click-to-enter, so
- 1938: alt-tabbing out and back would otherwise leave the player with
- 1939: dead movement keys until they click again. Esc stays the only
- 1940: way out (see `should_release_pointer_lock`).
- 1942: Trust the platform result: if the OS refused the grab, do
- 1943: not record "locked", or the cursor stays visible and free
- 1944: while the camera keeps eating WASD and mouse deltas.
- 1952: Raw device increments in locked state (winit: `DeviceEvent::MouseMotion`,
- 1953: Win32: re-center mouse capture offset).
- 1959: Touch does not go through mouse button simulation: multi-touch must retain touch id and phase,
- 1960: project module via `InputSnapshot::touches` (host fills table, at most
- 1961: `InputSnapshot::MAX_TOUCHES` points) self-interpreted, instead of being compressed to left button.
- 1992: Key code conversion + Esc unlock + write to InputManager collected in `apply_key_event`,
- 1993: only unit tests can cover "platform key → engine key → key_held" entire chain;
- 1994: only real window-touching release_pointer left outside.
- 2002: Size change also updates aspect ratio (render thread reads real size from platform context).
- 2009: Unmodeled events: keep explicitly listed, avoid `_ => {}` silently swallowing new variants
- 2010: (`PlatformEvent::Touch` is how entire touch chain was lost).
- 2021: Host hook **not called here**: two event loop entries (platform event pump and winit
- 2022: `window_event`) both already send same key event to hook before calling this function,
- 2023: if consumed skip this function directly. Converting again here equals same key pressed twice --- project side
- 2024: of "press once toggle once" will self-cancel (F1 panel never opens).
- 2025: 
- 2026: Binding lookup and "number key + Shift = another PBR component" both decided by engine's
- 2027: `KeyBindings` decide; host here only responsible for applying action to `RenderSettings`.
- 2028: Unbound keys all return false: not consumed, project input available as-is.
- 2029: When debug layer not compiled table is empty: F3 / Tab / T / number keys all left as-is for project.
- 2078: 0 = Reinhard, 1 = ACES, 2 = passthrough (2D / pixel art).
- 2082: Already handled above, here just writing branches complete.
- 2106: ===========================================================================
- 2107: ApplicationHandler
- 2108: ===========================================================================
- 2113: Drive simulation/extraction continuously. With the default Wait mode
- 2114: winit only re-enters `about_to_wait` after input/window events, which
- 2115: freezes animation when the user is idle.
- 2118: Reset suspend flag (must precede any subsystem gate --- audio resume doesn't depend on Render).
- 2130: Main thread only creates window (fast, ~few ms), then immediately returns --- window events
- 2131: (close/move/scale) from now can be dispatched by winit, not blocked by any init.
- 2134: Create hidden: the first frame is hundreds of milliseconds away
- 2135: (renderer build + pipeline/font warmup), and an empty window in
- 2136: that gap reads as a hang. `tick_frame` shows it once a frame has
- 2137: actually been presented.
- 2147: Render thread asynchronously builds renderer + starts frame loop (heavy init doesn't block main thread).
- 2152: Record startup milestone: resumed return time = event_loop_free (event loop unlocked).
- 2162: After Android suspend rebuild surface: this design already stopped render thread at suspended and
- 2163: dropped window, so here re-walk above "first resume" branch. Reserved log only.
- 2188: First forward events to frame hook (host UI etc); if consumed no longer walk app
- 2189: own hotkey handling (input still routed to InputManager to keep key state consistent).
- 2199: Route window events → InputManager (always route, including hotkey-unhandled repeat events).
- 2211: Modifier key state only updates with `ModifiersChanged` (route already written), event itself needs no handling.
- 2216: Keyboard hotkeys (triggered on press only). Here directly reuse already computed above
- 2217: `platform_event`, no longer re-parse keys from `WindowEvent`: two paths each compute once
- 2218: then each uses different conversion (`Key::from_winit` vs `winit_to_vk`), same key under
- 2219: "platform event pump" and "winit event" becomes two different `Key`.
- 2231: In pointer lock mode, mouse movement arrives as raw device increments.
- 2247: Game loop tick (main thread) --- background suspend pauses simulate and returns early (T4):
- 2248: not feeding dt → simulate frozen → background process CPU zeroed, and audio decode already
- 2249: stopped with suspended, no queue idle spin.
- 2302: Background no longer needs per-frame polling; `resumed()` will restore `Poll`. Otherwise winit at
- 2303: Android background still keeps spinning idle, CPU usage stays 100%.
- 2306: Set suspend flag (must precede any subsystem gate --- audio suspend doesn't depend on Render).
- 2309: --- Audio subsystem: return audio device (T4). drop cpal stream → stop callback thread;
- 2310: Firewheel graph and active playback nodes retained, on resume re-registered with pre-suspend config and resume playback. ---
- 2322: Stop background thread.
- 2326: Stop render thread (renderer owned by render thread, stop releases Vulkan surface).

## crates/prism-app/src/app/tests.rs

- 104: Integer floor down: 2x -> 1280x720, centered leaves 160/90 black borders.
- 111: Window center is layout center (screen center), click where you select.
- 154: Clamped cursor position arrives first (previously delta was accumulated first).
- 216: Shift+6 is skin weight heatmap (#560), not PBR component, but also debug bit 15.
- 225: 15 debug bits already used; Shift+7..0 with normal letters should not grab hotkeys.
- 272: Unlisted keys cannot be dropped: degrade to raw code (winit discriminant / Win32 VK).
- 301: Keys only covered by `LETTER_KEYS` fallback also cannot be dropped.
- 322: Pressing Esc again when already unlocked does not re-trigger.

## crates/prism-app/src/audio_decode_runner.rs

- 17: ── Messages ──────────────────────────────────────────────────────────
- 51: ── Thread entry point ────────────────────────────────────────────────

## crates/prism-app/src/bench_report.rs

- 249: The temporary file is renamed into place, not left behind.

## crates/prism-app/src/crash_dialog/android.rs

- 25: Full error already logged by `show_crash_dialog`; also surface title.
- 42: SAFETY: the pointers were obtained from `AndroidApp::vm_as_ptr()` and
- 43: `activity_as_ptr()` and remain valid for the process lifetime. We only
- 44: touch them from the main thread, which is the thread that registered.
- 47: Synchronization: block this thread until the dialog's OK button fires.
- 51: The dialog must be created + shown on the UI thread. We call
- 52: `Activity.runOnUiThread(Runnable)`. Building the Runnable requires
- 53: implementing `java.lang.Runnable.run()`; we can't easily do that from
- 54: JNI without a registered native class. Instead, use the simpler approach
- 55: of `AlertDialog` via the `android.app.AlertDialog.Builder` class, shown
- 56: directly from the current thread — Android permits calling
- 57: `Builder.create().show()` from any thread that has a Looper, but the
- 58: main thread is the one with a Looper. The winit event loop on Android
- 59: runs on the main thread, so this is fine.
- 60: 
- 61: We catch any exception and fall back to plain exit.
- 83: Block until the dialog is dismissed and signals `done`.
- 103: We need a way for the OK button click to signal Rust. The cleanest
- 104: approach without registering a native method on a Java class is to
- 105: subclass `OnClickListener` — but JNI can't subclass Java classes
- 106: directly. Instead we use `android.content.DialogInterface.OnClickListener`
- 107: via a dynamic proxy built with `java.lang.reflect.Proxy`.
- 108: 
- 109: That requires a class loader + InvocationHandler, which is also heavy.
- 110: Given the goal (show the error, then exit), we instead show the dialog
- 111: and *don't* block on the button: the user reads the message, taps OK,
- 112: and the OS terminates the process when the Activity finishes. We set
- 113: `done` immediately so the caller proceeds to exit the event loop.
- 114: 
- 115: This keeps the implementation robust on every Android version without
- 116: fragile reflection.
- 125: setTitle(title)
- 132: setMessage(message)
- 139: setCancelable(false)
- 147: Build a positive ("OK") button. We pass a null listener — tapping the
- 148: button will auto-dismiss the dialog; we don't need a callback because
- 149: we exit the process regardless.
- 159: create() -> AlertDialog
- 168: show()
- 171: Keep a global ref so the dialog isn't GC'd before the user dismisses it.
- 174: Poll until the dialog is dismissed (e.g. user tapped OK) or the activity finishes.
- 198: Suppress "unused" — the JNI return code of show() is void.

## crates/prism-app/src/crash_dialog/linux.rs

- 15: zenity --question: exit 0 = Yes (Copy & Exit), 1 = No (Exit).
- 47: Try xclip first; fall back to xsel.

## crates/prism-app/src/crash_dialog/macos.rs

- 13: Escape double-quotes and backslashes for the AppleScript string literal.
- 16: Replace literal newlines with AppleScript newline concatenation so the
- 17: dialog shows real line breaks instead of a single run-on line.
- 20: "Copy & Exit" is the default (first button). `giving up after 0` means
- 21: "no timeout" here is not used; we let it block.
- 30: `osascript` prints `button returned:Copy & Exit` to stdout.

## crates/prism-app/src/crash_dialog/mod.rs

- 40: ---------------------------------------------------------------------------
- 41: Android app registration (only compiled on Android
- 42: ---------------------------------------------------------------------------
- 52: SAFETY: the handles are raw pointers into the JVM/Activity, which live for
- 53: the entire process. They are only dereferenced from the main thread (the
- 54: winit event loop thread), which is also the thread that registered them.
- 88: ---------------------------------------------------------------------------
- 89: Public entry point
- 90: ---------------------------------------------------------------------------
- 99: Always log the full error first so it's in the log even if the dialog
- 100: backend fails (e.g. zenity not installed).
- 112: ---------------------------------------------------------------------------
- 113: Platform dispatch
- 114: ---------------------------------------------------------------------------
- 175: Register initial handle
- 184: Re-entry replaces the previous handle
- 193: Exit / unregister clears the reference so it never dangles

## crates/prism-app/src/crash_dialog/windows.rs

- 34: Build a combined body: the message + a hint about the buttons.
- 44: HWND owner = null (no parent window; render window might be in bad state)
- 56: IDNO or any error/cancel path falls through to plain Exit.
- 62: Win32 clipboard wants a NUL-terminated UTF-16 buffer in a global alloc.
- 63: We go through the Rust allocator + raw HGLOBAL via `GlobalAlloc` is the
- 64: textbook approach, but `SetClipboardData` can also receive a HANDLE we
- 65: allocate ourselves. Use the standard `GlobalAlloc` flow.
- 71: NUL terminator

## crates/prism-app/src/debug_hud.rs

- 51: When full-frame interval exists use full-frame time, otherwise fall back to pass sum: two metrics must be distinguishable.
- 53: Device does not support timestamps: explicitly write n/a, do not fake GPU time with CPU wall clock.
- 67: Percentiles need a few dozen frames before they mean anything.
- 102: Most expensive 3 passes: same source as render graph panel (last frame readback timestamp results).
- 113: Part not covered by pass inside full-frame interval: barrier, out-of-graph dispatch, copy, etc.
- 114: Only show when really got interval, otherwise 0 would be misread as "no out-of-graph overhead".

## crates/prism-app/src/lib.rs

- 190: ===========================================================================
- 191: Android entry helper
- 192: ===========================================================================
- 230: The engine is filesystem-based; stage the APK asset tree before App
- 231: construction so `assets/scenes.pak` etc. resolve exactly like desktop.
- 232: This also injects the hub's launch config, which carries `log_level`.
- 236: android_logger reads its level once at init; apply the launch override
- 237: first (default: Debug, matching the historical behavior).
- 296: Initial launch with config A
- 304: Re-entry with updated config B
- 312: Re-entry when launch config is deleted

## crates/prism-app/src/logging.rs

- 42: Android installs `android_logger` in `run_on_android` before the App is
- 43: built; `env_logger` there would only fight it.

## crates/prism-app/src/one_shot.rs

- 104: Same id replay first stops old one, avoid handle leak.
- 130: Looping sound: mixer has no loop flag, replay after finish.
- 154: No mixer: request dropped, queue must be cleared (otherwise retry every frame).

## crates/prism-app/src/physics_runner.rs

- 54: ── Commands (main → physics) ─────────────────────────────────────────
- 238: ── Results (physics → main) ──────────────────────────────────────────
- 256: ── helpers ───────────────────────────────────────────────────────────
- 329: Deleting body also deletes colliders hanging under it.
- 345: ── Thread entry point ────────────────────────────────────────────────
- 362: Entity → Rapier handle map.
- 366: Terrain is fixed heightfield body, does not participate in `entity_map` pose write-back (#517).
- 369: Character move must wait for `step` after broad phase to be this frame's, so collect first.
- 372: rapier reports Started/Stopped via EventHandler inside `step`; here use official
- 373: ChannelEventCollector queues events to channel, map back to entities uniformly after step.
- 379: 1. Apply commands.
- 398: Collider density is 0 for dynamic bodies, so the authored
- 399: mass is the whole mass (instead of volume + mass).
- 475: Swapping heightmap / moving terrain first tears down old body, avoid leaving two colliders.
- 487: rapier heightfield row = z, col = x; its `Array2` underlying is
- 488: column-major (`index = row + col * nrows`), while `Heightfield` is row-major
- 489: `(z * cells + x)`, so transpose once here.
- 532: 2. Step simulation.
- 544: PhysicsHooks
- 548: 2b. Drain collision events and map collider handles back to entities.
- 559: One side despawned before event arrives: keeping for previous step's consumer meaningless.
- 573: 3. Collect dynamic body transforms.
- 579: only send back dynamic body positions
- 596: 2d. Kinematic character moves (#560 P2). Run after the step so the
- 597: broad phase reflects this frame's colliders; each request resolves
- 598: against the current pose and writes the corrected position back.
- 645: Keep the next kinematic target in sync: a kinematic body is
- 646: integrated towards `next_position`, so setting only the
- 647: current pose would be undone by the next step.
- 673: ── Main-thread runtime (App holds) ────────────────────────────────────
- 842: Gravity / jump are not wired yet: the kinematic controller does
- 843: not settle onto the floor with small downward steps, so the
- 844: vertical axis stays with the caller until that is understood.
- 863: Keep requests written by gameplay this frame; ours are appended.
- 872: Collect entities first, avoid get component inside query borrow.
- 883: Despawn already-disappeared entities first (including scene-switch unloaded).
- 907: Parent scale also enters collision shape; without parent equals old `transform.scale`.
- 936: rapier does not support in-place shape / type change; despawn + spawn simplest.
- 949: Entities moved by animation / code driven by ECS, send new pose each fixed step.
- 961: Terrain heightfield (#517): `TerrainData` is runtime-system-generated component, no
- 962: `Rigidbody` / `Collider`, so separately sync into static heightfield collider.
- 1034: Character moves are per-step requests (#560 P2): take them out of the
- 1035: world so a stale request is not replayed on every step.
- 1048: Close step channel -> thread recv loop ends.
- 1080: Events kept only when both sides still alive; residual Stopped after scene switch / despawn no longer exposed.
- 1101: rapier works in world space; entities parented to something else need
- 1102: the delta rotated (and unscaled) back into parent space.
- 1138: rapier returns world pose; entity with Parent must convert back to parent local space,
- 1139: otherwise `hierarchy_system` next frame would multiply parent transform again.
- 1342: The spawn step answers with its own (empty) result; drain it so every
- 1343: send below is paired with the result of the same step.
- 1348: Fall onto the floor first.
- 1369: Walk into the wall: the allowed translation must stop short.
- 1461: Terrain destroyed, safe to despawn next step.
- 1533: The request resource was consumed: another step must not move again.
- 1607: Releasing the input decelerates to a stop instead of sliding forever.
- 1658: Radius follows horizontal scale only, not affected by stretched height.
- 1692: Delete entity: next step sends DespawnBody, must not panic / hang.
- 1893: `PhysicsRuntime` only syncs entities with LocalTransform; trigger also needs position,
- 1894: otherwise it would never be spawned, test would false-pass.

## crates/prism-app/src/plugin.rs

- 90: ---------------------------------------------------------------------------
- 91: Standard Default Plugins
- 92: ---------------------------------------------------------------------------

## crates/prism-app/src/render_runner.rs

- 23: Directly use `raw-window-handle` crate (winit's `winit::raw_window_handle` is its
- 24: re-export): `win32-native` build must not have any `winit::` path.
- 52: Raw handle guaranteed valid by main thread; here only re-borrow.
- 118: -------------------------------------------------------------------
- 119: Async build renderer (~hundreds ms, but runs on render thread, does not block window events)
- 120: -------------------------------------------------------------------
- 121: Raw handle wrapped into Has*Handle---raw handle is process-level, cross-thread safe.
- 201: Build IBL per "scene declarative lighting": only runs CPU pre-convolution when scene carries EnvironmentLighting/env_map
- 202: (or `.rscn` carries env texture); otherwise rely on cheap empty IBL at startup.
- 214: Install external overlay (host UI etc); GPU resource lazily created at record.
- 219: Frame timing
- 222: ~2 minutes at 60 fps; the summary is sorted, so it is refreshed a few
- 223: times per second instead of every frame (#1002).
- 236: Check PT reset request from main thread
- 241: ---- Asset upload request (main thread CPU side -> render thread GPU side) ----
- 253: Apply overlay messages delivered by main thread (e.g. "new host UI frame").
- 259: Apply renderer commands delivered by main thread (e.g. set_environment rebuild IBL).
- 265: The host drives this loop. Do not add a frame-rate sleep here.
- 330: Device-lost / surface failure is terminal: retrying only spins
- 331: the CPU and floods the log (observed: >1M lines). Report it to
- 332: the host so the viewport can stop and surface the reason.
- 363: The first frame is on the surface: reveal the window now.
- 364: Signalling after warmup instead would still leave a visible gap,
- 365: because the render thread only picks up the queued packet after
- 366: the warmup finishes.
- 391: Compute render stats.
- 416: Benchmark report (#1002): warm-up frames only count down, and the file
- 417: is written once on the frame that completes the sample set.
- 447: Render graph report (Web editor visualization): first frame + low-frequency refresh.
- 469: Device not supporting timestamps keeps None; HUD explicitly shows "n/a" instead of faking GPU time with CPU wall clock.
- 474: ---- Host (editor) frame readback: only read back PostPass tonemapped offscreen image ----
- 498: Return packet: main thread next frame clear() + refill, skip Vec re-allocation each frame (#66).
- 566: #502: global LUT intensity + .cube path under assets/ (volume tone mapping does not cover LUT).
- 574: #613: internal resolution must take effect before `begin_frame`---`begin_frame` uses it to decide
- 575: HDR intermediate target size, `execute` uses it again for PostPass target rect.
- 579: GI / SHARC knobs: the renderer owns its own copy of these (they decide
- 580: whether the SHARC passes run at all), so forward them each frame.
- 581: `render_mode` and the PT knobs travel through `FrameInput` instead.
- 582: 
- 583: `sharc_enable_clear` is deliberately not copied: it is a one-shot latch
- 584: that the renderer consumes and resets, so copying it every frame would
- 585: wipe the cache every frame. Callers that need a reset set it through
- 586: `GraphRenderer::settings_mut()`.
- 622: #1007: last frame coarse depth read back in `begin_frame`, so filtering placed after it.
- 686: `GraphRenderer::present` returns whether the swapchain was out of date,
- 687: not whether queue submission/present succeeded. Reaching this point
- 688: means the frame was submitted to the native surface.

## crates/prism-app/src/render_shared.rs

- 47: ---------------------------------------------------------------------------
- 48: FrameImage --- render thread -> host (editor)
- 49: ---------------------------------------------------------------------------
- 62: ---------------------------------------------------------------------------
- 63: FrameStats --- render thread -> main thread
- 64: ---------------------------------------------------------------------------
- 84: ---------------------------------------------------------------------------
- 85: RenderShared
- 86: ---------------------------------------------------------------------------
- 166: -------------------------------------------------------------------
- 167: Packet (main → render)
- 168: -------------------------------------------------------------------
- 208: -------------------------------------------------------------------
- 209: Offscreen frame readback (host mode)
- 210: -------------------------------------------------------------------
- 231: -------------------------------------------------------------------
- 232: Renderer messages (main -> render thread)
- 233: -------------------------------------------------------------------
- 247: -------------------------------------------------------------------
- 248: Asset parsing (main thread CPU side <-> render thread GPU side)
- 249: -------------------------------------------------------------------
- 277: -------------------------------------------------------------------
- 278: Startup timing (main thread + render thread shared)
- 279: -------------------------------------------------------------------
- 321: -------------------------------------------------------------------
- 322: RenderStats (render → main)
- 323: -------------------------------------------------------------------
- 335: -------------------------------------------------------------------
- 336: PT reset (main → render)
- 337: -------------------------------------------------------------------
- 349: -------------------------------------------------------------------
- 350: Render-thread failure (render -> main)
- 351: -------------------------------------------------------------------

## crates/prism-app/src/scene_audio.rs

- 151: Failure not cached: will retry next time (file may appear later).
- 215: Path changed: stop old voice first.
- 251: Finished playing: loop restarts, otherwise recycle.
- 299: Entity destroyed / component removed: stop corresponding voice.
- 595: Default listener = world origin
- 618: `Master` multiplies all buses.
- 623: Other buses do not affect Music.
- 628: Illegal values ignored, will not turn bus into NaN.
- 631: When source itself is on `Master`, only multiply once.
- 638: Default listener = world origin
- 664: Move closer: volume updates, but does not re-play.
- 681: Listener faces +X: right vector is -Z, so source at +X in world lands dead ahead.
- 687: Non-finite input does not produce NaN.
- 692: Pan strength: 2D and non-finite are both 0, upper clamped to 1.
- 701: Listener at origin, no rotation (right vector = +X), source at +X: offset should be (5, 0, 0).
- 729: Default listener = world origin
- 750: Move left: same voice only updates spatial params, not re-play.
- 824: Only allowed to judge "finished playing" after audio thread started playing.
- 849: One clears path, other directly destroys.

## crates/prism-app/tests/hosted_no_scene.rs

- 14: No `PRISMA_ASSET_ROOT` pointing at a project, no `assets/scenes.json` in the
- 15: working tree root: the player path would fail here.
- 29: Contrast for the test above: the same checkout, the player path. There is an
- 30: `assets/` directory but no `assets/scenes.json`, so the scene manager marks
- 31: the startup scene as missing and the placeholder is drawn. If this ever stops
- 32: holding, the hosted assertion above stops proving anything.

## crates/prism-app/tests/hosted_plugins.rs

- 30: Whatever the renderer does is irrelevant and async; the plugin drain happens
- 31: synchronously before it is asked for work.

## crates/prism-app/tests/hosted_viewport.rs

- 43: Renderer asynchronously created in render thread (hundreds of ms), frames only after.
- 71: All black usually means PostPass did not really write to offscreen target.
- 80: Scaling should not break: run `resize_offscreen` then take a frame.

## crates/prism-app/tests/physics_harness.rs

- 139: Capsule total half-height = half_height + radius = 1.0.

## crates/prism-app/tests/pointer_lock_policy.rs

- 23: Hosted mode is about scenes, not input: an FPS-style host can still use it.

## crates/prism-app/tests/scene_switch.rs

- 67: Same scene loaded again does not duplicate; engine is same instance.
- 92: Mark first scene entity `Persistent` (game side equivalent `DontDestroyOnLoad`).
- 113: Same entity handle still valid (engine not rebuilt).

## crates/prism-app/tests/startup_logging.rs

- 17: Precondition: nothing has installed a logger in this process yet.
- 24: Keep the app away from the real assets and the user's save directory.

## crates/prism-app/tests/support/mod.rs

- 8: This module is compiled into several test binaries; each one exercises a
- 9: different subset, so per-binary dead-code expectations cannot all be met.

## crates/prism-asset/src/core/asset_data.rs

- 37: ---------------------------------------------------------------------------
- 38: AssetData trait
- 39: ---------------------------------------------------------------------------
- 62: Downcasting goes through the `Any` supertrait. Unlike a user-provided
- 63: `type_id`/`as_any` pair, the `Any` vtable is generated by the compiler
- 64: and cannot be spoofed by a safe trait impl — a mismatched downcast can
- 65: never reinterpret the allocation.
- 140: ---------------------------------------------------------------------------
- 141: AssetHandle — typed, serializable cross-reference
- 142: ---------------------------------------------------------------------------
- 192: Manual PartialEq/Eq/Hash so we avoid deriving T-bound variants.
- 225: Serde: serialize only guid + path, ignore T and resolved data.

## crates/prism-asset/src/core/asset_data_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------

## crates/prism-asset/src/core/asset_type.rs

- 112: Binary
- 114: Already cooked runtime format
- 127: Textures / images
- 131: Mesh / geometry
- 133: Material
- 135: Shader
- 137: Prefab
- 139: Scene
- 141: Audio
- 143: Font (for UI text; packed as-is, no cook needed)
- 150: ---------------------------------------------------------------------------
- 151: AssetRef – editor-side reference to another asset
- 152: ---------------------------------------------------------------------------

## crates/prism-asset/src/core/asset_type_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 52: #583: fonts must be recognized by cooker/pack, otherwise won't enter `.pak`.

## crates/prism-asset/src/core/guid.rs

- 71: Standard UUID hex format: 8-4-4-4-12
- 82: Manual Hash so we don't rely on derive for [u8; 16].
- 89: ---------------------------------------------------------------------------
- 90: Serde — serialize as a hex UUID string for human readability
- 91: ---------------------------------------------------------------------------

## crates/prism-asset/src/core/guid_tests.rs

- 41: 8-4-4-4-12

## crates/prism-asset/src/core/handle.rs

- 29: Manual Clone/Copy to avoid deriving T: Clone/T: Copy bounds.
- 37: Manual PartialEq/Eq/Hash to avoid deriving T-bound versions.
- 128: ---------------------------------------------------------------------------
- 129: Type-erased handle (for heterogeneous storage)
- 130: ---------------------------------------------------------------------------
- 207: ---------------------------------------------------------------------------
- 208: Type alias for asset handles
- 209: ---------------------------------------------------------------------------

## crates/prism-asset/src/core/handle_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 50: Copy
- 57: Even a !Send/!Sync payload must not leak into the handle's auto traits:
- 58: the handle is an index, not an owned reference.

## crates/prism-asset/src/core/id.rs

- 4: ---------------------------------------------------------------------------
- 5: Global atomic counter used by AssetId::generate()
- 6: ---------------------------------------------------------------------------
- 11: ---------------------------------------------------------------------------
- 12: AssetId — globally unique u64-based identifier
- 13: ---------------------------------------------------------------------------
- 31: ------------------------------------------------------------------
- 32: Construction
- 33: ------------------------------------------------------------------
- 102: ---------------------------------------------------------------------------
- 103: Display / Debug
- 104: ---------------------------------------------------------------------------
- 124: ---------------------------------------------------------------------------
- 125: Serde
- 126: ---------------------------------------------------------------------------
- 140: ---------------------------------------------------------------------------
- 141: Persistent ID generator (editor)
- 142: ---------------------------------------------------------------------------

## crates/prism-asset/src/core/id_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 90: #530: packer derives ID from path, texture IDs baked in material won't misalign due to an extra file.
- 94: Windows-style input normalized to same ID.
- 102: Different cook forms of same image are different paths, must get different IDs.

## crates/prism-asset/src/core/meta_tests.rs

- 58: Sidecar without the asset file: deleted asset, must not resolve.

## crates/prism-asset/src/formats.rs

- 10: ---------------------------------------------------------------------------
- 11: RTEX `format` byte values
- 12: ---------------------------------------------------------------------------
- 13: 
- 14: The cooker writes one of these into the header; runtime consumers use them to
- 15: interpret the mip payload. RTEX does **not** encode colour space: an RGBA8
- 16: payload can be sRGB (albedo/emissive) or linear (normal/MR/AO), so the caller
- 17: picks sRGB-ness per texture slot.
- 282: ---------------------------------------------------------------------------
- 283: RATL (.ratlas 2D Sprite Atlas)
- 284: ---------------------------------------------------------------------------
- 478: ---------------------------------------------------------------------------
- 479: RANIM2D (.ranim2d 2D Animation Format)
- 480: ---------------------------------------------------------------------------
- 565: Frames
- 574: Tags
- 586: Slices
- 619: Frames
- 637: Tags
- 664: Slices
- 712: ---------------------------------------------------------------------------
- 713: RTILEMAP (.rtilemap 2D Tilemap Format)
- 714: ---------------------------------------------------------------------------
- 779: Layers
- 797: Objects
- 815: Collision boxes
- 847: Layers
- 891: Objects
- 936: Collision boxes
- 980: No texture in any of the five slots.
- 1019: Unknown spellings fall back to the legacy always-test behaviour.
- 1083: wrong version

## crates/prism-asset/src/formats_anim_graph.rs

- 298: v2: node end appends per-sample velocity table; v1 has no table.

## crates/prism-asset/src/formats_anim_graph_tests.rs

- 17: parameters
- 19: nodes
- 23: speed
- 24: weight
- 25: clips
- 27: positions
- 31: inputs
- 33: sample speeds
- 47: input node
- 49: mask
- 51: sample speeds
- 54: output
- 101: node 0 kind byte: header(9) + name_len(2) + name(4).
- 110: ClipPlayer flags (byte after kind)

## crates/prism-asset/src/formats_skinned.rs

- 110: ---------------------------------------------------------------------------
- 111: Reader
- 112: ---------------------------------------------------------------------------
- 200: ---------------------------------------------------------------------------
- 201: Skeleton
- 202: ---------------------------------------------------------------------------
- 344: ---------------------------------------------------------------------------
- 345: Skinned mesh
- 346: ---------------------------------------------------------------------------
- 402: Zero-weight slots allow any index (cooker writes unused slots as 0).
- 527: ---------------------------------------------------------------------------
- 528: Animation clip
- 529: ---------------------------------------------------------------------------
- 712: v1 has no event table: version number itself declares "no events". v2+ every clip has table (may be empty),
- 713: so future fields can just keep appending.
- 729: v3 appends sync marker table; v1/v2 don't have.
- 746: v4 appends Montage segment table; v1-v3 don't have.
- 776: v5 appends MotionWarping window table; v1-v4 don't have.

## crates/prism-asset/src/formats_skinned_tests.rs

- 13: vertex_count
- 14: index_count
- 15: joint_count
- 16: submesh_count
- 17: flags
- 98: Submesh index range out of bounds.
- 118: Valid magic, unknown version.

## crates/prism-asset/src/formats_state_machine_tests.rs

- 76: u32::MAX = any state.
- 129: Too short / wrong magic / wrong version.
- 139: State name truncated, trailing extra bytes, too many states.
- 150: Unknown condition kind / unknown state flag.

## crates/prism-asset/src/lib.rs

- 16: Re-exports so macros using `$crate::` resolve correctly

## crates/prism-asset/src/package/mod.rs

- 32: ---------------------------------------------------------------------------
- 33: Constants
- 34: ---------------------------------------------------------------------------
- 46: ---------------------------------------------------------------------------
- 47: Errors
- 48: ---------------------------------------------------------------------------
- 83: ---------------------------------------------------------------------------
- 84: Header
- 85: ---------------------------------------------------------------------------
- 113: 52
- 191: ---------------------------------------------------------------------------
- 192: Runtime Asset Record
- 193: ---------------------------------------------------------------------------
- 288: ---------------------------------------------------------------------------
- 289: Package Builder
- 290: ---------------------------------------------------------------------------
- 319: 0 = no compression
- 379: Compute compressed data if needed.
- 397: Build flat dependency array and record each asset's dependency range.
- 418: filled below
- 429: Calculate offsets.
- 437: Fill in data offsets for each record.
- 444: uncompressed size
- 448: Serialize registry + deps + data.
- 456: Checksum covers header[4..44] (version, asset_count, offsets/sizes) + registry +
- 457: deps + data; `checksum` field itself and fixed `magic` do not participate.
- 466: placeholder
- 479: Write final header with checksum.
- 484: Assemble final file.
- 523: ---------------------------------------------------------------------------
- 524: Package Reader
- 525: ---------------------------------------------------------------------------
- 677: Integrity takes precedence over any structural judgment; see `from_owned_bytes` (#89).
- 707: Integrity takes precedence over any structural judgment: checksum only looks at "header except magic/checksum fields"
- 708: fields” + remaining file bytes, does not depend on offsets in header that haven't been verified yet, so tampered
- 709: `version` / `asset_count` will become ChecksumMismatch here, instead of letting subsequent
- 710: registry parsing continue on erroneous data (#89).
- 750: Dependency array is between registry and data, length must be a multiple of u64.
- 774: Read registry.
- 785: Read dependency array.
- 793: dependency range and data range in record also come from file content; if not verified then in
- 794: dependencies() / read_asset_record_data out-of-bounds slice panic.
- 1069: From end of header, feed blocks to hasher; do not read offsets in header that haven't been verified yet.

## crates/prism-asset/src/package/tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 17: An empty package is still a valid package: header only, no 0-byte file.
- 126: Corrupt one byte in the data section.
- 127: header + one record
- 128: skip registry and deps too... let's just find it
- 155: version
- 186: Change asset_count + registry_size together to self-consistent 2 assets, still can't escape.
- 252: Streamed asset slice in package matches uncompressed data directly.
- 258: Both read back correctly through read_asset_data.
- 277: #509: `open` used to `std::fs::read` the whole archive, so a 1 GB `.pak`
- 278: was 1 GB of resident memory from startup. It now keeps the handle and
- 279: reads payloads on demand.
- 296: The file is read lazily, so it must still be usable after `open`.
- 329: Flip a byte inside the data chunk (last byte of the file).
- 371: The in-memory backing is the buffer we handed over, not a copy of it.
- 392: ---------------------------------------------------------------------------
- 393: Malformed header (regression #60): these inputs previously caused integer overflow panic in debug,
- 394: release wraps around to huge offset then slice panic, never reaches checksum check.
- 395: ---------------------------------------------------------------------------
- 413: checksum, filled in below
- 415: Structural checksum test cases must have **valid** checksum, otherwise will hit ChecksumMismatch first (#89).
- 433: Reproduce issue with 100-byte construction: registry covers 52..100, but data_offset is still 52.
- 493: Legal-shaped 100-byte package, but record claims data at offset 0 (data chunk starts at 100).
- 508: dependency_start = 1, dependency_count = 1, but dependency array is empty.

## crates/prism-asset/src/runtime/hot_reload.rs

- 19: ---------------------------------------------------------------------------
- 20: Errors
- 21: ---------------------------------------------------------------------------
- 29: ---------------------------------------------------------------------------
- 30: Events
- 31: ---------------------------------------------------------------------------
- 42: ---------------------------------------------------------------------------
- 43: Hot Reload Watcher
- 44: ---------------------------------------------------------------------------
- 134: Snapshot of modification times at start.

## crates/prism-asset/src/runtime/hot_reload_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 22: Ensure a clock-tick boundary so the subsequent write has a different
- 23: mtime even on filesystems with 1-second granularity (Android/Termux
- 24: FUSE, etc.).
- 30: Modify the file.
- 36: Wait for poller to detect.
- 40: Best-effort assertion: on fast filesystems this will fire, on
- 41: 1-second-granularity filesystems (Android/Termux) it may not.
- 55: #495 ⑤: Rebuild within same second, mtime unchanged, length change still counts as "changed".
- 76: Jitter within 100ms + unchanged length = treated as unchanged (avoid splitting one write into multiple events).
- 111: Cross a timestamp boundary, ensure mtime truly advances (consistent with existing tests).

## crates/prism-asset/src/runtime/mod.rs

- 27: ---------------------------------------------------------------------------
- 28: Hot-reload support
- 29: ---------------------------------------------------------------------------
- 31: Poller depends only on `std`, runtime and build can both use; previously it hid behind `hot-reload` feature,
- 32: and only build-time `prism-cooker` opened that feature, so runtime never got watcher (#495).
- 37: ---------------------------------------------------------------------------
- 38: Typed asset wrappers (RTEX/RMES/RMAT/SPIR-V/RSCN decoders)
- 39: ---------------------------------------------------------------------------
- 54: ---------------------------------------------------------------------------
- 55: Errors
- 56: ---------------------------------------------------------------------------
- 97: ---------------------------------------------------------------------------
- 98: Slot (internal storage)
- 99: ---------------------------------------------------------------------------
- 135: ---------------------------------------------------------------------------
- 136: Eviction Policy
- 137: ---------------------------------------------------------------------------
- 151: ---------------------------------------------------------------------------
- 152: Memory Tracker
- 153: ---------------------------------------------------------------------------
- 204: ---------------------------------------------------------------------------
- 205: Resource Manager
- 206: ---------------------------------------------------------------------------
- 264: 0 reserved for null handle
- 270: ------------------------------------------------------------------
- 271: Memory budget control
- 272: ------------------------------------------------------------------
- 317: Build eviction candidate list: (slot, last_access, size, load_epoch)
- 338: Sort by access time (oldest first)
- 342: FIFO orders by when the data was loaded, not last access.
- 366: ------------------------------------------------------------------
- 367: Package management
- 368: ------------------------------------------------------------------
- 447: `id` is a hex string like "0x0000000100000001".
- 489: Extend the vec to include this index.
- 498: track uncompressed size
- 513: ------------------------------------------------------------------
- 514: Asset lookup
- 515: ------------------------------------------------------------------
- 546: ------------------------------------------------------------------
- 547: Dependency resolution (topological)
- 548: ------------------------------------------------------------------
- 560: Now load the requested asset itself.
- 576: false = temp (in-progress)
- 581: Load in order (dependencies first).
- 583: Only load if not already loaded.
- 598: already processed
- 600: Cycle detected — just warn and skip this branch
- 607: mark as in-progress
- 609: Find the package that contains this asset and get its deps.
- 622: mark as done
- 632: ------------------------------------------------------------------
- 633: Synchronous loading
- 634: ------------------------------------------------------------------
- 640: If already loaded, return a copy.
- 649: Find and read from a package.
- 652: Check memory budget before storing.
- 656: Try to evict.
- 657: free +1MB margin
- 675: Store.
- 707: If data is already loaded, update access time and return handle.
- 708: Validate `T` even on the cached path: callers such as
- 709: `load_with_deps` may have populated the slot as raw bytes first.
- 722: Read raw data.
- 725: Deserialize through the Asset trait.
- 731: Check memory budget.
- 752: Store data and update metadata.
- 819: ------------------------------------------------------------------
- 820: Streaming reads
- 821: ------------------------------------------------------------------
- 840: A zero chunk size would make `StreamIter` yield empty chunks forever
- 841: and make `size_hint` divide by zero; clamp it to a single byte.
- 844: 1. If already cached in slot, stream directly from the cached slice without copying.
- 857: 2. Otherwise stream from the package. A file-backed reader (#509) has no
- 858: slice to borrow, so the stored block is read into memory once and the
- 859: iterator owns it — still bounded by one asset, not by the whole `.pak`.
- 900: ------------------------------------------------------------------
- 901: Get typed references
- 902: ------------------------------------------------------------------
- 936: ------------------------------------------------------------------
- 937: Unloading
- 938: ------------------------------------------------------------------
- 983: ------------------------------------------------------------------
- 984: Pinning / lease management
- 985: ------------------------------------------------------------------
- 1118: ------------------------------------------------------------------
- 1119: Hot-reload
- 1120: ------------------------------------------------------------------
- 1166: Not loaded / already evicted: do not eagerly load it.
- 1184: Deliberately **no** `generation` bump: the slot still holds
- 1185: the same asset, just newer bytes. Bumping it here would make
- 1186: every `Handle` handed out before the reload fail with
- 1187: `GenerationMismatch` — a hot reload must refresh content
- 1188: without invalidating live handles. Generation only moves
- 1189: when a slot is recycled for a different asset.
- 1206: ------------------------------------------------------------------
- 1207: Iteration
- 1208: ------------------------------------------------------------------
- 1249: ---------------------------------------------------------------------------
- 1250: Streaming iterator
- 1251: ---------------------------------------------------------------------------
- 1349: ---------------------------------------------------------------------------
- 1350: Asset trait (for deserialization)
- 1351: ---------------------------------------------------------------------------

## crates/prism-asset/src/runtime/tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 70: Load with deps should load  dependency first  then root.
- 75: Dependency should also be loaded.
- 92: load_with_deps loads the root as raw bytes first; the typed path must
- 93: still validate T instead of returning a mismatched handle.
- 124: Raw reads must count as accesses: ids[0] is now hotter than ids[1].
- 155: Touch the first asset; FIFO must still evict it because it was loaded first.
- 178: Budget too small for both assets together.
- 183: Load first  (100 byte  should fit.
- 187: Load seconds (200 bytes — total would be 300, budget is 250).
- 188: With LRU, should evict first  to make room.
- 191: id2 should be loaded
- 206: no eviction
- 209: Budget is 100, asset is 500, no eviction → out of memory.
- 325: unload + reload changes generation
- 330: Old handle should fail now
- 343: Simulate: write  to temp file and load  into ResourceManager.
- 350: Stream without caching first
- 353: Verify all data accounted for.
- 371: A zero chunk size is clamped to one byte instead of looping forever.
- 383: #509: the reader holds the `.pak` open instead of slurping it. Replacing
- 384: the file (what `build-scenes` and the hot-reload poller do) must not
- 385: disturb the live reader; the next `on_pak_changed` then sees the new one.
- 403: Simulate a rebuild: `pack_directory` writes a temp file and renames it over
- 404: the old one, so the live reader keeps the generation it opened.
- 443: 1. Stream compressed asset from package in 128-byte chunks without caching
- 448: Asset was streamed directly and not cached in slot
- 451: 2. Stream uncompressed/streamed asset from package in 256-byte chunks
- 457: 3. Stream already loaded/cached asset
- 464: 4. Unknown asset returns None
- 489: -------------------------------------------------------------------
- 490: Typed asset decoders (TextureAsset / MeshAsset / MaterialAsset /
- 491: ShaderAsset / SceneAsset)
- 492: -------------------------------------------------------------------
- 496: Build a minimal SPIR-V buffer with magic + padding.
- 529: Minimal RSCN: magic + version 2 + entity_count 0 + env_len 0.
- 532: version
- 533: entity_count
- 534: env_len
- 544: too short
- 549: build  an RMAT blob by hand: magic + version + 18 scalars + alpha block
- 550: + 5 absent slots.
- 555: base_color
- 556: metallic, roughness
- 557: emissive
- 558: emissive_strength, normal_scale, occlusion_strength
- 559: transmission, ior, translucency, anisotropy
- 560: clearcoat, clearcoat_roughness
- 565: Mask
- 567: absent
- 600: -------------------------------------------------------------------
- 601: Path manifest -> id_by_path lookup
- 602: -------------------------------------------------------------------
- 607: No manifest -> always None.
- 627: Unknown path -> None.
- 647: Missing 'assets' arg.
- 651: ---------------------------------------------------------------------------
- 652: Hot reload (`--features hot-reload`)
- 653: ---------------------------------------------------------------------------
- 690: In-memory assets refreshed in place, and **old handle still valid** (generation unchanged) —
- 691: hot reload should not invalidate handles held by callers.
- 694: Unload then reload only goes through reader held by manager: old impl keeps opened bytes,
- 695: so this will read back `old-x`.
- 766: Budget fits 250 bytes. Both assets together would be 300 bytes.
- 771: 1. Load asset 1.
- 777: 2. Pin asset 1.
- 784: Pinning again increments count.
- 788: Unpin once.
- 793: 3. Try to load asset 2 (200 bytes).
- 794: Total would be 300 > 250. Eviction tries to free space, but asset 1 is pinned!
- 795: Eviction cannot evict asset 1, so load(id2) must fail with OutOfMemory.
- 802: Asset 1 must still be loaded and intact.
- 806: Direct evict call also refuses to evict pinned slot.
- 810: 4. Unpin asset 1 completely.
- 815: Unpinning when count is 0 returns PinUnderflow.
- 821: 5. Now that asset 1 is unpinned, loading asset 2 succeeds by evicting asset 1.
- 826: Asset 1 was evicted.
- 832: Pinning an evicted asset returns NotLoaded.
- 855: AnyHandle pinning
- 864: Stale generation handle should fail to pin / unpin.
- 876: Explicit unload resets pin count and frees data.

## crates/prism-asset/src/runtime/typed.rs

- 22: ---------------------------------------------------------------------------
- 23: Texture
- 24: ---------------------------------------------------------------------------
- 62: ---------------------------------------------------------------------------
- 63: Mesh
- 64: ---------------------------------------------------------------------------
- 102: ---------------------------------------------------------------------------
- 103: Material
- 104: ---------------------------------------------------------------------------
- 163: ---------------------------------------------------------------------------
- 164: Shader
- 165: ---------------------------------------------------------------------------
- 212: ---------------------------------------------------------------------------
- 213: Scene
- 214: ---------------------------------------------------------------------------
- 234: Light validation: RSCN magic check.
- 255: ---------------------------------------------------------------------------
- 256: Skeleton / skin / animation clip (#560 P0)
- 257: ---------------------------------------------------------------------------
- 403: ---------------------------------------------------------------------------
- 404: Atlas
- 405: ---------------------------------------------------------------------------
- 435: ---------------------------------------------------------------------------
- 436: 2D Animation (.ranim2d)
- 437: ---------------------------------------------------------------------------
- 467: ---------------------------------------------------------------------------
- 468: Tilemap (.rtilemap)
- 469: ---------------------------------------------------------------------------

## crates/prism-asset/src/runtime/typed_tests.rs

- 57: Mask
- 93: The three sub-buffer offsets are not part of `RmesInfo`; they survive
- 94: only because the original bytes are kept.

## crates/prism-asset/src/types/material_def.rs

- 5: ---------------------------------------------------------------------------
- 6: MaterialDef
- 7: ---------------------------------------------------------------------------
- 22: --- identity ---
- 28: --- PBR parameters ---
- 36: --- Texture references ---
- 43: --- IBL reference ---
- 47: --- Advanced PBR ---
- 86: ---------------------------------------------------------------------------
- 87: TextureDef
- 88: ---------------------------------------------------------------------------

## crates/prism-asset/tests/integration.rs

- 23: ---------------------------------------------------------------------------
- 24: Typetag polymorphic round-trip
- 25: ---------------------------------------------------------------------------
- 32: Must have a non-empty display name (set by `impl_asset_data!`).
- 51: ---------------------------------------------------------------------------
- 52: Tests
- 53: ---------------------------------------------------------------------------

## crates/prism-audio/src/decoder.rs

- 92: Convert planar → interleaved (L,R,L,R,...)

## crates/prism-audio/src/engine.rs

- 283: Convert interleaved samples to planar layout for DecodedAudioF32.
- 294: Build the sample resource as an ArcGc<dyn SampleResource>.
- 299: Create the sampler node with its initial state.
- 323: The sampler node registers its `SamplerState` as node state — the
- 324: only channel for observing playback completion, since Firewheel
- 325: emits no playback-finished event.
- 337: Insert a real-time spatial node on stereo outputs (#508, #1034);
- 338: mono devices keep the direct chain.
- 341: `prism-app` owns the distance curve, so the node must not
- 342: attenuate on its own (a non-positive factor disables it).
- 347: Centered until the host sends the first offset.
- 363: Connect to the graph output. Mono connects (0, 0); stereo connects
- 364: the stereo pair.
- 368: The sampler is always configured as STEREO on stereo devices
- 369: (`mono_to_stereo = true`).
- 462: `SpatialBasicNode` field order: volume(0), offset(1),
- 463: panning_threshold(2), ...
- 551: CpalStream must be dropped before FirewheelContext.
- 781: Without an audio device, play() must still create a trackable active
- 782: sound; update() can then drop the node that would never play. With a
- 783: device available the sound must be kept (the audio thread may not
- 784: have processed the first block yet).

## crates/prism-build-pipeline/examples/terrain_raw.rs

- 1: Output initial terrain (un-eroded) as raw f32

## crates/prism-build-pipeline/src/bake_gi.rs

- 73: ---- 1. Load scene into ECS and collect bake instances ----
- 87: Run hierarchy system to compute LocalToWorld from LocalTransform + Parent.
- 102: Compute AABB from instances.
- 115: ---- 2. Create headless Vulkan context ----
- 128: ---- 3. Command pool ----
- 139: ---- 4. Derive probe grid from the scene AABB ----
- 148: ---- 5. Build per-instance BLAS/TLAS + materials SSBO ----
- 158: ---- 6. Probe volume 3D texture ----
- 220: ---- 7. ProbeVolumeInfo UBO ----
- 244: ---- 8. Descriptor set layout + pool + sets ----
- 245: Set 0: b0=volume, b1=info UBO, b2=tlas, b3=vertex, b4=index,
- 246: b6=instance_meta, b7=materials
- 247: Set 1: bindless (samplers + SRVs)
- 338: ---- 9. Write descriptors ----
- 396: ---- 10. Dummy sampler + bindless writes ----
- 411: dummy view — not actually sampled
- 424: ---- 11. Create compute pipeline ----
- 456: Build push constants.
- 492: Transition volume image to GENERAL layout
- 526: Push constants.
- 535: Dispatch in 3D: numthreads is (4, 4, 1) in gi_bake.slang.
- 539: Barrier to make volume readable.
- 566: ---- 12. Submit + wait ----
- 577: ---- 13. Read back probe volume ----
- 589: Copy volume image to staging buffer
- 646: Read staging buffer
- 660: Compute hit ratios and solid counts.
- 702: ---- 14. Write .bin ----
- 732: ---- Cleanup ----
- 760: -------------------------------------------------------------------
- 761: Probe grid derivation
- 762: -------------------------------------------------------------------
- 862: 2 probes: probe 0, probe 1
- 866: Fill raw data for probe 0 and probe 1 across 9 coefficients
- 878: p=0 is valid, p=1 is solid
- 895: Check probe 0 coefficients
- 904: Check probe 1 coefficients
- 938: [numthreads(4, 4, 1)] with SV_DispatchThreadID:
- 939: group_x = (dims[0] + 3) / 4
- 940: group_y = (dims[1] + 3) / 4
- 941: group_z = dims[2]
- 943: Exact multiples of 4
- 947: Non-multiples of 4 rounding up
- 953: Zero dimensions

## crates/prism-build-pipeline/src/bake_prt.rs

- 92: 1. Load scene into ECS and collect world-space geometry.
- 115: 2. Identify active candidate leaves near scene geometry.
- 127: 3. Stage 1: Ray-traced SH64 transfer sampling.
- 140: 4. Stage 2: PCA + k-means clustering.
- 151: 5. Stage 3: Build 64-tree (BFS node table).
- 450: Power-iteration PCA for top components (up to 16).
- 493: Deterministic k-means initialization (strided).
- 540: Reconstruct full 64-coeff cluster centroids.
- 581: Build levels bottom-up: level 0 = root (1x1x1), ..., last = leaves (grid_res).
- 589: Store occupied nodes per level: (x, y, z) -> (child_mask, Option<leaf_index>)
- 611: Flatten into BFS-ordered PrtNode array.
- 706: Generate a synthetic 500m x 500m city block grid of triangles.
- 722: leaf_grid = 2

## crates/prism-build-pipeline/src/heightmap/hydraulic.rs

- 38: Only discard old buffer when image dimensions change (at most once per pipeline lifecycle).
- 79: 1. Generate particles (deterministically generated by round_seed)
- 82: 2. Chunk: block count not exceeding thread count, block size rounded up, ensuring all particles are processed.
- 83: old impl did `n_particles / n_threads` rounded down then `thread_idx * chunk_size`,
- 84: interval range; tail particles silently dropped when `particle_count % n_threads != 0`.
- 88: 3. Process particles in parallel: each block writes its own local buffer (one per block)
- 103: 4. Merge modifications into heightmap (per-pixel parallel; thread count × full-image ordered merge is expensive on large image)
- 115: 5. Update min/max (same parallel reduction)
- 138: Initial water amount slightly randomized to give particles variation
- 170: 1. Compute gradient
- 175: 2. Update direction (inertial mixing)
- 183: When gradient is zero, deterministic random walk using particle-embedded seed
- 192: 3. Velocity update & clamp
- 200: 4. Move
- 204: Boundary check
- 212: 5. Sediment capacity
- 216: 6. Erode / deposit
- 218: Deposit
- 221: Deposit into current pixel region (weighted spread, avoids salt-and-pepper noise)
- 224: Erode
- 228: Reduce erosion below sea level
- 238: 7. Update water amount & position
- 256: 3×3 kernel: center 4/9, four-neighbor 1/9, corner 1/18 (approximates Gaussian)

## crates/prism-build-pipeline/src/heightmap/hydraulic_tests.rs

- 11: Slope: left below low, right on high
- 26: should have changes (erosion in bottom, deposition)
- 46: Flat terrain should almost not change
- 69: Same size again for another round: pointer unchanged (no heavy new allocation), but content cleared to zero.
- 84: 12 lines × 200,000 particles not evenly divisible; old implementation drops tail 8 particles.

## crates/prism-build-pipeline/src/heightmap/mod.rs

- 20: ---------------------------------------------------------------------------
- 21: Heightmap
- 22: ---------------------------------------------------------------------------
- 158: ---------------------------------------------------------------------------
- 159: ErosionParams
- 160: ---------------------------------------------------------------------------
- 165: ---- Hydraulic parameters ----
- 178: ---- Thermal parameters ----
- 183: ---- Global controls ----
- 197: ---- Output controls ----
- 204: Hydraulic
- 214: Thermal
- 217: Control
- 226: output
- 232: ---------------------------------------------------------------------------
- 233: Particle
- 234: ---------------------------------------------------------------------------
- 264: ---------------------------------------------------------------------------
- 265: Public API
- 266: ---------------------------------------------------------------------------
- 275: Convert to relative height (optional)
- 280: First phase: strong thermal preprocessing
- 285: Second phase: hydraulic + thermal alternating iteration
- 286: 
- 287: Accumulation buffer (one full graph per thread) reused throughout pipeline: default 100 hydraulic rounds not again
- 288: Each round heavy new allocates a "thread count × full graph" `f64` table.
- 300: Restore absolute height (optional)

## crates/prism-build-pipeline/src/heightmap/terrain.rs

- 8: ---------------------------------------------------------------------------
- 9: Hash / noise
- 10: ---------------------------------------------------------------------------
- 30: Keep the 53 bits an f64 mantissa can represent exactly.
- 39: Smooth interpolation (3rd-order Hermite).
- 70: ---------------------------------------------------------------------------
- 71: Terrain synthesis
- 72: ---------------------------------------------------------------------------
- 85: Frequency scale: 512 m tile → 1.0; 1024 m → 0.5 (same physical period).
- 88: Each layer uses different seed bitwise, avoid all octaves aligning to produce mesh feel.
- 98: Domain distortion: low frequency noise offsets sample coordinates, ridge direction meanders with it.
- 103: Ridge noise: |fbm| absolute → f=0 is ridge line (1.0), both sides fall off (0.0).
- 114: base ∈ [−1,1]; ridge ∈ [0,1] → translate/scale back to [−1,1]; detail ∈ [−1,1]
- 121: Normalize to [0, 1], ensure output range consistent for different seeds.

## crates/prism-build-pipeline/src/heightmap/terrain_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 9: Lattice coordinates and the seed are the only inputs; integer mixing
- 10: must reproduce the same bits on every platform (a sine hash did not).
- 47: Different seeds must produce clearly different terrain (tolerance: at least 90% pixels different).

## crates/prism-build-pipeline/src/heightmap/tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 25: (1,1)
- 28: bilinear: (0+100+0+0)/4

## crates/prism-build-pipeline/src/heightmap/thermal.rs

- 22: Double buffering: src = hm.data (read-only), dst = write.
- 23: Each pixel independent net change calculation (stream out + stream in), rayon parallel no race.
- 29: Its own stream out amount: current pixel higher than neighbor and exceeds repose angle threshold
- 47: Neighbor stream into current pixel amount: neighbor higher than current pixel and exceeds repose angle threshold
- 71: Update min/max

## crates/prism-build-pipeline/src/heightmap/thermal_tests.rs

- 9: flat terrain should stay flat
- 17: Single spike in flat terrain
- 19: spike at center
- 28: spike should be lower
- 30: neighbors should be higher

## crates/prism-build-pipeline/src/main.rs

- 398: 1. Generate initial terrain: frequency scaled to physical tile size
- 409: 2. Water + thermal erosion (skipped with --no-erode)
- 431: 3. Map to real elevation range (e.g. −11 km ~ +8.85 km)
- 432: After erosion relative height is not [0,1] and distribution is skewed (water erodes valley floor),
- 433: first normalize, then use percentile to position sea level—sort normalized data,
- 434: take the sea_frac percentile value as sea level, ensuring seabed/land ratio
- 435: strictly equal to sea_level setting (0 m is exactly sea level).
- 436: Land segment with gamma stretch (<1 raises low areas) avoids 99% land bunched at low end.
- 454: Sea level = 0 m under absolute elevation
- 463: 4. Write f32 LE raw

## crates/prism-build-pipeline/src/migrate.rs

- 147: Old file kept, user can delete after confirming.
- 168: Second time won't write again (new file already exists).
- 183: Old project: only gave default new path.

## crates/prism-build-pipeline/src/scene_pack.rs

- 106: Clearing a generated staging directory is safe only when we created it.
- 107: The marker also survives a failed pack attempt, so the next run can retry.
- 158: Scene-referenced resources must all go into package (#530). Any reference that cannot be resolved is collected into
- 159: `missing`, all reported at build end---otherwise package looks fine but runs as
- 160: empty scene, just one warn in log.
- 163: Scene-declared env cube must be inside package, otherwise runtime IBL falls back to empty env (black screen).
- 166: Same for 2D sprite texture: `SpriteRenderer.sprite_path` PNG must be baked into package
- 167: RTEX format expected at runtime, otherwise sprite forever cannot find its texture.
- 169: 3D mesh & material (#530): previously only scene cooked; imported model disappears after packaging.
- 173: UI component texture / font (#591 / #583): `Style.image`, `Tilemap.tileset_path`,
- 174: `Text.font` similarly must go into package, otherwise icons, tilesets, and project fonts all disappear after packaging.
- 176: Scene audio (#990): `AudioSource.sound_path` was never packaged before; release build is silent.
- 178: Sprites generated at runtime (bg layer, fish, effects) not in scene JSON, so entire
- 179: `sprites/` directory must be baked into package---consistent with engine "assets dir IS content" principle.
- 181: One-shot sound (#987) same: `GameWorld::play_sound("audio/xxx.wav")` is runtime string; build-time cannot enumerate, so whole `audio/` directory is packaged as-is.
- 183: Project data (#1049): level config etc JSON also read at runtime by path string, no component reference to enumerate;
- 184: `data/` whole directory packaged as-is.
- 190: Runtime always looks up the manifest as `scenes.json` inside the package.
- 191: Rewrite authoring `.scene.json` paths to the cooked `.rscn` names.
- 205: #976: Write build stamp into package; runtime judges package freshness by it (old package doesn't have this,
- 206: will warn "built by older pipeline" on first mount).
- 265: When no author doc, only repackage existing `.rscn` as-is: it came from which tool,
- 266: which tool version baked it, build side completely cannot tell (cook stamp's
- 267: `sources_mtime` also only counts document). At least say it, so
- 268: "picture wrong but logs all green" happens again (#976).
- 773: glTF/GLB comes with PBR material and texture (#520): cook together as RMAT/RTEX,
- 774: Path and scene auto-completed `material_paths` completely consistent.
- 847: First register image path and ID into database, importer can parse dependencies by slot order.
- 866: Data DB key must exactly match material importer query string.
- 1107: Per-asset import settings (#514): `<asset>.import.json` overrides profile max_size / mips.
- 1255: Both candidates not present: return scene-relative path, letting caller print original target.
- 1276: Let `SceneCooker` produce the canonical parse error.
- 1346: Let `SceneCooker` produce the canonical parse error.
- 1444: Let `SceneCooker` produce the canonical parse error.
- 1492: Only skeleton no weight (or incorrect incoming) means no skinnable mesh to draw, keep static.
- 1527: Material follows MeshRenderer (author-written first, then auto-filled glTF material from one round).
- 1660: Material and shadow mode follow MeshRenderer (author-written one).
- 1684: MeshRenderer must stay: packer still relies on mesh_path to stage glTF.
- 1813: Fonts copied as-is into package (fontdue reads bytes directly), textures cooked into RTEX.
- 1866: When referencing non-existent audio, build must fail, not silently output a soundless package.
- 1996: Scene itself still referenced by base package's `scenes.json` manifest.
- 2127: RSCN is string table + binary, generated material path must appear in scene inside.
- 2349: #976: Package always includes a build stamp (sorted by path, `__` sorts before letters).
- 2364: The runtime reads this manifest from inside the package; every
- 2365: cooked scene path it declares must resolve through the same package.
- 2369: Author-written is `.scene.json`, package must rewrite to cooked `.rscn`.
- 2383: End-to-end: the packaged RSCN bytes must be loadable by the runtime
- 2384: decoder with the same component registry used by the game.
- 2416: A subsequent run may clear its own generated files.
- 2450: Uncompressed 2-pixel scanline (RGBE: 128,128,128,128 = ~0.5,0.5,0.5)
- 2465: Cook, then drop the authoring doc so only the `.rscn` remains.
- 2509: 1) First cook an "old" `.rscn` (empty box scene), then leave it in assets inside.
- 2517: 2) Author document changed to another scene (this inside only changes content, not delete old `.rscn`).

## crates/prism-cooker/src/aseprite.rs

- 132: CEL chunk
- 142: TAGS chunk
- 146: SLICE chunk
- 191: Raw cel
- 201: Compressed cel (zlib)
- 219: Blit RGBA8 cel onto canvas
- 300: Only the first key is read: the runtime slice format stores a single
- 301: rect per slice, so the remaining keys of a multi-key slice are ignored.
- 310: Skip 9-slice center (16 bytes)

## crates/prism-cooker/src/atlas.rs

- 119: version
- 173: Completely transparent sprite retains 1x1 placeholder to avoid zero-dimension divisions
- 195: ---------------------------------------------------------------------------
- 196: MaxRects 2D Bin Packing Algorithm (Best Short Side Fit - BSSF)
- 197: ---------------------------------------------------------------------------
- 353: ---------------------------------------------------------------------------
- 354: Atlas Packing Pipeline
- 355: ---------------------------------------------------------------------------
- 434: Sort descending by height, then width (standard bin packing heuristic)
- 462: Grow the atlas until every slot fits, then keep that placement.

## crates/prism-cooker/src/cooker/anim_graph.rs

- 94: V2: per-sample speed table.
- 235: Parse connection name -> index.

## crates/prism-cooker/src/cooker/anim_graph_tests.rs

- 73: cached -> slot
- 107: slot -> cached -> slot

## crates/prism-cooker/src/cooker/mod.rs

- 16: RTEX format bytes are part of the wire format; the single source of truth
- 17: lives in `prism-asset::formats` (shared with the runtime decoder).
- 34: ---------------------------------------------------------------------------
- 35: Errors
- 36: ---------------------------------------------------------------------------
- 53: ---------------------------------------------------------------------------
- 54: Cook Context & Result
- 55: ---------------------------------------------------------------------------
- 93: ---------------------------------------------------------------------------
- 94: Cooker trait
- 95: ---------------------------------------------------------------------------
- 109: ---------------------------------------------------------------------------
- 110: Cooker Registry
- 111: ---------------------------------------------------------------------------
- 171: ===========================================================================
- 172: Built-in Cookers
- 173: ===========================================================================
- 175: ---------------------------------------------------------------------------
- 176: Binary Cooker (pass-through)
- 177: ---------------------------------------------------------------------------
- 199: ---------------------------------------------------------------------------
- 200: Animation graph cooker (pass-through)
- 201: ---------------------------------------------------------------------------
- 229: ---------------------------------------------------------------------------
- 230: Texture Cooker — decodes intermediate RTXI → generates mip chain → RTEX
- 231: ---------------------------------------------------------------------------
- 270: Skip byte 12 (channels) and byte 13 (format).
- 319: Premultiplied R, G, B followed by the plain alpha sum.
- 356: 2×2 box filter in premultiplied space.
- 364: Guard against source dimensions (not the target).
- 512: Header: magic(4) + version(1) + w(4) + h(4) + levels(4) + format(1) + offsets(levels*4)
- 518: version
- 519: base width
- 520: base height
- 522: 0 = RGBA8
- 524: Reserve space for offsets.
- 528: Write mip data and record offsets.
- 559: Per-asset import setting (`<asset>.import.json`) can override profile max_size / mips:
- 560: First squash longest edge into max_size, then generate mip chain as needed.
- 568: Format byte: desktop BC1/BC3/BC5/BC7 use pure Rust encoder; mobile ASTC /
- 569: ETC2 use vendored astcenc / etcpak. Only BC6H (HDR) has no encoder yet,
- 570: Explicit warning then fall back to RGBA8 (not silently masquerade as already compressed, #519).
- 630: Block-compressed mip data already tight enough, adding zstd layer yields little; uncompressed RGBA8
- 631: pixels compress well (generally save 30~50% volume), so let it into pak compression stream (#519).
- 639: ---------------------------------------------------------------------------
- 640: RTEX decoder — cooked runtime texture → structured mip data
- 641: ---------------------------------------------------------------------------
- 662: unknown version
- 679: Read offset table.
- 686: Extract each mip's data.
- 717: ---------------------------------------------------------------------------
- 718: Mesh Cooker — validates RMXI intermediate → serialises RMES runtime format
- 719: ---------------------------------------------------------------------------
- 871: floats per vertex
- 885: stride in bytes
- 1058: Keep streamed RCLD pages uncompressed so runtime can pread single pages directly.
- 1074: ---------------------------------------------------------------------------
- 1075: RMES decoder — cooked runtime mesh → structured vertex/index data
- 1076: ---------------------------------------------------------------------------
- 1101: unknown version
- 1165: ---------------------------------------------------------------------------
- 1166: Material Cooker (RMATI intermediate -> RMAT runtime format)
- 1167: ---------------------------------------------------------------------------
- 1214: Header: magic(5) + version(1) + scalars(72) = 78 bytes minimum.
- 1301: The importer already resolved texture paths to AssetId dependencies
- 1302: stored on the record. We walk the path records in slot order and
- 1303: look each up in the dependency list by matching path -> id via db.
- 1304: However, dependencies is just Vec<AssetId> without path info, so we
- 1305: re-resolve paths against the database here (the db is not on
- 1306: CookContext). Instead, we rely on record.dependencies preserving the
- 1307: slot order from the importer. Map them positionally.
- 1336: ---------------------------------------------------------------------------
- 1337: RMAT decoder - cooked runtime material -> structured data
- 1338: ---------------------------------------------------------------------------
- 1397: ---------------------------------------------------------------------------
- 1398: Shader Cooker (RSLI intermediate -> SPIR-V via slangc)
- 1399: ---------------------------------------------------------------------------
- 1435: Header: magic(4) + version(1) = 5 bytes minimum.
- 1487: Write source to a temp file so #include / module resolution works.
- 1492: RAII guard: removes temp files on drop, success or error.
- 1547: Sanity check: SPIR-V starts with the magic 0x07230203 (LE).
- 1591: SPIR-V doesn't compress well; skip zstd overhead.
- 1597: ---------------------------------------------------------------------------
- 1598: Default Registry
- 1599: ---------------------------------------------------------------------------
- 1614: ---------------------------------------------------------------------------
- 1615: Cook Pipeline
- 1616: ===========================================================================
- 1646: Collect records in dependency order (topological sort).
- 1702: ===========================================================================
- 1703: Topological Sort
- 1704: ===========================================================================
- 1717: DFS-based topological sort with cycle detection.

## crates/prism-cooker/src/cooker/profile.rs

- 20: ===========================================================================
- 21: Errors
- 22: ===========================================================================
- 42: ===========================================================================
- 43: Core Enums
- 44: ===========================================================================
- 104: ===========================================================================
- 105: Settings Structs (final merged configuration)
- 106: ===========================================================================
- 114: 0–100
- 115: 0 = unlimited
- 169: "zstd" or "none"
- 209: ===========================================================================
- 210: Partial overrides (what a profile file actually contains)
- 211: 
- 212: A profile file only describes the fields it wants to change. Using the
- 213: fully-materialised `*Settings` structs for that is wrong: `#[serde(default)]`
- 214: fills every missing field with a default value, so merging cannot tell
- 215: "absent" from "explicitly set to the default".
- 216: ===========================================================================
- 362: unit separator: between key and value
- 368: record separator: between different entries
- 375: ===========================================================================
- 376: Profile Struct (JSON-deserializable, partial)
- 377: ===========================================================================
- 434: ===========================================================================
- 435: Profile Manager
- 436: ===========================================================================
- 459: Check built-in cache FIRST (only built-ins are cached).
- 464: Check for a user file. Not cached to avoid shadowing the built-in
- 465: when a user profile inherits via `base: "desktop"`.
- 477: Fall back to built-in default.
- 511: Start from either the parent's resolved settings or the defaults.
- 517: Overlay only the fields this profile explicitly specifies.
- 605: ===========================================================================
- 606: Built-in Default Profiles
- 607: ===========================================================================
- 612: ── base.json ────────────────────────────────────────────────
- 642: ── desktop.json ─────────────────────────────────────────────
- 666: ── android.json ─────────────────────────────────────────────
- 692: ── ios.json ─────────────────────────────────────────────────
- 718: ── embedded.json ────────────────────────────────────────────

## crates/prism-cooker/src/cooker/profile_tests.rs

- 1: ===========================================================================
- 2: Tests
- 3: ===========================================================================
- 41: Same key-value pairs, incorrect insert order, plus different HashMap capacity: capacity changes bucket layout,
- 42: therefore iteration order definitely differs from forward, able to stably expose "directly serialize HashMap" issue.
- 189: Desktop/default profiles keep the old all-compressed layout.
- 262: Write a custom  config  that inherits from built-in "desktop".
- 276: Should still inherit desktop base features.
- 279: Best-effort cleanup.
- 286: CLI overrides > resolved config
- 300: Other Android settings preserved.
- 361: Every value here equals the built-in default; they must still override
- 362: the parent profile instead of being treated as "not specified".
- 379: Sections the child does not mention still inherit from `desktop`.
- 392: `optimize` defaults to true, so `false` used to be indistinguishable
- 393: from an absent field.

## crates/prism-cooker/src/cooker/scene.rs

- 14: ---------------------------------------------------------------------------
- 15: SceneCooker
- 16: ---------------------------------------------------------------------------
- 48: `SceneJson::parse` migrates older authoring documents (e.g. v1
- 49: `transform.translation`) up to the current schema before decoding.
- 84: ---------------------------------------------------------------------------
- 85: RSCN header helpers
- 86: ---------------------------------------------------------------------------

## crates/prism-cooker/src/cooker/scene_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 9: ── helpers ───────────────────────────────────────────────────────
- 12: Fixtures below are v1 (`transform.translation`); `parse` migrates them.
- 34: ── sample  scene JSON ────────────────────────────────────────────
- 72: ── tests ─────────────────────────────────────────────────────────
- 137: magic + version + entity_count
- 148: transform
- 152: type_id
- 292: 4 UI entities + intro scene's own camera (black fade-in end after cannot reveal clear color, #409).
- 294: Each UI entity carries `Node` + `Style` (`ComputedLayout` is runtime output, not in scene).
- 295: Camera entity not counted in UI, so this filters by "has / no Node".

## crates/prism-cooker/src/cooker/skinned.rs

- 40: flags
- 41: pad
- 148: Joint index and weight are two independent streams (first all joints, then all weights) — decode side reads per stream.
- 240: Event table (#560 P1 Notify): written out sorted by time, so same source file always cooks same bytes.
- 241: Notify order inside clip has no semantic.
- 255: V3 sync flag (#560): normalize bitwise, ascending, deduplicate, guaranteeing same author data cooks same
- 256: out same bytes. Non-limited values directly discarded.
- 271: V4 Montage segments (#560 P4): written out sorted by start, segments with empty name / illegal time directly
- 272: discarded; same author data always gets same bytes each cook.
- 298: V5 MotionWarping window (#560 P4): same sort / filter rule.

## crates/prism-cooker/src/cooker/skinned_tests.rs

- 54: Deliberately not normalized: should be fixed at encode time.
- 79: Deliberately out of order: encoder must sort by time.
- 92: Non-normalized quaternion: decode side will normalize.
- 122: Decode side also blocks: manually change root bone parent to 1.
- 256: v2: event table written out sorted by time.
- 283: v4 compat: drop trailing warp window table, write version back to 4.
- 285: warp_window_count
- 293: v3 compat: again drop section table, write version back to 3, events / flags still decodable.
- 295: warp_window_count
- 296: section_count
- 304: v2 compat: again drop sync marker table, write version back to 2, event table still decodable.
- 306: warp_window_count
- 307: section_count
- 308: marker_count
- 317: v1 compat: again drop event table from v2 (notify_count = 0, 4 bytes), clip still decodable.
- 319: warp_window_count
- 320: section_count
- 321: marker_count
- 322: notify_count (empty)
- 355: Illegal segment: encoder discards, not written into asset.
- 381: Normalize, sort, deduplicate, drop non-limited values.

## crates/prism-cooker/src/cooker/state_machine_tests.rs

- 46: "*" → any state.
- 49: encode → decode must recover field by field (cooked out is what runtime sees).
- 134: Target index out of bounds.
- 139: Source state index out of bounds.
- 144: Empty clip path.
- 149: Condition with empty name: writing it out will never match on anyway.
- 156: Non-limited threshold rejected as well.
- 164: No state.
- 184: Negative duration = hard cut, ratio beyond range clamped to 1, NaN to 0.
- 197: Non-limited speed falls back to 1.0, not writing NaN into asset.

## crates/prism-cooker/src/cooker/tests.rs

- 1: ===========================================================================
- 2: Tests
- 3: ===========================================================================
- 50: A depends on B. B depends on C.
- 56: C must come before B, B before A.
- 70: A depends on B, B depends on A (cycle).
- 75: Both should be present despite the cycle.
- 191: No data for the resource
- 201: ── texture  Cooker new tests ─────────────────────────────────────
- 208: channels
- 209: format RGBA8
- 216: 4×4 RGBA red graph pixel
- 234: Verify RTEX magic.
- 236: version
- 238: Base width/height.
- 244: Mip level count: 4→2→1 = 3 levels.
- 248: Format
- 249: RGBA8
- 251: Offsets table (levels * 4 bytes after header).
- 265: Mip0: 4*4*4 = 64 byte  starting at header (18 + 12 = 30)
- 268: Mip1: 2*2*4 = 16 byte
- 271: RGBA8 mip chain still worth handing to pak zstd (#519).
- 277: Uncompressed pixels compress well (zstd generally saves 30~50%); only block-compressed ones should skip
- 278: outer layer compression. Previously always `compress: false` due to #519 issue.
- 302: Desktop profile BC1/BC3/BC5/BC7 all use real encoder; 4×4 fits exactly one block.
- 346: 8×8 → 4×4 → 2×2 → 1×1; BC1 each level is 4×4 block (8 bytes), edge padded.
- 372: ASTC 4×4/6×6/8×8/12×12 and ETC2 RGBA all use real encoder;
- 373: 4×4 input correct any block size produces at least one 16-byte block (#519).
- 418: 8×8 → 4×4 → 2×2 → 1×1; 6×6/4×4 block edges padded,
- 419: Each level is ceil(w/block) * ceil(h/block) * 16.
- 474: 8×4 red graph, longest edge squashed to 4 → 4×2; mip chain 4×2 → 2×1 → 1×1 = 3 levels.
- 496: Small graph will not be upscaled.
- 562: ── mesh  Cooker new tests ────────────────────────────────────────
- 571: version
- 575: Fill vertex data (positions + normals + uv
- 603: Verify RMES magic.
- 605: version (v2 = submesh table)
- 616: pos + nrm + uv = 8 floats * 4
- 618: Offsets.
- 620: after 33-byte header
- 677: ── Round-trip: cook → decode → assert ───────────────────────────
- 692: Binary cooker is pass-through; cooked data must be same
- 698: Build a small 8×6 gradient RGBA8 graph pixel
- 704: R varies with x
- 705: G varies with y
- 706: B constant
- 707: A opaque
- 724: decode  RTEX after
- 728: RGBA8
- 731: Mip0 must be byte-identical to the input  pixels (cooker copies mip0 verbatim).
- 737: Mip chain must be non-empty and each successive level must be
- 738: smaller (or equal to  at 1×1).
- 754: too short
- 755: Wrong version.
- 763: Build RMXI intermediate with 3 vertices (a triangle).
- 767: pos + nrm + uv
- 771: version
- 776: Positions: a simple triangle
- 778: Normals: all pointing on
- 780: UVs
- 793: Indices
- 802: Cook
- 815: decode  RMES.
- 821: Vertex data must match intermediate (after its 17-byte header).
- 828: Index data must match
- 839: 6 vertices, two triangles, no normals: two submeshes each using 3 indices.
- 844: v2: submesh table at end
- 847: uv channels
- 857: submesh count
- 957: Wrong version.
- 965: Use the same mode  as texture_cooker_generates_mips test.
- 988: mip0 = 4*4*4 = 64 byte
- 990: mip1 = 2*2*4 = 16 byte
- 992: mip2 = 1*1*4 = 4 byte
- 1002: 4 channels, so 2×2 graph pixel with 4 bytes per pixel = 16 bytes
- 1013: default  config  compress  (Rgba8) must produce RGBA 8 output
- 1021: Rgba8
- 1029: Must be RGBA8 format
- 1034: -------------------------------------------------------------------
- 1035: material  cooker round-trip
- 1036: -------------------------------------------------------------------
- 1046: Mask
- 1066: base_color
- 1067: metallic, roughness
- 1068: emissive
- 1069: emissive_strength, normal_scale, occlusion_strength
- 1070: transmission, ior, translucency, anisotropy
- 1071: clearcoat, clearcoat_roughness
- 1091: decode  and validate
- 1102: base_color (red)
- 1103: metallic, roughness
- 1104: emissive
- 1105: emissive_strength, normal_scale, occlusion_strength
- 1106: transmission, ior, translucency, anisotropy
- 1107: clearcoat, clearcoat_roughness
- 1109: Only albedo + occlusion textures present.
- 1119: The importer would have resolved these two paths to two AssetId deps
- 1120: stored on the record, in slot order (albedo first, occlusion seconds
- 1162: -------------------------------------------------------------------
- 1163: shader  cooker (intermediate parsing only; compile  needs slangc)
- 1164: -------------------------------------------------------------------
- 1170: version
- 1210: Truncated RSLI: magic + version + partial header.
- 1230: --- Premultiplied alpha in texture downscale / mip generation (#1037) ---
- 1234: Transparent red next to opaque green: the reduced pixel must stay green
- 1235: with half alpha instead of picking up the red.
- 1257: 2x2: three transparent red pixels and one opaque green pixel.

## crates/prism-cooker/src/db/mod.rs

- 18: ---------------------------------------------------------------------------
- 19: Errors
- 20: ---------------------------------------------------------------------------
- 48: ---------------------------------------------------------------------------
- 49: Asset State
- 50: ---------------------------------------------------------------------------
- 63: ---------------------------------------------------------------------------
- 64: Asset Record
- 65: ---------------------------------------------------------------------------
- 107: ---------------------------------------------------------------------------
- 108: Import Cache Entry
- 109: ---------------------------------------------------------------------------
- 124: ---------------------------------------------------------------------------
- 125: Asset Database
- 126: ---------------------------------------------------------------------------
- 156: ------------------------------------------------------------------
- 157: Accessors
- 158: ------------------------------------------------------------------
- 215: ------------------------------------------------------------------
- 216: Mutators
- 217: ------------------------------------------------------------------
- 225: Check for duplicate path.
- 235: Replace if exists, else push.
- 273: ------------------------------------------------------------------
- 274: Persistence
- 275: ------------------------------------------------------------------
- 313: ------------------------------------------------------------------
- 314: Helpers
- 315: ------------------------------------------------------------------
- 334: ---------------------------------------------------------------------------
- 335: Import Cache
- 336: ---------------------------------------------------------------------------
- 437: ---------------------------------------------------------------------------
- 438: Helpers
- 439: ---------------------------------------------------------------------------
- 478: Strip leading `./`
- 482: Strip leading `/`
- 486: Strip trailing `/`

## crates/prism-cooker/src/db/tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 82: Add a record so we have multiple
- 181: A second save replaces the file in place and leaves no staging file.
- 208: The next command can rebuild and save to the same path.

## crates/prism-cooker/src/import_settings.rs

- 117: Default fields don't override profile.

## crates/prism-cooker/src/importer/mod.rs

- 36: ---------------------------------------------------------------------------
- 37: Errors
- 38: ---------------------------------------------------------------------------
- 61: ---------------------------------------------------------------------------
- 62: Import Context
- 63: ---------------------------------------------------------------------------
- 86: ---------------------------------------------------------------------------
- 87: Import Result
- 88: ---------------------------------------------------------------------------
- 112: ---------------------------------------------------------------------------
- 113: Import File Result (returned by ImportPipeline::import_file)
- 114: ---------------------------------------------------------------------------
- 125: ---------------------------------------------------------------------------
- 126: Importer trait
- 127: ---------------------------------------------------------------------------
- 150: ---------------------------------------------------------------------------
- 151: Importer Registry
- 152: ---------------------------------------------------------------------------
- 223: ---------------------------------------------------------------------------
- 224: Import Pipeline
- 225: ---------------------------------------------------------------------------
- 286: Find importer.
- 292: Check cache (unless the caller asked for a forced re-import).
- 301: Run import.
- 311: Update database.
- 323: Update cache.
- 365: ===========================================================================
- 366: Built-in Importers
- 367: ===========================================================================
- 369: ---------------------------------------------------------------------------
- 370: Raw / Binary Importer
- 371: ---------------------------------------------------------------------------
- 386: catch-all
- 400: ---------------------------------------------------------------------------
- 401: Texture Importer (real decode)
- 402: ---------------------------------------------------------------------------
- 444: switched from raw pass-through to real decode
- 495: ---------------------------------------------------------------------------
- 496: glTF Mesh Importer
- 497: ---------------------------------------------------------------------------
- 583: Estimate capacity
- 590: positions
- 598: V2: index block after appending submesh table (#520). Importers without submesh
- 599: (OBJ / hand-made mesh) continue writing v1, cooker accepts both.
- 653: Skin bones + animation computed first: vertex stream JOINTS_0 needs "skin local index →
- 654: global bone index" heavy map table (#560).
- 666: Material / texture slice first into metadata (#520): many material render and texture asset storage need change
- 667: `.scene.json` and DrawItem, this version only keeps source file info fully below.
- 731: Scene graph: node-level TRS directly baked into vertices, so "one mesh asset" is entire tree
- 732: in world space appearance (not just first mesh first primitive again, #520).
- 740: No scene: treat all nodes without parent node as roots.
- 760: No node but file has mesh: fall back to "has mesh, single identity transform", don't let legal file be read for nothing.
- 767: Without node info cannot bind skin: vertices keep unskinned (mesh still renderable).
- 785: Any primitive missing JOINTS_0/WEIGHTS_0: entire mesh no skinning,
- 786: To avoid half-skinned data skewing entire skeleton.
- 1059: One RTEX per glTF image. Materials point at these by image index.
- 1155: The implicit glTF default material has no alpha modes: it is
- 1156: fully opaque and must not be alpha-tested.
- 1163: Skinning / animation (#560): derived like material by "main asset path + suffix",
- 1164: Guarantee same source file always cooks to same path each time.
- 1209: Author event table / sync flag (#560): merged by clip original name, key is glTF animation name.
- 1275: Wrongly named event table should not be silently lost: author thinks it's that clip, actually none merged.
- 1666: `node.skin()` only applies to this node's own mesh; child nodes each carry their own skin.
- 1684: Only bake triangles; line/point classes first skipped and counted (not in supported range in this version).
- 1747: JOINTS_0 / WEIGHTS_0 (#560): only retained when entire mesh has skinning.
- 1759: Indices outside skin all clamped to 0: bad data should not invalidate entire mesh.
- 1790: Non-indexed mesh: add ordered index, not directly reject (legal glTF).
- 1814: V2: metadata adds submeshes / materials / images (#520),
- 1815: Old import cache needs heavy re-run to get these fields.
- 1869: ---------------------------------------------------------------------------
- 1870: OBJ mesh  Importer
- 1871: ---------------------------------------------------------------------------
- 1982: `o` / `g` / `s` / `mtllib` / comments etc.: OBJ nodes and material groups
- 1983: ignored before entering intermediate format, just ignore.
- 1994: When `all_*` true, each vertex has that property; `unwrap` before length/content guaranteed by construction.
- 2160: ---------------------------------------------------------------------------
- 2161: Animation graph importer (*.animgraph.json -> RAGP intermediate)
- 2162: ---------------------------------------------------------------------------
- 2203: Clip / state machine path is resource dependency; path not found kept as-is (reported at build time),
- 2204: not rejecting entire graph because asset library not yet scanned.
- 2238: ---------------------------------------------------------------------------
- 2239: JSON Importer
- 2240: ---------------------------------------------------------------------------
- 2263: Validate JSON.
- 2281: ---------------------------------------------------------------------------
- 2282: Material Importer (.mat.json -> RMATI intermediate)
- 2283: ---------------------------------------------------------------------------
- 2396: 16 scalar floats (64 bytes).
- 2416: Alpha semantics: mode byte + cutoff float.
- 2429: 5 texture path records.
- 2434: present
- 2439: absent
- 2457: `.mat.json` or `.mat` files. We match `.mat.json` so plain JSON files
- 2458: still fall through to JsonImporter; bare `.mat` is also accepted.
- 2477: Collect the 5 texture paths in slot order.
- 2486: Resolve each path to an AssetId dependency via the database.
- 2498: The texture asset isn't registered yet; warn and leave
- 2499: the slot empty. The material still imports.
- 2527: ---------------------------------------------------------------------------
- 2528: Shader Importer (.slang -> RSLI intermediate)
- 2529: ---------------------------------------------------------------------------
- 2561: Path-tracing compute shaders use `ptMain` per compile.sh.
- 2609: version
- 2653: Resolve entry / stage / profile.
- 2681: ---------------------------------------------------------------------------
- 2682: Default Registry
- 2683: ---------------------------------------------------------------------------
- 2695: catch-all last
- 2699: ===========================================================================
- 2700: Helpers
- 2701: ===========================================================================

## crates/prism-cooker/src/importer/skinned.rs

- 75: Node index → global bone index (same node referenced by multiple skins keeps only one).
- 107: Node index → glTF node + parent node table (gltf 1.4 lacks `Node::parent()`, walk tree ourselves).
- 117: Each bone's parent bone = nearest joint node in parent chain.
- 132: Topological order: parent must come before child. Cyclic/broken bones fall to root (should not appear in glTF, but won't hang).
- 135: After remap: global index changes, remap table must change too.
- 153: Reference pose: inverse_bind comes in as bind pose world matrix.
- 195: ---------------------------------------------------------------------------
- 196: Animation
- 197: ---------------------------------------------------------------------------
- 214: Node index → track (same node can have many channels, merged by property into same track).
- 299: glTF does not carry frame rate (key frames are timestamps), here set 30 as editor default preview frame rate.
- 304: glTF animation has no event channel (#560 P1 Notify): Notify supplemented by author on engine side,
- 305: exported clip event table is empty.
- 359: ---------------------------------------------------------------------------
- 360: Skin mesh
- 361: ---------------------------------------------------------------------------
- 429: glTF vertices already in bind pose, mesh itself need not pose again.
- 434: ---------------------------------------------------------------------------
- 435: Small tools
- 436: ---------------------------------------------------------------------------
- 545: A quaternion and its negation describe the same rotation. Pinning
- 546: the sign (`w >= 0`) keeps the baked data stable instead of letting
- 547: the extraction branch decide it.

## crates/prism-cooker/src/importer/skinned_tests.rs

- 5: Deliberately place child node before parent face (glTF skin.joints has no order guarantee):
- 6: joint2 is root, joint0 attached to joint2, joint1 attached to joint0.
- 23: Cyclic skeleton is bad data, but must not make importer hang.
- 45: Rotate 90° around +Y:
- 46: [0, 0, -1]
- 47: [0, 1,  0]
- 48: [1, 0,  0]
- 49: Columns: x = [0,0,1], y = [0,1,0], z = [-1,0,0].
- 57: Rotating +X to +Z is −90° around +Y (right-handed), so y component is negative.
- 120: Sampler output is "in tangent, value, out tangent", time axis has only two sample points.

## crates/prism-cooker/src/importer/tests.rs

- 1: ===========================================================================
- 2: Tests
- 3: ===========================================================================
- 62: Write a real 2×2 PNG via the image crate.
- 135: RawImporter is catch-all
- 159: first  import
- 165: seconds  import  (cached).
- 197: The source is unchanged, but `force` must re-run the importer.
- 214: Write a real 1×1 red PNG.
- 269: ── Real glTF / GLB import  test ──────────────────────────────────
- 276: Positions: right triangle in XY plane at Z=0.
- 280: 36 + 6 = 42
- 282: includes chunk-header
- 298: FLOAT
- 306: UNSIGNED_SHORT
- 327: GLB header
- 328: magic
- 329: version
- 330: length
- 332: JSON chunk
- 336: space padding
- 338: BIN chunk
- 374: Validate RMXI header in output
- 390: -------------------------------------------------------------------
- 391: glTF scene graph (#520)
- 392: -------------------------------------------------------------------
- 483: glTF requires POSITION accessor to have min/max.
- 493: One mesh with two primitives referenced by two translated nodes — old implementation only kept below
- 494: "first mesh first primitive" at origin (#520).
- 498: 0..36
- 500: 36..42
- 566: Each primitive index range + material must be kept below (#520).
- 584: First node vertices translated +5, second node at z=7.
- 587: Vertices 7..9 belong to second node (each node mesh has 6 vertices).
- 607: 0..36
- 609: 36..42
- 662: 0..36
- 664: 36..42
- 797: Node rotates 90° around X, X scaled 4×: normal (0,0,1) should become (0,-1,0),
- 798: and must not be stretched or flipped by scale.
- 844: Header 17 bytes + vertex region (3 * 12) + UV (0 channel skipped) = normal region.
- 867: 0..36
- 869: 36..42
- 910: -------------------------------------------------------------------
- 911: material  Importer
- 912: -------------------------------------------------------------------
- 920: Plain .json falls through to JsonImporter.
- 931: Register two texture asset records in the DB so the importer can
- 932: parse  their paths to AssetId dependencies.
- 976: Two texture  deps resolved.
- 981: Intermediate must start with RMATI magic.
- 983: Version 2 adds the alpha-mode byte + cutoff after the scalar block.
- 986: Metadata carries the slot presence flags.
- 990: albedo
- 991: normal
- 992: occlusion
- 999: A texture  path not present in the DB should be dropped (warn), not
- 1000: Abort import; the material still imports with that slot empty
- 1019: Unresolved -> 0 deps, material  still imports.
- 1031: Empty texture -> all defaults.
- 1042: 78 byte  min  (5 magic + 1 version + 72 scalars), no textures.
- 1048: -------------------------------------------------------------------
- 1049: shader  Importer
- 1050: -------------------------------------------------------------------
- 1088: pt_* shaders use the `ptMain` entry per compile.sh convention.
- 1131: -------------------------------------------------------------------
- 1132: OBJ importer (#520)
- 1133: -------------------------------------------------------------------
- 1222: Four positions each appear once; two triangles share 1 and 3.
- 1270: 
- 1271: 
- 1281: JOINTS_0: vertices 0/1 attached to node1 (local index 1), vertex 2 attached to node2 (local index 0).
- 1287: 
- 1288: 
- 1294: Inverse bind given in `skin.joints` order, i.e. [node2(knee), node1(hip)]:
- 1295: knee at (0, 1, 0), so its inverse bind translation column is (0, -1, 0).
- 1298: hip at origin: identity matrix.
- 1374: Packer uses `rig_paths` (read-only glTF) to fill components into scene, `side_assets` truly
- 1375: encodes and writes to disk — both paths must be byte-identical, otherwise scene points to missing asset (#560 P0-4).
- 1414: Synthesized skeleton: child before parent, vertices attached to respective joints, one linear animation (#560).
- 1444: Reference pose from inverse bind: knee relative to hip is +1m.
- 1451: Local indices in JOINTS_0 already remapped to global indices after topological sort:
- 1452: vertices 0/1 first influence node1(hip) → global 0, vertex 2 is node2(knee) → global 1.
- 1453: Spare slots with weight 0 also remapped (kept in range, no effect during shading).
- 1515: Clip name in file matches glTF animation name (`walk`).
- 1543: Negative times dropped, remaining sorted by time.
- 1598: -------------------------------------------------------------------
- 1599: Animation graph importer
- 1600: -------------------------------------------------------------------
- 1851: Scale x by 4 and y by 0.5. A normal of (1, 1, 0) on that surface must
- 1852: come out as (1/4, 1/0.5, 0) normalized — the inverse transpose, not the
- 1853: plain model matrix (which would stretch it the wrong way).

## crates/prism-cooker/src/main.rs

- 44: ---------------------------------------------------------------------------
- 45: Helpers
- 46: ---------------------------------------------------------------------------
- 90: ---------------------------------------------------------------------------
- 91: CLI
- 92: ---------------------------------------------------------------------------
- 177: ---------------------------------------------------------------------------
- 178: Helpers
- 179: ---------------------------------------------------------------------------
- 267: ---------------------------------------------------------------------------
- 268: Commands
- 269: ---------------------------------------------------------------------------
- 308: Skip if already known.
- 339: First, register all source files in the DB so we have records.
- 355: Now import each file.
- 363: Collect records as vec since import_file borrows db mutably.
- 431: Resolve the platform/profile settings *before* cooking: the texture and
- 432: mesh settings used by the cookers come from here, not from hardcoded
- 433: defaults.
- 446: Load imported data: read import-time data from Library/Intermediate/;
- 447: If not yet imported, execute importer on demand to generate import-time data, avoid directly passing original files (like PNG)
- 448: to cooker expecting RTXI/RMXI.
- 488: The record may have picked up new hashes while importing,
- 489: so write to the name derived from the updated record.
- 528: ── Write a human-readable .meta.json alongside the .pak ──────────
- 529: 
- 530: This file is purely for inspection (validate / pak-info).  It
- 531: pairs the AssetDatabase records (which have paths, types,
- 532: importer names) with the per-asset sizes from the actual .pak.
- 533: The runtime never reads this file.
- 618: Optionally enrich with the .pak.meta.json if it exists alongside.
- 637: Column widths in display columns.
- 643: Header line — bilingual, EN padded to 5 so `/` aligns.
- 654: Separator — all columns same width.
- 668: Best-effort label from metadata, or fall back to type name + raw ID.
- 689: Only show compression ratio when it actually saved space (< 95%).
- 706: All columns left-aligned using Unicode-aware padding.
- 717: Hint about metadata when the .meta.json was absent.
- 751: The file name only stays correct while the database record matches
- 752: the source on disk; a stale record would point at an intermediate
- 753: built from different bytes.
- 833: Try to parse as numeric ID first.
- 869: ---------------------------------------------------------------------------
- 870: Entrypoint
- 871: ---------------------------------------------------------------------------
- 906: No subcommand and no .pak → show help.
- 916: ---------------------------------------------------------------------------
- 917: Helpers
- 918: ---------------------------------------------------------------------------
- 969: The database serial must not influence the name: two projects that
- 970: imported the same source end up with the same intermediate file.
- 975: Changing any of the three keys changes the name.
- 992: Regression for #81: `build --platform android` used to cook with
- 993: `CookSettings::default()` (i.e. desktop values).
- 1040: The platform flag is still authoritative for the recorded platform.

## crates/prism-cooker/src/pack.rs

- 152: Longer prefix matched first: when `textures` and `textures/4k` given same time, 4k assets only go to latter.
- 192: Stable order + first write optional package: base manifest must reference them, write base last to guarantee
- 193: manifest will not point to a not-yet-generated / write-failed package.
- 224: pack_files already written and read back to verify base package; this inside then merges optional package manifest into same one
- 225: manifest. On failure base package still available; next build will heavily rewrite manifest.
- 253: 32-bit serial ID collision probability small within import-time internals, but once collided it is "two files share one
- 254: handle", at runtime will silently read wrong asset — rather report error at build time.
- 289: First write temp file in same directory then rename: after #509 at runtime not reading entire `.pak` into memory,
- 290: but reads same file by offset; truncate + heavy write in place will let editor/game running read half package.
- 291: Same directory rename is atomic, old handle continues pointing to old file (Windows also allows shared-delete of
- 292: file being replaced).
- 367: `/absolute` in Windows not counted as `is_absolute()` (no drive letter), but it is still root path;
- 368: `C:` / relative drive letter same — all three must be rejected on each platform, otherwise same parameter
- 369: in CI (Linux) rejected, on dev machine accepted.
- 436: Overwrite an existing package: the rename must replace it in place.
- 471: Add a lexicographically sorted prefix face, will push after original order files.
- 478: Same input heavily packed still byte-consistent.
- 521: A streamed asset must still round-trip through the normal read path.
- 615: audio/beep and scenes/default stay in base package; `textures/4k` belongs to longest prefix,
- 616: will not again fall into `textures` when same.
- 638: Splitting package does not change asset ID: ID still derived from package-relative path.
- 645: Base package manifest lists optional packages, for on-demand mounter to find files by group.

## crates/prism-cooker/src/tiled.rs

- 44: ---------------------------------------------------------------------------
- 45: JSON (.tmj) Schema Deserialization
- 46: ---------------------------------------------------------------------------
- 211: Extract <layer> tags
- 222: Extract CSV data inside <data> ... </data>
- 284: ---------------------------------------------------------------------------
- 285: Greedy 2D Box Collision Contour Consolidation
- 286: ---------------------------------------------------------------------------
- 311: Expand horizontally to find maximum width
- 322: Expand vertically row by row while all cells in the span (x..x+w) are solid & unvisited
- 340: Mark the rectangular merged region as visited
- 384: If this layer is designated as collision or named "Collision" / "Collisions",
- 385: mark its non-zero tiles as solid.
- 411: Consolidate adjacent solid tiles into rectangular collision boxes
- 443: 4x4 grid:
- 444: 1 1 0 0
- 445: 1 1 0 0
- 446: 1 1 1 1
- 447: 0 0 0 0
- 454: Should consolidate into 2 boxes:
- 455: 1) (0, 0) of size 2x3 (32x48)
- 456: 2) (2, 2) of size 2x1 (32x16)
- 514: Collision layer has 3 consecutive solid tiles: (0, 0), (1, 0), (2, 0)

## crates/prism-cooker/tests/asset_harness.rs

- 242: Only trust "os error <n>" part: it's hardcoded in Rust's `io::Error`, while the preceding part
- 243: is **system-localized** (on Chinese Windows, missing directory is `os error 3`, not 2).
- 244: Asserting exact English text/error code would cause the same code path to false-positive on
- 245: different language/platform machines.

## crates/prism-cooker/tests/cli.rs

- 13: ---------------------------------------------------------------------------
- 14: Helpers
- 15: ---------------------------------------------------------------------------
- 30: Pop the binary name, then the "deps" directory if present.
- 31: binary name
- 33: deps
- 41: Fallback: maybe we're in a workspace target dir.
- 77: ---------------------------------------------------------------------------
- 78: Tests
- 79: ---------------------------------------------------------------------------
- 84: Should show usage text on stdout.
- 99: The validate command returns an error → should appear on stderr.
- 104: The binary should exit non-zero, but we only check output text.
- 112: Write a .pak.meta.json alongside the .pak.
- 151: Intentionally *not* writing a .pak.meta.json.
- 157: Fallback tag should mention binary and asset
- 162: Tip about rebuilding should appear.
- 200: 1. init
- 205: 2. create a small PNG texture in Assets/
- 211: 3. scan
- 215: 4. import
- 219: Verify intermediate file was written into Library/Intermediate/
- 228: 5. build
- 235: 6. validate built pak
- 247: 1. init
- 252: 2. create texture
- 258: 3. scan
- 262: 4. build directly without running import
- 269: 5. validate
- 294: Intermediate names are content addressed instead of per-database serials.
- 315: A source that changed without a re-import makes the record outdated.
- 327: Re-importing repairs the record and the intermediate.
- 333: The intermediate built from the old source hash is now unreferenced; it
- 334: stays on disk until a gc pass exists, and `verify` lists it as an orphan.
- 340: A missing intermediate is reported and fails the command.
- 349: An intermediate no record points at is reported as an orphan.

## crates/prism-cooker/tests/e2e.rs

- 29: ---------------------------------------------------------------------------
- 30: Helpers
- 31: ---------------------------------------------------------------------------
- 167: ---------------------------------------------------------------------------
- 168: Real asset source paths (Sponza scene)
- 169: ---------------------------------------------------------------------------
- 179: ---------------------------------------------------------------------------
- 180: E2E Test: full pipeline with real Sponza texture
- 181: ---------------------------------------------------------------------------
- 191: ======================================================================
- 192: 1. SETUP: create project with real + generated assets
- 193: ======================================================================
- 202: --- Real Sponza 4 K texture ---
- 214: --- Generated assets ---
- 222: ======================================================================
- 223: 2. IMPORT: run ImportPipeline on all files, capture intermediate data
- 224: 
- 225: NOTE: import_file stores the full absolute path (forward-slash
- 226: normalized) in the database, NOT a relative path.  All lookups
- 227: below use suffix matching to work around that.
- 228: ======================================================================
- 236: Import the real Sponza texture.
- 250: Import the JSON file.
- 261: Import the GLB file.
- 271: Assert database has 3 records.
- 275: Verify each record has the correct asset type and importer.
- 293: Verify we captured intermediate data for all three assets.
- 300: Save and reload the database (simulates persistence).
- 310: ======================================================================
- 311: 3. COOK + BUILD: run CookPipeline with real intermediate data → .pak
- 312: ======================================================================
- 329: Save .pak to disk for runtime loading.
- 333: ── Write a .pak.meta.json alongside the .pak for inspection ──────
- 379: ======================================================================
- 380: 4. VALIDATE: verify .pak structure with PackageReader
- 381: ======================================================================
- 392: Verify we can read each asset's cooked data back.
- 401: --- Verify cooked texture format ---
- 413: A real 4 K texture (e.g. 4096×4096) should produce 13 mip levels
- 414: (4096→2048→1024→512→256→128→64→32→16→8→4→2→1).
- 418: The Sponza metal_door_01 texture is 4096×4096 → log2(4096) = 12 → 13 mips.
- 426: --- Verify cooked mesh format ---
- 443: ── Round-trip: decode cooked data back → compare vs intermediate ──
- 444: Texture: RTEX mip0 must match RTXI pixels byte-for-byte.
- 458: RGBA8
- 460: mip0 must be byte-identical to the import-decoded pixels.
- 466: Optional: save decoded mip0 as PNG for visual inspection.
- 481: Mesh: RMES vertex/index data must match RMXI data byte-for-byte.
- 493: Vertex data (positions + normals + UVs) must match.
- 498: Index data must match.
- 505: Binary: cooked data must be identical to intermediate data.
- 522: ======================================================================
- 523: 5. RUNTIME LOAD: use ResourceManager to load the .pak
- 524: ======================================================================
- 526: 200 MB (a 4K RGBA8 texture is ~67 MB)
- 532: Load and verify each asset.
- 547: Verify memory tracking.
- 557: ======================================================================
- 558: 6. UNLOAD + RE-LOAD: verify handle generations
- 559: ======================================================================
- 572: ======================================================================
- 573: 7. DEPENDENCY LOADING: verify topological load
- 574: ======================================================================
- 595: ======================================================================
- 596: 8. MEMORY BUDGET / EVICTION
- 597: ======================================================================
- 644: ======================================================================
- 645: 9. STREAMING
- 646: ======================================================================
- 665: ======================================================================
- 666: 10. HOT-RELOAD (manual trigger)
- 667: ======================================================================
- 683: Modify the file.
- 715: ======================================================================
- 716: 11. CLI COMMANDS (simulate via library calls)
- 717: ======================================================================
- 735: ---------------------------------------------------------------------------
- 736: E2E Test: incremental import (cache hit)
- 737: ---------------------------------------------------------------------------
- 754: First import — should run (not cached).
- 760: Second import — should be cached.
- 766: Modify the file, should re-import.
- 774: ---------------------------------------------------------------------------
- 775: E2E Test: empty project
- 776: ---------------------------------------------------------------------------
- 797: ---------------------------------------------------------------------------
- 798: E2E Test: package corruption detection
- 799: ---------------------------------------------------------------------------

## crates/prism-cooker/tests/support/mod.rs

- 7: This module is compiled into several test binaries; each one exercises a
- 8: different subset, so per-binary dead-code expectations cannot all be met.

## crates/prism-editor-api/src/asset_importer.rs

- 185: probe_volume.bin etc binary data goes to DefaultImporter, avoid mistaking for model
- 202: Normal JSON is text, not scene.

## crates/prism-editor-api/src/asset_inspect.rs

- 218: Keyframe events (#560 P1 Notify): time must fall within clip range, name must not be empty.

## crates/prism-editor-api/src/asset_inspect_tests.rs

- 117: Only 1 bone declared, but track points to joint 1.
- 124: No issue when enough bones.
- 130: Weights 0.2 and 0.8: cooker will normalize, but hand-built bad file must be detected.
- 133: Bypass encoder normalization directly: first encode valid, then corrupt weight stream.
- 135: Weight stream begins after joint stream: header(25) + positions(3*12) + joints(3*8).
- 150: only to illustrate original value
- 174: Empty file is also safe.
- 176: File with only magic, no content, also does not panic.

## crates/prism-editor-api/src/asset_ops.rs

- 99: #466: asset identity in `<path>.rmeta`, sidecar follows on rename (sidecars inside
- 100: directories move with directory, here only handle single file).
- 136: Documents contained in directory itself also count as "affected", user should see before deleting.
- 188: #466: sidecar is not user asset, delete together with file.
- 196: After deleting scene document / artifact, entries in `scenes.json` pointing to it must also disappear,
- 197: otherwise build-scenes fails on dangling entries and runtime scene list keeps invalid items.
- 236: Target file does not yet exist (rename_asset already checked), leftover orphan sidecar simply overwritten.
- 275: ---------------------------------------------------------------------------
- 276: Path validation
- 277: ---------------------------------------------------------------------------
- 359: ---------------------------------------------------------------------------
- 360: Reference rewriting
- 361: ---------------------------------------------------------------------------
- 432: Parse reference using old directory; write paths to document using directory after move (when directory renamed as whole the two differ).
- 813: ---------------------------------------------------------------------------
- 814: Path utils
- 815: ---------------------------------------------------------------------------
- 1036: The reader resolves the stale path through the GUID.
- 1234: Reference fields not ending in `_path` rely on explicit registration, here assert they weren't removed.
- 1336: In-place rename, move back to default dir, and move from default dir into subdir all allowed.
- 1431: Deleting scene document itself needs no force: manifest entry cleared along with it.

## crates/prism-editor-api/src/assets.rs

- 99: Stable order: sort by filename then recurse.
- 125: ---------------------------------------------------------------------------
- 126: Asset metadata
- 127: ---------------------------------------------------------------------------
- 168: `path` relative to project root (not relative to `assets/`).
- 182: ---------------------------------------------------------------------------
- 183: Asset preview
- 184: ---------------------------------------------------------------------------
- 239: Read one extra byte to determine whether truncated.
- 256: Truncation may land mid multi-byte char; only roll back to nearest char boundary;
- 257: if the rest of file is not UTF-8 still report error, do not silently swallow content.
- 280: ---------------------------------------------------------------------------
- 281: Reference analysis
- 282: ---------------------------------------------------------------------------
- 327: Scenes and prefabs share the same document shape, both need scanning; prefabs under `assets/` are enumerated recursively.
- 683: Scene-dir relative: ../valley.hdr
- 685: Project-root relative: assets/valley.hdr
- 693: Unrelated strings do not produce references.
- 747: `.rscn` and its authored document `.scene.json` treated as same scene.
- 758: Unregistered asset does not produce manifest reference.
- 792: Truncating to 7 bytes lands inside a multi-byte char; should fall back to 6 bytes (two CJK chars).

## crates/prism-editor-api/src/lib.rs

- 49: Shape gates (`delete_asset` / `move_asset` / `rename_asset` /
- 50: `instantiate_prefab` / `revert_prefab_instance` / `sync_prefab_instances` etc.
- 51: same names as existing exports, keep module path access, do not re-export again at root).
- 58: Generic (scene / prefab)
- 61: Scene aliases (legacy calls)

## crates/prism-editor-api/src/presets.rs

- 49: Sun: `Light` default type is Point, so directional must override `light_type`;
- 50: direction = `Quat::from_rotation_arc(+Z, (−0.5, √½, 0.5))`,
- 51: corresponds to old `DirectionalLight.euler_xyz = (45, −45, 0)` lighting direction.
- 62: Axis (−√½, −0.5, 0) / |axis|, angle 60°, exact constants avoid transcription error.
- 73: Spot default direction should point down (+Z → −Y): +90° around X.
- 96: Equivalent of `GameObject → 3D Object`: `mesh_path` points to built-in procedural mesh
- 97: (`prism_render::primitives`), no `.pak` read, no cook.
- 210: Camera preset must genuinely carry camera component.
- 266: Point light uses `Light` default type: no override needed.

## crates/prism-editor-api/src/property_drawer.rs

- 150: Duplicate same class attribute keeps only last (only one Range/Tooltip).
- 462: min > max normalized to max = min.
- 473: Range only one.
- 493: Register type-level drawer for type demo::X (`[CustomPropertyDrawer(typeof(X))]`)
- 496: Host has value field of type demo::X: hits type-level drawer
- 504: Register field-level drawer for specific field: overrides type-level
- 513: Other same-type fields of same host still use type-level declaration
- 521: Fields of other value types not affected
- 564: Undeclared field has no drawer.

## crates/prism-editor-api/src/reflect_edit.rs

- 142: Whole replace: single normalization validation.
- 149: Get target value from "modified" object (already reflection type, enum / single-field correct).
- 152: Top-level field name + index / sub-field below it; top-level write goes through set_field (read-only rejected).
- 223: Nested struct field.
- 256: Modified whole value cannot be restored into component → reject.
- 262: Enum written as serde string shape.
- 269: Newtype component is bare string.
- 293: Dragging field may write from a different thread pool thread; shared registry must be built once,
- 294: and readable concurrently (`ComponentRegistry` callbacks are `Send + Sync`).

## crates/prism-editor-api/src/scene.rs

- 222: `SceneJson::parse` first migrates old document to current schema (e.g. v1's
- 223: `transform.translation` → `position`), then normalized round-trip.
- 295: Run the same migration/normalisation as `write_doc` so GUID syncing sees
- 296: the final entity/component shape.
- 318: Migrate first then validate, so v1 documents can also pass.
- 350: ---------------------------------------------------------------------------
- 351: Document name / path
- 352: ---------------------------------------------------------------------------
- 454: ---------------------------------------------------------------------------
- 455: Scene templates
- 456: ---------------------------------------------------------------------------
- 566: "camera" and unknown id
- 630: Prefab template = single entity preset.
- 676: ---------------------------------------------------------------------------
- 677: Prefab: extract from subtree / instantiate
- 678: ---------------------------------------------------------------------------
- 751: Cycle exists in data, conservatively give up
- 811: First extract child, then insert at target position.
- 821: Only change moved entity's `parent`: id and array position decoupled, other entities completely untouched.
- 878: Deleted parent → becomes root; remaining parents are ids, unaffected by array shrink.
- 936: Prefab file has own id space: subtree ids renumbered 0..k, internal parent-child references changed accordingly.
- 947: Parent not in subtree (including root entity's original parent) → null.
- 957: Strip instance link from root.
- 1008: Prefab has own id space: allocate a new id block, remember old id → new id.
- 1054: Prefab root entity records source (for editor reverse link).
- 1125: `parent` is id, deleting subtree does not invalidate it (exactly the effect v3 wants);
- 1126: `instantiate_prefab` input is **index**, and must be **trimmed document's** index:
- 1127: array order unrelated to hierarchy (see `SceneJson::topological_order`), deleted entities may completely
- 1128: be before parent entity, deleting will shift parent entity forward. Using original array index directly will attach wrong parent (or
- 1129: error in bounds check).
- 1135: Only happens when hierarchy cycles (`validate` only rejects self-parent).
- 1143: Root position in trimmed array: entities before it minus those also in this subtree.
- 1276: Writing back illegal document must be rejected, not persisted to disk.
- 1295: Every entity must have complete transform / components shape.
- 1301: Should remain unchanged after SceneJson normalization (template already in normalized shape).
- 1308: Unknown id falls back to camera.
- 1335: Unknown id falls back to empty entity.
- 1337: Prefab has its own directory.
- 1360: Extracted root disconnects parent
- 1362: Parent is root (new index 0)
- 1364: Parent is child (new index 1)
- 1392: Name deduplicated with existing entities.
- 1394: Prefab root attached to specified parent entity.
- 1396: Internal parent indices shifted.
- 1399: Out-of-bounds parent entity rejected.
- 1439: Strip link when extracting subtree (prefab should not record where it comes from).
- 1445: Break link.
- 1457: Corrupt instance content (change name + remove component).
- 1467: Name and parent retained, content from prefab.
- 1474: Link still present, and only this one instance.
- 1502: Rebuilt subtree returns to original position (not appended to end), name uses instance's original name.
- 1532: In v3 document `parent` is id; here id and index deliberately different (10/30/40).
- 1548: Parent still is id (10), not same-value index.
- 1568: Array order deliberately opposite to hierarchy order:
- 1569: idx0 `grandchild` (id 9) before its parent `Door`, also in Door's subtree;
- 1570: idx1 `host` (id 0) is Door's parent, but sorted after subtree.
- 1571: After deleting subtree host moves from index 1 to 0: if using original array index (1), instance will be attached
- 1572: to `tail` (silently attach wrong parent).
- 1592: Instance root re-attached to host (id 0 / trimmed index 0), not pre-trim index 1 pointing to tail.
- 1601: Hierarchy cycle (`validate` only rejects self-parent): Door(id 1)'s parent is its descendant grandchild(id 9).
- 1634: Insert a normal entity between two instances, check after sync it and both instances stay in place.
- 1680: Corrupt two instances' content (but keep link component).
- 1697: host + 2×(Door + Handle)
- 1728: Attach c under d (keep c's subtree).
- 1734: Become root.
- 1737: b's subtree unchanged (c still points to b).
- 1740: Reject: self-parent / attach to descendant / out of bounds.
- 1742: c is a's descendant
- 1751: Move a(id 0) to end: order b,c,d,a, but parent refs are ids, unaffected by position.
- 1758: still points to a (id 0, position-independent)
- 1760: still points to b (id 1)
- 1764: Insert before b, and attach under d.
- 1770: c's parent becomes d's id (3), independent of moved entity position.
- 1773: cycle
- 1780: Delete b: c becomes root.
- 1788: Delete a and d simultaneously: b's parent a deleted → becomes root.
- 1795: c still points to b (id 1, unchanged by deletion)
- 1807: Same name does not overwrite.
- 1858: Saving the resolved document keeps both fields.
- 1866: Saving a path-only document adds the sidecar GUID.

## crates/prism-editor-api/src/scene_manifest.rs

- 244: Saving a path-only entry adds the sidecar GUID.

## crates/prism-editor-api/src/schema.rs

- 134: Components first, then nested types; within same group sort by name, keep UI order stable.
- 258: `Camera.clear_flags` is `CameraClearFlags`: u8 encoding + variant names.
- 280: `Text.alignment` uses same mechanism (proves no special-case for Camera).
- 300: Normal numeric fields have no variants.
- 309: Components sorted first, and each component carries default value.
- 328: `aspect` is read-only field, `enabled` is not.
- 346: Nested type `glam::Vec3` appears in table, and Style.anchors points to struct.
- 361: Non-component types have no default value.
- 380: —— field drawers (`PropertyDrawer` / `PropertyAttribute`) ——
- 399: `[Range]` → slider + upper/lower bounds.
- 405: `[ColorUsage]` → color picker.
- 411: `[TextArea]` → multi-line text box.
- 416: Fields without declarations carry no drawer (frontend falls back to default control).
- 422: Same process builds only once (field write is input event hot path).
- 425: Cache content must match manually registered result, avoid initializing wrong.
- 433: `build_schema()` goes through this shared registry.
- 474: Later registration for same field overrides built-in `[Range(1,179)]`.
- 485: Un-overridden built-in declarations remain.
- 496: Type-level drawer (`[CustomPropertyDrawer(typeof(Anchors))]`):
- 497: any field whose value type is `prism_engine::ui::Anchors` (like `Style.anchors`) gets this drawer.
- 518: Host registry is local override: default schema still has built-in `[ColorUsage]`.
- 528: —— enums inside containers: `Vec<Enum>` / `Option<Enum>` / `[Enum; N]` ——
- 579: Container itself carries no variants, variants live on innermost scalar leaf.

## crates/prism-editor-api/src/settings.rs

- 154: ---------------------------------------------------------------------------
- 155: Project settings (`[app]` / `[window]` / `[build]`)
- 156: ---------------------------------------------------------------------------
- 254: ---------------------------------------------------------------------------
- 255: Only write fields **actually changed**
- 256: ---------------------------------------------------------------------------
- 257: 
- 258: Multiple panels in editor edit the same settings file at once (project settings panel handles
- 259: `[app]`/`[window]`/`[build]`/`[render]`, build panel also edits `[build]`). Each panel reads a
- 260: snapshot on open, writes whole section back on save, so one panel overwrites sections (and fields
- 261: within same section) that another panel **never touched** with old values.
- 262: 
- 263: Here frontend sends both "snapshot before open" and "current values after" together, only writing
- 264: changed leaves to disk current settings: untouched fields always defer to disk.
- 286: Panel did not change this key (or it was not in snapshot) → keep disk value.
- 520: Build panel snapshot on open.
- 523: Project settings panel changed [app] meanwhile and already wrote disk.
- 528: Build panel only changed [build].output_dir then saved.
- 547: Another field in same section changed elsewhere.
- 552: Panel only changed window width (build.profile in snapshot still release, but untouched).
- 568: Hand-written file (writeback drops comments) proves "no change does not write file".
- 587: Elsewhere changed tone map.
- 592: Panel only changed max bounce count.

## crates/prism-editor-mcp/src/editor_egui.rs

- 65: The port file identifies *this* process: delete it first and wait for
- 66: it to reappear before considering the editor ready.
- 92: Returning as soon as the port appears would let the first
- 93: `editor_ui` see an empty dock (panels are not drawn on the
- 94: first frame), so wait until a few frames have actually been
- 95: drawn before handing back control.
- 370: The control channel also works when `--project` cannot find a project
- 371: root: use the known port file directly.
- 428: Keep the raw reply (including `kind`: bad_request / not_ready / denied /
- 429: internal) in the error text so the tool caller can decide whether to retry.
- 450: Fall back to cargo if the editor has never been built (it compiles
- 451: first: slow but usable).

## crates/prism-editor-mcp/src/game.rs

- 209: A stale file would make the wait-for-file loop return immediately;
- 210: delete it first.
- 238: The file may have just been created and not fully written;
- 239: wait for its size to settle.

## crates/prism-editor-mcp/src/main.rs

- 101: Tool bodies read and write files and spawn `cargo`/the game binary;
- 102: `block_in_place` keeps that off the async scheduler without turning
- 103: every tool into an async function.

## crates/prism-editor-mcp/src/tools.rs

- 202: Always expose project-relative paths so agents do not have to
- 203: deal with an absolute prefix when building `path`.
- 299: `set_component_field` reads the new value from the "after" object, so
- 300: patch the path first; an invalid type is rejected inside its
- 301: `object_from_json`.

## crates/prism-editor-mcp/tests/mcp_harness.rs

- 188: The wire format itself is `rmcp`'s responsibility now; what this crate
- 189: still owns is the tool table, so the schema checks above are the part
- 190: that has to keep holding.

## crates/prism-engine/src/anim_ground.rs

- 140: The probe starts inside the capsule, so the caster is the first hit.

## crates/prism-engine/src/asset/mod.rs

- 25: ===========================================================================
- 26: Handle
- 27: ===========================================================================
- 81: ===========================================================================
- 82: Slot
- 83: ===========================================================================
- 93: ===========================================================================
- 94: AssetManager<T>
- 95: ===========================================================================
- 166: Bump generation so stale handles are rejected, and keep the
- 167: bumped value in the free slot so reuse cannot resurrect the
- 168: old handle by resetting to generation 0.
- 210: -- internal helpers ------------------------------------------------
- 241: ===========================================================================
- 242: Handle type aliases
- 243: ===========================================================================

## crates/prism-engine/src/asset/procedural.rs

- 8: ===========================================================================
- 9: Unit cube (1×1×1 centred at origin)
- 10: ===========================================================================
- 15: Each face has its own set of 4 vertices so face normals are flat.
- 21: Front (+Z), Back (-Z), Top (+Y), Bottom (-Y), Right (+X), Left (-X)
- 23: (normal, tangent, positions, uvs)
- 119: 36 indices (6 faces × 1 quad = 2 triangles × 3 indices)
- 136: ===========================================================================
- 137: Default material
- 138: ===========================================================================

## crates/prism-engine/src/asset/types.rs

- 8: ===========================================================================
- 9: MeshAsset
- 10: ===========================================================================
- 41: ===========================================================================
- 42: MaterialAsset
- 43: ===========================================================================

## crates/prism-engine/src/asset_paths.rs

- 77: Every asset query will come here (tens of calls in one session), so resident log stays at
- 78: `trace`; at startup `prism-app` prints a separate `asset root:` line.
- 163: ---------------------------------------------------------------------------
- 164: Internals (pure functions, easy to unit test)
- 165: ---------------------------------------------------------------------------
- 178: cwd first, exe directory second: in development (cargo run / editor) cwd better matches user intent,
- 179: after deployment cwd is often just "start position when double-clicked", exe directory is the reliable one.
- 288: ---------------------------------------------------------------------------
- 289: Packaged asset fallback (#1049)
- 290: ---------------------------------------------------------------------------
- 348: Package may be mid-write during hot reload; next query will retry.

## crates/prism-engine/src/asset_paths_tests.rs

- 78: cwd has no assets/ at all: the previous implementation failed here, which
- 79: is exactly the "launch the exe from somewhere else" case.
- 87: …and cwd still wins when both are usable.
- 108: Exercise the same joining rule as `asset_path` without touching the
- 109: process-global env/cwd that the public helper reads.
- 130: Sort stable; only files, only `.pak` extension.
- 218: Incorrect slashes and leading `./` normalize to same package-internal path.
- 221: Cache after rebuild must switch to new content (both mtime and length changed).
- 232: <repo>/crates/prism-build-pipeline + <repo>/projects/demo/assets
- 240: `canonicalize` returns a verbatim `\\?\C:\...` path on Windows, so compare
- 241: path components instead of the raw string's separators.
- 253: The log form is the two steps joined, never a bare "build-scenes".
- 266: No `crates/prism-build-pipeline` next to the manifest: the fallback keeps
- 267: the package name explicit instead of inventing a manifest path.

## crates/prism-engine/src/asset_resolver/decoders.rs

- 36: Block-compressed format color space also decided by slot bit (albedo/emissive = sRGB,
- 37: normal/MR/AO = linear), consistent with RGBA8 (#519).
- 46: ASTC / ETC2 also decided by slot bit for color space (#519).
- 80: -----------------------------------------------------------------------
- 81: Internal CPU load helpers (pure data, no renderer dependency)
- 82: -----------------------------------------------------------------------
- 147: Built-in primitive meshes (editor 3D Object presets): no `.pak` read, no cook.
- 256: Color texture is sRGB encoded: let hardware decode on sample. Normal / MR / AO store linear
- 257: data, decoding as sRGB would squash 0.5 to ≈0.214 (normal Z negative, lighting totally wrong).
- 265: The renderer keeps its own mirror of the alpha enum (it must not
- 266: depend on the asset crate), so map it here at the upload boundary.
- 353: Neutral mid grey: a texture that failed to decode reads as an
- 354: untextured surface rather than as a loud error marker.
- 467: Two legal combos: 3 + uv floats per vertex (no normals), or 6 + uv (with normals).
- 490: Segment header (identity: float).
- 511: Asset without normals has no reliable shading normal; give a fixed direction rather than zero vector,
- 512: to avoid lighting degenerating to NaN / full black.
- 521: default tangent

## crates/prism-engine/src/asset_resolver/mod.rs

- 39: ---------------------------------------------------------------------------
- 40: GpuAssetResolver
- 41: ---------------------------------------------------------------------------
- 349: Entity destroyed after, registration of "graph belongs to which slot bit" not yet returned is useless.
- 409: -----------------------------------------------------------------------
- 410: Package loading
- 411: -----------------------------------------------------------------------
- 427: #530: Mount `assets/` below **all** `.pak`, not hardcoded `scenes.pak`.
- 428: Build can split packages by content (scene / model graph / subsequent streaming packages), runtime does not need
- 429: to know how many few packages split into.
- 478: Only record true positive load success packages; failure packages will heavy retry on next call.
- 533: Each package beside has `<name>.pak.meta.json` (path list) loaded together.
- 593: Release builds ship no `assets/scenes/*.scene.json`, so the newest source
- 594: timestamp is 0 and only the schema fingerprint is compared.
- 653: -----------------------------------------------------------------------
- 654: Main thread: CPU segment --- collect pending-parse entities and produce upload requests
- 655: -----------------------------------------------------------------------
- 673: #976: build stamp validated only at **first** resource access. Package mounting at startup occurs in
- 674: `App::build`, at that time `env_logger` not initialized, any warn/error will be dropped
- 675: --- this positive "old package silently draws wrong" is one reason it can long exist.
- 677: First recycle GPU resources already invalidated this frame (#495): entity gone, component deleted, or
- 678: `generation` zeroed (hot reload), slot bit returned to renderer, again heavy re-upload.
- 680: Mesh generated at runtime (terrain, #517): path with `__proc/` prefix, taken directly from world inside
- 681: table, not through `.pak`.
- 683: Collect pending entities (first), avoid borrowing world & self at same time.
- 685: Failure cache only meaningful for entities still present; entity deleted afterward cleaned up incidentally.
- 688: Entity deleted while upload in progress, bounding box also should not remain in table.
- 715: Only counts as pending when "has path" and "handle not yet parsed":
- 716: - path empty = author did not provide resource, nothing to load (e.g. mesh with no material,
- 717: renderer falls back to default material slot);
- 718: - component missing at runtime = result has nowhere to write back, submitted will heavy re-enqueue every frame;
- 719: - failure cache hit = same bad path will not heavy retry again.
- 720: Skinned entity (#560 P0-4) `MeshRenderer.mesh_path` is only "source glTF path"
- 721: --- packer relies on it to stage resources. True positive draws `.rskin`, so inside do not again upload
- 722: a static RMES (wastes VRAM, static draw will also be skipped by `skinned_draw`).
- 732: many materials (#520): as long as there are paths not yet failed, continue trying. Failure cache by
- 733: path recorded, so some submesh's bad material will not drag down other materials.
- 771: 1b. Skinned mesh (#560 P0-3b): `.rskin` comes with vertices + skin weights, takes dedicated decode;
- 772: Material parsing shares same set as static mesh (`renderer` equivalent MeshRenderer).
- 820: 2D sprite: only has graph one resource, `SpriteRef.generation == 0` means pending parse.
- 829: No `SpriteRef` (runtime newly generated sprite) or path rewritten counts as pending parse.
- 853: Tile map graph: same only has tileset one graph (#529).
- 886: UI background graph (#591): same as sprite only needs one graph, result written to `UiImageRef`.
- 943: Skinned mesh (#560 P0-3b): vertices + skin weights uploaded together, rest of stream same as static mesh.
- 963: Mesh already parsed, this frame only supplements material: use submesh table cached at runtime.
- 977: Material expanded by submesh (#520): same path uploaded only once, returned after by
- 978: `pending_material_map` copied to each submesh.
- 1050: Bounding box calculated when upload request sent: GPU upload after handle returns, while CPU segment
- 1051: has already resolved vertex positions (#491).
- 1199: No old-style results / single material path through `prepare_requests`: keep
- 1200: Old behavior, only writes back first slot bit.
- 1273: Tile map graph tileset takes same upload result (#529).
- 1294: UI background graph takes same upload result (#591).
- 1323: -----------------------------------------------------------------------
- 1324: Hot reload: let already parsed assets heavy re-run once
- 1325: -----------------------------------------------------------------------
- 1351: UI background graph (#591) also takes same graph upload path, hot reload must invalidate together.
- 1438: Failure cache (same bad path will not heavy retry again) and queue flag will block heavy re-parse.
- 1449: -----------------------------------------------------------------------
- 1450: Skeletal animation assets (#560 P0-2)
- 1451: -----------------------------------------------------------------------
- 1462: Skinned entity does not necessarily have `MeshRef` / `MaterialRef` (loader only for `MeshRenderer`
- 1463: insert), without them `prepare_requests` will not schedule upload (#560 P0-3b).
- 1512: State machine (#560 P1): machine's own clip, after it loads, again supplement request.
- 1516: Animation graph (#560 P1): graph's own clip / state machine, after it loads, again supplement request.
- 1543: Parameter table (project side writes `speed` / `grounded` / trigger each frame). Engine does not guess value, only guarantees
- 1544: Resource present: without it `animator_system` will become empty table (all parameter conditions constantly false).
- 1567: Skeleton and clip types differ, distinguished by extension (paths written by cook end carry `.rskel` / `.ranim`).
- 1641: Second round: graph reference state machine. Must first load machine, third round can supplement machine state clip together.
- 1686: Second round: state machine reference clip. Only after machine just loaded knows who they are, so can only after this supplement.

## crates/prism-engine/src/asset_resolver/tests.rs

- 32: #583: fonts referenced by `Text.font` are read from `.pak` and handed to renderer for registration.
- 59: Same path not re-read repeatedly; text without a font produces no request.
- 87: Simulate the failure before lazy package is mounted.
- 149: Path rewritten → new upload gets new slot, old slot must be returned, otherwise each graph switch leaks a texture.
- 170: Same slot uploaded again (result reused) does not count as invalidation.
- 195: `prepare_requests` opens with recycling of invalidated resources.
- 200: Release only once.
- 207: Hot reload clears generation; old graph images / meshes must be returned this frame.
- 242: sprite entity itself remains, next upload will re-account it.
- 295: Resource package empty, so texture load will fail; key is "no `SpriteRef` still enqueues".
- 322: Already parsed and path unchanged: not re-submitted.
- 376: #529: tilemap goes through same texture parse path as sprite, but handle written into `TilemapRef`.
- 394: Resource package empty → failure cache, not retried every frame.
- 436: Empty package → failure cache, not retried every frame.
- 517: Simulate render thread callback: mesh handle parsed, material result empty.
- 575: Render thread callback: mesh upload success, material no handle.
- 675: Two vertex counts differ: any "read segments as interleaved" would mix channels.
- 695: Without normals stride is 3 + 2 floats, uv segment tightly follows positions segment.
- 727: Hand-write an RMES v2: 3 vertices, 6 indices, two submeshes.
- 733: uv channels
- 734: stride bytes
- 735: positions offset
- 736: normals offset
- 737: uv0 offset
- 802: Color texture needs sRGB sampling, normal / MR stay linear (#519).
- 987: built-in cube needs no `.pak`, but goes through same prepare → upload → writeback path.
- 1031: Unit cube (edge 1, centered at origin).
- 1035: Another submission will not re-upload: handle already written back (`generation = 1`).
- 1108: A path not in any package: first prepare fails and enters failure cache,
- 1109: then not retried. Hot reload must allow one retry (new package may have added it).
- 1117: Upload failure also counts as "having a result": clear queued flag, only failure cache blocks retry.
- 1161: mesh already parsed (this frame only fills material), submesh table from runtime cache.
- 1201: Both paths not in package, but requests still expanded by unique path (upload phase will fail).

## crates/prism-engine/src/audio.rs

- 77: Start new playback.
- 84: Update volume on the existing playback.
- 89: Was playing, now stopped.

## crates/prism-engine/src/config/mod.rs

- 16: ---------------------------------------------------------------------------
- 17: Top-level config
- 18: ---------------------------------------------------------------------------
- 27: ---------------------------------------------------------------------------
- 28: Render defaults (`[render]` in settings.ini)
- 29: ---------------------------------------------------------------------------
- 230: Boxes with zero or negative size have no meaning, reject and log.
- 258: Negative exposure would flip the image to negative; clamp to 0 here.
- 422: ---------------------------------------------------------------------------
- 423: App identity (company name, app name — like 's PlayerSettings)
- 424: ---------------------------------------------------------------------------
- 462: ---------------------------------------------------------------------------
- 463: Window defaults
- 464: ---------------------------------------------------------------------------
- 576: ---------------------------------------------------------------------------
- 577: Loading
- 578: ---------------------------------------------------------------------------
- 587: The engine does not read `[build]` (`lib_name` / `output_dir` / `profile` are used by the editor and packaging
- 588: pipelines), but it is still a legal project config section. List it as "known but no keys" so
- 589: `log_unknown_entries` does not report on every settings.ini read:
- 590: "ignoring unknown section [build]" (#985).
- 790: #502 color grading knobs ride in the same section.
- 811: #526: shadow coverage distance and map edge length are both configurable quality knobs.
- 817: Unspecified fields keep their defaults.
- 820: White balance is clamped to the shader's [-1, 1] range.
- 873: #613: `[render] internal_resolution = 640x360` + `internal_scale_mode = integer`
- 882: Other separators and disable spellings are accepted.
- 896: On parse failure keep defaults (do not invalidate the entire config).

## crates/prism-engine/src/dirty_router.rs

- 17: ---------------------------------------------------------------------------
- 18: DirtyFlags
- 19: ---------------------------------------------------------------------------
- 56: ---------------------------------------------------------------------------
- 57: DirtyRouter
- 58: ---------------------------------------------------------------------------
- 102: First frame: everything is dirty.
- 115: `light_view_proj` is deliberately not compared: the shadow
- 116: cascade matrix is derived from the camera eye
- 117: (`shadow_cascade_matrices` in `crate::render_system`), so any
- 118: camera motion changes it. Treating that as a lighting change
- 119: would clear the world-space SHaRC cache whenever the player
- 120: walks, while the cache keys on world position and only needs
- 121: reprojection when the camera moves (repair spec §8).
- 280: First frame: every flag starts dirty.
- 284: A moving camera also moves the shadow cascade matrix (it is derived
- 285: from the camera eye), so `light_view_proj` changes as well. Only the
- 286: camera flag may light up: the world-space SHaRC cache keys on world
- 287: position and merely reprojects on camera motion.

## crates/prism-engine/src/ecs/components.rs

- 22: ---------------------------------------------------------------------------
- 23: MeshRenderer
- 24: ---------------------------------------------------------------------------

## crates/prism-engine/src/ecs/mod.rs

- 18: Re-export the ECS core so consumers only need `crate::ecs::*`.

## crates/prism-engine/src/engine.rs

- 62: ===========================================================================
- 63: Engine
- 64: ===========================================================================
- 89: ======================================================================
- 90: Construction
- 91: ======================================================================
- 107: One-shot audio queue (#987): project code queues, host takes each frame.
- 123: ======================================================================
- 124: Init
- 125: ======================================================================
- 150: Runtime prefers reading manifest from asset package, avoid depending on cwd.
- 151: Old packages packed manifest as `scenes.toml` (TOML format before RSCN), still parseable.
- 164: Dev compatibility: when package lacks manifest scene system reads file.
- 239: Return entity list in **document order**: `spawn_from_json_text` spawns in document array order,
- 240: host (editor) uses this subscript to match `.scene.json` `entities[i]`.
- 317: Default systems (UI infrastructure) are **merged** into the existing
- 318: schedule rather than replacing it, so systems a user registered before
- 319: `runtime_initialize` survive.
- 323: ======================================================================
- 324: Timer API
- 325: ======================================================================
- 350: ======================================================================
- 351: Frame tick phases
- 352: ======================================================================
- 384: 2D physics step simulation (#1160): default gravity 9.81 m/s² downward (-Y)
- 426: ── fill UI input state ──────────────────────────────────────
- 428: Window pixels → UI layout space (#988): UI drawn inside PostPass target rect,
- 429: without conversion clicks would mismatch visible positions.
- 436: Wheel and modifiers fold into layout space here too: host UI never sees
- 437: platform events, and dropping either deletes an entire interaction path.
- 452: ── same input also given to gameplay code (#589) ──────────────────
- 453: Static Player's `GameWorld::input()` reads this World resource.
- 454: Previously only editor plugin backend could get input, `GameWorld::input()` in release
- 455: was always all-zero. Size from `ScreenSize` (host writes before each frame update).
- 465: Run the ECS system schedule (user‑registered + built‑in systems).
- 468: Quads submitted by user systems via `GameWorld::draw_quad` this tick are
- 469: ordered after `ui::render`, merge here once, else static Player is one frame late (#1048).
- 472: "No scene" placeholder image: only appears when startup manifest has no loadable scene (usually
- 473: `.pak` not packed). Goes through existing UI channel, no new pass, clear color untouched.
- 479: ── world matrix propagation (end of simulation step) ─────────────────────────────
- 480: Entities spawned by user systems this tick / modified `LocalTransform` must
- 481: become `LocalToWorld` before update returns, otherwise consumers below see previous-frame
- 482: or even missing world matrices:
- 483: - later in same tick (next frame) game logic / physics / camera;
- 484: - render extract (`extract_frame_packet` now read-only, no longer computes);
- 485: - scene state save (`save_scene_state` reads `LocalToWorld`).
- 486: Placed after schedule rather than inside a system because user systems can register
- 487: after `runtime_initialize` (App::add_system), can't rely on registration order.
- 489: `Active` propagates along parent-child (`activeInHierarchy`), render / lighting read derived value.
- 495: 2D virtual cameras follow the frame-interpolated target before the
- 496: render packet is extracted. Recompute the camera world transform
- 497: after writing its local position.
- 504: Distance LOD (#491): select level by this frame's world matrix, write runtime `LodHidden`.
- 505: Must be after `hierarchy_system` so extract sees current-frame distance.
- 508: Tick the main‑thread timer (dispatch expired callbacks).
- 518: ======================================================================
- 519: Shutdown
- 520: ======================================================================
- 531: ======================================================================
- 532: Suspend / resume (platform lifecycle)
- 533: ======================================================================
- 541: ======================================================================
- 542: Accessors
- 543: ======================================================================
- 625: ── sprite animation ────────────────────────────────────────────
- 626: Advance `SpriteAnimation` → `SpriteRenderer.frame_index`; before render extract.
- 631: ── keyframe animation ──────────────────────────────────────────
- 632: Sample `KeyframeAnimation` → `LocalTransform`. `Engine::update` runs `hierarchy_system`
- 633: after schedule, so same frame's world matrix is post-animation pose.
- 638: ── skeletal animation ────────────────────────────────────────────
- 639: Sample `Animator` → bone matrices. P0-2 only computes CPU-side palette (GPU skinning in P0-3).
- 644: ── terrain ────────────────────────────────────────────────
- 645: `Terrain` is data only: here generate mesh from heightmap and register into `ProceduralMeshes`,
- 646: asset parser can pick it up in next frame's upload phase (#517 Phase 1).
- 651: ── particle simulation ──────────────────────────────────────────
- 652: CPU particle integration + emission (#501 Phase 1). Also before `hierarchy_system`,
- 653: extract sees already-advanced particles this frame.
- 658: ── 2D particle simulation ─────────────────────────────────
- 659: CPU particles in world XY plane (#594). Parallel to 3D particles for same reason: consistent with ECS /
- 660: scene format, extract sees already-advanced particles this frame.
- 665: ── screen-space (UI layer) particles (#1057) ────────────────────────
- 666: Simulated in pixel space, output merged into `ExternalUiQuads`; must be before this frame's `ui::render`
- 667: so quads enter same frame's `UiDrawList`.
- 672: ── day-night (#983) ─────────────────────────────────────────
- 673: Advance `TimeOfDay` and refresh ambient light. Placed before particles and `hierarchy_system`:
- 674: this frame's `sprite_light_entries` must read advanced ambient light, else image is one frame late.
- 679: ── UI Layout ─────────────────────────────────────────────
- 680: Recompute screen-space rects of all UI elements each frame.
- 685: ── UI Input ──────────────────────────────────────────────
- 686: Hit test, update `Interaction` component (needs after layout).
- 689: ── UI Render ─────────────────────────────────────────────
- 690: Collect UI draw commands into UiDrawList resource.

## crates/prism-engine/src/engine_scene_tests.rs

- 80: After unload then load same scene again: count must still be 2.

## crates/prism-engine/src/input.rs

- 104: Bit graph 43..=255 has 213 slots, only fits raw key codes vk <= 212.
- 105: 
- 106: Out-of-range key codes must be **discarded**, not modulo: modulo would make vk 226 and vk 452 land on
- 107: same bit, player presses an obscure key but sees another held, harder to debug than dropping key.
- 166: Persistent (accumulated across frames)
- 171: Transient (cleared each frame by begin_frame)
- 180: Pointer-lock state
- 201: --- Query helpers ---
- 261: --- Low-level event handlers (insert raw data) ---
- 348: ---------------------------------------------------------------------------
- 349: Cross-ABI input snapshot (#589)
- 350: ---------------------------------------------------------------------------
- 374: Key codes not fitting bit graph must be dropped cleanly: rather miss a key than let it masquerade as another.
- 416: `keys` bit graph and `KeyCode::bit_index` use same numbering (see ABI comment above).
- 463: `KeyT` / `F1..=F12` sit before `Other`, so every previously assigned
- 464: key keeps its bit; `Other`'s base moved 30 -> 43 (an ABI shift that
- 465: `keys` documents).

## crates/prism-engine/src/input_actions.rs

- 104: Without Shift: `1..=9` are components 0..=8, `0` is component 9 (consistent with keyboard number layout
- 105: and engine historical PBR isolation shortcuts).

## crates/prism-engine/src/input_tests.rs

- 51: transient cleared
- 68: duplicate
- 70: only once
- 81: full delta from origin
- 86: position persists
- 95: accumulated
- 106: transient cleared
- 168: held persists
- 179: 452 previously would fold to 30 + 452 % 226 = 30, looking like another key held.
- 180: Bit graph base now is 43 (`KeyT` + `F1..=F12` occupy 30..=42).
- 182: A normal, fitting key code also mapped below to confirm other keys unaffected.
- 215: Next frame: edges cleared, held keys remain.

## crates/prism-engine/src/launch_config.rs

- 109: The default is still "load the manifest's first resolvable scene".
- 111: Absent fields are not serialized, keeping the historical `{}` shape.

## crates/prism-engine/src/physics.rs

- 184: Cylinder segment + two end hemispheres: covered by "box + capsule radius" OBB, enough for query.
- 260: Inside box: pushed out along most shallow penetration axis.
- 295: Slab method: transform raycast into box's local coordinate system (axis vectors can carry scale, first normalize to take lengths).
- 333: Record entering face: first plane entered is the hit face.
- 366: Origin inside sphere: shoot from inside, use far intersection point.

## crates/prism-engine/src/physics2d.rs

- 142: Degenerate to box
- 314: Simplified to line-segment intersection then degenerate to circle
- 337: Capsule body segment
- 353: ---------------------------------------------------------------------------
- 354: Spatial query API
- 355: ---------------------------------------------------------------------------
- 521: Advance detection by step sampling
- 558: ---------------------------------------------------------------------------
- 559: Collision and contact solver
- 560: ---------------------------------------------------------------------------
- 691: SAT box-box collision detection
- 722: ---------------------------------------------------------------------------
- 723: Physics simulation step system
- 724: ---------------------------------------------------------------------------
- 741: 1. Integrate dynamic rigid body velocities
- 759: Collect all entities with PhysicsCollider2D
- 771: 2. Narrow-phase collision detection and solving
- 795: Solve rigid body impulse
- 846: Baumgarte penetration correction
- 871: 3. State transition compare and generate Started / Stopped events
- 916: 4. Advance dynamic and kinematic rigid body position and rotation

## crates/prism-engine/src/physics2d_tests.rs

- 89: After collision the two balls should bounce off each other or reverse direction.
- 98: Static floor.
- 116: Falling dynamic box.
- 139: After hitting static ground, falling speed blocked or bounced.
- 147: sensor entity
- 160: Object passing through sensor.
- 179: Step 1: overlap, triggers Started event.
- 187: Step 2: move out of sensor range (x = 1.0 + 10 * 0.5 = 6.0 > 2.5).
- 211: Cast raycast from origin toward +X.
- 233: Cast raycast from below toward +Y.
- 237: Box center y=5, half height=1, bottom face y=4.

## crates/prism-engine/src/physics_tests.rs

- 53: Direction too short / distance is 0 also should not hit.
- 111: Rotate 45° around X axis: box half width along z becomes 1·cos45 + 1·sin45 = √2.
- 118: Hit face normal is tilted: length 1, and pointing toward raycast direction.
- 137: Grazing raycast does not hit.
- 164: Shoot vertically from above: hit height = half_height + radius.

## crates/prism-engine/src/render_settings.rs

- 409: Bit 16 is skinning weight stream (#560), not a PBR component; combined value still counts as custom.
- 428: Only width given: derive height by window ratio (consistent with internal_extent).
- 432: Only height given: use directly.
- 436: Degenerate input does not produce 0 / NaN.
- 479: #613: when internal resolution active camera must project by internal ratio, not window ratio.
- 483: Without internal resolution use window ratio.
- 489: When only one dimension given aspect same as window, fall back to window ratio.
- 493: Bad window ratio also has deterministic result, won't pass NaN to projection matrix.

## crates/prism-engine/src/render_system/mod.rs

- 52: ---------------------------------------------------------------------------
- 53: SceneChanges
- 54: ---------------------------------------------------------------------------
- 123: ---------------------------------------------------------------------------
- 124: Extract frame packet (sim phase)
- 125: ---------------------------------------------------------------------------
- 245: 1. Collect per-frame scene state (needs orientation).
- 255: 2. Build the flat draw list and ECS-authored render helpers.
- 257: Frustum cull (#491): under multi-camera composition (#461), submit if **any** enabled camera can see
- 258: entity. All cameras at each layer share same draw list, culling only by main camera would miss objects
- 259: appearing only in second camera. When all camera matrices degenerate `camera_frustums` empty =
- 260: no cull (same as old behavior).
- 267: Engine's `renderQueue` order: opaque group ascending queue, transparent group far to near.
- 268: Sort must complete before submitting to renderer (`ForwardPass` draws only in list order).
- 270: Skinned entities: static loop skips them, here append draw items and put each bone palette
- 271: into same buffer (#560 P0-3b).
- 279: Light frustum cull (#526): casters outside shadow box need not enter shadow pass. Must be placed
- 280: after main camera cull — objects outside camera neither drawn in main pass nor waste shadow map.
- 297: Only `clearFlags == Skybox` draws skybox: other clear modes (solid color /
- 298: depth only / no clear) don't draw even if scene has enabled `Skybox` entity. `ForwardPass`
- 299: `None` directly skips skybox draw, so here is its switch.
- 305: 2D also uses camera frustum: tilemap clipped per cell (#529). With multiple cameras no single frustum can
- 306: represent all layers, conservatively don't clip 2D (2D rarely mixed with multi-camera 3D).
- 312: Line pixel ratio (#978): convert "1 pixel" to world units using main camera projection.
- 313: Without render height (offline extraction / unit tests) keep pure world-space geometry.
- 321: 2D lighting (#611): only sprites with `receive_lights` use it, empty when no lights.
- 323: 3D particles: world-space billboards, sorted far to near by distance to eye (transparent blend).
- 326: 3. Build the UI overlay from the ECS world (includes UiQuad/TextCmd).
- 329: 4. Extract three-tier sparse voxel chunks and cursor highlight with version caching.
- 332: Render settings overridden by app layer (`App`) before sending; engine defaults ensure independent use
- 333: of `extract_frame_packet` still works.
- 416: Encode any dirty or newly spawned chunks into the cache first, without aliasing `world`.
- 482: ---------------------------------------------------------------------------
- 483: FramePacket
- 484: ---------------------------------------------------------------------------
- 526: ---------------------------------------------------------------------------
- 527: Scene collection
- 528: ---------------------------------------------------------------------------
- 572: 1. cameras — main camera (`active_camera`: renderable enabled camera with max `depth`) responsible for
- 573: framing, clearing, shadow framing and aspect writeback; other enabled cameras compose as overlay layers (#461).
- 575: Aspect written to **camera that will be rendered** (active_camera consistent with framing selection below).
- 615: 1.2 multi-camera composition (#461): all enabled cameras each emit a layer in ascending `Camera.depth`.
- 616: Full-screen overlay (no viewport rect), each layer's `clearFlags` decides color / depth clear.
- 617: All cameras use same display aspect: this version has no per-camera viewport.
- 620: Only cameras truly able to produce image (have `LocalToWorld`) write back viewport aspect.
- 633: 1.5 orthographic flag — from **same camera** as framing / clearing. PT main ray decides
- 634: whether shared camera origin (perspective) or per-pixel near-plane origin (orthographic, issue #10).
- 639: 2. lights — unify `Light` (shaped) and old three components into `LightSample`.
- 653: 2.5 shadow switch: directional light's `Light.shadows` / `shadowStrength`.
- 654: Without directional light shadow meaningless (fallback direction just constant), turn off directly.
- 658: 3. local lights (point + spot) into forward SSBO, pick by importance when over LIGHT_MAX.
- 659: `color.w` is type bit (0 = point, 1 = spot), spot carries direction and inner / outer cone cosines.
- 660: 
- 661: Previously `.take(LIGHT_MAX)`: who's kept depended on ECS iteration order (≈ document order),
- 662: ninth light silently dropped, deleting previous one made it reappear (#468). Now sorted by
- 663: "intensity × range² / camera distance²", near / bright / long-range first.
- 676: Importance descending; ties by document order, stable across frames.
- 683: Back to document order when writing SSBO: same content, only easier to match per-light with editor panel when debugging.
- 741: 4. path tracing parsed lights: only feed point lights (PT shader has no spot cone branch yet).
- 758: 5. Light-space view-projection (shadow map).
- 759: 
- 760: Coverage tightly fits **main camera frustum**: project frustum corners into light space for AABB, rather than
- 761: placing fixed-size box in front of camera. Distant scenery camera can't see won't inflate shadow map, near plane keeps one box-height
- 762: margin for upstream occluders "able to cast shadow into view", far plane stops at farthest receiver in view.
- 763: Near plane depth: perspective projection's `p32 / p22` is near; orthographic projection that ratio is (near+far)/2,
- 764: meaningless, use a tiny value (orthographic frustum corners uniquely determined by far plane).
- 788: 6. clear color — from **same camera** as framing (`Camera.backgroundColor`).
- 792: Clear mode also from same camera (`Camera.clearFlags`).
- 819: ---------------------------------------------------------------------------
- 820: Render system
- 821: ---------------------------------------------------------------------------
- 823: Clear color no longer hardcoded in render system: comes from `SceneChanges::clear_color` via active camera
- 824: (`Camera::background_color`); when no camera use
- 825: `scene::systems::camera::DEFAULT_CLEAR_COLOR` (neutral gray).
- 839: Render settings come with packet from main thread (see `FramePacket::settings`).
- 856: 1. Diff against the previous frame via DirtyRouter.
- 882: Clear color read directly from `scene` (`scene.clear_color`), not destructured here.
- 884: Clear mode only affects skybox switch in extract phase, render system no longer needs.
- 888: Light budget only used in editor stats (`#468`), render side doesn't need.
- 893: 2. Build the per-frame UBO.
- 894: FrameUBOData is #[repr(C)] GPU data — convert glam → raw arrays at this boundary.
- 919: 2.5. Unified dirty-dispatch plan (DESIGN §9 / #1157): a change to the
- 920: draw_items contents (matrix / mesh / material / shadow flags) also
- 921: counts as a dirty scene and converges on DirtyDispatchPlan.
- 930: 3. Drive the render-graph phase API.
- 936: #1007: occlusion cull clones draw list only when needed this frame; when disabled zero overhead, list passed as-is.
- 945: #502: PostProcessVolume can override grading params, but LUT is global resource; strength
- 946: always follows global setting, path resolved to assets/ at parse time (renderer keeps previous on failure).
- 998: §8 five phases: prepare phase batch sync (currently dirty-log + future upload points)

## crates/prism-engine/src/render_system/shadow.rs

- 7: ---------------------------------------------------------------------------
- 8: Euler angle ↔ direction helpers
- 9: ---------------------------------------------------------------------------
- 30: ---------------------------------------------------------------------------
- 31: Matrix helpers
- 32: ---------------------------------------------------------------------------
- 121: Square off the X/Y extent so texels stay square, then snap the centre to
- 122: the texel grid (depth stays continuous so casters are not quantised).
- 127: Snapping can move the centre by up to half a texel, so grow the box by
- 128: one texel afterwards: coverage must stay conservative.
- 135: Depth: near margin of one box extent covers upstream casters that can
- 136: still throw a shadow into view; far stops at the farthest receiver.
- 140: `fwd·eye = near_s` puts view-space z = 0 on the near plane.
- 155: clip = projection * view * world (column-vector convention).
- 194: Two equal slabs along the camera axis: [near, d/2] and [d/2, d]. Each
- 195: cascade fits the frustum corners inside its own slab.
- 201: `view` is only read through the callers' depth split; keep it explicit so a
- 202: future change to the split metric cannot silently disagree with the shader.

## crates/prism-engine/src/render_system/tests.rs

- 51: Depth: clear depth only, keep whatever the previous layer drew.
- 55: SolidColor: clear colour (and depth), no skybox.
- 106: Main camera looks −Z, so object at +Z only visible in second (rotation 180°) camera.
- 179: A point in the middle of the visible slab is inside the map.
- 184: Scenery the camera cannot see must not be inside the map either: a
- 185: point far off to the side and a point past the shadow distance are out.
- 203: The x scale is 1/half, so the box half-extent is the reciprocal of the
- 204: clip-space width of one world unit.
- 461: With no renderable camera falls back to fallback perspective camera.

## crates/prism-engine/src/scene/loader.rs

- 55: ---------------------------------------------------------------------------
- 56: Component flags (must match the cooker's constants)
- 57: ---------------------------------------------------------------------------
- 65: ---------------------------------------------------------------------------
- 66: Light type bytes in the RSCN light record
- 67: ---------------------------------------------------------------------------
- 73: ---------------------------------------------------------------------------
- 74: SceneSource — what to load
- 75: ---------------------------------------------------------------------------
- 87: ---------------------------------------------------------------------------
- 88: SceneInstance — result of a load
- 89: ---------------------------------------------------------------------------
- 101: ---------------------------------------------------------------------------
- 102: ParsedRscnEntity — intermediate deserialised from the binary stream
- 103: ---------------------------------------------------------------------------
- 166: ---------------------------------------------------------------------------
- 167: RSCN parser
- 168: ---------------------------------------------------------------------------
- 199: v2: skip skybox HDR path in header (used by read_env_path_from_rscn
- 200: for the app layer; the per-entity skybox data is parsed below).
- 242: Transform: tx(12) + rot(16) + scale(12) = 40 bytes.
- 266: Flags.
- 282: Optional components.
- 312: type(1) + color(12) + intensity(4) + range(4) + inner_cone(4) + outer_cone(4) = 29
- 371: ---------------------------------------------------------------------------
- 372: Quaternion -> yaw/pitch conversion (for scene-loaded cameras)
- 373: ---------------------------------------------------------------------------
- 434: Normalize to avoid degenerate input skewing the result.
- 445: ---------------------------------------------------------------------------
- 446: Public helpers for env path extraction
- 447: ---------------------------------------------------------------------------
- 468: ---------------------------------------------------------------------------
- 469: SceneLoader
- 470: ---------------------------------------------------------------------------
- 569: Runtime GPU handles are derived from the authoring bundle.
- 595: A cooked camera without an explicit controller gets the default
- 596: free-fly controller, matching the legacy loader behaviour.
- 597: Orthographic cameras are 2D cameras: a fly controller would fight
- 598: the game's own camera follow, so they never get one implicitly.
- 607: Orientation follows rotation stored in scene. Only changing translation but leaving rotation gives post-load view unchanged,
- 608: but WASD slides along world axes (Sponza scene camera is such hand-written JSON).
- 669: `SceneJson::parse` migrates older authoring documents (v1
- 670: `transform.translation`) up to the current schema.
- 674: #466: dev scenes load the authoring JSON directly. Resolve stable
- 675: GUID references here too so a renamed asset keeps working without
- 676: waiting for a cook. Packaged scenes already contain resolved paths.
- 697: Phase 1: spawn all entities, insert transform + scene components.
- 705: Name
- 710: Stable id: `.scene.json` v3+ `id`, cross-session identity (scene_state / editor sync).
- 713: Transform
- 727: Components via registry
- 732: Runtime GPU handles are derived from the authoring bundle for
- 733: JSON scenes too. Without this, editor-authored MeshRenderer
- 734: entities never enter the draw query because MeshRef is absent.
- 759: Phase 2: hierarchy.
- 760: 
- 761: `.scene.json` v3+ `parent` is **stable entity id**, not array index
- 762: (see `prism_scene::json::EntityJson::parent`); id and index only happen to be equal when
- 763: `migrate_scene_doc` fills ids for old document by array order.
- 764: Taking id directly as index attaches wrong parent after reorder / deletion, even out-of-bounds becomes root.
- 774: `SceneJson::parse` only validates author writing, does not run `validate()`;
- 775: when dev scene has dangling id, keep entity and treat as root node, do not fail whole scene.
- 807: Phase 1: spawn all entities, insert scene & transform components.
- 813: Always-added components.
- 816: Optional human-readable name (empty string -> no Name component,
- 817: the inspector falls back to the raw entity id).
- 822: Local transform.
- 835: Initial world transform = local (will be recomputed by hierarchy system).
- 838: Mesh reference (generation 0 = unresolved; resolved by
- 839: resolve_assets_system once the .pak runtime is wired).
- 851: Material reference (unresolved).
- 863: MeshRenderer bundle — inserted when both mesh and material are
- 864: present. Carries the path strings for later resolution.
- 878: Light component.
- 918: Camera component.
- 920: Data component: projection + exposure + runtime aspect cache.
- 921: `aspect` is a placeholder; `app.rs` writes the real value on
- 922: the first resize / orientation change.
- 932: Free-fly input controller. yaw/pitch are derived from the
- 933: entity's quaternion so the scene file fully determines the
- 934: initial viewpoint; the camera position lives on the sibling
- 935: LocalTransform (translation). `move_speed`/`look_sensitivity`
- 936: keep their defaults.
- 948: Skybox component (the skybox entity is a regular entity in the
- 949: entities array; its Skybox component is inserted here).
- 967: Phase 2: build hierarchy via HierarchyHelper (must happen after all
- 968: entities exist so that parent indices resolve).
- 978: Collect root entities (no parent).
- 1003: ---------------------------------------------------------------------------
- 1004: ECS-driven scene geometry collection for offline baking
- 1005: ---------------------------------------------------------------------------
- 1029: Resolve material slot.
- 1051: Get the CPU vertex data for this mesh.
- 1139: The bake path builds GPU material records above, so the alpha
- 1140: semantics ride along: mode 0 is Opaque.
- 1150: -------------------------------------------------------------------
- 1151: Internal helpers for collect_bake_instances
- 1152: -------------------------------------------------------------------
- 1154: (transform_point / transform_normal replaced by glam Mat4 methods)
- 1216: Conservative: the error material has no alpha data, so never let it
- 1217: be treated as opaque anywhere.

## crates/prism-engine/src/scene/loader_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 9: ── helpers ───────────────────────────────────────────────────────
- 20: version 2 (v2 = skybox support in header + per-entity)
- 23: v2 header: env_len(2) + env_path empty  = no skybox at header level).
- 27: Name.
- 32: Parent.
- 36: Transform.
- 47: Flags.
- 66: Mesh path.
- 73: Material path.
- 80: Light.
- 92: Camera.
- 99: Skybox.
- 165: ── parse_rscn tests ────────────────────────────────────────────
- 226: directional
- 282: ── spawn_from_parsed tests ─────────────────────────────────────
- 314: Check hierarchy.
- 370: Identity model  matrix
- 417: directional
- 439: point
- 462: spot
- 523: entity  with parent index  beyond the entity  array
- 524: This is a malformed scene — the loader checks bounds.
- 537: The orphan entity should exist but have no Parent component
- 540: It should be counted as a root since it has no Parent.
- 546: Camera at [1,2,3], identity rotation (looks down −Z), 60° field of view
- 565: Data component read by scene::systems::camera::collect_camera).
- 573: Free-fly controller (yaw/pitch derived from entity quaternion
- 574: Position lives on the sibling LocalTransform, not on the controller.
- 578: Identity quaternion -> forward (0,0,-1) -> yaw=0, pitch=0.
- 582: Position is the LocalTransform translation
- 591: 90° rotation about +Y: quaternion (0, sin45, 0, cos45). A −Z forward
- 592: rotated +90° about Y points to −X, which FlyCamera expresses as
- 593: yaw=π/2 (forward = [-sin(π/2), 0, -cos(π/2)] = [-1, 0, 0]).
- 594: sin(45°)
- 615: `atan2` only gives value between ±π; this orientation yaw is π/2.
- 624: ── RSCN v4 ─────────────────────────────────────────────────────
- 664: Go through migration funnel: v1 document has no entity id, directly `from_slice` will by design report wrong.
- 679: RSCN stores entities parent-first, but sibling/root order is not the
- 680: authoring order. Resolve by name instead of assuming indices.
- 808: ── integration via load_and_spawn ──────────────────────────────
- 834: search  both the repo root and the crate dir so the test works
- 835: regardless of which directory `cargo test` was invoked from.
- 856: 6 entities: 1 skybox + 1 camera  + 1 directional light source  + 3 point lights.
- 859: Exactly one camera  entity  with a FlyCameraController + camera  data
- 860: Component + LocalTransform (position lives on the transform
- 864: The camera  should be positioned at [0, 2.5, 18] (per default.scene.json).
- 870: And it should carry a free-fly controller.
- 874: ── `.scene.json` parent = entity id (not array index) ─────────────
- 896: Array order is [child, root], but parent reference is id 9 (bit set below marker 1).
- 921: root(3) ← mid(7) ← leaf(1): three layers strung by id, not close to array order.

## crates/prism-engine/src/scene/scene_system.rs

- 134: Skybox is the authoritative source. A scene with a disabled Skybox
- 135: deliberately opts out of IBL, so do not fall back to the legacy
- 136: EnvironmentLighting component in that case.
- 148: Legacy compatibility for scenes authored before Skybox owned the
- 149: environment map.
- 380: Author format has no manifest name: clear last manifest scene identity,
- 381: otherwise `current_scene_name()` points to an already unloaded scene.
- 526: Component declares env_map but file missing: treat as no IBL.
- 530: Found EnvironmentLighting component but no env_map (or file missing) → don't build IBL.
- 553: -----------------------------------------------------------------------
- 554: internal: manifest-driven loading
- 555: -----------------------------------------------------------------------
- 625: Scene content always loaded by AssetId from asset package: scenes.json `path` only
- 626: lookup key in path manifest (.pak.meta.json), first parsed as AssetId, then
- 627: ResourceManager reads RSCN bytes — no disk file fallback.
- 638: Only unload old scene after new scene spawned successfully: on failure keep current
- 639: scene unchanged (see `load_named_from_manifest` convention).
- 687: IBL resolved per scene: after switching can't keep reusing previous scene's injected bytes,
- 688: otherwise old HDR covers new scene's skybox.
- 732: ---------------------------------------------------------------------------
- 733: Manifest helpers (migrated from old engine.rs) — scene system internal implementation
- 734: ---------------------------------------------------------------------------

## crates/prism-engine/src/scene/scene_system_tests.rs

- 32: Engine-owned entities (e.g. `runtime_initialize` fallback camera) have no `SceneMember`,
- 33: must not be mistakenly deleted on scene switch.
- 94: Manifest has this scene, but asset pack has no registered corresponding path → load failure.
- 125: ---------------------------------------------------------------------------
- 126: Manifest parsing: scenes.json priority, legacy scenes.toml fallback
- 127: ---------------------------------------------------------------------------
- 203: ---------------------------------------------------------------------------
- 204: Scene unload (#482)
- 205: ---------------------------------------------------------------------------
- 225: A normal intermediate node in scene, to confirm tree structure not wholly preserved.

## crates/prism-engine/src/scene/systems/animation.rs

- 80: One-shot / after manual pause replay: start from frame 0.
- 87: Paused keeps playhead; next playing=true will reset.
- 106: Absolute frame number within row; clamp to last column in row, avoid wrapping to next row.
- 224: Sampling borrow block: release `&World` borrow after computing, later can write component.
- 242: One-shot stops at end: playhead no longer crosses `duration - phase`,
- 243: otherwise increasing `duration` would resume mysteriously from middle.
- 253: #490: fire events for forward and backward playback; paused no emission.
- 261: Crossed one (or more) loops: first scan to end of loop, then from 0 to new playhead.
- 265: Include 0-second event at start (otherwise first event always off by one dt).
- 271: Backward: playhead drops from `previous_local` to `local`, events collected by traversed interval.
- 274: From current loop position back to 0, then from end back to new playhead.
- 297: Entities without `LocalTransform` (theoretically should not exist) get no playhead.
- 313: Rewrite each frame: when no animation entities also clears old events.

## crates/prism-engine/src/scene/systems/animation_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 46: t = 0.15s -> frame 1
- 64: 0.4s = exactly one cycle.
- 69: 0.5s -> frame 1
- 112: 96 px wide sheet with 16 px frames -> 6 frames.
- 123: frame 5
- 125: 0.65 -> wraps to 0
- 167: #584: same row columns 2..3 as independent action, plays 2 frames and loops.
- 182: t = 0.15 -> frame 2
- 184: t = 0.25 -> loop back to frame_start
- 202: 4 columns, starting from column 2 -> only 2 frames left.
- 225: phase 0.2s at 10 fps -> frame 2 immediately.

## crates/prism-engine/src/scene/systems/animator.rs

- 367: Frozen / frame-skipped entities keep evaluators (state machine / Montage progress comes back and continues), this frame does not advance.
- 375: Project-side written float / bool / trigger synced into graph evaluator.
- 930: Updated on a frame, this frame skips and clears flag.
- 964: debug fast path only maintained when caller actively inserts resource (see `AnimGraphDebug`).
- 967: State machine parameters (project-side written, engine reads every frame). Clone a copy: `take_trigger` needs `&mut`,
- 968: cannot continue borrowing World while iterating entities.
- 974: Root motion consumption (#560 P2): subtract this frame's root translation from pose inside, hand to entity's
- 976: Project-side requests live only one frame; grouped by entity, consumed when driving graph.
- 987: Foot IK (#560 P4) smoothing state: engine-owned, loaded like the runtime map
- 988: above and written back after the pass.
- 996: Animation LOD (#560 P6): far entities frozen, near entities evaluated every other frame; graph run retains state to resume later.
- 1003: Graph evaluation first runs in parallel by entity (#560 P6); the loop below only orders World reads/writes.
- 1025: Animation graph (#560 P1): when path non-empty and graph already loaded, takes priority over state machine / blend space / single clip.
- 1026: Evaluator persisted per entity (state machine nodes need cross-frame state); rebuilding heavily when graph path changes.
- 1080: State machine (#560 P1): first check whether this frame will transition, then use (possibly already swapped-out) state to decide what to play.
- 1081: 
- 1082: Transition criterion uses **this frame** playback time vs current state's clip duration: that frame confirms real playback reached
- 1083: this position, closer to author's correct `min_state_fraction` expectation than using "time after this frame advances".
- 1116: Empty / unknown state names also become true real names: editor and save see "current state",
- 1117: not an empty string.
- 1129: State machine owns speed / loop semantics; when no state machine attached, fall back to reading component.
- 1141: 2D blend space priority (#560 P1): at least three samples and able to triangulate to take effect; coordinates
- 1142: not limited / collinear fall back to 1D or single clip -- a few degrees of freedom is always better than entity not moving.
- 1163: 1D blend space (#560 P1): listed clips inside need at least two truly loaded,
- 1164: otherwise fall back to single clip. When 2D active, do not rebuild 1D samples to avoid wasted work.
- 1196: Playhead: switching clips starts from author-provided start time, otherwise continues on this frame.
- 1200: Switch clip: freeze old clip's pose at the moment it stopped as fade-out source (#560 P1).
- 1201: Old clip already not in library (asset deleted / this frame is blend space), fall back to already-held fast snapshot --
- 1202: switching twice in a row will not lose the positive fade-out portion.
- 1235: Blend space has no length of its own: take the longest clip, shorter ones naturally loop to catch up.
- 1252: Local time already in this playback content this frame: only triggers notify if it exists --
- 1253: just created / just switched clip's first frame should not blast out all clip events at once.
- 1269: Whether looped: state machine state takes priority, otherwise `Animator` component decides (clip's internal
- 1270: `looped` is only an import default value).
- 1273: Close key frame events: only trigger before playback, and only when really in this clip this frame.
- 1274: Reverse playback (`speed < 0`) needs incorrect reverse scanning interval, temporarily not triggered -- better to miss a footstep once
- 1275: than play all events backwards during reverse playback.
- 1287: Animation LOD subset: keep this frame's pose, only advance time + sample root joint (and `lod_bones`).
- 1288: When no name list, it means "root bone only"; first entry fully evaluates one frame to avoid T-pose pop.
- 1348: Cross-fade: old clip frozen at transition open moment, blended into new pose by weight.
- 1349: 
- 1350: First advance then take weight: on clip-switch frame `dt` is "distance from real time since last frame",
- 1351: transition should start timing from that moment, otherwise most frames never reach weight 1.
- 1357: No blend duration = hard cut: drop fast snapshot, otherwise the first frame when duration is adjusted back
- 1358: will suddenly fade in an already-expired pose.
- 1367: `fading` is old pose, `pose` is new pose: `t = weight` 0 -> old, 1 -> new.
- 1372: Weight already reached 1: new pose is the full new clip, fast snapshot can be dropped.
- 1380: Additive layer (#560 P1): incremental relative (bind) pose, weighted into final pose.
- 1381: Placed after cross-fade -- additive layer should not participate in "transition between two base clips".
- 1396: Animation graph layer (#560 P1): after base clip + cross-fade + single additive layer,
- 1397: stack `Animator::layers` in author order. Each layer samples on same time axis,
- 1398: per-bone mask lets only listed bones participate; BlendStack handles order, weight and
- 1399: additive semantics; missing clip / 0 weight layers are skipped.
- 1430: Root motion (#560 P0): record root joint relative increment this frame; when consumption open, hand translation
- 1431: to entity Transform, and subtract from pose inside (#560 P2).
- 1441: This pose is the fade-out source for the next transition (no need to clone: `pose` already used up on this face).
- 1471: State written back to component by engine: editor / save see current state, not asking playhead again.

## crates/prism-engine/src/scene/systems/animator_tests.rs

- 355: Back near: next frame immediately resumes normal dt, no leftover skip flag.
- 512: At 0.5s child rotated 45°: endpoint should leave +X axis.
- 516: (2,1,0) rotated 45° about joint: x = 2 - sin45, y = cos45.
- 545: Final pose: child rotated 90°, (2,1,0) about joint becomes (1,0,0).
- 559: 2.25s / 1.0s loop → samples 0.25s (about 22.5°).
- 647: Start: bend at 0s (child 0°).
- 653: First frame: weight still near 0 → still old pose (this is "no hard cut").
- 661: Halfway: weight 0.5 → about 45°.
- 671: Done: weight 1 → full new clip (90°), source cleared.
- 742: Transition source is **pose snapshot**, so removing old clip from library (asset delete / hot reload) doesn't affect fade-out:
- 743: previously this hard-cut, now must still finish transition.
- 752: Remove old clip from library on same frame transition starts.
- 772: Done: pure new clip, snapshot dropped.
- 909: parameter 0 → pure bend.ranim (0° at 0s).
- 923: parameter 1 → pure bent90 (90°).
- 932: parameter 0.5 → 45°.
- 948: Two paths but one not in library → only one sample, falls back to single clip.
- 962: Only one path + one position: not a blend space.
- 1001: First run single clip one frame, establishing playhead on bend.ranim.
- 1007: Transition start: weight 0 → still bend pose.
- 1018: Transition done: pure blend-space pose (90°), source cleared.
- 1135: First frame only establishes playhead: shouldn't fire whole clip events at once.
- 1139: 0 → 0.4: not yet.
- 1143: 0.4 → 0.8: crosses 0.5, fires once.
- 1155: One more frame (no time point crossed): event table rebuilt empty, won't refire.
- 1169: advance to 0.9
- 1172: 0.9 → 1.0 → wrap to 0.0: this segment crosses 0.95.
- 1213: 0.9 → 0.5 crossing 0.5 backwards: reverse playback doesn't fire (prefer missing one over replaying backwards).
- 1220: Previously "blend space → single clip" was hard cut (source is placeholder key, no sample in library).
- 1221: Now transition source is **pose snapshot**, so here must truly fade out.
- 1233: Switch to single clip (90°), 1 second transition.
- 1250: Done: pure 90°.
- 1281: Base 0° + additive 90° = 90°.
- 1302: Bad path (clip not in library).
- 1315: Weight 0 / NaN.
- 1399: Parameter 0: stay in idle (bend.ranim 0°).
- 1405: Parameter past threshold: next frame switch to run (bent90.ranim 90°).
- 1413: State change restarts from 0, doesn't continue idle's time.
- 1424: Transition carries own 2s duration; component set to 0 (hard cut) must defer to transition.
- 1437: First frame of transition: weight 0 → still idle 0° pose.
- 1445: After 2 seconds: 90°.
- 1469: Only 0.2s played (<50%): condition true still doesn't transition.
- 1473: Criterion uses **previous frame** playback time (that frame actually reached there), so frame crossing 50% is
- 1474: still blocked, next frame allows it.
- 1521: Origin sample = bend at 0s (0°).
- 1535: Midpoint along x axis: 0° and 90° half each → 45°.
- 1545: Edge between two 90° samples: still 90°.
- 1560: Only two samples: no triangle, falls back to single clip.
- 1573: Three collinear samples: triangulation fails, same fallback to single clip.
- 1632: Weight 0.5: only rotate 45°.
- 1780: Evaluator persists per entity, second half-second continues to 90°.
- 1848: 0.25s during freeze not backfilled: after resume only advances this frame's 0.5s.
- 1880: Two frames of 0.1s: first frame 0.2s, second skipped, total still 0.2s.
- 1988: Evaluator persists across frames: second frame continues to 90°, parallel get/put didn't lose state.
- 2218: 0.1 → 0.8: fade-in already full, remaining 0.2s / blend_out 0.4s → 0.5 weight.

## crates/prism-engine/src/scene/systems/camera.rs

- 42: `total_cmp` gives NaN a total order, avoiding unwrap risk from `partial_cmp`.
- 59: Main camera (max `depth`) will be drawn
- 126: Simple look-at: eye at (0,0,5), target at origin, +Y up.
- 127: Right-handed, camera looks down -Z per engine convention.
- 134: Standard perspective: 75° FOV, 16:9-ish aspect, near=0.01, far=500.
- 186: Eye position = world-space translation (column 3, rows 0..3 of the
- 187: column-major matrix). For a root camera this equals LocalTransform
- 188: translation; for a nested camera the hierarchy system already baked the
- 189: parent transform in.
- 195: Identity transform → camera looks down −Z (project convention); any rotation carried by LocalToWorld.
- 229: Input must drive the camera **actually being rendered**, else in multi-camera scenes WASD/mouse will move another one.
- 230: First get entity id so &self borrow ends here, then can take &mut components.
- 236: Scope the &mut borrow of ctrl so we can later borrow LocalTransform.
- 237: Pointer lock state is the only switch for camera input: when unlocked keyboard and mouse belong to project UI, camera
- 238: neither rotates nor moves (previously right-drag could still rotate view, i.e. one boolean governed only half).
- 243: Clamp just shy of straight up/down. The yaw-based `right()` keeps
- 244: the basis well-defined at any pitch, and ~89° reads as "looking
- 245: straight up" while avoiding pole-crossing roll.
- 249: Mouse wheel adjusts base move speed. In unlocked state wheel belongs to project UI,
- 250: so scrolling the menu does not accidentally change camera speed.
- 260: Translation: WASD/QE/Space/Ctrl relative to the yaw/pitch basis. Position
- 261: lives on the sibling LocalTransform (roots) - nested cameras use the
- 262: LocalToWorld derived by the hierarchy system  and shouldn't be moved by
- 263: input directly, so we only write to LocalTransform.
- 279: Movement keys only go to camera in pointer lock state: unlocked keyboard belongs to project UI (menus often use WASD
- 280: navigation), one keypress must never flip menu and move camera at same time.
- 300: Orientation must also be written back: `hierarchy_system` derives child node
- 301: (billboard / weapon / spot light) `LocalToWorld` only from `LocalTransform.rotation`, writing only yaw/pitch
- 302: would leave them stuck at old orientation.
- 303: 
- 304: `yaw`/`pitch` (and WASD basis vectors) are in **world** space, while `LocalTransform` is relative to parent:
- 305: under a rotated parent must first subtract parent's world rotation, otherwise child orientation
- 306: gets rotated twice by parent, and WASD would add world axes directly to local coordinates.
- 333: --- fly-camera math (ported from the deleted `FlyCamera`) ---------------

## crates/prism-engine/src/scene/systems/camera2d.rs

- 27: If generated id is 0 or mismatched, try finding by entity ID
- 137: ---------------------------------------------------------------------------
- 138: Name-based virtual camera follow (EchoWell slice).
- 139: ---------------------------------------------------------------------------

## crates/prism-engine/src/scene/systems/camera2d_tests.rs

- 76: Target inside deadzone (1.0 <= 2.0), camera stays still
- 82: Target moved out of deadzone (5.0 > 2.0), camera follows move 5.0 - 2.0 = 3.0
- 130: Viewport half width = 5.0 * 2.0 = 10.0, half height = 5.0
- 131: Boundary limit: X range [-20 + 10, 20 - 10] = [-10, 10], Y range [-10 + 5, 10 - 5] = [-5, 5]
- 172: 1.033 * 16 = 16.528 -> 17 / 16 = 1.0625
- 173: 2.088 * 16 = 33.408 -> 33 / 16 = 2.0625

## crates/prism-engine/src/scene/systems/camera_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 56: ECS query order is deterministic - first inserted should be first
- 184: Main camera = max `depth` **enabled** camera.
- 186: Disabled camera is neither main nor in ignored list (it should not participate).
- 291: Shortcut key only returns camera when pointer locked, so here pass `look_active = true`.
- 343: ---------------------------------------------------------------------------
- 344: Orientation writeback (issue #121)
- 345: ---------------------------------------------------------------------------
- 403: An identity child node in front of camera (local −Z)
- 417: yaw = π/2 → looking −X, so child world position is (−1, 0, 0)
- 448: yaw = 0 → world −Z
- 459: Compute parent world matrix first so controller reads parent rotation.

## crates/prism-engine/src/scene/systems/foot_ik.rs

- 335: Lifting the character by 5 m puts the ground far below the ankles.

## crates/prism-engine/src/scene/systems/hierarchy.rs

- 41: Root without `LocalTransform` (only `Child`) propagates downward with identity matrix.
- 83: Index into the parent's child list instead of cloning it: the recursive
- 84: calls mutate components, so the `&Child` borrow cannot be held across
- 85: them, but re-reading one element per child is still allocation-free
- 86: (#66 — this used to clone the whole list for every node, per frame).
- 98: `visited` also catches cycles in dirty data (A.Child=[B], B.Child=[A]).
- 108: No-transform intermediate node: does not write `LocalToWorld` itself, but passes parent matrix as-is
- 109: to descendants (missing Transform equivalent to identity transform).
- 132: Multiply along parent chain, reuse already-computed ancestor (deep tree also O(n)).
- 143: Dirty data can cycle (A→B→A): finish with already accumulated result, avoid infinite loop.
- 176: Distance LOD (#491): runtime system selects level then marks others hidden. It is separate from editor
- 177: own `Hidden`, to avoid LOD overriding author / editor visibility choice.

## crates/prism-engine/src/scene/systems/hierarchy_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 14: dummy
- 52: Expect translation  = [2, 3, 0]
- 78: grandparent
- 79: parent
- 80: child
- 111: gp: [1,0,0], p: [1,2,0], c: [1,2,3]
- 124: Parent entity despawned after child entity's `Parent` still points to dead entity (despawn only deletes own
- 125: component). Before such child node neither root nor reachable, `LocalToWorld` will forever freeze in
- 126: old value; now promoted to root by unified semantics, equal to its own `LocalTransform`.
- 156: Only has `Parent`, parent's `Child` inside but doesn't list it: tree inconsistent,
- 157: cannot let it forever wait for a parent node that will never be traversed.
- 190: root(x=10) → mid(no  LocalTransform) → leaf(x=0)
- 191: Mid itself should not get `LocalToWorld`, but leaf must inherit root's translation.
- 230: Dirty data forming cycle cannot let DFS recurse infinitely: A is root and lists B, B lists A again.
- 250: A is root (no Parent) → [1,0,0]; B is child → [2,0,0].
- 258: should not panic
- 263: entity  without LocalTransform — should be skipped silently.
- 266: No LocalTransform
- 321: Own `Active` not changed: checker displays `activeSelf`.
- 324: Parent heavy new opened, child tree comes back.
- 380: Bypass `HierarchyHelper` directly write `Parent`, simulate corrupted save.

## crates/prism-engine/src/scene/systems/keyframe_animation_tests.rs

- 69: 1.5 seconds → one full loop + half.
- 109: dt 0.25 × speed 2 = 0.5 seconds, plus phase 0.5 → sample 1.0 seconds = intermediate value.
- 129: Position taken over by track...
- 131: ...rotation / scale have no key frames, keep author value.
- 139: Looping clip wraps to 0 at `time == duration`, single play used to check last frame.
- 151: +90° around Y: +Z becomes +X.
- 224: No re-fire after clamping to last frame.
- 262: 0.9 → 1.2: wraps to 0.2, not yet 0.25.
- 265: 0.2 → 0.5: second loop tick.
- 285: Event present this frame, absent next frame must also be cleared.
- 319: 0.9 → 0.7: not yet 0.5.
- 322: 0.7 → 0.4: crossing 0.5 downward.
- 347: 0.1 → -0.2 wraps to 0.8: reverse crossing 0 continues from end, hits event at 0.8.

## crates/prism-engine/src/scene/systems/lights.rs

- 68: `Active` only describes self (Unity `activeSelf`); when ancestor disabled whole child tree excluded,
- 69: derived value written each frame by `active_hierarchy_system` (#433).
- 100: Area light source / box / tube: schema aligned, renderer temporarily unsupported.
- 160: 1. Shaped `Light`.
- 171: 2. Legacy components: skip entities already carrying `Light`, avoid double lighting.
- 180: `euler_xyz_deg_to_dir` gives "pointing to light source" direction, but `LightSample.direction`
- 181: uniformly stores light propagation direction (`transform.forward`), here take negative.
- 218: Legacy components store **half** cone angle (radians).
- 236: Here **no truncation**: local lights wait until `render_system` knows camera position to pick
- 237: most important LIGHT_MAX by importance (#468). Previously cutting extras here by ECS order made
- 238: which light is ninth dropped entirely depend on document order, with no user notification.

## crates/prism-engine/src/scene/systems/lights_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 57: #468: limit picked uniformly by `render_system` after knowing camera position, old component query no longer culls.
- 120: ---------------------------------------------------------------------------
- 121: Unified `Light` (shape) sampling
- 122: ---------------------------------------------------------------------------
- 156: Rotate 90° around Y: engine forward(+Z) becomes +X.
- 185: Old component `euler_xyz` represented "pointing to light source" direction; sampling stores propagation direction.
- 219: Full cone angle 60° → half angle 30° → cos = √3/2.
- 256: Area light source only for schema verification, renderer skips.
- 321: #468: collect phase does not truncate, budget picked by `render_system` by importance.
- 350: Larger range lights reach farther, preferentially kept.
- 355: Directional light not counted in local light budget.
- 361: ---------------------------------------------------------------------------
- 362: Directional light shadow state (`Light.shadows` / `shadowStrength`)
- 363: ---------------------------------------------------------------------------

## crates/prism-engine/src/scene/systems/lod.rs

- 24: First clear all previous-frame flags: when `LodGroup` deleted, child moved out of `Child`,
- 25: or camera deleted, leftover `true` must not make object disappear forever.
- 36: Screen-ratio mode needs each camera projection y scale; correct camera does not shrink with distance.
- 59: Distance mode uses nearest camera; screen-ratio mode uses max-ratio one, both semantics are
- 60: "any camera seeing nearest level displays nearest level".

## crates/prism-engine/src/scene/systems/lod_tests.rs

- 51: Child entity position does not affect level selection: distance measured from parent entity.
- 79: distance  0 -> LOD0
- 87: distance  20 -> LOD1
- 99: Distance 100 -> beyond all thresholds -> last level.
- 132: No enabled camera -> conservatively display LOD0.
- 165: Non-limited / negative distance treated as 0, not jumping to last level.
- 169: No thresholds -> always LOD0.
- 191: Non-limited ratio treated as 0, not jumping to last level.
- 193: No child entities / no valid threshold returns None, caller falls back to distance mode.
- 212: Distance 10m would select LOD2, but screen ratio only reaches LOD1.
- 220: Default 60° FOV: 1 / tan(30°) ≈ 1.732; ratio = 1 * 1.732 / 10 ≈ 0.173.
- 226: Distance 2m -> ratio ≈ 0.866, back to LOD0.
- 251: When radius illegal cannot hide whole group by screen ratio, should fall back to distance: 100m -> LOD2.

## crates/prism-engine/src/scene/systems/particle_tests.rs

- 59: 10 / seconds × 0.5 seconds = 5 particles.
- 63: Same seed in another world gives same positions.
- 87: Emit 0.5 seconds = 2 particles, then run to end of lifetime with no new emission.
- 147: First 0.1 seconds only spawn (spawn after integration), second 0.1 seconds starts falling.
- 151: Semi-implicit integration: first add v then move position → v = g * t = -1, displacement ≈ -0.1.
- 310: Camera at origin looking −Z: z = −9 farther, drawn first.
- 354: dt of 10 seconds clamped to 0.1 seconds → only 1 particle emitted (not 100).
- 358: New particle spawned after integration, so age this frame still 0.
- 383: Linear interpolation between two stops: t = 0.25 → scaled 0.5 → half red half green.
- 389: Beyond range clamps to endpoint; non-finite value treated as dead.
- 446: Non-finite multiplier as 1; negative multiplier clamped to 0, never produces negative size / NaN.
- 483: t=0.1: offset = 1 + (3-1)*0.1 = 1.2 → x += 0.12
- 493: t=0.2: offset = 1.4 → x += 0.14
- 508: Default curve = no change.

## crates/prism-engine/src/scene/systems/particles.rs

- 83: 0 is illegal xorshift state, here force non-zero.
- 114: Rejection sample: normalize random point in sphere; hit rate ~52%, a few retries enough.
- 155: Cone around emitter local +Y: first pick point on sphere surface, then compress
- 156: polar angle to [0, cone] by half-angle.
- 207: #501: collision shapes snapshot once per frame; can still accept this overhead even with no emitter collision
- 208: (usually scene has only a few colliders).
- 217: #501: terrain height field uses same per-frame snapshot; `TerrainData` written by terrain runtime system.
- 222: Author component may have just been deleted; copy first to avoid borrow conflict.
- 231: Invalid matrix (NaN / zero scale) treated as identity transform, don't shoot particles to infinity.
- 377: Paused: particles stay in place, emission count not accumulated.
- 381: 1. Integrate + recycle.
- 382: Drag handled as "decay drag fraction per second": `(1 - drag)^dt`, frame-rate independent.
- 394: Speed curve only changes **displacement**, not writes back `velocity`: otherwise it would stack with gravity / drag,
- 395: turning "wind speed" into permanent acceleration.
- 404: 2. Emission window (duration / looping). `duration == 0` = always emit.
- 405: 
- 406: First emit based on window state at "step start", then advance `cycle_time`: this way emitter with
- 407: `duration = 0.2` can fill the final small segment, rather than stopping early on the frame
- 408: that happens to hit the endpoint.
- 417: 3. Burst: emit all at once at start of each loop.
- 429: 4. Continuous emission.
- 448: 5. Advance emission window: loop back to 0 at endpoint (re-count burst) or stop at endpoint.

## crates/prism-engine/src/scene/systems/particles2d.rs

- 74: 0 is illegal xorshift state, here force non-zero.
- 139: sqrt gives uniform distribution in disc, not clustered at center.
- 144: 2D cone: base direction rotated around Z by `[-spread, +spread]`.
- 214: Author component may have just been deleted; copy first to avoid borrow conflict.
- 223: 2D only cares about Z rotation; invalid matrix treated as identity transform.
- 253: After author deletes component, runtime state must go too, no invisible orphan particles.
- 275: Paused: particles stay in place, emission count not accumulated.
- 279: 1. Integrate + recycle. Drag as "decay drag fraction per second", frame-rate independent.
- 306: 2. Emission window (duration / looping). `duration == 0` = always emit.
- 315: 3. Burst: emit all at start of each loop.
- 327: 4. Continuous emission.
- 346: 5. Advance emission window: loop back to 0 at endpoint (re-count burst) or stop at endpoint.
- 421: Solid-color quad: standard alpha blend, same as polyline.

## crates/prism-engine/src/scene/systems/particles2d_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 36: 10/s, each frame 0.05s → 0.5 per frame, two frames to make one.
- 39: Single step limit `MAX_STEP = 0.1s` (stutter protection), so 0.5s only advances 0.1s.
- 107: Spawn happens after integration, so at first frame end age is still 0.
- 168: After window end no more emission, existing particles keep alive.
- 222: Emitter rotated 30° around Z + (1, 2, 0) translation.
- 273: First frame only emits (age 0), second frame advances to 0.1s.
- 285: Quad size = start_size, center = emitter world position.
- 286: `to_cols_array_2d`: model[0] / model[3] are X axis and translation column.
- 291: Alive 0.1 / 1.0 → alpha 0.9.
- 299: Sprite layer 10, particle layer 20 → particle drawn after (higher layer).
- 344: Same path as `SceneLoader`: registry.apply is entry that writes
- 345: components into World from `.scene.json` / RSCN.
- 379: burst 1 + 0.1s × 50/s = 6 particles.

## crates/prism-engine/src/scene/systems/post_process.rs

- 37: Axis lengths as scale; rotation deliberately ignored (box stays axis-aligned).
- 53: Stable order: priority ascending, same priority by entity id, ensures reproducible result.
- 78: Signed distance to box: <= 0 when all axes inside.
- 123: LUT is global resource; volume only blends on-screen per-pixel parameters, intensity is
- 124: `render_system` uniformly overridden after parse to `settings.lut_strength`.

## crates/prism-engine/src/scene/systems/post_process_tests.rs

- 79: Inside box (near center).
- 82: Outside box and blend_distance = 0 → completely not involved.
- 95: Outside box 2 meters = half fade-out → contrast 1 + (3 - 1) * 0.5 = 2.
- 98: Beyond 4 meters → fully out.
- 165: Box half-size 1 * scale 2 = 2; x = 1.5 inside box.
- 168: x = 2.5 outside box.
- 185: Non-limited value falls back to default inside PostGrade::sanitized.

## crates/prism-engine/src/scene/systems/render.rs

- 131: Skinned entities go through `skinned_draw::append_skinned_draw_items` (#560 P0-3b):
- 132: same entity with two render components at once still drawn once.
- 155: `MeshRenderer.shadow_casting_mode` / `cull_mode` (
- 156: `shadowCastingMode` / `CullMode`). Runtime entities without that component use
- 157: defaults (`On` / `Back`).
- 186: With submesh table split into multiple DrawItems by range (#520); otherwise keep old behavior:
- 187: one entity one draw, covering entire index buffer.
- 207: Skinned draw items generated by dedicated `SkinnedMeshRenderer` branch (P0-3b).
- 231: Skinned draw items generated by dedicated `SkinnedMeshRenderer` branch (P0-3b).
- 397: 2D particles are solid-color quads, share same channel and sort as sprites (#594).
- 399: tilemap: one entity one whole mesh (#529).
- 500: Place the entity origin on the sprite pivot (convention:
- 501: (0,0) is the bottom-left corner).
- 565: Sub-pixel lines widened to 1 render pixel (#978): when pixel center outside triangle,
- 566: screen shows evenly spaced dots rather than a line. Ratio converted by depth of line's first point under main camera
- 567: (independent of orthographic camera and position).
- 578: All triangles of same polyline share sort key: `sort_by` is stable, triangles drawn in
- 579: generation order, and they don't overlap, so internal order doesn't affect result (#604).
- 593: Polyline has no authored blend field: keep standard alpha blend.
- 653: left normal, length = half line width.
- 659: `start[i]` = (left, right) on both sides of segment i start; `end[i]` = both sides of segment i end.
- 664: Endpoints: square cap extends half line width outward along segment direction, round cap filled by later fan.
- 682: `n` is left normal: right turn (cross < 0) puts outer side on left edge.
- 685: When turn approaches 180° two offset lines nearly parallel, miter point runs to infinity.
- 701: Bevel: inner side shares miter point, outer segments stop at own offset points.
- 742: Start sweeps toward -dir, end toward +dir: +90° turns from n to -dir, -90° to +dir.
- 774: Sheets are laid out row-major. `frame_index` counts within the selected
- 775: row first; only an index past the last column continues to the next row.

## crates/prism-engine/src/scene/systems/render_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 36: No active → defaults to true.
- 171: No MeshRef, no MaterialRef → no draw  item.
- 193: No `MaterialRef`: `MeshRenderer { material_path: "" }``
- 194: mesh-only entity (editor fixtures are this shape).
- 362: No `MeshRenderer`: default opaque, queue 2000, back-face cull, depth write.
- 369: Material transparency state must pass through to draw item (`CullMode.Off` / queue / blend).
- 401: `ShadowCastingMode::TwoSided`: both forward and shadow pass disable back-face cull.
- 407: ---------------------------------------------------------------------------
- 408: 2D sprite
- 409: ---------------------------------------------------------------------------
- 451: 32 px / 100 ppu = 0.32 world units, centred pivot → no offset.
- 479: Since flip_x is true, u0 and u1 are swapped: [0.5, 0.2, 0.1, 0.8]
- 492: bottom-left
- 500: Pivot at bottom-left: the quad shifts +X and +Y by half a unit so the
- 501: entity origin lands on the sprite's bottom-left corner.
- 528: #584: 8x4 atlas (128x64 / each frame 32x32), select row 2, column 2 within row.
- 550: Texture can split into 4 columns at 32px width, but author declares 2 columns: frame_index=2 should enter row 2.
- 572: #610: additive glow = SrcAlpha / One, not default alpha blend.
- 595: #582: reflection only needs single half-plane "below water surface", clip rect passed straight into instance data.
- 632: Polyline / particle / tile do not participate in authored clipping, all use NO_CLIP.
- 668: Spawned out of order on purpose.
- 703: layer 0 first (order 0 → z ascending, then order 1), then layer 1.
- 725: ---------------------------------------------------------------------------
- 726: 2D polyline (LineRenderer)
- 727: ---------------------------------------------------------------------------
- 774: Miter joint: two segments share one edge at (10, 0) — the two triangles of segment 0 and
- 775: the two triangles of segment 1 meet exactly at two vertices (the two ends of the miter edge).
- 807: V-shaped polyline: two segments each length 10, line width 2. Equal-width miter stroke area exactly equals
- 808: `width * centerline_length`; sum of triangle areas exceeding it means overlap at joint (#604).
- 849: Bevel has one fewer outer-corner triangle than miter, but not so few that only line segments remain: area between 0.9x and 1.05x.
- 906: ---------------------------------------------------------------------------
- 907: Sub-pixel line width (#978)
- 908: ---------------------------------------------------------------------------
- 989: After widening to 1 pixel: area = 1 x centerline length (magnitude of (10,5)).
- 1076: Parent re-enabled → child returns.
- 1082: ── frustum culling (#491) ──────────────────────────────────────────────
- 1125: Two entities share same mesh handle: differ only in model matrix.
- 1173: No `MeshBounds`: procedural mesh / not-yet-parsed mesh submitted as before.
- 1202: Main camera looks −Z, second camera looks +Z: each entity seen by exactly one.
- 1337: ---------------------------------------------------------------------------
- 1338: 2D lighting (Light2D, #611)
- 1339: ---------------------------------------------------------------------------
- 1414: Direction normalized to unit vector, position does not participate.
- 1434: Disabled parent likewise hides light.
- 1439: Parent switch is derived value propagated via `ActiveInHierarchy`, only computed at end of simulation step.
- 1446: 3D lighting does not participate in 2D: table empty, sprite remains unlit (and prints one hint).

## crates/prism-engine/src/scene/systems/skinned_draw.rs

- 1: Skinning (#560 P0-3b): hook `SkinnedMeshRenderer` entities into draw items.
- 2: 
- 3: Separate file instead of stuffing into `render.rs`: static mesh loop already full, this branch
- 4: also needs to append each entity bone palette into frame pack.
- 67: Cull uses **post-skinning** bounding box: bind pose box is off when character in large pose,
- 68: using it would cull character still on screen (#560 P0-3c).

## crates/prism-engine/src/scene/systems/skinned_draw_bounds_tests.rs

- 27: One bone stays in place, another moves +X 4m: skinned vertices fall between these two boxes,
- 28: so bounding box must cover both (−0.5..0.5) and (3.5..4.5).
- 41: Other axes unchanged.
- 54: Good/bad blend: only take the valid one.
- 61: Camera at origin looking −Z, object at (0,0,-5). Bind box very small near origin — here
- 62: place entity off-screen then bone pulls it back to screen: looking only at bind box will miscull.
- 88: Bind box position far at x = +1000 (off-screen); palette pulls mesh back in front of camera.
- 104: Bind pose box at (0,0,-5) (in frustum), after skinning moves to x=1000 (outside).
- 105: With palette cull must disappear; without palette (bind pose) still appears.
- 111: Return bone to original position: same entity appears in draw list again.

## crates/prism-engine/src/scene/systems/skinned_draw_tests.rs

- 33: `MeshBounds` is a World resource (#491), not an entity component.
- 77: Second palette actually begins after offset 2.
- 122: Camera at origin looking −Z, push object to x=1000 (outside screen) → whole box outside frustum.
- 188: When no `MaterialSlots`, fall back to entity-level material slot.

## crates/prism-engine/src/scene/systems/terrain.rs

- 112: Mesh is equidistant samples over world size, so gradient only needs scaling by cell distance.
- 284: Chunk center position in whole terrain local space; vertices subtract it.
- 632: Origin already below surface (e.g. camera buried in mountain): no "crossing from inside" hit.
- 636: Coarse sample step: shorter ray finer, longer ray at most 512 samples; then 16 binary steps.
- 745: `resolution = 0` = follow file size; otherwise must match exactly, avoid drawing wrong terrain.
- 911: Terrain without world matrix yet (just spawned): distance by origin.
- 1175: Old mesh no longer referenced, recycle it too.
- 1191: At least one quad per chunk; small mesh (cells = 2) degrades to single chunk.
- 1212: MeshRenderer is author component: only change path, keep material and shadow settings.
- 1264: Multi-chunk / LOD: owner only keeps material / shadow settings, no longer draws directly.
- 1265: Each LOD level is a set entity, under it all chunks of that level.
- 1379: Owner no longer has MeshRef, so won't be drawn again in main pass.

## crates/prism-engine/src/scene/systems/terrain_tests.rs

- 10: Go directly through from_raw, avoid opening a test-only constructor.
- 60: 2×2 slope: center interpolates to 0.5, normal should be tilted.
- 119: Corner is original value.
- 122: Center interpolates.
- 125: Out-of-bounds clamped to boundary.
- 132: heightmap CLI writes meters (can be negative), must not be clamped to [0, 1].
- 164: Fully flat ground: normals all +Y, height is 0.
- 182: Endpoint color of pure-height band (flat normal).
- 190: LOD mesh also carries color, count matches vertex.
- 289: Param change: rebuild, count follows density, old instance recycled.
- 445: Param unchanged: second run does not rebuild (path unchanged, ref still there).
- 450: Param changed: new path, old mesh recycled.
- 476: Default terrain is flat: everywhere 2.0 (entity height).
- 479: Outside range has no terrain.
- 542: 32 samples / 2 chunks: seam row stored twice by both sides, so vertex total is (16+17)²;
- 543: triangles still cover only 31×31 quads.
- 560: Param unchanged: no rebuild.
- 564: Switch back to single chunk: child entities and old mesh recycled, owner re-enters upload queue.
- 591: Left chunk right edge at local x=+2.5, right chunk left edge at local x=-2.5; after adding center offset both land at world x=0.
- 609: UV uses whole terrain [0,1], both sides of seam are u=0.5.
- 676: Four corners must be fully consistent, otherwise adjacent chunks / level switches reveal cracks.

## crates/prism-engine/src/scene/systems/tilemap_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests: tile map → SpriteDrawItem (#529)
- 3: ---------------------------------------------------------------------------
- 54: 2x2, only upper-left and lower-right cells have tiles.
- 68: Row-major: upper-left drawn first.
- 76: Tileset 64x32 + 16x16 tiles = 4 cols 2 rows; index 5 (6th block) in row 2 col 2.
- 94: Index 99 exceeds 4x2 tileset: clamp to last block, cannot sample outside image.
- 111: 2 cols x 2 rows, cells 2x1: row 0 is upper (larger y), larger row number goes lower.
- 167: z participates in sort key (camera looks −Z, larger z is closer).
- 174: generation = 0: tileset not yet uploaded.
- 185: Empty path / fully empty cells / illegal cell size do not draw.
- 247: 8x1 horizontal map, cell width 10; frustum only sees x ∈ [-5, 5].
- 257: Use engine own projection (Vulkan depth + y-flip), no need to write another formula.
- 282: Background tile layer -1 (far), sprite layer 0 (near).

## crates/prism-engine/src/scene/systems/time_of_day.rs

- 24: Collect first, avoid borrowing `TimeOfDay` and `Light2D` at same time.
- 36: Point lights controlled by author, only ambient light follows time.

## crates/prism-engine/src/scene/systems/time_of_day_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 53: Author writing -3 / 30 also readable, normalized then reformatted.
- 62: Sunrise / sunset themselves lie within transition interval.
- 79: Sunrise midpoint should be exactly half day/night (smoothstep midpoint is 0.5).
- 84: Default intensity is 1.0 for both day and night; need "night darker" config to see interpolation.
- 128: Midnight: ambient light takes night color.
- 133: Point light unaffected (lantern bright at night, relies on ambient light darkening).

## crates/prism-engine/src/scene/systems/ui_particles.rs

- 89: `playing = false` only stops spawning; live particles still finish.

## crates/prism-engine/src/scene/systems/ui_particles_tests.rs

- 30: Rain keeps its configured trail length; the spark preset ignores it.
- 61: Spawn positions stay above the top edge so particles fade in on screen.

## crates/prism-engine/src/scene_state.rs

- 47: ---------------------------------------------------------------------------
- 48: File path
- 49: ---------------------------------------------------------------------------
- 102: ---------------------------------------------------------------------------
- 103: Data structures  (new scene-component types)
- 104: ---------------------------------------------------------------------------
- 158: ---------------------------------------------------------------------------
- 159: Serialisation (hand-rolled JSON — no serde)
- 160: ---------------------------------------------------------------------------
- 202: --- DirectionalLight (new scene component; no enabled field) ---
- 303: ---------------------------------------------------------------------------
- 304: Save / load
- 305: ---------------------------------------------------------------------------
- 380: Camera
- 385: Directional light
- 390: Point lights
- 400: Transforms
- 410: Clocks
- 474: --- camera (on rendered camera entity, data components, see `state_camera`) ---
- 477: First parse entity once, subsequent per-component `get_mut` will not conflict with
- 478: world borrow of query.
- 497: --- Directional light ---
- 506: --- Point lights ---
- 518: Version 1 wrote point-light and transform arrays independently. At
- 519: startup the point-light transforms occupied the leading slots, so
- 520: recover those positions before applying the remaining transforms.
- 533: Persisted state is authoritative only for existing scene lights.
- 534: Never create an entity for an unmatched saved record.
- 547: v4+: match by stable id; old saves without id fall back to "match by order".
- 564: Records without id (v3 and earlier) still land on remaining lights by pool order.
- 570: --- Time of day ---
- 571: v5 pairs clocks with the scene by stable id; records without an id fall
- 572: back to pool order, like every other array in this file.
- 604: --- Non-light transforms ---
- 605: v4+ matches by `.scene.json` stable id, inserting / deleting entities no longer misaligns later poses
- 606: (#118); old records without id fall back to order matching.
- 689: ---------------------------------------------------------------------------
- 690: Minimal JSON helpers (shared with camera.rs internals; duplicated here to
- 691: keep scene_state.rs self-contained).
- 692: ---------------------------------------------------------------------------

## crates/prism-engine/src/scene_state_tests.rs

- 51: Quaternion q and -q represent same rotation, compare absolute dot product.
- 62: Old file (v1-v3) only wrote x,y,z; on read falls back to identity quaternion, no panic.
- 185: ---------------------------------------------------------------------------
- 186: #45 — save / restore must use "camera being rendered", not first camera in insert order
- 187: ---------------------------------------------------------------------------
- 232: Save uses same selector: highest `depth` **enabled** camera, not first in insert order.
- 270: Lower `depth` camera stays unchanged.
- 278: When loading scene state, layer-level propagation may not have run: without `LocalToWorld`,
- 279: `active_camera` returns `None`, falls back to `main_camera` (highest depth enabled camera).
- 373: With no directory at all, fall back to current directory (no panic).
- 409: Missing slot bit = no save, must not silently succeed.
- 470: A fresh world at the authored hour must pick the saved one back up.

## crates/prism-engine/src/transform_propagation_tests.rs

- 73: A user system moving the parent: the child's world matrix must reflect it
- 74: as soon as `update` returns, not one frame later.
- 95: Systems registered after `runtime_initialize` (the normal `App::add_system`
- 96: path) run after the built-in UI systems; propagation must still happen last.

## crates/prism-engine/src/ui/input.rs

- 77: Step zero: `Button` is authorable interactive element, scene need not explicitly attach
- 78: `Interaction` (that is runtime state); fill it here so author only writes `Button`.
- 86: Step one: clear all previous-frame state.
- 97: Step two: record target on press; click = release on same target.
- 105: Step three: write state only to topmost element.
- 121: Step four: keyboard activation. Focused element need not be under cursor (cursor may have moved away).
- 142: After release invalidate press target (regardless of release inside / outside element).
- 155: Elements disabled by `Active(false)` (including parent disable) do not participate.
- 159: Invisible elements neither drawn nor clickable.
- 177: Larger depth first; same depth smaller area first; fully equal keep later traversed
- 178: (later into draw list = drawn on top).

## crates/prism-engine/src/ui/input_tests.rs

- 192: Empty cell also submits.
- 208: Author only writes `Button` (no runtime state in scene): input system adds `Interaction`.
- 238: Two same-depth siblings overlapping: smaller area more "specific", treated as upper layer.
- 256: Next frame cursor moves out: clicked/hovered/pressed must all be cleared.
- 269: Manually create bad data (`HierarchyHelper` rejects cycles): depth calc must fallback itself.

## crates/prism-engine/src/ui/layout.rs

- 83: First collect all `Node + Style` entities and Parent.
- 84: Layer level only recognizes scene's `Parent` (the one maintained by `HierarchyHelper`), no second
- 85: `UiParent` state. `ComputedLayout` is this system's **output**: author does not write it,
- 86: missing ones get defaults filled here, otherwise layout / hit test / draw (all query on it)
- 87: would silently skip the entity.
- 88: Two passes: first root nodes with no Parent, then child nodes
- 103: Scale of Canvas each node is in (#463). Nearest Canvas ancestor decides;
- 104: without Canvas it is 1.0, behavior same as before Canvas introduced.
- 120: Parent-child relationship of layout group (#527). Sibling order must match draw order: by scene `id`
- 121: (runtime `spawn`ed entities come after), so "first added on left/top" is intuitive,
- 122: and fully same source as `assign_draw_order`.
- 139: Parent-first child-after traversal order. `seen` also guards against Parent cycles.
- 164: First pass measure: children first, parents after. Layout group measures own content size from children's sizes,
- 165: shrink accordingly when `fit_content` (#527).
- 168: Second pass arrange: parents first, children after. Layout group children positioned by arrangement order, other nodes continue
- 169: with original "anchor × parent rect" algorithm.
- 172: Main-axis cursor of each layout group (virtual coords, relative to content area start).
- 186: Parent is not a UI entity (no Node + Style): reuse its existing output rect.
- 310: Depth-first pre-order; `depth` only guards Parent cycles (nodes in cycle appear again within own subtree
- 311: but no infinite recursion).
- 507: Anchors → four rect edges
- 518: Fixed size vs anchor stretching; layout group `fit_content` uses caller-provided content size.
- 530: Stretch axis: anchors + margins decide origin; point anchor axis: pivot participates.
- 531: Point anchors must apply right/bottom margin only when aligned accordingly, otherwise element pinned to bottom-right corner
- 532: would hug screen edge (#593).

## crates/prism-engine/src/ui/layout_tests.rs

- 33: Center anchor + 200x100, parent rect = screen 800x600.
- 42: STRETCH anchor + CENTER pivot
- 46: Pivot only applies with point anchor; stretching origin cannot be pulled negative by pivot.
- 55: #463: 1920×1080 design scaled to 0.5 on 960×540 screen.
- 61: STRETCH root node
- 75: Centered 400×300 in design → virtual (760, 390) → screen (380, 195).
- 104: 1 screen pixel = 1 design pixel: behavior fully consistent before Canvas introduced.
- 140: 960×540 design upscaled to 1920×1080 = 2×.
- 145: Leaf centered in 960×540 design: (430, 245) → screen (860, 490).
- 180: Parent = [200, 150, 400, 300]; child centered in parent → center (400, 300), size 100x50.
- 195: Deliberately create child first then parent: query order puts child node before parent node.
- 196: After pre-inserting `ComputedLayout::default()`, parent component always "exists",
- 197: old impl took this as parent ready and positioned child using default rect once.
- 200: stretch to fill parent container
- 210: Parent = [200, 150, 400, 300] (screen centered 400x300), child must use its own rect this frame.
- 225: Order likewise "grandchild → child → parent", and parent positioned at screen center (nonzero origin),
- 226: so any level using default rect will be caught by assertion.
- 281: Bottom-right corner (800,600) - margin(24,20) - size (100,40).
- 298: stretch to fill parent container
- 303: Parent = [200, 150, 400, 300] (screen centered 400x300)
- 370: Centered + 400x200 → root [200,200,...]; content area starts at +padding.
- 398: 200x300 centered → [300,150,...].
- 428: Stretch needs another layout group: first confirm Center cross-axis offset is (300-20)/2.
- 454: Height stretched to full content area (300), width still own 40.
- 477: Do not hardcode width/height: fit_content needs something to shrink.
- 489: Content width = 60 + 10 + 40 = 110, plus left/right padding 8 → 118;
- 490: Content height = max(20,30) = 30, plus top/bottom padding 12 → 42.
- 493: Center anchor + pivot 0.5 → origin = screen center - half size.
- 516: Child element writes "center anchor" itself, but in layout group must be positioned by cursor.
- 541: Default STRETCH anchor + fixed size → centered (legacy behavior).
- 552: 1600×900 window, 640×360 internal → target rect 1280×720 centered, offset (160, 90).
- 557: Point `p` in layout drawn at `offset + p * 0.8`, inverse must recover `p`.

## crates/prism-engine/src/ui/mod.rs

- 111: Max 64 levels up on cyclic dirty data (consistent with depth limit in `ui::input`).
- 119: Scene view speech-level hide (#417) also must block UI draw and hit test; at runtime
- 120: This component absent, so game process unaffected by editor visibility.

## crates/prism-engine/src/ui/no_scene.rs

- 47: Too small for a readable panel; the pattern alone is already unmistakable.
- 51: The panel keeps the text readable over the pattern at any scroll offset.
- 93: Tip with full command: if you only write "run build-scenes", the reader still has to guess
- 94: `cargo run build-scenes` style incorrect usage.
- 122: Wrapping at one full period keeps the pattern seamless: the grid shifts by
- 123: exactly two cells, which has the same parity.
- 125: one point of overlap, so no seam shows between cells

## crates/prism-engine/src/ui/no_scene_tests.rs

- 24: A 1280 pt window with a 48 pt cell needs at least 26 x 15 cells.
- 27: Every quad is inside the extended grid and has a positive size.
- 36: The hint must be copy-pasteable, not a bare "run build-scenes".
- 42: Text is centred on the window and drawn above the pattern.
- 63: The scroll wraps at two cells, so a whole period looks identical again.

## crates/prism-engine/src/ui/panel_base.rs

- 22: ── Timer forwarding ─────────────────────────────────────────

## crates/prism-engine/src/ui/particles.rs

- 40: ── single particle ───────────────────────────────────────────────────
- 65: ── emission shape ───────────────────────────────────────────────────
- 79: ── particle system ───────────────────────────────────────────────────
- 255: Snow is the only preset that sways; the component has no knob for it.
- 268: 1. Emission (modulated by intensity; 0 means no emission, existing particles die naturally).
- 277: 2. Integrate motion + recycle (off-screen or expired).
- 278: Hoist scalars used below into locals, avoid closure capturing whole `self`
- 279: (which conflicts with mutable borrow of `self.particles`).
- 288: Sine sway driven by sway amplitude + phase (using age).
- 342: Spawn fade-in (first 0.18s) avoid appearing out of nowhere.
- 344: Fade out near bottom, avoid hard cut off screen.
- 354: Trail extends upward (y downward, so top = y - len).
- 364: capsule-shaped ends

## crates/prism-engine/src/ui/render.rs

- 10: ── draw commands ─────────────────────────────────────────────────
- 161: (layer, order, type: 0 = rect / 1 = text, sequence) — sequence makes sort stable.
- 164: Merge externally submitted quads first; sorting places them in EXTERNAL_LAYER
- 165: at EXTERNAL_UI_ORDER after all ECS UI entities, preserving intra-frame seq order.
- 183: Collect background rects
- 191: #528: `Button` changes color by interaction state, without `Button` continue using
- 192: `Style::background` (both can coexist, button first).
- 201: #591: with background texture `background` only as tint; if alpha 0 means author did not write
- 202: tint, display texture original colors as opaque white.
- 208: fully transparent skip
- 211: #463: corner radius is "design pixels", convert to screen pixels by containing Canvas scale.
- 227: Collect text
- 232: Consistent with quad above: `Style::visible == false` entities produce no draw commands.
- 233: Text entities usually also carry `Style` (`Node + Style + Text`), without `Style` treated visible.
- 247: #463: font size likewise by Canvas scale, otherwise text on 4K screen half size.
- 260: First by layer, then layer order, finally "background before text within same node".
- 296: List already ordered, `sort_by_key` stable, so just insert later elements at correct position.
- 341: Text commands pass through pixel coords as-is (expanded per-glyph quad when recording).
- 362: pixel left, top, width, height
- 363: NDC: [-1,1] x [-1,1], origin center. **Vulkan Y axis points downward**:
- 364: `y = -1` is screen top, `+1` bottom (camera projection
- 365: `p[1][1] = -inv_tan` is flipped for this convention).
- 366: Pixel y also grows downward, so here **no flip**:
- 367: flipping again would turn whole UI layer upside down.
- 372: NDC border radius: normalize by quad own size, render side multiplies back by pixel size
- 373: to get corner consistent with author writing (#634).
- 380: NDC [xmin, ymin, xmax, ymax]

## crates/prism-engine/src/ui/render_tests.rs

- 60: #528: `Button` overrides `Style.background`, changes color with hover / pressed.
- 102: #591: after `Style.image` resolves to bindless slot, this cell uses texture + `background` tint.
- 126: When author writes only `image` not `background`, alpha 0 treated as opaque white tint.
- 186: 200px wide quad → NDC width 0.5; 20px radius → 20 / 200 * 2 = 0.2.
- 192: #583: `Text.font` is asset path; empty string means system default font.
- 211: #527: wrapping is author switch, default single line.
- 259: Hidden entity will not enter on-screen data.
- 264: Active state only a filter condition, not deletion: component still there.
- 279: Child itself has no + only parent disabled.
- 333: Two fully overlapping interactive elements: later traversed (same area → more "top") should win,
- 334: but it is Active(false) disabled, cursor should fall to the one below.
- 346: #528: click holds on release, add a release frame.
- 373: Only change `Style::visible` (not `Active`): background quad and glyph should disappear together.
- 408: On-screen data likewise only visible text.
- 414: `visible` only a filter condition, not deletion: component stays, re-enable resumes drawing.
- 436: #463: font and corner radius are "design pixels", convert by containing Canvas scale.
- 476: Bad values (0 / NaN) treated as 1, cannot make vertices NaN.
- 523: Vulkan NDC: `y = -1` is screen top. Pixel top-left must land at (-1, -1),
- 524: otherwise whole UI layer flipped upside down (intro title displayed upside down was this bug).
- 525: 800x600
- 535: Vertical monotonic: lower pixel produces larger NDC y.
- 545: ── hierarchy draw order (#462) ─────────────────────────────────────────
- 584: Text must be drawn before black overlay, otherwise overlay cannot cover it.
- 613: Parent drawn first, child after: child panel always on parent background.
- 622: First spawned gets larger id: draw order follows document order.
- 638: id 2 (red) is earlier in document, despite spawned later.
- 649: The packet type carries no layer: externals are stamped with
- 650: EXTERNAL_UI_ORDER, which sorts past every ECS UI entity.
- 682: Project system registers after `ui::render` (static Player, see #1048).

## crates/prism-engine/src/user_data.rs

- 95: Save human-readable and hand-editable; only two fields, no worry about field order changes.
- 184: Parse failure: keep original file (may be from newer version), read-only this time.

## crates/prism-engine/src/user_data_tests.rs

- 1: ---------------------------------------------------------------------------
- 2: Tests
- 3: ---------------------------------------------------------------------------
- 41: Empty save also allowed to write: parent directory will be created.
- 57: New-version save key unreadable (parse failure), but **file must be preserved as-is**.
- 106: Override variable with highest priority, other branches not involved (test cannot depend on local APPDATA).

## crates/prism-engine/src/util/timer.rs

- 23: ── SlotMap ──────────────────────────────────────────────────────────────
- 67: ── type  ─────────────────────────────────────────────────────────────────
- 100: ── Public types ─────────────────────────────────────────────────────────────
- 146: The service owns the callback/params; tell it to drop them too so a
- 147: reused slot id cannot dispatch the previous timer's callback.
- 227: 1. Handle register / destroy messages (same mpsc, consumed in send order).
- 259: 2. Collect expired events
- 277: 3. Dispatch callbacks & reschedule new interval
- 314: ── Background worker ──────────────────────────────────────────────────────────
- 365: ── TimerSet ─────────────────────────────────────────────────────────────
- 444: register params for old_id

## crates/prism-engine/src/util/timing.rs

- 302: Without a logger the trace is off, so the tests force it on.

## crates/prism-engine/tests/animation_harness.rs

- 65: Re-sampling the same time must be bit-stable in this pure path.
- 69: Before the first key: endpoint hold.
- 79: Halfway through joint 0's translation (0→4 over 1s), rotation (0→90°
- 80: over 2s) and scale (1→2 over 2s); joint 1's scale peaks at 1s.
- 94: Second key boundary: every channel has an explicit value.
- 108: Between the two 1s keys on joint 1's scale: interpolate back down.
- 122: Endpoint hold after the clip and before it are the same sample.
- 144: Exactly at duration wraps to the first key...
- 149: ...and just after the boundary is just after the first key.
- 154: Multiple wraps stay deterministic.
- 175: Step 5 reached 0.5s: position 5, euler 45°, scale 1.5.
- 182: A one-shot clip holds the last key and reports finished thereafter.
- 190: Holding the endpoint must not replay the last event.
- 202: Times sit away from exact tick boundaries so f32 accumulation cannot
- 203: move the crossing into a neighbouring step.
- 222: Step 6 is at 0.4s: position 4, euler 36°, scale 1.4.
- 232: The playhead reaches 0 without claiming the one-shot clip finished.
- 274: Step 10 is exactly the wrap: local time 0, so the pose is back at start.
- 277: Step 11 is 0.1s into the second cycle.
- 288: The playhead reaches 1.0 on step 4 and is clamped there afterwards.

## crates/prism-engine/tests/animation_support/mod.rs

- 16: This module is compiled into several test binaries; each one exercises a
- 17: different subset, so per-binary dead-code expectations cannot all be met.
- 30: ---------------------------------------------------------------------------
- 31: Pose sampling (prism-anim)
- 32: ---------------------------------------------------------------------------
- 200: ---------------------------------------------------------------------------
- 201: Playback + events (engine)
- 202: ---------------------------------------------------------------------------

## crates/prism-engine/tests/support/mod.rs

- 11: This module is compiled into several test binaries; each one exercises a
- 12: different subset, so per-binary dead-code expectations cannot all be met.
- 616: 24 high-quality random bits mapped to [-0.5, 0.5).

## crates/prism-game-api/src/jobs.rs

- 318: The single slot is taken by the running job.
- 325: Capacity is released once the job finishes.
- 332: One worker plus a gate makes the ordering deterministic: every job
- 333: queues while the gate is held, then they pop in priority order.
- 379: Dropping joins the worker, so the running job still completes.

## crates/prism-game-api/src/lib.rs

- 230: Static Player: the engine writes this copy into the World resource
- 231: each frame (#589).
- 254: The plugin backend reads the input the host hands down every tick
- 255: through `HostApi::input`, so there is nothing to write here.
- 387: The host backend cannot switch scenes itself; the runtime
- 388: host (HOST_CAP_SCENE) executes the request after the tick.
- 431: `Engine::empty` inserts this resource; add one for custom hosts.
- 758: Project quads are solid colour only (no bindless texture slot);
- 759: layering happens in the merge, which orders externals past all
- 760: ECS UI entities.
- 812: Kept in sync with `prism-runtime-host::host_log`: 0 error / 1 warn /
- 813: 2 debug / 3 info.
- 848: `Backtrace::force_capture` does not depend on `RUST_BACKTRACE`; a stack
- 849: is wanted by default while playing.
- 856: Keep the original behavior (stderr) so terminal debugging still works.
- 869: SAFETY: as in `HostLogger::log`; the host log callback does not use
- 870: `user_data`, and the string is only valid for this call.
- 903: Install the logger the first time; afterwards just refresh the sink
- 904: (`OnceLock::set` failing means it is already there).
- 977: Fall back to "handle is non-empty" when the host provides no liveness
- 978: query; reads and writes still validate separately.
- 1157: The pointer is only valid until the next host call: copy it out
- 1158: immediately.
- 1529: Casting in the opposite direction misses.
- 1771: Do not call the callback when the host did not declare the capability bit.
- 1858: The same slot is reused, but with a different generation: the old handle
- 1859: must stay invalid.
- 1874: The `GameWorld` is gone; the quad survives in the world resource the
- 1875: engine merges into `UiDrawList` (#1048).

## crates/prism-game-api/src/project_entry.rs

- 90: SAFETY: the host provides a valid writable table pointer.
- 116: The module is its own dylib with its own `log` instance and
- 117: panic hook; without these the host console sees nothing.

## crates/prism-gfx/src/hdr.rs

- 38: --- Header: lines until the resolution line ("±Y <H> ±X <W>"). ---
- 56: The scanline count and the pixels-per-scanline can each be either the
- 57: image height or width, depending on which axes the file uses.
- 64: Checked so hostile/corrupt dimensions cannot overflow the allocation
- 65: arithmetic before we read a single scanline.
- 198: Detect RLE: a scanline starts with the 2-byte marker 0x02 0x02,
- 199: followed by the scanline width (big-endian u16).
- 209: Each of the 4 channels is RLE-encoded across the whole scanline.
- 246: Uncompressed: 4 bytes per pixel, row-major within the scanline.

## crates/prism-gfx/src/hdr_tests.rs

- 11: E=128 → f = 2^(128-128-8) = 2^-8 = 1/256. R=128 → 128/256 = 0.5.
- 18: Hand-built RLE RGBE file: 1 scanline, width 4.
- 19: Scanline header: 0x02 0x02 then width (big-endian) = 4.
- 20: Each channel is a single RLE run of 4 identical values.
- 23: RLE marker + width=4 (big-endian)
- 24: R: run of 4, value 10
- 25: G: run of 4, value 20
- 26: B: run of 4, value 30
- 27: E: run of 4, value 128
- 35: E=128 → f = 1/256.
- 44: Locks the literal-run convention: a count byte <= 128 means exactly
- 45: `count` literal values (NOT count+1). Width 4, all channels literal.
- 48: RLE marker + width=4 (big-endian)
- 49: R literal: count=4, values 10,20,30,40
- 50: G literal: count=4, values 1,2,3,4
- 51: B literal: count=4, values 5,6,7,8
- 52: E literal: count=4, values 128,128,128,128
- 60: E=128 → f = 1/256.
- 76: RLE marker + width, then nothing for the four channels.
- 86: A zero count would never advance `x`; it must be rejected, not looped.
- 95: R: run of 5 values on a 4-pixel scanline.
- 133: `+Y` runs bottom-to-top: the first scanline is the last image row.
- 141: Row 0 (top) comes from the second scanline, row 1 from the first.
- 150: `-X` runs right-to-left: the first pixel of the scanline is the last column.
- 163: `+X` first: scanlines are columns, so the file is stored transposed.
- 165: Column 0 top-down, then column 1 top-down.
- 172: (x, y) order: (0,0), (1,0), (0,1), (1,1) in the output buffer.
- 181: Same RLE scanline as `decode_rle_scanline`, but stored bottom-up.
- 184: RLE marker + width=4

## crates/prism-gfx/src/ibl.rs

- 351: CPU convolution helpers

## crates/prism-gfx/src/ibl_tests.rs

- 159: Bump version to 99
- 163: Truncated payload

## crates/prism-gfx/src/lib.rs

- 96: Same order as the declaration; these strings are the values used in `.scene.json`.
- 850: sRGB → linear: the engine's light colors are linear in the renderer.
- 920: Only Skybox draws the skybox background (`Camera.clearFlags` semantics).
- 957: Ordinals: Spot=0, Directional=1, Point=2, Rectangle(Area)=3, Disc=4,
- 958: Pyramid=5, Box=6, Tube=7.
- 1036: Reflection uses u8 encoding (like `CameraClearFlags`) so no extra
- 1037: type-table entries are needed.
- 1071: Only `TwoSided` disables back-face culling in the shadow pass.
- 1092: Low temperatures are warm/red, high temperatures cool/blue; 6500 K is near neutral white.
- 1114: Non-finite input falls back to 6500 K instead of producing NaN.
- 1121: Ordinals: Off=0, Front=1, Back=2.
- 1148: Ordinals: Disabled=0 … Always=8.
- 1166: The shader's default `ZTest` value is LessEqual.
- 1208: Opaque blending = One / Zero.
- 1251: The ranges are bounded by GeometryLast; any value is preserved losslessly.
- 1257: Negative values are preserved losslessly but are outside the engine's
- 1258: opaque range `[0, GeometryLast]`.
- 1264: Reflection uses i32 encoding (not a u8 enum) so negative values survive.

## crates/prism-ini/src/lib.rs

- 76: Index of the current section; `None` means no `[section]` seen yet,
- 77: so keys before the first header are dropped.
- 91: A repeated section is merged: keep writing into the first one.
- 251: Unknown escape: keep it, so `C:\temp` does not lose a char.
- 341: No inline comments: `#` / `;` inside quotes survive verbatim.
- 401: `[window` never closes, so it is a malformed line and `width` stays
- 402: before any section.
- 434: The editor writes only a few sections and must keep the rest.

## crates/prism-motion-matching/src/foot_lock.rs

- 128: Compute velocity from the un-locked position so locking does not cancel itself.

## crates/prism-motion-matching/src/tests.rs

- 120: Root sits at -1.0 m at t = 0.5 and moves 2 m/s towards -Z, so a sample
- 121: 0.25 s ahead is at -1.5 m.
- 185: Pretend frame 1 is what is playing right now: a cheap frame change is not
- 186: worth a 20% worse match.
- 215: Stiff weights still win: a clearly better frame overrides the bonus.
- 249: Frame 0 was picked before, frame 2 is playing now.

## crates/prism-platform/src/event.rs

- 138: Digit row: the `VK_*` codes are the ASCII codes for `0`-`9`.
- 416: 'Z'
- 417: VK_HOME
- 418: VK_F13

## crates/prism-platform/src/thread.rs

- 63: SAFETY: `GetCurrentThread` returns a pseudo handle for the calling
- 64: thread; `SetThreadPriority` only changes that thread's scheduling
- 65: priority.
- 80: SAFETY: the function only changes the QoS class of the calling thread.
- 86: Linux and Android keep the default priority: raising it needs
- 87: CAP_SYS_NICE or a matching RLIMIT_NICE.
- 99: A relative increment keeps a thread that is already nicer (for example
- 100: because the shell started the process that way) relatively behind,
- 101: instead of silently raising it back to an absolute value.
- 102: SAFETY: `nice` only adjusts the calling thread's nice value.

## crates/prism-platform/src/win32.rs

- 180: No logic in `WndProc` can panic; even if the mutex is poisoned, keep going
- 181: (losing the window is worse).
- 214: Creation: store `CreateWindowExW`'s `lpCreateParams` (a raw pointer to the Arc)
- 215: in USERDATA.
- 246: Vulkan presents directly; GDI only needs to mark the dirty region
- 247: valid.
- 251: Handled (equivalent to returning 1): do not paint the background,
- 252: avoiding a white flash.
- 269: Locked: the cursor is pinned to the client-area center, so
- 270: the offset is the raw delta (both are client-area coords).
- 277: Recentring must happen after the state lock is released:
- 278: `SetCursorPos` may dispatch a new mouse message, and
- 279: re-entering `WndProc` on the same thread would lock the
- 280: mutex again. (The recenter itself emits another move with
- 281: dx = dy = 0, which the check above swallows.)
- 316: XBUTTON1 = back button, XBUTTON2 = forward button.
- 325: HIWORD(wParam) is a signed multiple of 120 per notch.
- 332: While the cursor is hidden, do not let the DC paint the class cursor
- 333: back (return 1 = handled).
- 351: Follow the system-suggested rectangle (no content scaling; see the
- 352: module docs).
- 389: The return value of `WM_SETCURSOR` / `WM_ERASEBKGND` is a convention: 1 =
- 390: handled.
- 427: HWND is a process-level handle; this type is used only on the main thread that
- 428: created it (only `SendWindowHandles` crosses threads).
- 436: Destroy the window first (`WndProc` still touches the state), then
- 437: reclaim the Arc.
- 446: Borderless / fullscreen: title bar and resizing do not apply, so use a popup
- 447: directly.
- 461: Per-Monitor-V2: without this declaration the process is treated as
- 462: DPI-unaware and Windows stretches the whole frame bitmap (blurry). Setting
- 463: it twice fails, which is fine to ignore.
- 479: No background: Vulkan presents the whole frame, and a GDI repaint would
- 480: only flicker.
- 489: ERROR_CLASS_ALREADY_EXISTS = 1410: a second window in the same process
- 490: is normal.
- 494: The configured width/height follow winit's `LogicalSize` semantics (logical
- 495: pixels); here they are scaled by the system DPI into client-area physical
- 496: pixels so both backends open a window of the same size.
- 509: Borderless fullscreen: fill the primary monitor directly (no exclusive
- 510: fullscreen).
- 541: Window creation failed: reclaim the Arc reference handed out, avoiding a
- 542: leak.
- 608: `hinstance` must be set: without it `create_surface` returns
- 609: `ERROR_INITIALIZATION_FAILED` (a pitfall hit by the editor's native
- 610: viewport).
- 620: `ShowCursor` is a counter: call it once per state flip. The call is made
- 621: after releasing the state lock (it may trigger `WM_SETCURSOR`, and
- 622: re-entering on the same thread would lock the mutex again).
- 697: WM_QUIT does not go through `DispatchMessageW`.
- 736: Negative coordinates (mouse moved outside the client area's top-left).
- 739: One wheel notch = 1.0 line.

## crates/prism-platform/src/window.rs

- 37: Win32 HWND and Android ANativeWindow are process-level handles; the window
- 38: outlives the render thread.
- 242: FPS pointer lock requires `Locked` (cursor pinned in place, only original delta moves).
- 243: `Confined` only clamps cursor within window rectangle; on macOS / X11 can still freely
- 244: slide, mouse look follows window edge. System not supporting `Locked` falls back to
- 245: `Confined` (Windows Confined + hide cursor also gets clamped to 1px),
- 246: both failing counts as not locked.
- 284: Win32 backend in `set_pointer_confined` sync maintains `state.grab`, no need to record again.

## crates/prism-platform/src/winit_backend.rs

- 57: winit's `Window` is the window owner itself; the render thread only takes raw
- 58: handles, so the `Window` is no longer needed.
- 91: Preserve dedicated Key variants (W/A/S/D/Q/E/T) for camera input; map remaining letters to Key::Letter.
- 105: Map editing, navigation, and punctuation keys to Win32 VK_* codes so
- 106: the winit backend emits the exact same Key::Other payload as win32.
- 231: Normalize Key::Other to Win32 VK codes to match the Win32 backend.
- 356: A high-precision wheel reports fractional lines, passed through directly.
- 399: Unlisted keys must not be dropped: they degrade to the winit discriminant.

## crates/prism-project-api/src/lib.rs

- 212: These fields were appended in #485: the earlier `left_clicked` /
- 213: `left_held` / `keys` covered only "left button held", with no outlet for
- 214: the right button, wheel, mouse delta, touch, or key edges. They sit at the
- 215: end so ABI differences show up only in `ABI_VERSION`.
- 742: Out-of-range indices must not write past the end.
- 764: Truncates beyond the fixed array length instead of panicking.
- 771: `InputSnapshot` must be able to cross the ABI as a block: its fields
- 772: can only be POD, and it must not silently grow until the `#[repr(C)]`
- 773: layout depends on the compiler.

## crates/prism-reflect/src/desc.rs

- 216: field separator
- 325: editor flag flipped
- 426: Nested struct schema change MUST change the outer struct's schema_hash!
- 429: Also test nested inside List
- 445: Also test self-referential / recursive struct termination
- 464: Must terminate without stack overflow

## crates/prism-reflect/src/glam_impls.rs

- 35: A SIMD newtype has no usable field offsets; use scalar-array layout.

## crates/prism-reflect/src/rtti_bin.rs

- 146: The encoder must mirror the decoder's `MAX_TYPES` check, otherwise it
- 147: can write a file the decoder rejects (#87).
- 158: TypeKind::Struct
- 573: `#87`: the limit used to be `1 << 16`, so exactly 65 536 fields
- 574: passed the `>` guard and were truncated to 0 by `as u16`. The limit
- 575: must equal the carrier width's maximum.
- 590: The encoder's guard must mirror the decoder's `type_count >
- 591: MAX_TYPES` check, and the limit itself must fit in `u32`.

## crates/prism-reflect/tests/reflect.rs

- 113: The deserialization path ignores read_only.
- 127: Default kept
- 129: read_only field restored via the force path
- 130: skip fields are never restored from the object.
- 161: Reverse direction: Any -> Object.
- 203: Re-registering must not break the restore path.
- 216: Encoding the same input twice must produce identical bytes.
- 222: Outer + Inner + glam::Vec3
- 232: Restore the decoded result to a concrete type via the registry.
- 236: Nested objects are present in the type table.
- 272: Truncated input.
- 331: Non-enum fields are still None (an absent enum must not be confused with
- 332: an empty variant list).

## crates/prism-render/src/acceleration_structure.rs

- 202: Make the built BLAS visible to subsequent acceleration-structure
- 203: reads (the TLAS build references it). Without this barrier the
- 204: TLAS build can read a stale/empty BLAS and every ray misses.
- 285: ---- 1. Get build sizes for every BLAS, find max scratch. ----
- 310: ---- 2. Allocate one scratch buffer (max size across all chunks). ----
- 323: ---- 3. Allocate AS buffers and create BLAS handles for ALL at once. ----
- 347: Temporary placeholder — we'll fix up handle/address after building.
- 348: (The handle is already valid; we just need the struct for the build.)
- 359: ---- 4. Submit in chunks (each chunk < TDR timeout). ----
- 438: ---- 5. Clean up scratch. ----
- 523: Pair each instance with its BLAS address by array position. We do NOT
- 524: use `inst.custom_index` to index `blas_addresses` - that field is
- 525: reserved for shader-visible per-instance data (e.g. a material slot),
- 526: so it must stay decoupled from the BLAS-address array position.
- 527: Callers must pass `blas_addresses.len() == instances.len()`; a
- 528: missing entry falls back to address 0 (produces no hits for that
- 529: instance - safe, just invisible).
- 665: Make the built TLAS visible to subsequent ray-query traces
- 666: (compute shader). The dst stage covers both further AS builds
- 667: and the compute stage that issues OpRayQueryInitializeKHR.
- 705: `Default`-constructed (null) TLAS instances are safe to drop.

## crates/prism-render/src/anf.rs

- 55: ---------------------------------------------------------------------------
- 56: Native ANF FFI definitions
- 57: ---------------------------------------------------------------------------
- 119: RTLD_LAZY
- 166: ---------------------------------------------------------------------------
- 167: AnfPass — RenderGraph node
- 168: ---------------------------------------------------------------------------
- 267: Rebuild SR instance.
- 406: Convert output image to SHADER_READ_ONLY_OPTIMAL for next PostPass sampling.

## crates/prism-render/src/anf_tests.rs

- 23: NVIDIA vendor ID
- 40: Intel

## crates/prism-render/src/asset_decoders.rs

- 161: The base level is the only level in `pixels`; asking the manager to
- 162: derive the chain would misinterpret the count.
- 170: The cooked format does not record colour space yet; RGBA8 textures are
- 171: treated as sRGB like every other albedo path in the engine.
- 175: BC1 / BC3 without color space info; treated as sRGB same as RGBA8 (albedo / emissive is
- 176: primary use. When linear needed, use `texture_format_for` on the layer, it knows slot bit semantics) (#519).
- 250: Channel 0 of an interleaved uv block: `uv_channels` pairs per vertex.

## crates/prism-render/src/asset_decoders_tests.rs

- 90: 2x2 RGBA8 with a 1x1 follow-up mip.
- 102: The buffer has to satisfy RenderTextureManager's exact-size check.
- 144: Block compressed texture can upload directly (#519): BC1 is 8 bytes / block, BC3 is 16 bytes / block.
- 169: Point the first mip at byte 0: inside the header/offset table.
- 176: 2x2 RGBA8 needs 16 bytes; hand it 12 so the manager would reject it later.
- 217: v2 table: one submesh, covers all indices.

## crates/prism-render/src/auto_exposure.rs

- 122: `.Load()` needs no filtering, but the SPIR-V still declares the image
- 123: as a sampled image, so a sampler has to be bound.
- 194: The histogram and luminance buffers are frame-independent, so they
- 195: are written once here and only the source image is rewritten per frame.
- 291: `AutoExposurePass` descriptor bindings do not carry UPDATE_AFTER_BIND
- 292: (the metering inputs are stable), so a write to a set the GPU may
- 293: still be reading is a validation error. The source view changes only
- 294: on resize or a render-mode switch, so skip the write otherwise.
- 330: Frame 0 has no history to adapt from, so snap to the metered
- 331: value instead of easing in from an arbitrary exposure.
- 337: Clearing the histogram is a transfer operation; the dispatch that
- 338: follows accumulates into it.
- 381: The adaptation reads the histogram the histogram dispatch just
- 382: filled, and writes the luminance buffer PostPass samples.
- 418: Hand the result to PostPass, which reads it in the fragment stage.
- 453: Declare every candidate HDR input so the graph inserts the barrier
- 454: into SHADER_READ_ONLY_OPTIMAL whichever one `execute` meters. The
- 455: same three handles `PostPass` reads, so the layouts are cache hits.
- 478: Metering is opt-in. Off writes a zero adapted luminance, which the
- 479: tonemapper's `auto_exposure()` reads as "no metering" and turns into
- 480: a gain of 1.0, so the image matches the pre-auto-exposure pipeline.
- 501: Meter whatever the tonemapper is about to display. Mirror
- 502: `PostPass`'s selection so the metered image and the displayed image
- 503: are always the same one, including the temporal resolve that now also
- 504: runs on the path-traced image.
- 562: Nothing rendered yet; leave the previous adapted luminance alone
- 563: so the tone curve does not jump on a skipped frame.
- 587: The struct is generated from the Slang reflection, so this only
- 588: guards against a stale binding file after a shader edit.
- 598: One u32 per bin, which is what `MeteringBuffers::new` allocates.
- 604: `exposureAdaptation` assumes low < high; inverting them makes the
- 605: weighted sum zero and freezes the exposure at its previous value.
- 606: These are compile-time constants, so the check is a const block
- 607: rather than a runtime assertion clippy would flag as constant.

## crates/prism-render/src/bake_common.rs

- 16: -------------------------------------------------------------------
- 17: Per-instance PT geometry types
- 18: -------------------------------------------------------------------
- 98: -------------------------------------------------------------------
- 99: Emissive triangle extraction
- 100: -------------------------------------------------------------------
- 116: GpuMaterial is 96 bytes
- 193: -------------------------------------------------------------------
- 194: BLAS / TLAS build
- 195: -------------------------------------------------------------------
- 215: ---- 1. Concatenate all instances into one combined vertex/index buffer.
- 253: ---- 2. Build all BLAS in one batch (single submit + wait).
- 319: -------------------------------------------------------------------
- 320: Buffer upload helpers
- 321: -------------------------------------------------------------------

## crates/prism-render/src/batch.rs

- 175: Staging buffer for the base mip — suballocated from the staging arena
- 176: using 256-byte alignment (matching optimalBufferCopyOffsetAlignment).
- 179: UNDEFINED -> TRANSFER_DST_OPTIMAL (all mip levels).
- 195: Copy staging -> mip 0.
- 216: Generate mip chain (mirrors buffer.rs but uses the shared cmd).
- 217: 
- 218: When `pixels` carries a fully baked chain (KTX2), upload the remaining
- 219: levels straight from staging — baked compressed blocks survive and no
- 220: blit feature is required. Level 0 was copied above.
- 267: Blitting is a format feature, not a given: BC7/ASTC/ETC2 often
- 268: lack BLIT_SRC/BLIT_DST (notably on mobile), so fail loudly rather
- 269: than record a command the driver rejects.
- 336: src level done -> shader read
- 368: final level -> shader read
- 384: single mip: dst -> shader  read  (the baked-chain path above already
- 385: transitioned every level, so it must not fall through to here)
- 430: Unmap all staging chunks BEFORE queue submission.
- 431: Critical for software rasterizers (llvmpipe) to ensure host writes
- 432: are properly synchronized before vkCmdCopyBuffer / vkCmdCopyBufferToImage execute.
- 446: Clean up all staging chunks now that GPU transfer is complete.
- 454: Safety: if `finish` was called, `started` is false and the command
- 455: buffer has already been freed. If the uploader was dropped without
- 456: finishing (early return on error), we must release the command
- 457: buffer + staging resources ourselves so nothing leaks.

## crates/prism-render/src/bindless.rs

- 270: --- Create the 4 global samplers ---
- 285: --- Descriptor set layout: two bindings ---
- 286: Bindings are listed in ascending binding-number order so the binding
- 287: flags (which must be in the same order as the bindings array) line up
- 288: correctly. The variable-count SRV array is the highest binding.
- 290: binding 0: global samplers (fixed count)
- 300: binding 1: SRV array (textures without samplers)
- 312: Binding flags: samplers are immutable; the SRV array gets the bindless
- 313: flags. VARIABLE_DESCRIPTOR_COUNT must be on the highest-numbered binding
- 314: (binding 1 = SRV), which it now is.
- 331: --- Pool ---
- 349: --- Allocate the set with variable descriptor count ---
- 361: Write the global samplers into binding 1 immediately (they never change).
- 393: Released slots are reused before the bump pointer advances.

## crates/prism-render/src/bindless_tests.rs

- 28: Mimic the relevant fields to exercise the bookkeeping logic.
- 29: without touching Vulkan
- 33: Equivalent of the public method writes a slot and bumps next.
- 34: past it.
- 36: Place slot 0 (the fallback).
- 38: The next `register` call must use slot 1, not 0.
- 43: --- SlotAllocator (pure slot bookkeeping, no Vulkan device needed) ---
- 53: Released slots are reused before the bump pointer advances.
- 62: LIFO: the most recently released slot comes back first.
- 75: Never-claimed slots are unused too.
- 87: A rejected release must not widen the pool: the slot is handed out once.
- 97: Slots below the high-water mark stay bumpable-to, not reusable.
- 129: Freeing one slot makes room again.

## crates/prism-render/src/buffer.rs

- 45: Buffers created with SHADER_DEVICE_ADDRESS require the backing memory to
- 46: be allocated with VK_MEMORY_ALLOCATE_DEVICE_ADDRESS_BIT (chained via
- 47: VkMemoryAllocateFlagsInfo). The validation layer rejects the bind
- 48: otherwise (VUID-vkBindBufferMemory-bufferDeviceAddress-03339). We chain
- 49: the flags struct only when the usage requests device addressing, since
- 50: the flag also forces allocation from a device-address-capable heap.
- 105: Create staging buffer (host-visible, host-coherent).
- 114: Map and copy data.
- 124: One-shot command buffer to copy staging -> destination.
- 149: Submit with a dedicated fence so we only block on THIS transfer, not the
- 150: entire graphics queue (queue_wait_idle would stall unrelated work).
- 164: Wait only for this submission to finish before cleaning up.
- 174: Clean up staging resources.
- 219: `cmd_pipeline_barrier2` lives in VK_KHR_synchronization2. On a Vulkan 1.2
- 220: device the core `vkCmdPipelineBarrier2` symbol is not exposed, only the
- 221: `...KHR` variant. `ash`'s `Device` wrapper only loads the core symbol, so
- 222: we use the KHR extension wrapper, which resolves the KHR entry point.
- 224: `cmd_blit_image2` (mip generation) comes from VK_KHR_copy_commands2 on a
- 225: 1.2 device; same reason as `sync2` above, use the KHR wrapper.
- 241: TRANSFER_SRC is needed to blit each mip level from the previous one.
- 269: Stage the pixels and copy them into the image.
- 270: `pixels` may hold a whole pre-baked mip chain (KTX2), so the staging
- 271: buffer is sized by what the caller provided, not by the base level.
- 341: A KTX2 texture ships a fully baked mip chain; `pixels` then holds every
- 342: level back to back. Upload those levels straight from staging instead of
- 343: blitting: the baked blocks survive and compressed formats never need
- 344: BLIT_SRC/BLIT_DST. Level 0 was copied above, so this copies 1.. and then
- 345: moves the whole chain to shader-readable in one barrier.
- 396: Only level 0 exists: generate the lower levels by blitting each one
- 397: from the previous level. Algorithm mirrors ibl.rs but uses the
- 398: synchronization2 barrier API to match the rest of this function Levels
- 399: 1..mip_levels are still UNDEFINED (transitioned to TRANSFER_DST above).
- 400: 
- 401: Blitting is a format feature, not a given: BC7/ASTC/ETC2 often lack
- 402: BLIT_SRC/BLIT_DST (notably on mobile), so refuse instead of recording a
- 403: command the driver will reject.
- 410: Level 0 is now TRANSFER_DST; promote it to TRANSFER_SRC so we can
- 411: blit from it into level 1.
- 479: Source level is done being read; move it to shader-readable.
- 500: Prepare this destination level as the next source (unless it is
- 501: the last level, which stays TRANSFER_DST for the final barrier).
- 525: Final level (mip_levels - 1) is still TRANSFER_DST_OPTIMAL; move it
- 526: to shader-readable.
- 548: mip_levels == 1: no blits, just transition the single level to
- 549: shader-readable. (The `has_all_mips` path above already moved the
- 550: whole chain, so it must not fall through to here.)

## crates/prism-render/src/buffer_tests.rs

- 35: Every subsequent level is the same function of the halved extent.
- 51: 8x8 RGBA8: 64 + 16 + 4 + 1 texels * 4 bytes.
- 58: A single-level "chain" is just the base level.
- 64: ASTC 6x6 blocks 16 bytes: an 8x8 level needs a 2x2 block grid.
- 67: 4x4 level still needs one 6x6 block (partial blocks are padded).
- 69: BC4 is 8 bytes per 4x4 block.
- 74: Degenerate extents clamp to 1, not 0.

## crates/prism-render/src/capabilities.rs

- 95: -- Layer 1: Vulkan 1.2 promoted features (foundation for RT) --
- 123: -- Layer 2: acceleration structures (prerequisite for any RT) --
- 129: -- Layer 3: RT-core pipeline --
- 133: -- Layer 4: inline ray queries --
- 137: -- RT pipeline properties (only meaningful when ray_tracing_pipeline) --
- 233: The instance targets Vulkan 1.1, so promoted Vulkan 1.2
- 234: dependencies must still be enabled by their extension names.
- 241: Ray-tracing pipeline and ray-query both require SPIR-V 1.4.
- 263: --- api version ---
- 268: --- advertised extensions ---
- 303: --- feature chain: query what the driver actually supports ---
- 304: We chain Vulkan12Features + the three RT feature structs (when their
- 305: extensions are advertised) and read back the support bools.
- 328: Layer 1: Vulkan 1.2 promoted features.
- 337: `shaderInt64` lives in the legacy `VkPhysicalDeviceFeatures`, not in the
- 338: Vulkan12 struct, so it needs its own query.
- 342: Layer 2: acceleration structure (real only when ext + feature agree).
- 350: Layer 3/4: RT pipeline / ray query (independent of each other).

## crates/prism-render/src/capabilities_tests.rs

- 5: extension_name is [c_char] (= i8 on Windows). Build the byte arrays
- 6: and cast so the copy is type-correct on both signed and unsigned
- 7: c_char platforms.
- 55: RT pipeline alone (without acceleration structure is not usable.
- 63: With acceleration structure it becomes usable.
- 104: Vulkan 1.1 also needs the promoted dependency extensions.
- 131: accel + deferred + ray_query plus promoted dependencies.

## crates/prism-render/src/context.rs

- 73: Held for drop ordering / FFI lifetime.
- 92: Streamline's interposer must be loaded before the Vulkan entry so every
- 93: Vulkan call is routed through it; without the `dlss` brick there is no
- 94: interposer to load and the system loader is used directly.
- 139: Probe ray-tracing capabilities *before* device creation so we can
- 140: conditionally enable extensions and chain the right feature structs.
- 164: Load RT extension function pointers if the extension was enabled.
- 310: Externally (host) created objects destroyed by host: UI framework device lives longer than renderer,
- 311: destroying here would leave UI side handles dangling.
- 316: Destroy the debug messenger *before* the instance. The old code
- 317: guarded the destroy on `debug_utils_instance()`, which itself
- 318: checks `self._debug_messenger.is_some()` -- but we `take()` the
- 319: messenger first, so the guard saw `None` and the messenger leaked
- 320: (VUID-vkDestroyInstance-instance-00629). Build the ext handle
- 321: unconditionally; it's a no-op when the extension isn't enabled.
- 332: ---------------------------------------------------------------------------
- 333: instance
- 334: ---------------------------------------------------------------------------
- 410: Query loader instance version; request Vulkan 1.2 if supported (required
- 411: for core 1.2 entry points like vkGetBufferDeviceAddress without KHR suffix).
- 429: Instance extensions: surface + platform. Debug utils only in debug builds
- 430: (it's a debugging aid; the validation layer warns if enabled in release).
- 440: validate  layers only in debug  builds — and only when actually installed.
- 441: Unconditionally requesting layers would fail on environments without them (like Termux/CI without
- 442: validation package) at create_instance; first enumerate available layers then enable.
- 482: ---------------------------------------------------------------------------
- 483: physical device
- 484: ---------------------------------------------------------------------------
- 498: Prefer a discrete GPU, fall back to anything with a graphics queue.
- 499: Bonus points for ray-tracing support: if two GPUs tie on device type,
- 500: the one with RT wins. RT is *not* required -- a non-RT GPU is still
- 501: selected and simply renders via the raster path.
- 515: Must have a graphics queue family or it's useless to us.
- 522: The path-trace pass uses a 176-byte push-constant block (PtPushConstants,
- 523: padded per std140: the trailing `ray_max_distance` float rounds the struct
- 524: up to a 16-byte multiple). Vulkan only guarantees 128 bytes, so verify
- 525: the device actually supports more before we write past the guaranteed
- 526: range. All desktop GPUs and modern mobile parts advertise 256; very old
- 527: / emulator devices may only do 128 and would silently truncate the PT
- 528: push constants.
- 552: ---------------------------------------------------------------------------
- 553: device
- 554: ---------------------------------------------------------------------------
- 570: Query the available legacy features (validation layer wants this) and
- 571: mirror the ones we need into the Features2 chain.
- 575: MRT pipelines (ForwardPass writes color + view-space normal use
- 576: different blend states per attachment (color = alpha blend, normal =
- 577: no blend). Vulkan requires `independentBlend` for that; without it
- 578: every attachment must share the same blend config. Universally
- 579: supported on desktop + modern Android
- 581: Wireframe (POLYGON_MODE_LINE) needs this feature; do not request if unsupported.
- 588: --- Build the extension list: swapchain (windowed only) + RT (conditional) ---
- 589: VK_KHR_swapchain requires VK_KHR_surface on the instance; a headless
- 590: context (baker) enables no surface instance extensions, so requesting
- 591: swapchain there trips validation. Only enable it when a surface exists.
- 596: `cmd_pipeline_barrier2` / `ImageMemoryBarrier2` (used unconditionally in
- 597: `buffer.rs` for texture uploads and mip generation) come from
- 598: VK_KHR_synchronization2. We target a Vulkan 1.2 instance, where the core
- 599: `vkCmdPipelineBarrier2` symbol is not exposed; only the `...KHR` variant is
- 600: available once this extension is enabled. `buffer.rs` therefore drives the
- 601: barrier through `ash::khr::synchronization2::Device`, which resolves the
- 602: KHR entry point. The extension must be enabled here or that fails to load.
- 606: `cmd_blit_image2` (used by mip generation in `buffer.rs`) is a Vulkan 1.3
- 607: core symbol not exposed on a 1.2 device; it is promoted from
- 608: VK_KHR_copy_commands2. Enable the extension so the KHR entry point loads,
- 609: and call it through `ash::khr::copy_commands2::Device` in `buffer.rs`.
- 633: Host extra required extensions (e.g. UI framework `VK_KHR_swapchain`). Deduplicate, avoid
- 634: `VUID-VkDeviceCreateInfo-ppEnabledExtensionNames-00374` (duplicate extension).
- 646: --- Build the VkPhysicalDeviceFeatures2 pNext chain ---
- 647: Each feature struct is declared out here so it outlives the create_info
- 648: borrow (same pattern as validation_features in create_instance).
- 654: `synchronization2` is a Vulkan 1.3 promoted feature. On this 1.2 device
- 655: it is only available via the VK_KHR_synchronization2 *feature* (not just
- 656: the extension): enabling the extension exposes the entry points, but the
- 657: feature bit must also be turned on or `vkCmdPipelineBarrier2KHR` is
- 658: illegal. `buffer.rs` issues these barriers unconditionally for texture
- 659: upload + mip generation.
- 662: Vulkan 1.1 feature: shaderDrawParameters is needed when a shader
- 663: references SV_VertexID (DrawParameters SPIR-V capability). The skybox
- 664: vertex shader uses vid%8 to select cube corners without a vertex buffer.
- 667: Layer 1: Vulkan 1.2 promoted features that RT depends on.
- 673: Bindless (see bindless.rs): a runtime-sized, partially-bound,
- 674: update-after-bind array of sampled images indexed by u32 handle.
- 675: These sub-features are all part of Vulkan 1.2 descriptor indexing;
- 676: enabling them here lets BindlessTextureTable allocate its set.
- 682: PathTracePass (and potentially other compute passes) updates
- 683: STORAGE_IMAGE and STORAGE_BUFFER descriptors every frame while
- 684: previous-frame command buffers are still in flight, which requires
- 685: the descriptor-binding-level update-after-bind feature.
- 695: RTXGI SHARC packs its hash-grid key as a `uint64_t` and its cached
- 696: radiance as `float16_t4`. The SPIR-V module declares the `Int64`,
- 697: `Float16` and `StorageBuffer16BitAccess` capabilities, so the device must
- 698: enable the matching features or `vkCreateShaderModule` rejects it.
- 709: Vulkan 1.1 promoted this from VkPhysicalDevice16BitStorageFeatures,
- 710: so it is set on the vk11 struct; chaining the standalone 16-bit
- 711: struct as well is a VUID violation.
- 724: Layer 2-4: RT features only when the caps say they're supported.
- 727: PathTracePass updates the TLAS descriptor (binding 2) every frame;
- 728: this sub-feature is required when the descriptor set uses
- 729: UPDATE_AFTER_BIND on ACCELERATION_STRUCTURE bindings.
- 763: ---------------------------------------------------------------------------
- 764: debug messenger
- 765: ---------------------------------------------------------------------------

## crates/prism-render/src/culling.rs

- 168: left:   w + x >= 0
- 169: right:  w - x >= 0
- 170: bottom: w + y >= 0
- 171: top:    w - y >= 0
- 172: near:   z >= 0 (Vulkan depth range [0, 1])
- 173: far:    w - z >= 0
- 196: Take box corner "farthest" along this plane's normal (p-vertex):
- 197: if it is on outside side, no visible part inside box.
- 278: Tiles with no finite sample keep clear depth (farthest), no false culling.
- 328: Corner on or behind camera plane: no reliable screen projection, conservatively keep.
- 421: ── hand-written small matrix utils (this crate no glam dependency) ─────────────────────────
- 462: Vulkan y-flip
- 505: At z = −5 half image width ≈ 2.07 (45° fov), this box half outside.
- 521: All-zero / zero-length-after-normalize matrix: no usable planes.
- 539: z from +1 to −1: has part behind camera and part outside near plane.
- 542: Same box pushed entirely behind camera (z 1..2) becomes invisible.
- 565: After camera translated to (0, 0, -10) looking −Z: (0,0,-15) inside frustum, (0,0,-5) behind.
- 579: Invalid one ignored; both invalid then no result.
- 591: Wall depth 0.99 (≈ z = −10), box at z = −50 (depth ≈ 0.999).
- 599: Depth at z = −5 ≈ 0.981 < 0.99: box nearer than wall, must not cull.
- 607: Right half is background (1.0): if box projection covers it there's no occluder there.
- 616: Box at z = −50, x = 6: projection lands in right half.
- 624: Outside screen (x = 100) and crossing near plane (z from −0.05 to +0.05) both conservatively kept.
- 631: Invalid box likewise not culled.
- 673: Shadow still decided by `cast_shadow`, culling doesn't touch it.
- 684: Non-finite depth treated as farthest, never cull.

## crates/prism-render/src/ddgi.rs

- 111: R16G16B16A16_SFLOAT
- 113: R16G16_SFLOAT
- 114: RGBA16F relocation/state

## crates/prism-render/src/descriptor.rs

- 36: `UNIFORM_BUFFER_DYNAMIC` so the forward pass can point the same
- 37: descriptor set at a different camera slot (#461).
- 237: 0=sun, 1..PT_LIGHT_MAX=analytic
- 238: effective sample count
- 239: sum of RIS weights (target_pdf / p_init)
- 240: π(selected_light) for ReSTIR pdf = W/(M*π(y))
- 282: 416
- 299: Update descriptor set.

## crates/prism-render/src/descriptor_tests.rs

- 5: P3b: point light + spot light share one SSBO, plus direction / params two float4s.

## crates/prism-render/src/dlss_pass.rs

- 739: ValidUntilEvaluate

## crates/prism-render/src/forward_pass.rs

- 34: The reflected push-constant block and the Rust mirror must stay in lockstep;
- 35: a mismatch would make `cmd_push_constants` write past the pipeline range.
- 71: Skinned draws need their own bone offset and transparent draws need
- 72: their back-to-front order; both stay single-draw batches.
- 128: Capacity exhausted: keep correctness by falling back to one draw
- 129: per entity instead of truncating the instance range.
- 333: HDR intermediate target (linear). PostPass tonemaps this to the
- 334: sRGB swapchain. The old `_swapchain_color_format` argument is
- 335: kept for API stability; PostPass owns the swapchain-format
- 336: pipeline + render pass.
- 338: R16G16B16A16_SFLOAT: signed float so view-space normals (which
- 339: can be negative in any axis) store without bias/packing. 4th
- 340: channel unused (shader writes 0).
- 343: RGB = base color, A = perceptual roughness. RGBA16F keeps the
- 344: same sampling / format support as the other G-buffer targets.
- 485: The render pass must exist before we build a framebuffer against it.
- 486: `ensure_render_pass` is idempotent (early-returns once set).
- 489: If the swapchain image count changed (recreate with a different
- 490: image count) or the extent changed, tear everything down and resize
- 491: the per-image vectors. This is the only place we destroy framebuffers
- 492: wholesale; per-frame we only (re)build the single entry for this
- 493: `image_index` - so an in-flight frame's framebuffer is never touched.
- 507: Build this image's framebuffer + color + depth + normal if not
- 508: already current.
- 515: Replace the HDR color image for this slot.
- 524: Replace the depth image for this slot (create new, destroy old).
- 532: Replace the view-space normal MRT image for this slot.
- 541: #883 Phase 2: per-pixel motion vectors for TAA.
- 550: Albedo + perceptual roughness MRT image (SV_Target3), consumed by
- 551: the ReSTIR GI pass.
- 560: Destroy the old framebuffer for this slot BEFORE creating the
- 561: new one (order doesn't matter for validation here since both
- 562: reference the same slot, but destroy-old-first is tidy).
- 572: render pass attachment order: color depth normal velocity albedo
- 611: Framebuffers first (they reference color + depth + normal views).
- 615: Then HDR color images.
- 620: Then depth images (destroys each depth view).
- 625: Then view-space normal MRT images.
- 630: Then motion-vector images (#883 Phase 2).
- 635: Then albedo + roughness MRT images.
- 654: Framebuffers + depth images (swapchain-derived).
- 657: Render pass.
- 661: Pipelines (three cull variants, frees pipeline + layout via GraphicsPipeline::Drop).
- 665: set 0: frame UBO + materials SSBO layout + pool (sets freed with pool).
- 674: Shadow descriptor set layout + pool (the set itself is freed with
- 675: the pool).
- 683: Light SSBO.
- 714: Skybox pass (its own pipeline + set-2 layout).
- 716: Gizmo (its own pipeline + vertex buffer; Drop frees them).
- 720: set 4: AO descriptor set layout + pool + sampler.
- 735: set 5 (GI probe volume) is borrowed from SceneScope — not destroyed here.
- 783: Skybox reuses the IBL env cubemap descriptor set + layout (set 0).
- 786: ---- set 0: per-frame UBO (binding 0) + materials SSBO (binding 1)
- 787: + light SSBO (binding 2) ----
- 788: One descriptor set per frame-in-flight; each binds its own UBO
- 789: buffer at binding 0 and the (shared) materials SSBO at binding 1
- 790: and the (shared) light SSBO at binding 2.
- 791: Built once here; never rewritten at runtime.
- 793: Binding 0 is DYNAMIC: the pass composites several cameras from
- 794: one set by passing `camera_index * stride` as a dynamic offset
- 795: (#461). A plain `UNIFORM_BUFFER` here made every
- 796: `vkCmdBindDescriptorSets` report
- 797: `dynamicOffsetCount-00359` (0 dynamic descriptors, 1 offset).
- 813: binding 3: bone palette (#560 P0-3), read by the vertex stage.
- 819: binding 4: previous-frame model matrices (#883 Phase 2), one
- 820: entry per draw-list item. Read by the vertex stage to build
- 821: object motion vectors.
- 836: Tear down any prior set0 layout/pool/sets (e.g. on re-init).
- 846: Pool needs: UNIFORM_BUFFER_DYNAMIC fif_count (the frame UBO is bound
- 847: with a per-camera dynamic offset), plus three STORAGE_BUFFER
- 848: bindings per set (materials, point lights, bone palette).
- 879: ---- light source  SSBO (bind  2) ----
- 880: create  host-visible, coherent buffer  for up to `LIGHT_MAX` point
- 881: lights, one per frame-in-flight to avoid tearing and data races.
- 882: The buffer  is zero-initialized here; `ForwardPass::update_lights`
- 883: rewrites the contents every frame from the ECS `PointLight` query  (see `render_system`).
- 898: Zero-initialize so the first frame (before any `update_lights` call)
- 899: doesn't read  garbage.
- 912: ---- Bone palette SSBO (binding 3, #560 P0-3) ----
- 913: One host-visible buffer per frame in flight. `BONE_CAPACITY` is the
- 914: number of mat4 entries a single frame can address; the engine packs
- 915: every skinned draw's palette into it each frame.
- 938: ---- Previous-frame model SSBO (binding 4, #883 Phase 2) ----
- 939: One mat4 per draw-list item, uploaded by `GraphRenderer::execute`
- 940: before the graph runs. Separate from bones so a frame with both
- 941: skinned and moving static draws cannot overwrite either palette.
- 965: Destroy old light buffer if any.
- 1003: Write each set: binding 0 = this frame's UBO, binding 1 = materials SSBO,
- 1004: bind  2 = this frame's light source  SSBO.
- 1006: SSBO: whole buffer is fine.
- 1021: Collect all per-frame UBO infos first so the `writes` slice
- 1022: references below don't conflict with mutating `ubo_infos`.
- 1058: must be consistent with DYNAMIC type in layout, otherwise write ignored.
- 1097: ---- set 3: shadow map (SAMPLED_IMAGE + comparison SAMPLER) ----
- 1179: ---- set 4: previous-frame GTAO R8 visibility (combined image sampler) ----
- 1180: The AO view is updated every frame by `set_ao` (GraphRenderer passes
- 1181: the GTAO pass's double-buffered view for the frame the scene reads).
- 1182: Here we only create the layout + pool + sampler + descriptor set; the
- 1183: image_info write happens in `set_ao` once a view is available.
- 1184: Binding 0 is the GTAO visibility texture; binding 1 is the
- 1185: previous-frame ReSTIR GI signal. Both are written by per-frame
- 1186: setters (`set_ao` / `set_gi_view`) once the producing pass has run.
- 1207: Tear down any prior set4 layout/pool/sampler (e.g. on re-init).
- 1236: One AO descriptor set per frame-in-flight so `set_ao` can update
- 1237: frame N's set without disturbing frame N-1's still-in-flight set
- 1238: (VUID-vkUpdateDescriptorSets-None-03047). The frame-in-flight count
- 1239: matches `frame_ubo_buffers.len()` (== `frame_sets.len()`).
- 1271: The actual image_info write happens in `set_ao` once the GTAO pass
- 1272: produces its first AO view. Until then the descriptors point at null;
- 1273: `PBR_FLAG_AO` is off by default so nothing samples it.
- 1274: 
- 1275: `giTex` is different: it is declared unconditionally in scene_frag.slang,
- 1276: so a never-written binding is a descriptor-validity error on every draw
- 1277: even when `gi_mode == 0` never samples it. Seed it with the 1x1 dummy.
- 1294: set 5 (GI probe volume) is borrowed from SceneScope — already wired
- 1295: via `gi_descriptor_set` / `gi_layout` parameters above.
- 1307: TEMP PROBE: confirm set_ao runs with valid inputs. Throttled to once
- 1308: per second so the log isn't flooded at frame rate; emitted at
- 1309: `trace!` so it stays quiet under the default `info` filter.
- 1327: No AO yet (first frame or GTAO disabled) - leave the descriptor
- 1328: unbound. The shader's `aoTex.SampleLevel` is only reached when
- 1329: PBR_FLAG_AO is set, which the app leaves off until the user
- 1330: toggles it (by which time `view` is non-null).
- 1562: Clear the tail so a shader that indexes past the live range
- 1563: reads identity-ish zeros instead of last frame's bones.
- 1630: SSBO not allocated yet (set_resources not called). Nothing to do;
- 1631: the first frame after set_resources will see zeros.
- 1649: Zero the whole buffer then copy in the active lights up to LIGHT_MAX.
- 1650: Cheaper than tracking which slots changed, and keeps unused slots well-defined.
- 1673: attachment 0: swapchain color (HDR lit color, post-tonemap).
- 1677: #972: `initial_layout` is UNDEFINED (the render  graph emits no
- 1678: barrier before this pass, see `setup`), and Vulkan forbids
- 1679: LOAD with an UNDEFINED initial layout — the load would read
- 1680: undefined texels. Every camera clears explicitly with
- 1681: `vkCmdClearAttachments` according to `Camera.clearFlags`, and
- 1682: the "preserve what came before" case (#461) happens **inside**
- 1683: this single render  pass instance, where DONT_CARE never applies.
- 1689: Leave the swapchain image in COLOR_ATTACHMENT_OPTIMAL so a
- 1690: subsequent external overlay pass can load it and transition it to
- 1691: PRESENT_SRC_KHR. When the external overlay is disabled the caller
- 1692: (GraphRenderer::render) records a fallback pipeline barrier to
- 1693: PRESENT_SRC_KHR after this pass ends.
- 1700: attachment 1: scene depth (D32_SFLOAT). STORE because the GTAO pass
- 1701: samples it after ForwardPass (it was DONT_CARE before GTAO existed).
- 1702: Final layout is DEPTH_STENCIL_ATTACHMENT_OPTIMAL; the GTAO pass
- 1703: transitions it to DEPTH_STENCIL_READ_ONLY_OPTIMAL before sampling.
- 1707: Same #972: UNDEFINED starting layout can only use CLEAR / DONT_CARE, depth is
- 1708: explicitly cleared by each camera's clear flags.
- 1720: attachment 2: view-space normal MRT (R16G16B16A16_SFLOAT). STORE so
- 1721: the GTAO pass can sample it. Final COLOR_ATTACHMENT_OPTIMAL; the GTAO
- 1722: pass transitions it to SHADER_READ_ONLY_OPTIMAL before sampling.
- 1726: Same #972.
- 1738: Attachment 3: screen-space motion vectors (#883 Phase 2), RGBA16F (.xy used). Cleared
- 1739: to zero; every mesh draw overwrites its footprint. Stored so TaaPass
- 1740: can sample it after the pass.
- 1755: Attachment 4: albedo + perceptual roughness (SV_Target3). Written every frame
- 1756: by the scene fragment shader; read by the ReSTIR GI pass.
- 1760: Same #972.
- 1811: Lazily build the world-space gizmo pipeline against this render pass
- 1812: (the gizmo draws inside the same render pass, on top of the scene).
- 1852: Vertex: reuse mesh_vert.vert.spv (`MeshPush { model }`, 64 bytes). The
- 1853: pipeline pushes the same `shader_bindings::scene_frag::PbrBindlessPush`
- 1854: block (120 bytes); the vertex stage only reads the leading `model`, and
- 1855: Vulkan allows the push range to exceed what a stage actually reads.
- 1856: Fragment: scene_frag.frag.spv (bindless PBR + shadow) or custom shader.
- 1865: Skinned variant: same module family, `vertexSkinnedMain` declares the
- 1866: extra joints/weights inputs from vertex buffer 1 (#560 P0-3). The
- 1867: reflection JSON only carries the first entry point, so the skinned
- 1868: entry name stays a literal here.
- 1900: set 0: frame UBO (binding 0) + materials SSBO (binding 1).
- 1904: set 1: bindless texture table (samplers + SRV array).
- 1906: set 2: IBL resources (3 combined image samplers: env, irradiance, prefiltered).
- 1907: Use the IBL's own descriptor set layout (set via `set_resources`) so the
- 1908: stage flags match exactly (`FRAGMENT | COMPUTE` — the path-tracing pass
- 1909: samples the same IBL cubemap in a compute shader). Creating a separate
- 1910: layout with mismatched stage flags would trigger VUID-vkCmdBindDescriptorSets-
- 1911: pDescriptorSets-00358.
- 1913: set 3: shadow map (SAMPLED_IMAGE + SAMPLER).
- 1917: set 4: previous-frame GTAO R8 visibility (combined image sampler).
- 1921: set 5: probe volume GI (SAMPLED_IMAGE + UBO), borrowed from SceneScope.
- 1933: Push constants: PbrBindlessPush (116 byte  std140, VERTEX|FRAGMENT).
- 1934: The reflected size is also what `cmd_push_constants` sends, and the
- 1935: module-level assertion guarantees the Rust mirror matches it.
- 1943: MRT: attachment 0 = color (blend by `DrawState`), attachment 1 = view normal (no blend).
- 2006: transparent objects usually `ZWrite Off` (`_ZWrite`).
- 2033: set2_layout (IBL) is borrowed from `IblResources` (set via
- 2034: `set_resources`) — do NOT destroy it here. `IblResources` owns the
- 2035: layout lifetime; it outlives all passes.
- 2036: set0/set1/set3 are owned elsewhere (frame_set_layout /
- 2037: BindlessTextureTable / shadow_ds_layout).
- 2085: Attachment 0 writes solid color, no blend; attachment 1 (view normal) not written.
- 2107: Front-face cull: only keep backfaces expanded outward by normal, forming silhouette.
- 2191: Negative bias pulls lines toward camera; wireframe only covers filled face edges, no depth write.
- 2215: Declare output handles (well-known, so downstream passes read our
- 2216: depth / normal / HDR views by handle). The graph does NOT allocate
- 2217: the underlying images in PR-1 (ForwardPass still owns its
- 2218: framebuffers); only the handle->view mapping is published in
- 2219: `execute`.
- 2280: Declare read edge for directional shadow map sampled in fragment shader (set 3).
- 2281: When `shadows` disabled there's no ShadowMapPass in graph, nobody writes this handle,
- 2282: so shouldn't declare read edge either (forward samples a 1×1 dummy outside graph).
- 2291: Declare write edges so the render graph's layout cache records the
- 2292: layout this pass leaves each attachment in (matching the render pass
- 2293: `final_layout`). Downstream `GtaoPass` / `PostPass` read edges then
- 2294: trigger the ATTACHMENT -> READ_ONLY / SHADER_READ_ONLY barriers
- 2295: automatically, with `src` stage/access taken from these write edges.
- 2296: No barrier is emitted for the writes themselves: ForwardPass's render
- 2297: pass performs the UNDEFINED -> ATTACHMENT transitions via
- 2298: `initial_layout` (see `create_render_pass`).
- 2332: Framebuffer + HDR/depth/normal lifecycle now owned here (driven by
- 2333: the graph), not by `GraphRenderer::render`. `ensure_target` is
- 2334: idempotent: (re)builds only the slot for `image_index` when missing
- 2335: or the swapchain changed. `set_ao` rebinds the previous-frame GTAO
- 2336: visibility view (1-frame latency); `update_lights` rewrites the
- 2337: point-light SSBO from the ECS-collected lights for this frame.
- 2338: #613: when internal render resolution on target smaller than swapchain.
- 2345: Collect all pipeline states needed this frame before recording commands (opaque default state always needed:
- 2346: descriptor set binds with its layout).
- 2347: When device lacks `fillModeNonSolid` silently fall back to fill, no impact on normal render.
- 2367: Pick the per-swapchain-image framebuffer. Indexed by `image_index`
- 2368: (NOT `frame_index`): with N swapchain images and 2 frames in flight,
- 2369: several command buffers reference different framebuffers
- 2370: concurrently, so each swapchain image has its own.
- 2378: Descriptor / push constant layouts fully identical across all state variants, so use default
- 2379: (opaque, back-face cull) variant's layout bind once; loop only rebinds pipelines as needed.
- 2380: Copy the Vulkan handles out: `pipeline` borrows `self.pipelines`,
- 2381: and the per-camera loop below may lazily create the outline pipeline
- 2382: (`&mut self`). Handles are `Copy`, the borrow is not.
- 2391: Resolve the per-frame descriptor set now (used after the skybox draw,
- 2392: when we re-bind the scene pipeline + descriptors).
- 2399: Clear values indexed by attachment number: 0 = HDR color, 1 = depth,
- 2400: 2 = view-space normal MRT. Even though attachment 2 is cleared, its
- 2401: clear value is irrelevant (the fragment shader overwrites every
- 2402: pixel); use opaque black. The count must be >= the highest cleared
- 2403: attachment index + 1 (VUID-VkRenderPassBeginInfo-clearValueCount-00902).
- 2404: Color attachment 0 uses the frame's clear_color (from the app-level
- 2405: `clear_color` parameter) so the "no camera" fallback shows gray etc.
- 2424: Attachment 3: motion vectors. Zero means "no motion".
- 2446: #461: composite every enabled camera, in `Camera.depth` order.
- 2447: The legacy path (`cameras` empty) synthesises one camera from the
- 2448: frame UBO so existing callers keep the exact old frame.
- 2465: Count draw calls actually submitted (`ShadowsOnly` skipped). Accumulated with multiple cameras.
- 2468: Clear according to `Camera.clearFlags`: `Skybox` / `SolidColor`
- 2469: clear colour (and the view-normal MRT), everything except
- 2470: `Nothing` also clears depth. Camera 0 always ends up with a
- 2471: defined image because the default flags clear both.
- 2520: Establish the camera viewport before any draw, including the skybox.
- 2521: The preceding shadow pass uses a different viewport and scissor.
- 2545: draw  the skybox first  (background) for cameras whose clear
- 2546: flags ask for it. It uses its own pipeline + IBL env set and writes
- 2547: no depth, so scene geometry always occludes it.
- 2571: Re-bind the scene pipeline + all descriptor sets. Required after the
- 2572: skybox draw (incompatible layout at set 0) and per camera because
- 2573: set 0 carries a camera-slot dynamic offset (#461).
- 2581: set  0: frame  UBO + materials SSBO + light source  SSBO
- 2590: set 1: bindless texture table
- 2599: set  2: IBL cubemap
- 2608: set 3: shadow map table
- 2617: set 4: previous-frame GTAO visibility (combined image sampler).
- 2631: Set 5: probe volume GI (PRT64 dynamic volume if active, else SceneScope).
- 2651: Pure 2D mode skips 3D opaque / transparent mesh draws, wireframe overlay and outline
- 2653: Loop starts with just-bound default (opaque) pipeline; only rebind when encountering other state
- 2654: (cull / depth write / blend all pipeline state). Descriptor and push constant layouts
- 2655: identical across variants, no need to rebind descriptor sets.
- 2656: #491: pack static instance matrices after the live bone
- 2657: palette in the same SSBO. Skinned draws keep their existing
- 2658: offsets; instanced draws start at `bone_count`.
- 2731: #491: multi-item batches read their model matrices from
- 2732: the bone SSBO; count 0 keeps the old push-model path.
- 2750: Draw-list index for the previous-model SSBO and the
- 2751: TAA reactive mask (transparent draws drop history).
- 2759: 0 = baked probe volume / IBL, 1 = ReSTIR GI, 2 = PRT64.
- 2817: Fill + wireframe overlay (#431): fill already drawn, draw again with outline shader
- 2818: `POLYGON_MODE_LINE`; negative depth bias + no depth write keep lines stable on surface.
- 2846: Dark lines readable on bright surface; not material-dependent, close to Unity's
- 2847: Shaded Wireframe.
- 2885: Selected outline (#487): draw selected mesh again, front-face cull + normal outward expansion.
- 2886: Placed after opaque meshes, before particles / sprites: parts occluded by other geometry will normally
- 2887: be discarded by depth test, unlike screen-space wireframe that floats on topmost surface.
- 2921: Line width scales with object size, very large objects won't be left with a thin seam.
- 2968: World-space particles: depth test enabled (occluded by foreground geometry), so drawn after opaque meshes
- 2969: and before 2D sprites; transparent sort done by engine before submission (far to near).
- 2985: 2D sprites, drawn after the mesh list so they overlay 3D geometry.
- 2986: The pass disables depth test/write; ordering comes from the engine's
- 2987: CPU-side sort (sorting_layer -> order_in_layer -> z).
- 3005: Draw ground reference mesh (depth test enabled, occluded by scene entities)
- 3012: Draw the world-space XYZ gizmo on top of the scene (its pipeline has
- 3013: depth test disabled so it is never occluded). Uses the same
- 3014: view-projection the scene was drawn with.
- 3022: #1007: at end of main pass, while depth still DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
- 3023: copy to host-visible buffer; after copy resume layout, later GTAO still samples with original layout.
- 3102: Publish our output views under the handles declared in `setup` so
- 3103: downstream passes (`GtaoPass`, `PostPass`) read them by handle
- 3104: instead of `GraphRenderer` reaching into ForwardPass internals.
- 3142: Shadow view  is declared as an input (SHADOW_MAP_H); IBL and
- 3143: previous-frame AO are bound via `set_resources` / `set_ao` and
- 3144: bypass `GraphResources`, so they aren't listed as graph edges.
- 3170: Safety net: if `destroy` wasn't called explicitly, tear down using
- 3171: the cached device handle. `destroy` is the preferred path (it runs
- 3172: after `device_wait_idle`); this only fires on leaks / early drops.
- 3174: `drop_target` drains framebuffers + depth images.
- 3185: GraphicsPipeline::Drop frees the pipeline + layout.
- 3187: set 0 (frame UBO + materials SSBO) layout + pool.
- 3200: Light SSBO.
- 3211: Skybox pass (its own pipeline + set-2 layout).
- 3213: Gizmo (its own pipeline + vertex buffer; Drop frees them).
- 3216: set 5 (GI probe volume) is borrowed from SceneScope — not destroyed here.
- 3241: View-space normal + reactive mask (alpha) are never blended.
- 3245: Motion vectors are per-fragment and overwrite the previous value.
- 3249: Albedo + roughness: opaque surface properties, never blended.

## crates/prism-render/src/frame_stats.rs

- 318: ceil(0.95 * 5) = 5 -> the largest sample.
- 326: 99 fast frames and one 100 ms hitch.
- 331: ceil(0.01 * 100) = 1 -> only the hitch.
- 333: 0.1% of 100 frames still rounds up to one frame.
- 342: 0.5 -> bucket of edge 1.0 (index 1); 16.0 -> edge 16.67 (index 12);
- 343: 16.67 is not < 16.67 -> edge 20 (index 13); 2000 -> overflow.

## crates/prism-render/src/gi.rs

- 46: Order-2 real SH basis constants (Ramamoorthi & Hanrahan 2001, orthonormal).
- 47: 0.5 * sqrt(1/pi)
- 48: 0.5 * sqrt(3/pi)
- 49: 0.5 * sqrt(15/pi)
- 50: 0.25 * sqrt(5/pi)
- 51: 0.25 * sqrt(15/pi)
- 72: -------------------------------------------------------------------
- 73: Bake-time directional light (shared default for baker + runtime)
- 74: -------------------------------------------------------------------
- 75: 
- 76: The offline baker lives in `prism-build-pipeline` (`prism-build-pipeline
- 77: bake-gi`), which links this crate for the probe math but cannot depend on
- 78: `prism-engine` (the dependency runs engine -> render, so importing
- 79: `engine::DirectionalLight` would form a cycle). To keep the baked
- 80: indirect sun in sync with the runtime sun without that cycle, the canonical
- 81: default light parameters are mirrored here. **These constants MUST stay in
- 82: lock-step with `prism_engine::render_system::DirectionalLight::default()`**
- 83: (euler_xyz, intensity, color). The runtime inserts that default into the ECS
- 84: (`app.rs` `create_default_scene`), so as long as the two match, a bake's
- 85: direct-sun bounce uses the same direction/color/intensity the player sees.
- 86: 
- 87: The direction is stored as XYZ Euler angles (degrees) and converted with
- 88: [`bake_euler_xyz_deg_to_dir`], a byte-identical copy of
- 89: `prism_engine::render_system::euler_xyz_deg_to_dir` (see that function's
- 90: docs for the right-handed Rx·Ry·Rz convention, +Y up, base vector +Z).
- 113: Roll (e[2]) does not affect a pure +Z base direction; intentionally unused.
- 176: Band 0 (l=0, c=0): A_0 = π
- 177: Band 1 (l=1, c=1,2,3): A_1 = 2π/3
- 178: Band 2 (l=2, c=4..8): A_2 = π/4
- 227: Clamp base so base+1 <= dims-1 (i.e. base <= dims-2); the .max(0) keeps
- 228: dims==1 axes at base 0.
- 276: Trilinear-blend the 9 SH coefficients across the 8 corner probes.

## crates/prism-render/src/gi_tests.rs

- 12: ---- Bake-time directional light source (must match engine default ----
- 16: The runtime inserts DirectionalLight::default() (euler=[45,-45,0],
- 17: intensity=3.0, color=white) into the ECS, and render_system derives
- 18: the light source  direction via euler_xyz_deg_to_dir. The baker must use the
- 19: SAME euler angles + conversion so the baked sun bounce matches the
- 20: real-time sun. validate  the conversion produces a unit vector  in the
- 21: documented upper-left direction (y>0). Exact components come from
- 22: the formula [cp*sy, sp, cp*cy] with p=45deg, y=-45deg.
- 26: Upward component (y) must be sin(45deg) = 1/√2 (light source is above
- 27: the horizon), and horizontal components from cos(45)*sin/cos.
- 42: ---- ABI: ProbeVolumeInfo mirrors the Slang std140 layout  ----
- 46: std140: three vec4 (origin, spacing, dims) = 48 byte  16-aligned.
- 57: ---- SH basis / evaluation ----
- 61: The DC basis value is direction-independent.
- 74: A field with only the DC coefficient set  is uniform the reconstructed
- 75: Irradiance is sh[0] * Y_0^0 for every normal.
- 91: Coefficient 3 is the x lobe (basis SH_C1 * x). It must flip sign.
- 92: between +X and -X and vanish on the Y/Z axes.
- 105: DC-only radiance SH: irradiance = sh[0] * Y_0 * A_0
- 106: = sh[0] * SH_C0 * PI
- 107: which is PI * eval_sh9 since A_0 = PI.
- 120: Band-1-only radiance SH: irradiance = sh[1] * (SH_C1 * y) * A_1
- 121: = sh[1] * (SH_C1 * y) * (2*PI/3)
- 122: = (2*PI/3) * eval_sh9.
- 124: Y lobe (y)
- 136: ---- World -> mesh map ----
- 141: Probe (0,0,0) sits at the origin.
- 146: One spacing step along +X -> coord x = 1.
- 151: Fractional position.
- 158: ---- trilinear weights ----
- 187: coord == dims-1 -> fully the (1,1,1) corner of the last  cell.
- 198: Midpoint of cell (0,0,0): all 8 corners weight 1/8.
- 208: Far beyond the mesh -> snaps to far-corner probe.
- 211: all weight concentrates at the far-corner probe
- 218: dims == 1 on every axis: a single probe, weight 0 = 1, no panic.
- 224: ---- Full pipeline sample_probe_irradiance ----
- 228: Every probe holds the same DC-only coefficient -> a uniform radiance
- 229: field. sample_probe_irradiance applies A_0 = PI for the cosine
- 230: convolution, giving irradiance = sh[0] * Y_0 * PI.
- 254: Trilinear interpolate  reproduces linear  functions exactly. Make the
- 255: DC coefficient vary linearly with the probe's x index  sh[0] = i.
- 256: Sampling at fractional coord x = 1.5 yields blended sh[0] = 1.5,
- 257: irradiance = 1.5 * Y_0^0 * A_0 = 1.5 * SH_C0 * PI.

## crates/prism-render/src/gizmo.rs

- 78: --- Static geometry (generated once on the CPU) ---
- 102: --- Shaders ---
- 107: Entry-point names from Slang reflection (slangc keeps vertexMain /
- 108: fragmentMain via -fvk-use-entrypoint-name; see shader_bindings).
- 123: --- pipeline layout: push constants = viewProj + model (2 mat4) ---
- 133: --- Fixed-function state ---
- 159: Depth OFF → always drawn on top of the scene.
- 166: The gizmo draws inside ForwardPass's render pass, which has 4 color
- 167: attachments (color + view-normal MRT + motion vectors + albedo/roughness).
- 168: Every pipeline bound in that render pass must declare a matching
- 169: attachmentCount, so we list 4 blend states: attachment 0 (color) writes
- 170: RGBA, while 1-3 keep an empty write mask so the cleared values survive.
- 173: Attachment 1 = view-space normal MRT: empty write mask, keep values ForwardPass cleared.
- 177: Attachment 2 = motion vectors (#883): likewise only write slot 0, otherwise
- 178: `pColorBlendState->attachmentCount` inconsistent with render pass color attachment count
- 179: (#973, VUID-...-renderPass-07609).
- 183: Attachment 3 = albedo + roughness (ReSTIR GI G-buffer): gizmo does not write.
- 282: ---------------------------------------------------------------------------
- 283: Geometry generation
- 284: ---------------------------------------------------------------------------
- 289: shaft length
- 290: shaft half-thickness
- 291: cone height
- 292: cone base radius
- 293: cone radial segments
- 295: (direction, perpendicular-1, perpendicular-2, color )
- 318: 1. Translate (translation arrows + 2D plane handle corner marks)
- 330: XY plane (normal +Z, blue)
- 332: XZ plane (normal +Y, green, ground translation)
- 334: YZ plane (normal +X, red)
- 338: 2. Rotate (rotation rings)
- 357: 3. Scale (scale axes + endpoint cubes + center uniform scale cube)
- 396: -Z
- 399: +Z
- 402: -X
- 405: +X
- 408: -Y
- 411: +Y
- 447: In-plane ribbon surface
- 457: Ribbon surface perpendicular to plane (visible from all angles)
- 523: -p1 face
- 526: +p1 face
- 529: -p2 face
- 532: +p2 face
- 535: -dir face (base)
- 538: +dir face (tip)
- 569: side
- 570: base cap
- 600: positive face
- 603: incorrect face
- 607: Outer edge L border (p1 -> p2 -> p3)

## crates/prism-render/src/gpu_timer.rs

- 401: Only read queries already written: unwritten queries (even if reset) have undefined
- 402: results without `WITH_AVAILABILITY`.
- 404: Don't use `WITH_AVAILABILITY`: caller guarantees this slot's fence already waited,
- 405: so results definitely written; with avail bit returned value becomes (avail, value) interleaved layout.
- 427: Skip incomplete / out-of-order pairs, don't guess results.
- 486: When no whole-frame span (only passes recorded) fall back, don't fabricate zero out-of-graph overhead.
- 495: Timestamp quantization may make sum of passes slightly larger than frame span, UI must not show negative numbers.

## crates/prism-render/src/graph_renderer/assets.rs

- 54: No IBL build: ignore environment texture, always use neutral cube.
- 120: The previous IBL (and its BRDF LUT view) may still be sampled by
- 121: frames that are in flight. Finish them before overwriting the
- 122: bindless slot or destroying the old images/samplers; otherwise the
- 123: GPU can read freed resources (device-lost / validation error).
- 124: Environment switches are rare, so an idle wait is acceptable.
- 128: `None` = no lighting scene (splash / UI): cheap neutral empty IBL (new_empty, no convolution, ≈0ms).
- 129: Shared set2 descriptor layout stays stable.
- 139: Overwrite BRDF LUT at same bindless slot (table does not grow).
- 145: Switch forward pass / skybox set2 (layout shared, no pipeline rebuild).
- 154: Destroy old IBL (not shared layout) after taking over new.
- 158: Sky / IBL is incident light source for radiance cache training; environment changed, old voxel radiance
- 159: no longer belongs to this scene (repair spec §8).
- 304: Released meshes/materials mean the geometry (or its shading) that the
- 305: world-space cache was trained on no longer exists.
- 500: Scene binding check: reject a volume baked for a different scene.
- 513: All-miss bake check.
- 638: Remember the failed path so a broken file is not re-read every
- 639: frame; editing the path (or reloading the project) retries.

## crates/prism-render/src/graph_renderer/mod.rs

- 176: Owned for RAII; IBL cubemap + descriptor set are consumed via the
- 177: Descriptor set handle stored in `forward_pass`. Explicitly destroyed
- 178: in `destroy()` so the device handle is valid during cleanup.
- 271: ── last field ──────────────────────────────────────────────────
- 302: Window mode: IBL driven by scene `EnvironmentLighting` at load via `set_environment`
- 303: driven; here ignores startup env_bytes (keeping engine "scene declarative lighting").
- 346: Build the GPU runtime: descriptor infrastructure, command pool,
- 347: per-frame command buffers, and frame UBOs.  These survive swapchain
- 348: recreation; the runtime is independent of scene-level resources.
- 351: IBL: open screen/no lighting scene starts with cheap "in-screen empty IBL" (no CPU pre-convolution, ≈0ms).
- 352: True positive IBL pre-convolution heavy built on demand when `set_environment` loads 3D scene with lighting.
- 353: Descriptor set layout created once, ensuring pipeline layout stable when scene switches heavy IBL rebuild.
- 405: Build graph with ShadowMapPass. Call setup() on the pass before
- 406: adding it so it registers its shadow-map resource, then allocate the
- 407: graph's Vulkan resources (the shadow map depth image) and fetch its
- 408: Graph texture view for ForwardPass to sample
- 409: 
- 410: `shadows` feature disabled: whole block not compiled; graph starts from empty open, ForwardPass's
- 411: set 3 re-binds one 1×1 far depth dummy (descriptor layout unchanged, still able to pass validation layer).
- 442: Create forward_pass and wire its resources: IBL set + shadow map table view +
- 443: comparison sampler, bindless texture table, material SSBO, and the
- 444: per-frame UBO buffers (one set0 descriptor set per frame-in-flight).
- 445: ForwardPass is executed directly by GraphRenderer (it targets the
- 446: swapchain, not a graph-managed resource).
- 457: Register the BRDF LUT in the bindless texture table.
- 467: Scene-level global illumination probe volume (SceneScope). Created before ForwardPass
- 468: wiring so its descriptor set + layout can be borrowed (set 5).
- 489: ReSTIR GI pass (raster path, independent of the path tracer). It
- 490: builds its own BLAS/TLAS from the draw list and traces secondary
- 491: rays with ray queries, so it only exists when the device can do
- 492: that; otherwise `gi_mode = Restir` falls back to the baked probe
- 493: volume and the warning below says why.
- 514: GTAO pass half-resolution screen-space ambient occlusion Runs after ForwardPass
- 515: every frame and produces a double-buffered R8 AO texture the scene
- 516: samples (1-frame latency) to attenuate IBL diffuse + specular.
- 517: 
- 518: `gtao` feature disabled: whole block not compiled; set 4 re-binds 1x1 white (no occlusion) dummy.
- 531: TAA resolve (#883). Always registered; `execute` is a no-op when
- 532: `RenderSettings::taa` is false.
- 539: PostPass parameters: one descriptor set per swapchain image. The sets
- 540: are picked by `image_index`, so allocating fewer than the swapchain
- 541: has would make two images share a set and rewrite a descriptor the
- 542: GPU may still be reading.
- 552: AutoExposurePass meters the HDR image PostPass is about to tonemap and
- 553: publishes the adapted luminance PostPass divides AUTO_EXPOSURE_KEY by.
- 554: Registered ahead of PostPass so the ordering falls out of the graph.
- 559: PathTracePass — real-time path tracing compute pass.
- 560: Registered when hardware acceleration structures are supported;
- 561: checks RenderSettings.render_mode internally to decide whether to
- 562: dispatch. Scene geometry is uploaded later via set_geometry.
- 563: Omitted entirely when `pathtrace` feature is disabled (building blocks, see DESIGN §12).
- 584: CRT debugger: uniformly render registered RenderTexture (engine-style).
- 587: Register all passes into the graph in execution order.
- 588: Shadow -> Scene -> GTAO -> RenderTextures -> PathTrace -> Resolve -> Post.
- 590: Startup path: sync per-swapchain-image slot bit count. `ForwardPass::execute`'s
- 591: `ensure_target` builds framebuffer by `image_count`, `recreate_swapchain`
- 592: only covers below one step extend heavy build --- first create must initialize inside here, otherwise `set_target` inside
- 593: `idx >= image_count` guard will silently skip, forever cannot build framebuffer.
- 609: ReSTIR GI reads the forward G-buffer this frame and publishes the
- 610: signal the forward pass samples on the next one.
- 616: SHaRC is a graph step, not a frame preamble: the trainer deposits
- 617: radiance, the resolve collapses it, and only then does the tracer
- 618: query it (repair spec §4 — no invisible training or resolve outside
- 619: the graph).
- 632: The temporal resolve runs after the path tracer so it can consume the
- 633: traced image and its guides in path-trace mode (#883 follow-up). In the
- 634: raster path it still resolves the forward pass output.
- 653: Per-pass GPU timing (#518): when device does not support timestamp is `None` (headless falls back to CPU wall clock).
- 664: No environment was requested yet. The first explicit
- 665: `set_environment` must rebuild and queue the SHARC sky
- 666: invalidation, even when it asks for the empty IBL again.
- 711: -------------------------------------------------------------------
- 712: Public API
- 713: -------------------------------------------------------------------
- 827: ── headless mode ──────────────────────────────────────────────
- 914: Runtime with 2 command buffers (minimal for headless ops).
- 917: Headless mode still builds the full render graph so the
- 918: graph's topology is validated even without a display.
- 979: Graph: Shadow -> Scene -> GTAO -> PathTrace -> Post.
- 1045: Same ReSTIR GI pass as the windowed path; the offscreen extent is
- 1046: resolved lazily from `ctx.render_extent` on the first dispatch.
- 1064: Headless contexts do not enable `VK_KHR_swapchain`, so the offscreen
- 1065: image must never end a render pass in `PRESENT_SRC_KHR`; it stays a
- 1066: color attachment until something explicitly transitions it.
- 1075: AutoExposurePass meters the HDR image PostPass is about to tonemap and
- 1076: publishes the adapted luminance PostPass divides AUTO_EXPOSURE_KEY by.
- 1077: Registered ahead of PostPass so the ordering falls out of the graph.
- 1082: `gtao` feature disabled: set 4 degrades to 1x1 white (no occlusion).
- 1099: CRT debugger (headless below similarly connects to graph, supports no-window offscreen RT debugging).
- 1144: Same order as the windowed path: train, resolve, then consume.
- 1155: Same ordering as the windowed path: resolve after the tracer.
- 1388: Do not use `return`: when closing `pathtrace` function body only has this line, `return None;`
- 1389: will make clippy's `needless_return` directly report wrong (-D warnings below compile failure).
- 1533: The UI overlay is otherwise built lazily inside the first `record_*`
- 1534: call, where the system font parse (up to ~200 ms for a CJK
- 1535: collection) stalls a frame the user is already looking at.
- 1582: Resume path same as startup path needs to sync ForwardPass per-image slot bit count
- 1583: (see comments inside `GraphRenderer::new`).
- 1610: From this point on, any early return must keep frame acquisition
- 1611: suspended until the swapchain and lazy pass targets are rebuilt.
- 1613: Wait for the GPU to finish all in-flight work BEFORE destroying any
- 1614: framebuffers. The previous frame's command buffer references both
- 1615: the ForwardPass framebuffers and the external overlay framebuffers; without
- 1616: this wait, vkDestroyFramebuffer fires while a command buffer is still
- 1617: executing (VUID-vkDestroyFramebuffer-framebuffer-00892).
- 1621: Place ForwardPass frame buffer + depth graph image BEFORE swap chain is
- 1622: recreated: the framebuffer wraps a swapchain image view, and
- 1623: `Swapchain::recreate` destroys the old views. Destroying the views
- 1624: while the framebuffer still references them triggers a validation
- 1625: error (VUID-vkDestroyImageView-imageView-01026) which cascades into a
- 1626: device-lost on the next queue submit.
- 1627: 
- 1628: This is the single entry point for swapchain recreation - the
- 1629: acquire/present out-of-date paths in `render` also route through
- 1630: here so the framebuffer is always torn down first.
- 1634: #883 Phase 2: the previous camera/model history refers to the old
- 1635: extent; clear it so the first frame after a resize does not emit
- 1636: bogus motion vectors.
- 1642: PostPass wraps swapchain views too (its framebuffers target the
- 1643: swapchain directly). Drop them on the same lifecycle.
- 1647: ECS UI overlay caches framebuffer per image view; must in `sw.recreate()`
- 1648: Destroy old image view **before** release, otherwise framebuffer will reference already destroyed
- 1649: view (VUID -vkDestroyImageView-imageView-01026), can cause validate error /
- 1650: device lost. Device already idle here.
- 1660: Notify outside overlay layer: per-swapchain framebuffers already heavy built with new views.
- 1666: Passes depending on swap chain size / image count must be in `sw.recreate()` **after** applying new value
- 1667: resize: before recreate, `sw.extent` and `sw.views.len()` still from old swap chain,
- 1668: `GtaoPass::recreate_target` takes old half resolution for early exit (old/2 == old extent),
- 1669: while `set_image_count` will make `ForwardPass::ensure_target` lack few new graph images correct for
- 1670: slot bits (`image_index >= image_count` directly returns).
- 1679: GTAO owns its own AO graph image (not from swap chain), but size is half of swap chain.
- 1721: CRT debugger RenderTextures follow swap chain size (demo RT full screen).
- 1736: All per-swapchain-image attachments (ForwardPass HDR/depth/normal,
- 1737: PostPass framebuffer) were just rebuilt, so the render graph's cached
- 1738: image layouts are stale. Clear them so the first frame after
- 1739: recreate re-transitions from UNDEFINED instead of trusting a layout
- 1740: that no longer matches the fresh images.
- 1767: #883 Phase 2: the previous camera/model history refers to the old
- 1768: extent; clear it so the first frame after a resize does not emit
- 1769: bogus motion vectors.
- 1778: UI overlay caches framebuffer per image view, must first destroy old view then release.
- 1857: -------------------------------------------------------------------
- 1858: Frame lifecycle — phase API
- 1859: -------------------------------------------------------------------
- 1873: Headless: no acquire, just grab cmd buf[0].
- 1897: Re-use the offscreen target extent as a stand-in.
- 1910: Per-frame depth copy already complete with headless fence, can safely readback.
- 1941: --- Acquire next image ---
- 1966: Per-frame (same frame slot bit) timestamp results definitely written (acquire already waited for fence).
- 1968: Depth readback buffer of same graph image also already written, can map.
- 1971: --- Reset & begin command buffer ---
- 2045: Consistent with `ForwardPass::pipeline_for` cache key: blend / no depth write is
- 2046: independent pipeline variant, replay can see why pipeline changed.
- 2161: #502: LUT file only heavy re-reads / uploads when path changes, avoiding per-frame I/O.
- 2164: #1157: Consume unified DirtyDispatchPlan and record dispatch
- 2187: Uniformly converge and batch execute staged GPU asset operations in prepare phase
- 2209: The ReSTIR GI pass traces the same world-space scene, but it runs in
- 2210: raster mode, where nothing else uploads geometry. Build its own copy
- 2211: of the acceleration structures from the same draw list.
- 2401: Only trust the slot's alpha semantics when the draw item
- 2402: actually names a material: the `0` fallback is just a shading
- 2403: default and may belong to an unrelated opaque material.
- 2476: No trainer node exists without the path tracer, so nothing trained.
- 2584: Fold this frame's dirty plan into the SHaRC world-cache invalidation.
- 2585: 
- 2586: `instances_dirty` is deliberately *not* consulted: the draw list is
- 2587: rebuilt every frame after frustum and shadow-caster culling, so its
- 2588: content (and therefore its signature) changes whenever the camera
- 2589: moves. The cache keys on world position and only needs reprojection
- 2590: for that. Genuine geometry changes reach us through
- 2591: `mesh_releases`/`material_releases` here and through
- 2592: [`Self::invalidate_pt_geometry`] on scene unload.
- 2607: #883 Phase 2: frame_data is copied so we can inject the previous
- 2608: unjittered view-projection without mutating the caller's value.
- 2611: Explicit camera slots are not TAA-jittered (TAA currently
- 2612: resolves the implicit single-camera path).
- 2633: Record into a `Result` rather than `?`-propagating: if any step
- 2634: fails we still must `end_command_buffer` below so the in-flight
- 2635: fence gets signaled in `present`. Otherwise the next frame's
- 2636: `wait_for_fences` would hang forever.
- 2639: --- update  frame  UBO (#461: one aligned slot per composited camera) ---
- 2667: --- Execute render graph (Shadow -> Scene -> GTAO -> Post) ---
- 2669: #1007: At frame end, copy main channel depth to readback buffer. Many camera layers share one
- 2670: depth graph composite; coarse depth can only correctly apply to one camera, first disable ensures no false culling.
- 2671: Place before borrowing `swapchain` / `forward`, avoiding borrow conflict with them.
- 2681: Per-frame ReSTIR GI output (1 frame delay). Pass not registered / not dispatched returns null,
- 2682: Before toward pass `gi_mode` push constant becomes 0, falls back to probe volume.
- 2697: Skinned draws (#560 P0-3): push this frame's palette into the
- 2698: per-frame bone SSBO before the graph runs.
- 2700: #883 Phase 2: match this frame's draws to last frame's
- 2701: models by entity id. First-seen entities fall back to their
- 2702: current model (camera-only motion).
- 2731: If scene has no directional light casting shadow, entire frame skips shadow pass.
- 2786: SHaRC has two independent consumers now: the path tracer (trains
- 2787: the cache, then queries it) and the ReSTIR GI pass (deposits and
- 2788: queries while tracing its secondary rays, no path tracer involved).
- 2789: Both need the same clear / buffer-handover bookkeeping, so the gate
- 2790: below covers either mode instead of hard-coding the path-traced one.
- 2791: 
- 2792: The training and resolve dispatches themselves are graph nodes
- 2793: (`SharcTrainPass` / `SharcResolveNode`); what stays here is the
- 2794: cache ownership: allocation, invalidation and the clear.
- 2801: The hash map is sized when it is allocated, so a capacity
- 2802: change has to rebuild the buffers. Voxel keys are meaningless
- 2803: across a resize, so the rebuilt cache starts cleared.
- 2811: Command buffers from the frames still in flight may
- 2812: reference the old buffers. Capacity changes are rare, so
- 2813: stalling the device once is the cheap way to be safe.
- 2826: The resolve node owns the descriptor set that binds
- 2827: the cache buffers, so rebind it whenever the
- 2828: buffers are (re)allocated.
- 2836: Hand the buffers to the trainer/consumer.
- 2868: Consume the recorded lifecycle reasons. A reason that could
- 2869: not be serviced because SHaRC is idle this frame stays
- 2870: pending until the cache is used again, which is exactly when
- 2871: the stale voxels would become visible.
- 2881: A voxel-scale change redefines the hash key for every world
- 2882: position, so the surviving entries describe the wrong voxels
- 2883: (repair spec §8: cache format changes clear).
- 2900: Pipeline barrier: TRANSFER_WRITE -> SHADER_READ | SHADER_WRITE
- 2921: Set `gi_mode = Sharc` but didn't compile `sharc`: degrades to pure path tracing, but must clearly
- 2922: Missing which feature (DESIGN §12.3 rule 3: closing must be diagnosable, not silent degradation).
- 2923: This branch is hit every frame, so report only once.
- 2937: Resolve is the only stage that normalises the accumulation
- 2938: counters (see `crate::sharc::training_enabled`). Training with
- 2939: resolve disabled would keep adding to counters that nothing
- 2940: resets, so the trainer node skips and the reason is logged once.
- 3108: Depth copy definitely recorded in command buffer after, before leaving matrix for next frame to configure correctly.
- 3126: --- ECS UI overlay (quads + fontdue glyph text) ---
- 3127: 
- 3128: Recorded between the render  graph  output and the external overlay
- 3129: (or the explicit PRESENT barrier). For swapchain path its render
- 3130: pass owns COLOR_ATTACHMENT_OPTIMAL → PRESENT_SRC_KHR; for the
- 3131: offscreen path it stays in COLOR_ATTACHMENT_OPTIMAL so the host can
- 3132: sample the image after `offscreen_present_for_sampling` (#597).
- 3133: 
- 3134: Graph final pass (RtPreview) = PRESENT_SRC_KHR, so swapchain
- 3135: image in graph after already is present layout; UI/overlay render pass expects
- 3136: COLOR_ATTACHMENT_OPTIMAL initial layout → each record before must explicitly convert back.
- 3151: UI background images (#591) sample the scene-shared bindless table.
- 3188: Offscreen target has no swap chain scale; UI fills entire graph image.
- 3213: --- Transition swapchain image to PRESENT_SRC_KHR ---
- 3214: 
- 3215: The external overlay (host UI) records after ECS UI: the image is then
- 3216: in PRESENT_SRC_KHR (the final layout of RtPreview or ECS UI at the end
- 3217: of the graph), while the overlay pass expects COLOR_ATTACHMENT_OPTIMAL
- 3218: as its initial layout, so transition back before recording. No overlay
- 3219: means no transition at all.
- 3266: --- Screenshot: swap chain graph image → host-visible buffer ---
- 3267: 
- 3268: Must place graph image back to PRESENT_SRC_KHR after, before `end_command_buffer`:
- 3269: Copy itself needs TRANSFER_SRC_OPTIMAL, after recording convert back to present layout.
- 3291: --- End command buffer (always attempted) ---
- 3314: Headless: just submit with a fence-out but skip present.
- 3372: `PRISMA_PASS_DUMP`: the previous `present` waited on the frame
- 3373: fence, so the readback buffers are safe to map and encode here.
- 3415: The shadow atlas was allocated at construction time; clamp the frame
- 3416: request so a runtime settings edit cannot make the shader sample a
- 3417: cascade that was never allocated.
- 3422: #883: subpixel jitter for TAA. The graph frame carries the applied
- 3423: NDC offset so `TaaPass` can recover the unjittered camera matrix.
- 3459: The convenience wrapper has no camera entity to inspect and
- 3460: always drove a perspective projection before, so keep that.
- 3471: propagate recording error after fence is safe
- 3542: Shut down Streamline while all tagged images, swapchain, and VkDevice are still valid.
- 3554: Destroy IBL resources (env/irradiance/prefiltered cubes, BRDF LUT).
- 3555: The shared set2 descriptor layout belongs to GraphRenderer and is
- 3556: destroyed here (grab the handle before `ibl.destroy()` clears it).
- 3563: Destroy scene managers.
- 3569: Destroy ForwardPass (framebuffers, depth images, render pass
- 3570: pipeline, shadow descriptor set). Without this, vkDestroyDevice
- 3571: reports leaked VkImage/VkDeviceMemory/VkImageView/VkRenderPass.
- 3576: Destroy scene-level GI probe volume (SceneScope). Must happen AFTER
- 3577: ForwardPass::destroy (ForwardPass borrows the descriptor set
- 3581: Destroy GTAO pass (AO images, render pass, pipeline, descriptor
- 3582: sets, sampler).
- 3589: Destroy PostPass (framebuffers, render pass, pipeline, descriptor
- 3590: set, sampler).
- 3595: Destroy AutoExposurePass (histogram/luminance buffers, descriptor
- 3596: pool, sampler). Must run before `self.graph.destroy`, which drops the
- 3597: pass objects themselves.
- 3605: Destroy PathTracePass (accumulation images, geometry buffers,
- 3606: BLAS/TLAS, descriptor set, layout, pool).
- 3622: Destroy ShadowMapPass (framebuffer, render pass, pipeline/layout).
- 3623: This MUST happen BEFORE scene_scope/graph destruction (which owns
- 3624: the Arc<VulkanContext>) because Rust field-drop order drops the
- 3625: context-holders (runtime/ibl/scene_scope) *before* the graph — if
- 3626: ShadowMapPass relied on its Drop alone, the device handle would be
- 3627: stale by the time it ran, causing leaked resources + access violation.
- 3636: Destroy outside overlay layer (render pass/framebuffers/renderer).
- 3639: Destroy shadow sampler.
- 3642: Destroy graph resources (shadow map images, etc.).
- 3645: Destroy command pool.
- 3648: Destroy swapchain.
- 3653: Destroy offscreen target (headless mode).
- 3665: Thread-safe UI state already moved out of render thread (main thread holds),
- 3666: render thread remaining fields only hold ash/Vulkan handles
- 3667: (vk::*, ash::Device → u64 wrappers Send+Sync), Arc<VulkanContext> (Send+Sync
- 3668: because all inner fields are vulkan handles), or container types whose
- 3669: elements are Send.  These fields are accessed exclusively from the render
- 3670: thread, so there is no concurrent &mut mutation.

## crates/prism-render/src/graph_renderer/temporal.rs

- 53: Halton(2,3), shifted by -0.5 so the sequence covers [-0.5, 0.5).

## crates/prism-render/src/graph_renderer/ui_host.rs

- 36: Each direction names the stages that performed the old writes and will
- 37: perform the new ones; an access mask outside its stage mask is invalid.
- 229: PostPass leaves the swapchain image in PRESENT; turn it
- 230: back into a color attachment for the UI load pass.
- 276: Update the frame slot's descriptor BEFORE any segment binds it
- 277: in `cmd`, because updating a bound descriptor set mid-command
- 278: buffer invalidates the recording.
- 350: The graph already leaves the target in COLOR_ATTACHMENT_OPTIMAL;
- 351: this barrier only makes its color writes visible to the UI pass.

## crates/prism-render/src/ground_grid.rs

- 152: The grid is already emitted as LINE_LIST geometry. Wireframe
- 153: polygon mode is redundant here and requires fillModeNonSolid.
- 164: Depth test enabled, no depth write; lets scene entities naturally occlude mesh.
- 175: Attachment 1 = view-space normal MRT: empty write mask, keep values ForwardPass cleared.
- 179: Attachment 2 = motion vectors (#883): likewise only write slot 0, otherwise
- 180: `pColorBlendState->attachmentCount` inconsistent with render pass color attachment count
- 181: (#973, VUID-...-renderPass-07609).
- 185: Attachment 3 = albedo + roughness (ReSTIR GI G-buffer): gizmo does not write.
- 289: 3D second axis is Z (blue), 2D is Y (green).
- 301: 1. Mesh lines parallel to X axis (Z for XZ / Y for XY varies)
- 305: X axis (red) split into positive and negative halves
- 336: 2. Mesh lines parallel to second axis (X varies)
- 340: Second axis split into positive and negative halves
- 383: 80 steps, -40 to +40: 81 X lines + 81 Z lines, center axes each split into 2 segments
- 384: (80 + 2) * 2 + (80 + 2) * 2 = 164 + 164 = 328
- 387: Validate axis line colors at origin
- 396: #602: 2D view camera looks −Z, mesh must lie on XY plane at Z = 0.

## crates/prism-render/src/gtao.rs

- 115: Half resolution, at least 1x1 to avoid zero-sized images.
- 121: ---- Double-buffered R8_UNORM AO images ----
- 132: ---- Sampler (linear, clamp-to-edge; AO is low-frequency) ----
- 149: ---- Descriptor set layout: one SAMPLED_IMAGE (binding 0) + one
- 150: SAMPLER (binding 1). The shader declares set 0 (depth) + set 1
- 151: (normal), both with this shape, so we reuse the layout 4x.
- 172: Set 2: previous resolved AO + velocity + sampler. This is separate
- 173: from the depth/normal layouts because it has two texture bindings
- 174: plus one sampler (#883 Phase 3).
- 200: 4 per-frame depth/normal sets + 2 temporal sets.
- 239: Bind the shared sampler to binding 1 of every set (the SAMPLED_IMAGE
- 240: at binding 0 is updated per-frame in `set_inputs`).
- 260: ---- Render pass (1 R8 color attachment, no depth) ----
- 263: ---- Framebuffers (one per AO image) ----
- 281: ---- Transition AO images to SHADER_READ_ONLY_OPTIMAL ----
- 282: The AO images are created with no defined initial layout. Before the
- 283: GTAO pass first runs (frame 1), the scene shader's AO descriptor may
- 284: already be written pointing at one of these views, expecting
- 285: SHADER_READ_ONLY_OPTIMAL. Transition them up-front so the descriptor
- 286: layout matches even on frame 0. The GTAO render pass uses
- 287: `initial_layout = UNDEFINED`, which tolerates any incoming layout
- 288: when it transitions back to COLOR_ATTACHMENT_OPTIMAL to write.
- 412: Destroy old framebuffers + AO images.
- 430: Create new AO images + framebuffers.
- 452: Transition the new AO images to SHADER_READ_ONLY_OPTIMAL (same
- 453: rationale as in `new`: the scene's AO descriptor expects this layout
- 454: before GTAO first writes the new images).
- 480: The depth/normal -> READ_ONLY barriers used to live here. They are now
- 481: inserted automatically by `RenderGraph::execute` from the `read_usage`
- 482: edges declared in `setup` (DEPTH_STENCIL_ATTACHMENT_OPTIMAL ->
- 483: DEPTH_STENCIL_READ_ONLY_OPTIMAL for depth, COLOR_ATTACHMENT_OPTIMAL ->
- 484: SHADER_READ_ONLY_OPTIMAL for normal). `inputs.depth_image` /
- 485: `inputs.normal_image` are now unused for barriers (kept for
- 486: descriptor wiring in `set_inputs`, which runs in the trait `execute`).
- 489: ---- Begin render pass (writes ao[i]) ----
- 490: Clear to white (1.0 = unoccluded) so any pixel the shader doesn't
- 491: write (there shouldn't be any - the fullscreen triangle covers the
- 492: whole AO target) reads as fully lit.
- 510: set 0: depth + sampler
- 519: set 1: normal + sampler
- 528: set  2: previous AO + velocity + temporal sampler.
- 559: Push constants: shader_bindings::gtao::GtaoPush (88 bytes, fragment only)
- 568: Fullscreen triangle (3 verts, no vertex buffer - SV_VertexID).
- 574: ---- Barrier ao[i] -> SHADER_READ_ONLY_OPTIMAL ----
- 575: GRAPH-EDGE exception  this is a cross-frame delayed edge. `ForwardPass`
- 576: reads this AO image *next frame* (1-frame latency, wired via
- 577: `GraphFrame.ao_view` -> `ForwardPass::set_ao`, NOT via a graph
- 578: `read_usage` edge), so the render graph cannot express or schedule
- 579: this barrier. It stays manual. SHADER_READ_ONLY_OPTIMAL stays valid
- 580: until the GTAO pass writes this slot again (2 frames later), whose
- 581: render pass `initial_layout = UNDEFINED` tolerates the incoming layout.
- 647: No vertex buffer (fullscreen triangle from SV_VertexID).
- 651: Sets 0/1 share the depth/normal layout; set 2 is temporal.
- 654: Push constants: shader_bindings::gtao::GtaoPush (88 bytes fragment only).
- 917: Inputs (depth/normal views) are published by ForwardPass under the
- 918: well-known FORWARD_DEPTH_H / FORWARD_NORMAL_H handles; GTAO reads them
- 919: from `resources` in `execute`. No graph-managed resources of its own.
- 920: 
- 921: Declare the read edges so the render graph inserts the
- 922: COLOR/DEPTH_ATTACHMENT_OPTIMAL -> *_READ_ONLY / SHADER_READ_ONLY
- 923: barriers automatically before this pass (replacing the hand-rolled
- 924: `cmd_pipeline_barrier` that used to live in `draw_ao`).
- 988: TEMP PROBE: confirm GTAO execute runs and inputs are valid. Throttled
- 989: to once per second so the log isn't flooded at frame rate; emitted at
- 990: `trace!` level so it stays quiet under the default `info` filter.
- 1004: The images themselves are needed only for the layout barriers; reuse
- 1005: The view's image handle via the ForwardPass-published view (vkImageView
- 1006: carries the image). We pass the same handle for image + view; the
- 1007: barrier only needs a valid image, and `vk::Image` from a view is not
- 1008: directly available here, so we use the ForwardPass-published image via
- 1009: the resource table's image (if present) else fall back to the view.
- 1017: Update the depth + normal descriptor sets for this frame-in-flight.
- 1018: This used to be called from `GraphRenderer::render` before PR-1; it
- 1019: MUST happen before `execute` (which binds the descriptor sets and
- 1020: draws), otherwise validation reports `depthTex` / `normalTex` as
- 1021: never updated via vkUpdateDescriptorSets.
- 1044: #883 Phase 3: 0.85 keeps most of the history while still
- 1045: converging within a few frames after disocclusion.
- 1067: Depth + normal come from ForwardPass via the well-known handles.
- 1069: AO is consumed by ForwardPass via `set_ao` (1-frame delay is intentional)
- 1070: graph edge - surfaced as a note by the viz instead.

## crates/prism-render/src/hdr_readback.rs

- 125: The staging buffer holds tightly packed `[f32; 4]` texels, so
- 126: view it as an array of texels instead of a flat f32 run.
- 137: Free the staging objects regardless of whether the map succeeded.

## crates/prism-render/src/ibl.rs

- 24: env cube
- 31: irradiance cube
- 38: prefiltered cube
- 45: brdf LUT
- 52: descriptor pool
- 144: Fast path: pre-baked `PIBL` payload from `.pak` — zero convolution.
- 163: Runtime `.hdr` or procedural sky: decode equirect on CPU, then run GPU
- 164: compute shaders (`ibl_bake.slang`) for cubemap & IBL convolutions.
- 193: 2. Create all images and fill staging buffers directly from `f16` slices.
- 194: 2a. Environment cubemap.
- 224: 2b. Irradiance cubemap.
- 257: 2c. Prefiltered cubemap.
- 313: 2d. BRDF LUT (RG16_SFLOAT, 1 layer).
- 346: 3. One-shot command buffer: upload all resources.
- 350: 3a. Env cube: transition all mips, copy mip0, generate mip chain.
- 387: Generate mip chain for env cube.
- 477: 3b. Upload irradiance cube.
- 521: 3c. Upload prefiltered cube (per-mip copies).
- 570: 3d. Upload BRDF LUT.
- 626: 4. Create image views + samplers.
- 628: 4a. Env cube view + sampler.
- 666: 4b. Irradiance cube view + sampler.
- 704: 4c. Prefiltered cube view + sampler.
- 742: 4d. BRDF LUT view + sampler.
- 780: 5. Descriptor set (set 2): 3 bindings envCube/irradianceCube/prefilteredCube.
- 781: Layout shared and passed by caller (ds_layout), ensuring forward pass / skybox pipeline
- 782: layouts stay stable when scene rebuilds IBL. brdfLUT registered separately to bindless table.
- 902: 1. Decode equirect HDR on CPU (or synthesize procedural sky) -> f16.
- 963: 2. Create output GPU images.
- 1062: 3. Temporary storage views & compute pipelines.
- 1178: Dummy 2D/array bindings for unused storage slot in shared set 0.
- 1271: Prepare PBR pipelines & descriptor sets when Full.
- 1300: For EnvOnly, upload 1x1 neutral dummies for the unused PBR slots.
- 1316: 4. Record command buffer: upload equirect -> dispatch compute shaders -> mipmap blit.
- 1320: 4a. Upload equirect texture.
- 1364: 4b. Transition envCube to GENERAL for compute write, dispatch equirectToCubeMain.
- 1402: Generate mip chain for envCube.
- 1508: 4c. Dispatch PBR compute shaders (Full) or upload 1x1 neutral dummies (EnvOnly).
- 1535: Irradiance
- 1556: Prefiltered mips
- 1588: BRDF LUT
- 1686: 5. Destroy all temporary equirect & compute objects.
- 1717: 6. Final image views, samplers & set 2 descriptor set.
- 1878: env / irradiance / prefiltered cube (1x1x6, 1 mip) — neutral gray (splash screen no geometry, not sampled)
- 1918: BRDF LUT (1x1, RG16) — neutral (1,1) fallback.
- 1956: One-shot upload of 4 images.
- 2053: views + samplers (max_lod 1, since only 1 mip)
- 2156: Descriptor set (set2) — reuse shared layout ds_layout.
- 2275: Guard: zero out handles so re-entrant Drop (if any) is safe.
- 2288: env cube
- 2294: irradiance
- 2301: prefiltered
- 2308: brdf LUT
- 2314: Descriptor pool (set layout shared and held by GraphRenderer, not destroyed here)
- 2318: Null out handles so re-entrant Drop is safe.

## crates/prism-render/src/lib.rs

- 23: Crate-level lint enforcement (applies only to this crate, not deps).
- 154: SceneDrawItem is the engine<->renderer exchange type for resolved draws.

## crates/prism-render/src/lut.rs

- 115: Strip comments, including the `#` that some exporters put inline.
- 147: Everything else must be a data row.
- 214: index 1 is r=1, g=0, b=0 (red changes fastest)
- 216: index 4 is r=0, g=0, b=1 (blue changes slowest)

## crates/prism-render/src/managers/material_manager.rs

- 103: Advanced PBR fields
- 240: Never treat an empty slot as opaque: a stale free slot must
- 241: not be able to make traversal skip an alpha test.
- 331: Use a tiny staging buffer to write the entire SSBO. We don't
- 332: bother with the dirty range optimization yet because the
- 333: upload size is so small (48KB) that the savings are noise.
- 347: Clear dirty bits; everything is now on the GPU.
- 357: Reset the GPU data so a future register at this slot
- 358: doesn't leak old values.
- 373: Never treat an empty slot as opaque: a stale free slot must
- 374: not be able to make traversal skip an alpha test.
- 400: no per-material GPU state to release

## crates/prism-render/src/managers/material_manager_tests.rs

- 102: emissive_strength
- 107: Advanced fields

## crates/prism-render/src/managers/mesh_manager.rs

- 22: Local handle. The engine layer translates asset mesh handles into
- 23: this when it calls `RenderMeshManager::register` so the render
- 24: crate stays decoupled from the asset pipeline.
- 273: Skin buffer goes through same shared uploader as vertex buffer (one submit/fence).
- 340: Imported meshes carry no tangent channel (the intermediate format has no
- 341: slot for one), so tangents are derived from the UVs here; meshes that do
- 342: carry real tangents (primitives, hand-authored) pass through unchanged.
- 506: U axis along +Y, V axis along −X: real tangent is +Y.
- 520: Meshes with own real tangent (not uniform) are not overwritten.

## crates/prism-render/src/managers/mesh_manager_tests.rs

- 15: Missing normal/uv/tangent fill with safe defaults.
- 50: 3 vertices, 1 skin entry: dropping the skin is the only safe option.
- 52: Empty skin is empty, not "all the same bone".

## crates/prism-render/src/managers/texture_manager.rs

- 51: Local handle. The engine layer translates asset texture handles
- 52: into this when it calls `RenderTextureManager::reserve` so the render
- 53: crate stays decoupled from the asset pipeline.
- 86: ── Block-compressed formats (DESIGN §7.2): offline encode, upload compressed blocks directly ──
- 114: Block-compressed formats approximate bytes per pixel (only for estimation, exact size uses compressed_byte_len)
- 115: 0.5B/px, rounded up
- 117: 4x4 block 16B = 1B/px
- 119: actually 0.5B/px, rounded up for lenient validation
- 121: 8 bpp
- 122: ~3.56 bpp
- 290: Missing-texture fallback: a neutral checkerboard, written into
- 291: bindless slot 0. It marks "nothing is bound here" without shouting:
- 292: the pattern is recognisable at any scale, and a 1×1 sample of it is
- 293: just mid grey, so a surface that samples one texel reads as
- 294: untextured rather than as an error.
- 312: Keep the fallback GPU objects alive for the manager's lifetime.
- 325: Store the fallback under a dedicated key so `destroy` frees it.
- 326: Its `srv` is fixed at slot 0 (register_with_handle advanced the
- 327: table's `next` past 0), so user textures start at slot 1.
- 367: Whether a block-compressed format is sampleable depends on the device
- 368: (many mobile GPUs lack BC, desktops lack ASTC). Checking up front
- 369: turns a VUID flood plus a failed upload into one explicit error that
- 370: the caller degrades to the fallback surface.
- 388: Compressed formats allow longer mip chain pixels, loose validation: at least base mip size
- 413: Upload pixels → VkImage + VkImageView (transferDst + SAMPLED).
- 417: Compressed format mips pre-generated by KTX2; 1x1 has no mip to generate.
- 436: Reserve the next bindless SRV slot (slot 0 is the fallback,
- 437: already taken, so this returns 1, 2, ...).
- 547: Rewrite the descriptor with the fallback view before the
- 548: image view is destroyed, so any shader holding the old
- 549: handle samples the fallback rather than a freed view.
- 634: Vulkan has no "block compressed" feature bit: whether a format can be sampled depends on its own
- 635: `optimalTilingFeatures` having `SAMPLED_IMAGE` (desktop generally supports BC, ASTC/ETC2
- 636: only on mobile GPUs).

## crates/prism-render/src/managers/texture_manager_tests.rs

- 15: We can't actually call `new` without a Vulkan device so we
- 16: test the validate  path directly by constructing the
- 17: Manager with `unsafe` minimal state. Easier: validate via
- 18: the bytes_per_pixel math at the call site.
- 24: wrong size
- 42: --- Pixel-buffer validation: baked mip chains must be accepted ---
- 46: 2x2 RGBA8 = 16 bytes.
- 48: A stray extra byte means the caller's layout is wrong: reject.
- 55: 8x8 RGBA8 chain (levels 4,1,0... all four) = 340 bytes > 256 base.
- 57: But never less than the base level, even when claiming a chain.
- 63: 8x8 BC7 base level = 2x2 blocks * 16 = 64 bytes.
- 75: --- Slot reclaim: released slots are reused before the bump pointer ---
- 81: We can't build a full Vulkan-backed manager here, so we exercise the
- 82: SlotAllocator→BindlessTextureTable wiring through a stand-in that
- 83: mirrors `unregister` semantics: claim two slots, release one, and
- 84: confirm the next claim reuses the released one (LIFO) and that
- 85: is_slot_used tracks release correctly.
- 94: Release slot 1 (like unregister releasing a texture's bindless slot).
- 99: Next claim reuses slot 1, not the bump pointer (slot 2).
- 104: Stale handles map back to INVALID/fallback semantics via get_srv,
- 105: which returns fallback_srv for unknown handles — unchanged behavior.
- 115: The slot must be handed out exactly once after release.
- 117: live=1, capacity=2: slot 1 is still free via the bump pointer.
- 119: Now live=2=capacity: the next claim must fail.
- 127: BC1 is 8 bytes / 4×4 block, BC3 is 16 bytes — upload computes buffer size by this (#519).
- 130: Round non-multiples of 4 up to whole block.
- 159: --- Missing-texture fallback pattern ---
- 169: Every texel is opaque and grey-valued: red, green and blue agree, so the
- 170: pattern cannot read as a colour-coded error.
- 180: Both tones are present, one of them black.
- 197: Four cells across the edge means the tile is centred and the pattern
- 198: wraps without a half-cell seam.

## crates/prism-render/src/math.rs

- 103: rows[r][c]
- 144: Vulkan-style y-flipped perspective (fov 65 deg, aspect 16:9, near
- 145: 0.05, far 1000) times a view looking down +X from eye (-10, 2, 0),
- 146: column-major [col][row] exactly as the engine stores it.
- 155: The inverse of this matrix reaches ~200 in the near/far terms, so
- 156: compare with a relative tolerance; f32 cannot hold 1e-3 absolute
- 157: there. Verified against the f64 Gauss-Jordan reference above.

## crates/prism-render/src/mesh.rs

- 171: Vertex buffer (device-local). On devices that support them, add
- 172: device-address + AS-build usages so the same buffer can feed BLAS
- 173: builds without reallocation.
- 195: Index buffer (optional).

## crates/prism-render/src/mesh_tests.rs

- 5: position(3) + normal(3) + color(3) + uv(2) + tangent(4) = 15 floats = 60 bytes

## crates/prism-render/src/mobile_upscaler.rs

- 634: Recover unjittered camera view-projection for accurate reprojection.
- 1056: Ensure prev history image is in SHADER_READ_ONLY_OPTIMAL if never initialized,
- 1057: and transition current output image to GENERAL for compute write.

## crates/prism-render/src/nrd.rs

- 236: SAFETY: Resolved NRD entry points point to immutable code pages.
- 238: SAFETY: Resolved NRD entry points point to immutable code pages.
- 306: SAFETY: `NrdRuntime` owns its NRD instance exclusively and is only mutated on the render thread.
- 308: SAFETY: `NrdRuntime` is synchronized by `GraphRenderer` on the render thread.
- 324: REBLUR_DIFFUSE_SPECULAR

## crates/prism-render/src/offscreen.rs

- 75: ---- device-local image ----
- 123: ---- host-visible staging buffer ----
- 155: ---- dedicated command pool ----
- 185: ---- one-shot command buffer ----
- 198: UNDEFINED → TRANSFER_DST_OPTIMAL
- 211: Clear
- 230: TRANSFER_DST_OPTIMAL → TRANSFER_SRC_OPTIMAL (for copy)
- 243: Copy image → buffer
- 272: Submit + fence
- 282: Cleanup per-submit objects
- 596: ---------------------------------------------------------------------------
- 597: helpers
- 598: ---------------------------------------------------------------------------

## crates/prism-render/src/particle_pass.rs

- 128: Camera basis vectors: columns 0 / 1 of `inv_view_rot` (w = 0).
- 157: 6 vertices = 2 triangles, one instance per particle.
- 197: ForwardPass' render pass has two color attachments (HDR color + view space normal).
- 198: Particles only blend to SV_Target0, normal target write mask empty.
- 215: Albedo + roughness (SV_Target3): particles write none.
- 234: Particle is semi-transparent world-space face patch: test depth (occluded by geometry), no depth write.
- 269: A previous frame's command buffer may still read the old buffer.
- 317: Callers wait for the device before `destroy`, so parked buffers can go.

## crates/prism-render/src/pass_dump.rs

- 382: Subnormal (or zero).
- 535: Smallest positive subnormal.
- 545: Two RGBA16F texels: (0, 0, 0, 1) and (1, 1, 1, 1).
- 568: cleared
- 569: at the near plane
- 593: RGBA16F with zero dx, dy
- 601: (0.0 * 4.0 + 0.5) * 255 = 127.5 -> 128
- 604: magnitude zero

## crates/prism-render/src/pipeline.rs

- 135: --- Pipeline layout ---
- 142: --- Vertex input ---
- 147: --- Input assembly ---
- 152: --- Viewport & scissor (dynamic) ---
- 153: State is set dynamically via cmd_set_viewport/cmd_set_scissor so the
- 154: pipeline does not need recreation when the window is resized.
- 159: Dummy viewport state (required by the API, but overridden by dynamic).
- 164: --- Rasterizer ---
- 165: `cull_mode` and `depth_bias_*` are optional overrides so shadow-map
- 166: pipelines can flip culling and apply slope/constant depth bias to
- 167: avoid self-shadow acne. Legacy callers pass `None` -> default BACK /
- 168: no bias.
- 175: View matrix is now a proper rotation (det +1); the projection's
- 176: y-flip (Vulkan NDC) is the single remaining reflection, so front
- 177: faces wind counter-clockwise in clip space.
- 183: --- Multisampling (none) ---
- 188: --- Depth/stencil ---
- 189: `depth_write_enable` is an optional override; shadow-map pipelines
- 190: may disable color writes but keep depth writes, so it defaults true.
- 196: --- Color blend ---
- 197: `color_blend_attachments` (when provided) drives the full MRT blend
- 198: state; otherwise `color_attachment_count` selects 0 (depth-only) or 1
- 199: (single attachment with the legacy alpha-blend config below).
- 227: --- Pipeline create ---
- 271: Default consistent with existing Prisma pipeline (back-face cull).
- 277: Pipeline cache keyed by `CullMode`, three values must be distinct.
- 287: Engine value order differs from Vulkan: Zero/One both 0/1, but engine
- 288: DstColor=2 corresponds to Vulkan DST_COLOR, SrcColor=3 to SRC_COLOR.

## crates/prism-render/src/placeholder.rs

- 55: Device handle explicitly passed by `GraphRenderer::destroy`; this has no handle available,
- 56: can only rely on explicit destruction (consistent with `IblResources` / `GtaoPass` convention).
- 80: `vkCmdClearDepthStencilImage` only accepts TRANSFER_DST_OPTIMAL / GENERAL layout,
- 81: so first transition to TRANSFER_DST, clear to 1.0 then transition to read-only sample layout.
- 97: Depth 1.0 = far plane: fragment depth `LESS` 1.0 always true → fully lit.
- 160: Visibility 1.0 = completely unoccluded (shader uses `1 - ao` to attenuate ambient light).

## crates/prism-render/src/post.rs

- 44: Keep the reflected push-constant size and the generated Rust layout in sync.
- 56: GPU layout comes from the Slang reflection; never mirror it by hand.
- 187: binding 0 = HDR colour, binding 1 = scene depth for depth of field
- 188: (#502 Phase 2), binding 2 = 3D color-grading LUT (#502). All three
- 189: bindings are always present so the pipeline layout never changes
- 190: between frames, even when no LUT is configured.
- 212: Adapted luminance from `AutoExposurePass` (set 0, binding 4).
- 213: Bound once by `set_adapted_luminance`; until then it points at a
- 214: 1-element zeroed buffer so the descriptor is always valid and the
- 215: shader's `auto_exposure()` returns its no-op 1.0.
- 238: One colour + one depth + one LUT per frame-in-flight.
- 266: Every project gets an identity LUT so the descriptor is valid even
- 267: before `[render] lut` is set. 32³ matches the common `.cube` size
- 268: and makes `lut_strength = 1` with no file a no-op.
- 284: Motion-blur UBO: one host-visible buffer per frame in flight. The
- 285: descriptor never changes; `set_motion_blur` only rewrites the bytes.
- 328: Zeroed stand-in for the adapted-luminance buffer. `AutoExposurePass`
- 329: replaces it via `set_adapted_luminance` on the first frame it runs,
- 330: but the descriptor must be valid from the start: a null buffer with
- 331: PARTIALLY_BOUND unset would trip validation, and a zero reading makes
- 332: the shader's `auto_exposure()` return its no-op gain of 1.
- 519: No frame may still reference the old image before it is freed.
- 692: The HDR input COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL
- 693: barrier used to live here. It is now inserted automatically by
- 694: `RenderGraph::execute` from the `read_usage` edge declared in
- 695: `setup`. `hdr_image` is therefore no longer needed in this function
- 696: (it was only referenced by the deleted barrier).
- 699: The swapchain image transitions UNDEFINED -> COLOR_ATTACHMENT_OPTIMAL
- 700: via the render  pass `initial_layout` (the external overlay or the caller's
- 701: PRESENT_SRC_KHR barrier handles the final transition out).
- 727: #613: internal resolution placed into `dest` by integer multiple / stretch, remaining region filled black by clear color.
- 748: Fullscreen triangle (3 verts, no vertex buffer - SV_VertexID).
- 890: high dynamic range  input  is published by ForwardPass under FORWARD_COLOR_H; read  in
- 891: `execute`. PostPass owns no graph-managed resources of its own.
- 892: 
- 893: Declare the read edge so the render graph inserts the
- 894: COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL barrier
- 895: automatically before this pass (replacing the hand-rolled
- 896: `cmd_pipeline_barrier` that used to live in `execute`).
- 903: TAA output (#883) when enabled; declared unconditionally so the
- 904: graph knows PostPass may depend on it.
- 911: PT output color (read in path-trace mode). Declare unconditionally
- 912: so the graph reserves the barrier slot even when in raster mode.
- 919: Debug RT viewer (Tab) can also sample the scene depth (mode 1) and
- 920: view-space normal (mode 2). Declare these read edges unconditionally
- 921: so the automatic barrier pipeline keeps them in a sampled layout
- 922: even when mode 0 doesn't read them (GTAO already transitions depth
- 923: and normal to read-only layouts, so this is usually a cache hit and
- 924: emits no extra barrier).
- 982: Pick the input RT based on the render mode and debug viewer (Tab).
- 983: In path-trace mode we read PT_COLOR_H instead of FORWARD_COLOR_H,
- 984: unless no usable Camera exists — in that case the PT pass was skipped
- 985: so fall back to FORWARD_COLOR_H (the cleared color from ForwardPass).
- 986: If PT_COLOR_H was not published (e.g. no RT hardware, pass skipped,
- 987: or geometry/materials not yet uploaded), also fall back to FORWARD_COLOR_H
- 988: so PostPass can tonemap and transition the swapchain image to PRESENT_SRC_KHR.
- 989: Debug modes 1 (depth) and 2 (normal) always read from scene output.
- 993: ANF super-resolution output highest priority when enabled (#1161)
- 1017: TAA output (#883) takes precedence when enabled and ANF / DLSS / mobile upscaler is off. In
- 1018: path-trace mode the temporal resolve consumes the traced image and its
- 1019: guides, so the resolved image replaces PT_COLOR_H here.
- 1068: (Re)build this swapchain image's framebuffer if missing or the
- 1069: Swapchain changed - mirrors `ForwardPass::ensure_target`. Before PR-1
- 1070: this was `GraphRenderer`'s job (it called `set_target` every frame);
- 1071: now the graph drives it so the framebuffer lifecycle is owned here.
- 1080: Bind the selected input view into this frame's descriptor set. The
- 1081: cache (`bound_hdrs`) keys on the view handle, so switching modes
- 1082: (which changes the view) triggers a rewrite automatically.
- 1083: #613: internal render resolution → target rect and sampling mode.
- 1084: If already upscaled to window full resolution by ANF or MobileUpscalerPass, PostPass directly full-screen linear samples.
- 1085: Nearest / Linear empty-space upscale mode samples directly in PostPass with matching hardware sampler stretched to full screen.
- 1117: Depth of field (#502 Phase 2) needs the scene depth in a read-only
- 1118: layout. `setup` already declares the edge; reuse the published view.
- 1124: Camera motion blur (#502 Phase 2). Keep the previous VP up to date
- 1125: even when the effect is off, so enabling it does not smear a jump
- 1126: from an arbitrarily old camera. Multi-camera composites and path
- 1127: tracing have no single valid previous view; they disable the effect.
- 1156: The generated PostPush carries tonemapMode (Reinhard / ACES /
- 1157: passthrough), aaMode (0 = off, 1 = FXAA) and invTexelSize (one texel in
- 1158: UV units, used by the FXAA taps). debug_rt, proj22/proj32, near/far
- 1159: etc. are consumed on the Rust side input  bind  selection,
- 1160: depth linearization) and are NOT part of the GPU push constant.
- 1161: proj22/proj32/near/far depth linearization values are still computed
- 1162: here as a reference for future shader extensions.
- 1175: Debug RT views show depth / normals, not the lit scene; blurring
- 1176: them would make the visualization useless. A missing depth view also
- 1177: disables the effect instead of sampling an unbound texture.
- 1200: Debug RT views (depth / normals) must stay readable; a grade LUT
- 1201: would tint the visualization, so it is disabled there like DOF.
- 1210: #613: FXAA samples **internal render target**, one texel UV size computed from its
- 1211: resolution; using swapchain size would make sample step too small when internal resolution downscaled.
- 1248: high dynamic range  color  comes from ForwardPass via FORWARD_COLOR_H.
- 1250: PostPass writes FINAL_COLOR_H (swapchain or offscreen image).
- 1321: Re-export the memory-type finder so this module is self-contained for
- 1322: future HDR-image helpers (currently none - the HDR image is owned by
- 1323: ForwardPass). Kept here as a placeholder import  to avoid an unused warn
- 1324: if no callers use it.
- 1640: PT requested with camera, but PT_COLOR_H was not published -> fall back to FORWARD_COLOR_H
- 1648: PT requested without camera -> forward color
- 1658: Depth / view-space normal debug views stay unfiltered on purpose.
- 1669: #502 color grading rides in the same push-constant block.
- 1677: #502 white balance rides after the Phase 2 effects.
- 1680: #502 depth of field: near/far planes plus focus / aperture / radius.
- 1686: #502 3D LUT: strength plus the edge length used for texel centres.
- 1689: #502 camera motion blur: strength then pixel cap.

## crates/prism-render/src/primitives.rs

- 68: (normal, tangent, in-face u axis, in-face v axis)
- 152: Skip degenerate triangles (poles).
- 199: Top / bottom caps (center point + fan), normals ±Y.
- 246: Top hemisphere (phi: 0 → π/2, from +Y pole down to equator).
- 269: Bottom hemisphere (phi: π/2 → π).
- 293: Top + bottom hemispheres each connect to themselves (equatorial ring vertices coincide, normals differ,
- 294: same as sphere, just split into two fan triangle segments).
- 357: Capsule = cylinder 1 + two radius 0.5 hemispheres → total height 2 (consistent with engine default).
- 371: Aliases: cube → box, plane → quad.

## crates/prism-render/src/probe_loader.rs

- 105: Magic check.
- 152: Scene name (64 null-padded bytes) + global hit ratio.
- 158: 3 floats per coeff, 4 bytes each
- 173: Parse coefficient body.
- 218: Header.
- 231: Scene name (64 null-padded bytes) + global hit ratio.
- 235: Body: RGB triplets per coefficient.
- 245: -------------------------------------------------------------------
- 246: Little-endian read/write helpers
- 247: -------------------------------------------------------------------
- 275: Keep at most SCENE_NAME_LEN - 1 bytes so the field is always NUL-terminated.

## crates/prism-render/src/probe_loader_tests.rs

- 1: -------------------------------------------------------------------
- 2: Tests
- 3: -------------------------------------------------------------------
- 80: wrong length
- 97: v1 files (48-byte header, no scene_name / hit_ratio) are no longer
- 98: supported. Build one by hand from a v2 buffer, rewrite the version
- 99: to 1 and place the v2 tail of the header, keeping the body intact.
- 105: magic..coeff_format
- 106: patch version to 1
- 119: A name longer than the 63-byte limit is truncated but still loads.

## crates/prism-render/src/prt_pass.rs

- 251: SceneScope-like PRT volume buffers (rebuilt on volume change).
- 393: ForwardPass-compatible set 5 layout (SAMPLED_IMAGE + UNIFORM_BUFFER).
- 514: Pack 64 coeffs * 3 f16 = 192 u16 -> 96 u32 words per cluster.
- 588: Update ProbeVolumeInfo UBOs for both ping-pong targets.
- 605: Bind descriptors for both compute sets (targeting targets[0] and targets[1]).
- 943: Stream newly resident / evicted cluster slices within the frame budget
- 944: (zero mid-frame vkFreeMemory; pre-allocated cluster buffer).
- 974: Check overflow flag from previous frame (explicit error on >64 lights).
- 982: Update sky/sun SH64 buffer from the directional light + ambient sky.
- 991: Baseline sky fill from ambient term in light_color.w.
- 999: Update dynamic lights buffer (point/spot lights).
- 1053: Transition gi_volume target to GENERAL on first use.
- 1087: Pass 1: ClusterEval
- 1113: Pass 2: Propagate dynamic lights (Q8.12 InterlockedAdd)
- 1131: Pass 3: ResolveVolume into 64^3 order-2 SH texture
- 1140: Make gi_volume writes visible to next frame's ForwardPass fragment reads.

## crates/prism-render/src/prt_runtime.rs

- 120: Count set bits below `slot` to find the dense child offset.
- 156: sqrt(3)/2
- 162: Proxy sphere-vs-cluster-volume culling.
- 352: leak_bias clamped to 0.25, so result > 0.75

## crates/prism-render/src/prt_volume.rs

- 457: Infinity keeps its exponent; NaN keeps enough mantissa to stay quiet.
- 467: Subnormal half: the value is M * 2^-24 with 24-bit M, so shift the
- 468: implicit-bit mantissa right by -(unbiased + 1) to land on M.
- 471: Below half the smallest subnormal: rounds to zero.
- 481: 0x0400 is the smallest normal half, which is the correct encoding when
- 482: the subnormal mantissa rounds all the way up.
- 512: Normalize the subnormal into an f32 normal.
- 513: 127 - 14

## crates/prism-render/src/prt_volume_tests.rs

- 70: f16 has 11 significant bits, so exact equality only holds for values that fit.
- 107: 8600 clusters * 384 B + node table stays under 3.5 MB, which is the M1 contract.
- 111: One more leaf breaks it: the constant is tight, not arbitrary.
- 218: Represents values the design uses: radiance coefficients in [0, 16).
- 229: Smallest positive subnormal half.
- 231: Beyond the half range saturates to infinity instead of wrapping.
- 234: Sign is preserved through the whole range.
- 240: Exhaustive: decoding then re-encoding must reproduce the identical bits for
- 241: every representable half, including subnormals, infinities and NaNs.
- 247: NaN payloads are normalized to a single quiet NaN by f32_to_f16.
- 260: 1 + 2^-11 is exactly halfway between two consecutive halves representable
- 261: near 1.0: the even neighbour (1.0) must win.
- 264: The next halfway point rounds up to the odd-then-even neighbour 1.001953125.

## crates/prism-render/src/pt_pass.rs

- 46: GPU layout comes from the Slang reflection; never mirror it by hand.
- 74: Deposits are meaningless without queries: the shader reads the buffers
- 75: from both paths, so a clear query bit must also clear the update bit.
- 126: Pipeline
- 134: Bindless texture table (set 1) - shared with the rasterizer. Not owned;
- 135: stored so the pipeline layout can reference its layout and the command
- 136: recorder can bind its set. Wired via `set_material_resources`.
- 140: IBL environment cubemap (set 2) - sampled on ray miss for HDR sky.
- 141: Wired via `set_ibl_resources`. Not owned.
- 145: Accumulation buffers (persistent across frames)
- 153: PT_COLOR_H output image (sampled+storage, published to graph for PostPass)
- 158: Guide images written by the trace shader (set 0 bindings 15/16) and
- 159: published as PT_DEPTH_H / PT_VELOCITY_H. They carry the primary-hit depth
- 160: and screen-space motion the temporal resolve needs, which the raster
- 161: G-buffer cannot supply in path-trace mode.
- 168: View-space normal.xyz + perceptual roughness.w of the primary hit,
- 169: published as PT_NORMAL_ROUGHNESS_H. DLSS-RR's packed normal/roughness
- 170: mode and NRD both consume exactly this layout.
- 175: Guide camera state (binding 17) — HOST_VISIBLE constant buffer rewritten
- 176: every dispatch with the current and previous frame view-projections.
- 180: Previous frame's unjittered view-projection, remembered across frames so
- 181: the motion vectors reproject against the camera the history was rendered
- 182: with. Reset on resize, like the other accumulation state.
- 185: Per-instance ray-traceable scene (combined vertex/index buffers,
- 186: per-instance BLAS, TLAS, instance_meta SSBO). Built once per scene by
- 187: `set_geometry` via `bake_common::build_pt_scene`. Owns its GPU resources.
- 190: Materials SSBO (shared `RenderMaterialManager::buffer()`), bound at set 0
- 191: binding 7. Wired via `set_material_resources`. The real-time pass does
- 192: NOT own this buffer (the material manager does) - it only holds the
- 193: handle for descriptor writes.
- 196: Lights SSBO (binding 8) — HOST_VISIBLE | HOST_COHERENT storage buffer
- 197: for PtAnalyticLight[PT_LIGHT_MAX]. Created once, written each frame.
- 202: Emissive triangle SSBO (binding 9) — device-local storage buffer built
- 203: from `set_emissive`. Read-only during PT dispatch; owned directly by
- 204: the pass (not through PtScene).
- 209: ReSTIR DI reservoir ping-pong buffers (bindings 10/11).
- 210: Two storage buffers, each sized for (width × height) ReSTIRReservoir
- 211: entries. Each frame one serves as prev (read, b10) and the other as
- 212: curr (write, b11); they swap roles every frame.
- 215: current buffer size in bytes
- 216: index of curr buffer (0 or 1) for this frame
- 218: SHARC cache buffers (bindings 12/13/14). The buffers are owned by
- 219: `GraphRenderer` (created lazily when `gi_mode` becomes `Sharc`); this
- 220: pass only holds the handles so `update_ds` can bind them. All `None`
- 221: when the cache is inactive, in which case `sharc_enabled` stays 0 and
- 222: the shader never touches the (unbound) bindings.
- 228: State tracking
- 234: Global accumulation-reset flag. Set by either (a) camera motion
- 235: (`should_reset`) or (b) an external `request_reset()` call when the app
- 236: knows a render parameter changed (max bounces, exposure, light
- 237: color/direction, scene reload, ...). Without this, stale samples
- 238: accumulated under the old parameters keep dominating the running
- 239: average and parameter tweaks look like they do nothing. This keeps the
- 240: "what changed?" decision in the caller rather than diffing every PT
- 241: input per frame.
- 244: Device handles
- 252: Set 0 bindings (must match pt_render.slang / path_integrator.slang):
- 253: b0: RWTexture2D<float4> accumImage
- 254: b1: RWTexture2D<uint>   sampleCount
- 255: b2: AccelerationStructure tlas
- 256: b3: ByteAddressBuffer vertexData  (combined world-space vertices)
- 257: b4: StructuredBuffer<uint> indices (combined index buffer)
- 258: b5: RWTexture2D<float4> outputImage
- 259: b6: StructuredBuffer<PtInstanceMeta> instance_meta (per-instance
- 260: material slot + vertex/index base offsets, indexed by
- 261: q.CommittedInstanceID())
- 262: b7: StructuredBuffer<GpuMaterial> materials (shared SSBO)
- 263: Set 1 (bindless, bound separately): globalSamplers[] + bindlessSrvs[]
- 319: prevReservoir (read)
- 324: currReservoir (write)
- 329: SHARC hash entries
- 334: SHARC accumulation
- 339: SHARC resolved
- 344: guideDepth (PT_DEPTH_H)
- 349: guideVelocity (PT_VELOCITY_H)
- 354: guides (PtGuides camera state)
- 359: guideNormalRoughness (PT_NORMAL_ROUGHNESS_H)
- 362: All bindings get UPDATE_AFTER_BIND + PARTIALLY_BOUND because
- 363: update_ds() is called every frame while previous-frame command
- 364: buffers may still be in flight. Without these flags Vulkan
- 365: validation complains about updating in-use descriptor sets.
- 368: b0  accumImage
- 370: b1  sampleCount
- 372: b2  TLAS
- 374: b3  vertexData
- 376: b4  indices
- 378: b5  outputImage
- 380: b6  instance_meta
- 382: b7  materials
- 384: b8  ptLights
- 386: b9  ptEmissive
- 388: b10 prevReservoir
- 390: b11 currReservoir
- 392: b12 sharcHashEntries
- 394: b13 sharcAccumulation
- 396: b14 sharcResolved
- 398: b15 guideDepth
- 400: b16 guideVelocity
- 401: b17 guides is a persistent host-visible UBO: it is rewritten through
- 402: the mapped pointer (not vkUpdateDescriptorSets), so it must NOT carry
- 403: UPDATE_AFTER_BIND. Enabling that flag requires the
- 404: descriptorBindingUniformBufferUpdateAfterBind device feature, which
- 405: this renderer deliberately does not request.
- 406: b17 guides
- 408: b18 guideNormalRoughness
- 462: Placeholder images (1×1 — resized on first execution)
- 477: Guide camera constant buffer (binding 17), rewritten every dispatch.
- 514: Bind b17 once. The contents change every dispatch, but the descriptor
- 515: itself never does, so it must not be part of the per-frame
- 516: `update_ds` writes (see the comment there).
- 528: Create persistent HOST_VISIBLE lights buffer for PtAnalyticLight[]
- 561: Write descriptor for the lights SSBO (binding 8, initially zeroed)
- 645: The materials SSBO bytes: if `set_material_resources` wired the
- 646: shared `RenderMaterialManager` buffer we use an empty slice (the
- 647: buffer is bound separately at b7 and never owned by the scene); if
- 648: not wired (shouldn't happen for the real-time pass), fall back to a
- 649: single neutral material so the build still succeeds.
- 650: `build_pt_scene` only allocates its OWN materials buffer from these
- 651: bytes; the real-time pass overrides it with the shared manager
- 652: buffer in `update_ds`, so we pass a minimal placeholder here.
- 662: Drop the previous scene (frees its buffers/BLAS/TLAS via Drop).
- 693: Force pipeline rebuild so it picks up the 2-set layout.
- 726: Force pipeline rebuild only when the set 2 layout actually changed.
- 744: Destroy previous buffer.
- 761: Reset accumulation so the new emissive data takes effect immediately.
- 780: Destroy previous
- 789: Create two device-local storage buffers
- 816: Force pipeline rebuild so it picks up the new buffer handles.
- 837: Zero remaining entries
- 886: Skip if size hasn't changed — avoids unnecessary destroy+recreate
- 887: cycles and keeps the graph's resource references valid across frames.
- 948: All variants share the transport in pt_render.slang; only PT_MODE
- 949: differs, so the layouts and push block are identical.
- 953: Three sets: set 0 = PT-local (accum/output/TLAS/vertex/index/meta/
- 954: materials), set 1 = shared bindless texture table (samplers + SRVs),
- 955: set 2 = IBL environment cubemap (HDR sky).
- 1016: Transition accum to GENERAL
- 1061: Transition sample count to GENERAL
- 1105: Guide images (b15/b16) are written as storage images in GENERAL and
- 1106: sampled later by the temporal resolve.
- 1116: Geometry buffers come from the PtScene (vertex/index/instance_meta).
- 1117: The materials buffer is the shared RenderMaterialManager SSBO (wired
- 1118: via set_material_resources), NOT the PtScene's placeholder buffer.
- 1147: ReSTIR reservoir buffers (ping-pong, b10/b11). Swap roles each frame:
- 1148: b10 gets the buffer written last frame (prev), b11 gets the buffer to
- 1149: write this frame (curr). They stay null until the first consumer
- 1150: dispatch allocates them, and the standalone trainer runs before that,
- 1151: so the writes are only recorded once the buffers exist. Leaving a
- 1152: binding unwritten is legal because the layout carries PARTIALLY_BOUND.
- 1171: SHARC cache buffers. The writes are only recorded when the cache is
- 1172: active: `nullDescriptor` is not enabled, so binding a null handle
- 1173: would be a validation error. Leaving the binding unwritten is legal
- 1174: because the set layout carries PARTIALLY_BOUND, and the shader never
- 1175: dereferences it while `sharc_enabled` is 0.
- 1259: b17 (guides UBO) is deliberately absent: it is written through the
- 1260: mapped pointer every dispatch, so the descriptor is bound exactly
- 1261: once in `new`. Re-writing it here would mark the set "updated while
- 1262: in flight", which is only legal with UPDATE_AFTER_BIND — and a
- 1263: UNIFORM_BUFFER binding cannot have that flag without
- 1264: `descriptorBindingUniformBufferUpdateAfterBind`.
- 1293: Acceleration structure write (binding 2) - done as a separate call
- 1294: because its push_next reference must stay alive for the update call.
- 1340: PtScene drops its own vertex/index/meta/materials buffers + BLAS + TLAS.
- 1343: SHARC buffers are owned by `GraphRenderer`; only drop the handles.
- 1355: Destroy emissive SSBO.
- 1364: Destroy ReSTIR reservoir buffers.
- 1373: Clear the cached device handle so Drop::drop becomes a no-op
- 1374: (graph_renderer calls destroy() explicitly after device_wait_idle).
- 1461: Guide camera state. The depth row only needs this frame transform, so
- 1462: it keeps the jitter; the motion vectors reproject against the previous
- 1463: frame's unjittered matrix so they measure surface movement instead of
- 1464: the sub-pixel jitter offset.
- 1469: `inv_view_rot` is the transpose of the view matrix's upper-left 3x3,
- 1470: so transposing it back yields the world -> view rotation with the same
- 1471: memory layout `frame.view` has in the shader. That lets the guide
- 1472: write use the identical `mul((float3x3)m, worldNormal)` expression
- 1473: scene_frag.slang uses for SV_Target1.
- 1510: The dispatch is already downscaled, so no stride is needed;
- 1511: the training shader forces deposits on and never queries.
- 1514: Deposits belong to the training dispatch, so the consumer
- 1515: only reads the cache.
- 1531: 16x16 threads per group.
- 1555: Declared as SHADER_READ_ONLY_OPTIMAL (not GENERAL) so that when this
- 1556: pass is skipped (e.g. no camera) the state tracker always reads the
- 1557: same layout that PostPass wants, avoiding a trampoline barrier with a
- 1558: stale old_layout (the image was never actually transitioned to GENERAL).
- 1559: The actual post-dispatch GENERAL→SHADER_READ_ONLY_OPTIMAL transition
- 1560: is emitted manually inside `execute`.
- 1563: Temporal-reprojection guides. Same rationale as PT_COLOR_H: declared
- 1564: with the layout the consumer (TaaPass) samples them in, so a skipped
- 1565: pass leaves the tracker consistent.
- 1622: #613: when internal render resolution enabled, PT also accumulates at internal size.
- 1626: Resize accumulation buffers if needed
- 1628: Resize ReSTIR reservoir ping-pong buffers if needed
- 1631: Camera detection. Compared against the unjittered transform: with TAA
- 1632: enabled the projection changes every frame by a sub-pixel offset, and
- 1633: treating that as camera motion would reset the accumulation every frame
- 1634: and leave the tracer stuck at one sample per pixel.
- 1641: Reset on camera motion, explicit accum-dirty (geometry), or the
- 1642: directional-light accumulation-dirty flag from the frame (intensity,
- 1643: color, or direction changed). Cleared after one frame in both cases.
- 1656: Barrier: accum → GENERAL (compute write).
- 1657: On frame 1 (after clear_accum_images) the previous write was a
- 1658: TRANSFER_WRITE from vkCmdClearColorImage; on subsequent frames it
- 1659: was a SHADER_WRITE from the previous-frame compute dispatch.
- 1660: Both access/stage masks must be present to properly order the
- 1661: dependency regardless of which producer ran last — omitting
- 1662: SHADER_WRITE would mean frame 2's compute shader reads undefined
- 1663: garbage from the accumulation images (causing all-white output).
- 1689: Barrier: sampleCount → GENERAL (compute write).
- 1690: Same dual-src logic as accum above.
- 1716: Barrier: PT_COLOR_H → GENERAL (compute write).
- 1717: Use UNDEFINED as old_layout — this is always valid per the Vulkan
- 1718: spec regardless of the image's actual layout (SHADER_READ_ONLY_OPTIMAL
- 1719: from PostPass's read barrier in the previous frame, or UNDEFINED on
- 1720: the first frame / after resize). The graph does not issue a barrier
- 1721: for write-only edges, so we must handle this ourselves.
- 1747: Barrier: guides → GENERAL (compute write). All three are written by
- 1748: the same dispatch, so one barrier array covers them; UNDEFINED as the
- 1749: old layout is always legal (same reasoning as PT_COLOR_H above).
- 1805: Post-dispatch barrier: PT_COLOR_H GENERAL → SHADER_READ_ONLY_OPTIMAL
- 1806: so PostPass can sample it. Must be manual because the write edge
- 1807: declares SHADER_READ_ONLY_OPTIMAL as the post-pass layout (to avoid
- 1808: stale-tracker barriers when the pass is skipped).
- 1834: Post-dispatch barrier: guides GENERAL → SHADER_READ_ONLY_OPTIMAL so the
- 1835: temporal resolve can sample them. Declared as a write edge with that
- 1836: final layout, so the graph does not need to transition them again.
- 1890: Advance reservoir swap index for next frame
- 1895: Publish for PostPass and the temporal resolve
- 2111: The three cache buffers are owned by GraphRenderer (it allocates and
- 2112: clears them), so the graph only carries their handles: declaring the
- 2113: edges here is what makes the training dispatch an explicit, ordered
- 2114: graph step instead of a hidden pre-graph call (repair spec §4).
- 2131: Only the SHaRC configuration trains the cache; the ReSTIR GI variant
- 2132: runs in raster mode and deposits into the cache itself while tracing
- 2133: its secondary rays.
- 2137: Resolve is the only stage that normalises the accumulation counters
- 2138: (see `crate::sharc::training_enabled`), so training without resolve
- 2139: would keep adding to counters nothing resets.
- 2163: ---- helpers ----
- 2270: RGBA16F is one of the formats Vulkan requires storage-image support for,
- 2271: unlike R32G32_SFLOAT, so the compute write needs no format feature probe.
- 2387: ~30° yaw
- 2419: A clip-space point at depth 0 (near plane) must unproject to a point
- 2420: ~znear in front of the camera along its viewing direction. With this
- 2421: identity-rotation view the camera looks down +Z_world toward the
- 2422: origin (eye = (3,2,6), target ~ (3,2,0)), so the near-plane center is
- 2423: at world z ≈ eye_z - znear = 5.9... but Vulkan's [0,1] depth maps the
- 2424: *near* plane to z_ndc=0 only for -z_view rays; here the view basis
- 2425: points the camera at +Z so the recovered point lands at z ≈ -6.1
- 2426: (i.e. znear beyond the eye in the look direction). The exact sign is
- 2427: convention-dependent; what matters is that x/y match the eye and z
- 2428: is within znear of it - i.e. the ray origin is sane, not garbage.
- 2429: This guards against the original symptom (nonsense world points ->
- 2430: all-white path-traced image).
- 2442: eye = (3, 2, 6)
- 2454: x/y must equal the eye (ray passes through the pixel column/row of
- 2455: the eye). A broken inverse would scatter these wildly.
- 2458: z is within znear (0.1) of the eye's z magnitude - i.e. the
- 2459: near-plane point, not a far-away garbage value.

## crates/prism-render/src/pt_pass_tests.rs

- 4: The auto-generated PtPush is 176 bytes (repr(C)) after the SHARC
- 5: fields were appended; the std140 block reflected from the shader and
- 6: the VkPushConstantRange must agree, otherwise the tail fields arrive
- 7: as garbage.
- 40: The frame counter must not overflow into the flag bits — the shader
- 41: unpacks with the same mask.
- 59: A disabled update must not set bit 1 even when the caller asks for it,
- 60: which is how a disabled cache avoids writes to unbound buffers.
- 63: The stride lives in its own byte and must never overflow into bit 0/1.
- 79: SPIR-V magic, little endian.
- 113: 3 x mat4 = 192 bytes; the shader block is std140 and holds no scalars.
- 121: All three transport variants write the same guides, so they must agree on
- 122: the binding numbers - the pass binds one descriptor set for all of them.
- 154: Round up so the last partial block still contributes samples.
- 161: A downscale of zero must not divide by zero or collapse the dispatch.

## crates/prism-render/src/render_graph/draw_items.rs

- 31: Engine non-transparent default (consistent with `MeshRenderer::default`).
- 128: Transparent: far drawn first.
- 353: The forward pass only reads `view` for its own bookkeeping; the
- 354: skybox reads `inv_view`, so the fallback keeps the legacy
- 355: rotation-only inverse it already had.

## crates/prism-render/src/render_graph/mod.rs

- 200: ---------------------------------------------------------------------------
- 201: Read-only snapshots for render-graph visualizers / diagnostics.
- 202: 
- 203: The engine-side viz must not borrow `RenderGraph` (its passes are private
- 204: trait objects) nor touch `vk::*` handles inside the UI closure. These
- 205: plain-data structs are produced by `RenderGraph::snapshot` + each pass's
- 206: `RenderPassNode::graph_info`, cloned once per frame, and consumed by the UI.
- 207: ---------------------------------------------------------------------------
- 525: ---------------------------------------------------------------------------
- 526: Resource state tracker — automatic barrier derivation
- 527: ---------------------------------------------------------------------------
- 672: Advance the tracked state to the post-barrier layout, matching
- 673: the GPU transition we just recorded. This must happen per-read,
- 674: not per-write: a layout change is driven by the read edge here,
- 675: and `apply_writes` only runs after the pass executes.
- 810: ---------------------------------------------------------------------------
- 811: Graph builder — collects passes and resource declarations, then compiles.
- 812: ---------------------------------------------------------------------------
- 931: Future: push to a dependency list for barrier generation
- 988: ---------------------------------------------------------------------------
- 989: Executable graph
- 990: ---------------------------------------------------------------------------
- 1129: Edges were recorded with pass_idx = offset + b.passes.len() == pass_idx
- 1130: (b is a fresh builder with no passes pushed); sanity-check before merging.
- 1157: Per-handle write-before-read check.
- 1178: Cycle detection: pass A -> pass B if A writes a handle B reads.
- 1190: 0=white,1=gray,2=black
- 1246: `PRISMA_PASS_DUMP`: decide once per frame whether to snapshot the
- 1247: published attachments at the end. A no-op unless the env var is set.
- 1250: Throttle the BARRIER_PROBE trace lines to once per second so the log
- 1251: isn't flooded at frame rate. The probe is a debugging aid for the
- 1252: automatic barrier pipeline; gating it here avoids passing `Instant`
- 1253: state through the free functions.
- 1284: Build and emit barriers for this pass's read edges, then
- 1285: update the tracker with write edges after the pass executes.
- 1379: Linear HDR readback (repair spec §7.2): same slot as the pass dump, so
- 1380: the copy sees the frame's final image after every pass ran. A frame
- 1381: without a published `PT_COLOR_H` is not an error - the path tracer is
- 1382: skipped for raster frames, and the caller can simply ask again.
- 1399: (handle, name, format, kind)
- 1516: ForwardPass (and other passes that manage their own textures)
- 1517: registers a 1x1 placeholder extent during setup. Fall back
- 1518: to the frame render extent when the declared extent is <= 1.
- 1570: The shadow atlas is graph-owned (ShadowMapPass does not publish it)
- 1571: and its extent is the atlas size, not the render extent.
- 1657: Put the image back so the recorded layout still matches reality.
- 1818: Previous frame's buffer was never taken; drop it so the map does
- 1819: not leak and the newest frame wins.
- 1860: Put the image back so the recorded layout still matches reality.
- 1935: already allocated
- 1995: Buffers allocated on demand by the pass that owns them
- 1996: (e.g. SHARC buffers are created in SharcPass::setup).
- 2032: Resources should be destroyed explicitly via destroy().
- 2033: If not, they leak — we can't call device destroy in Drop without
- 2034: holding a device reference. This is intentional: the owner of the
- 2035: graph (Renderer/Engine) must call destroy() before dropping.
- 2039: ---------------------------------------------------------------------------
- 2040: Helpers
- 2041: ---------------------------------------------------------------------------
- 2094: suppress unused warning
- 2117: For real transient attachments, prefer LAZILY_ALLOCATED memory type.
- 2132: Fallback to device-local if no lazy type available (non-TBDR GPU)

## crates/prism-render/src/render_graph/settings.rs

- 428: ---------------------------------------------------------------------------
- 429: Internal render resolution (#613)
- 430: ---------------------------------------------------------------------------

## crates/prism-render/src/render_graph_tests.rs

- 25: P0 default flipped to `true`: world-space normals from normal
- 26: maps need Rgba16F precision in GBuffer A. See Plan §4.3.
- 33: Without the SHARC feature no cache is ever allocated.
- 39: Use a model matrix with only translation, `world_position()` reads x-axis position.
- 90: Transparent segment: near drawn first, after sort must become far -> near.
- 92: Non-transparent but larger queue must be placed after transparent segment.
- 111: Transparent segment sorted by distance far to near (x = 9 before x = 1).
- 124: Non-transparent segment not re-sorted by distance, keeps submit order.
- 131: ForwardPass color: also STORAGE, so it can never be lazy.
- 137: Forward depth + shadow map: SAMPLED by later passes, so never lazy.
- 143: Storage images can not be lazy either.
- 149: A pure transient attachment (write-only, discarded) may be lazy.
- 218: Verify pass_edges has precomputed buckets
- 233: Verify last_writers
- 244: Now test add_pass updates precomputed cache
- 254: writes res_a again, updating last_writers
- 267: last_writers for res_a should now reflect Pass2
- 293: Simulate per-frame published data
- 304: Persistent resources table must be preserved
- 306: Per-frame items must be cleared
- 314: Author can write corners in any order.
- 319: Disabled / non-finite / zero area → no clip (just toggled on before rectangle filled, will not hide sprite).
- 324: Half-plane style clip (reflection below water surface) remains valid: only `max < min` is wrong.
- 345: Only width given: height derived by window ratio (640 / (2000/1125) = 360).
- 353: Empty window cannot compute target of size 0.
- 380: min(2000/640, 1125/360) = min(3.125, 3.125) → 3x, 1920x1080, centered.
- 387: When internal resolution larger than window, no 0x: degenerates to 1x centered crop.
- 458: Fsr placeholder behaves identically to Off.
- 465: #611: 2D light table fixed 16 entries, overflow drops weakest by "intensity × brightness".
- 476: An ambient light darker than all point lights: must not squeeze into top 16.
- 481: Strongest one 20 stays at very front.
- 483: Weakest 0.01 ambient dropped; min point light now is 5.0.
- 510: Direction will be normalized, zero vector falls back to "downward".

## crates/prism-render/src/render_pass.rs

- 65: Dependency: wait for acquire semaphore before writing color + depth.
- 128: DEPTH_STENCIL_ATTACHMENT for the scene render pass, SAMPLED so
- 129: the GTAO pass can read it back as a texture after ForwardPass
- 130: stores depth, TRANSFER_SRC so the pass-dump readback can copy it.

## crates/prism-render/src/render_runtime.rs

- 73: last — dropped last via declaration order

## crates/prism-render/src/render_texture.rs

- 65: ---- CRT update config (owned by resource, read by scheduler) ----
- 74: ---- Runtime state (maintained by scheduler) ----
- 181: ------------------------------------------------------------------
- 182: CRT config (owned by resource, read by scheduler)
- 183: ------------------------------------------------------------------
- 219: ------------------------------------------------------------------
- 220: Scheduler interface (called by `RenderTextureScheduler` each frame)
- 221: ------------------------------------------------------------------
- 226: render once on first schedule, then stop
- 273: ------------------------------------------------------------------
- 274: Resource access
- 275: ------------------------------------------------------------------
- 382: Overwrite original slot — handle unchanged, existing consumers stay valid.
- 419: staging buffer (host-visible)
- 453: any old layout → TRANSFER_SRC_OPTIMAL (contents may be discarded, readback semantics acceptable)
- 471: `vkCmdPipelineBarrier2` (1.3 core) is not exported on a 1.2 instance —
- 472: use KHR wrapper like `buffer.rs` does (see `context.rs`).
- 512: map readback
- 536: Pure logic test instance (see tests::test_rt) image null, no real resource.
- 541: fields are non-empty handles created by new / resize success, destroy directly.
- 562: `mem::zeroed` in debug build does invalid_value runtime check and abort;
- 563: `MaybeUninit` path has no runtime check, with above allow(invalid_value) compiles.
- 564: test never reads device (only state machine fields accessed), Drop returns early on null image.
- 599: render on first schedule
- 601: stop afterwards
- 615: Regression test: tick previously only advanced on render frame, with period>1 only first frame rendered.
- 622: counter advances every frame
- 628: no render by default
- 631: render once after flag
- 632: stop after clear
- 639: no init_shader
- 644: back to update_shader after init
- 650: RT without bound shader: under OnLoad `needs_render` always true,
- 651: scheduler attempts render every frame (skips if no shader), binds afterwards and executes next frame immediately.
- 662: stop after one render
- 669: Same seed → same sequence (deterministic).
- 673: xorshift32 sequence doesn't degenerate immediately

## crates/prism-render/src/restir_gi.rs

- 370: The real material table lives in the shared manager SSBO bound to
- 371: b7; only the per-instance vertex/index data comes from here.
- 395: A command buffer submitted last frame may still sample the old images and
- 396: reservoirs, so hand them to the retirement queue instead of stalling.
- 783: ---- Frame constants -------------------------------------------
- 830: The cache is keyed off the camera that produced the samples, so
- 831: the sharc camera position mirrors the current one.
- 838: ---- First-use layout transitions for the ping-pong images -----
- 878: ---- Clear the reservoirs when the history was dropped ----------
- 900: ---- Descriptor updates (set 0) --------------------------------
- 998: ---- Descriptor updates (set 3, G-buffer + history) ------------
- 1023: Build every image info up front: the write descriptors borrow them.
- 1060: ---- Dispatch --------------------------------------------------
- 1087: ---- Make the result visible to the next frame's forward pass ---
- 1109: Publish for the visualiser and hand the forward pass its input.
- 1128: Placeholder extent: the pass owns its own ping-pong images, so the
- 1129: graph resource only exists to make the edge visible to the viz.
- 1166: Raster path only: the path tracer owns GI when it runs.
- 1170: `gi_mode` selects the producer; the forward pass falls back to the
- 1171: baked probe volume when this pass is idle, so nothing runs unless the
- 1172: caller asked for it.

## crates/prism-render/src/retire.rs

- 115: The current frame's slot is the one whose fence guards this
- 116: submission, so the object has to outlive exactly that slot.

## crates/prism-render/src/rt_pass.rs

- 74: Full-screen coverage: CLEAR yields independent image (RT content is whole screen).
- 116: RtPreviewPush = { uint textureHandle }.
- 199: Read edge: topological order ensures after RenderTextureScheduler; barrier automatically
- 200: derived by graph (writer leaves SHADER_READ_ONLY_OPTIMAL == this pass expected layout
- 201: → no-op).
- 232: RT bindless slot (published by RenderTextureScheduler).
- 255: set 0 = bindless table (rt_preview.slang BINDLESS_SRVS).

## crates/prism-render/src/rt_scheduler.rs

- 127: ------------------------------------------------------------------
- 128: Internal: GPU infrastructure (lazily created)
- 129: ------------------------------------------------------------------
- 189: RtRenderPush = { uint pattern } (4 bytes, no std140 tail padding).
- 281: Each RT declares a write edge: after render layout = SHADER_READ_ONLY_OPTIMAL
- 282: (render pass final_layout), for downstream read-edge barrier derivation. RTs are external
- 283: persistent resources, graph does not allocate, no create_resource_at.
- 309: First phase: snapshot each RT render decision (shader + whether need render).
- 310: Next step extends execution phase needing both `&mut self` (pipeline / framebuffer creation) and
- 311: `&mut self.rts[i]` (pattern generation), snapshot first avoids borrow conflict.
- 318: Second phase: execute blits by snapshot.
- 330: Any old layout → COLOR_ATTACHMENT_OPTIMAL. Full-screen repaint,
- 331: no content preservation needed (discard-style update, same as Realtime default).
- 349: `vkCmdPipelineBarrier2` (1.3 core) is not exported on a 1.2
- 350: Instance — use the KHR wrapper like `buffer.rs` does.
- 391: Publish RT (image + view + bindless slot) for downstream sampling.
- 401: End-of-frame state advance: initialize render frame flag / clear pending, all RTs tick unconditionally
- 402: (counter must advance every frame, otherwise Realtime+period decision freezes).
- 434: Pipelines released by GraphicsPipeline RAII; rts each Drop.

## crates/prism-render/src/scene_scope.rs

- 25: ---- set 5 descriptor resources ----
- 31: ---- probe volume 3D texture ----
- 36: ---- ProbeVolumeInfo UBO (48 bytes) ----
- 48: ---- Descriptor set layout (set 5) ----
- 50: binding 0: probe volume 3D texture (SAMPLED_IMAGE, no sampler).
- 56: binding 1: ProbeVolumeInfo UBO (48 bytes).
- 113: Populate with synthetic sky SH field (Phase A default).
- 225: alpha unused
- 233: -------------------------------------------------------------------
- 234: Internal: upload helpers
- 235: -------------------------------------------------------------------
- 240: origin
- 241: spacing
- 242: dims
- 261: 9 coefficient layers
- 263: ---- Create 3D image (RGBA32SFLOAT, device-local) ----
- 304: ---- Upload via staging buffer ----
- 323: One-shot command buffer: layout transition + copy.
- 347: UNDEFINED -> TRANSFER_DST_OPTIMAL
- 369: Copy staging -> 3D image.
- 392: TRANSFER_DST -> SHADER_READ_ONLY_OPTIMAL
- 438: ---- Image view (3D, single mip/layer) ----
- 457: ---- ProbeVolumeInfo UBO (48 bytes, host-visible) ----
- 479: ---- Write descriptor set ----
- 524: samples per probe (uniform sphere)
- 526: RGBA pixels: (dims[0]) x (dims[1]) x (dims[2]*9)
- 529: Precompute SH projection of a sky hemisphere (same for all probes).
- 532: Fibonacci sphere for uniform direction distribution.
- 535: golden angle
- 537: Radiance: sky (upper hemisphere) + ground (lower).
- 539: sky blue
- 541: ground brown
- 544: Monte Carlo SH projection with cosine weighting:
- 545: c_l = (4*pi/N) * sum_i L(d_i) * max(d_i.y, 0) * B_l(d_i)
- 546: surface normal = up
- 555: Fill all probes with the same SH (position-independent).
- 568: alpha unused
- 574: -------------------------------------------------------------------
- 575: Teardown
- 576: -------------------------------------------------------------------

## crates/prism-render/src/screenshot.rs

- 105: Scale factor parsed here: caller need not know swapchain size first.
- 268: Swapchain common format is B8G8R8A8; PNG needs RGBA.
- 278: Swapchain may be sRGB storage; PNG stored as sRGB, no need to convert again.

## crates/prism-render/src/shader.rs

- 33: Align to u32. `include_bytes!` doesn't guarantee alignment, so we try
- 34: `align_to` first and fall back to a safe copy when misaligned.
- 36: Already aligned - reinterpret without copying.
- 41: Misaligned - copy byte-by-byte.

## crates/prism-render/src/shadow_map_pass.rs

- 180: CSM: expand to 3-4 cascades and slice by camera frustum
- 265: `UNDEFINED` + LOAD_OP_CLEAR is the Vulkan-idiomatic way to say
- 266: "discard incoming contents, I'll clear": the render pass performs
- 267: the implicit `any -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL` transition.
- 268: This removes the need for the hand-rolled `UNDEFINED -> ATTACHMENT`
- 269: `cmd_pipeline_barrier` that used to precede `cmd_begin_render_pass`
- 270: (the image is graph-managed and re-cleared every frame, so there
- 271: is nothing to preserve between frames).
- 283: Wait for prior shadow-map fragment shader sampling to finish reading
- 284: before writing depth again.
- 293: Ensure depth writes in subpass 0 are complete and visible before
- 294: subsequent fragment shaders sample the shadow map.
- 377: Depth-only pipeline + depth bias; cull mode decided by caller.
- 429: Reflection JSON only carries first entry point, bindgen therefore has no skinned constant;
- 430: keep consistent with `ForwardPass` style (same literal).
- 535: #491: the static entry point now reads the bone/instance SSBO when an
- 536: instance count is present, so every static pipeline carries set 0.
- 652: Clear tail: out-of-bounds read gets zero matrix, not previous frame's bones.
- 683: #526: size from RenderSettings (written when GraphRenderer constructed), clamped to safe
- 684: range; changing field at runtime won't rebuild already allocated image.
- 695: #526 CSM: one square tile per cascade. With one cascade the
- 696: atlas is just the legacy square map; with two it is 2×1.
- 713: Only render when the rasterized shadow path is active and not in Sprites2D mode.
- 714: The graph builder adds this pass only for `ShadowMode::Raster`, but guard
- 715: anyway so a misconfigured graph can't waste a depth pass.
- 762: Lazy-init pipeline + render pass + framebuffer.  If warmup already
- 763: created pipeline + render_pass, only the framebuffer still needs to
- 764: be created (it depends on `shadow_view` from the graph resources,
- 765: which are only available during execute).
- 770: Render pass — shared between warmup and execute.
- 777: Framebuffer — always needs the per-execute shadow_view.
- 791: if self.framebuffer.is_none()
- 793: Skinned casters' pipeline + user buffers (#560 P0-3c).
- 800: #491: static casters are batched by mesh / culling mode / submesh.
- 801: Instance matrices ride after the live bone palette in the same SSBO.
- 825: The shadow map's `UNDEFINED -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL`
- 826: transition used to live here as a hand-rolled `cmd_pipeline_barrier`.
- 827: It is now handled implicitly by the render pass: the attachment
- 828: `initial_layout = UNDEFINED` + `LOAD_OP_CLEAR` lets Vulkan perform
- 829: the transition inside `cmd_begin_render_pass` (see `create_render_pass`).
- 830: `shadow_img` is therefore no longer needed in this function body.
- 855: Count casters actually written into shadow map (`ShadowCastingMode::Off` skipped).
- 856: With two cascades same batch of casters drawn twice, here only count first pass.
- 860: Among them how many took skinned pipeline (#560 P0-3c).
- 862: Casters culled by light source frustum only count first cascade (#491).
- 868: Each cascade fixed occupies one square tile of atlas; single cascade uses only left half,
- 869: so PCF texel size fully consistent with old 2048² shadow map.
- 877: Light source frustum (#491): shadow map only covers small area near main camera,
- 878: casters kept by main camera in extraction phase not necessarily inside here. Matrix degenerate
- 879: `from_view_proj` returns `None`, below conservatively submits all.
- 908: Select pipeline by (skinned, double-sided), only rebind on switch.
- 911: Culling stays per logical caster so the diagnostics stay
- 912: exact. If at least one member survives, the whole batch is
- 913: submitted; clipped instances cost vertex work only.
- 936: Skinned casters are always singleton batches; static
- 937: casters can carry a shared instance range.
- 1079: Render pass — same layout as `execute` uses.
- 1083: Static pipelines are created by `ensure_skinned` once the bone /
- 1084: instance descriptor layout exists (#491).
- 1120: GraphicsPipeline::Drop frees the pipeline + layout.
- 1125: Descriptor set released together with pool.
- 1157: GraphicsPipeline's own Drop frees the pipeline + layout.
- 1179: `ShadowCastingMode::TwoSided`: disable back-face cull in shadow render.
- 1192: Under no-Y-flip orthographic lighting projection, front face winding is CLOCKWISE
- 1295: Partial overlap also kept (conservative cull, don't cut objects).
- 1300: No bounding box / no usable frustum: keep old behavior, no cull.
- 1307: All-zero matrix has no extractable planes; any cull would be blind clipping.

## crates/prism-render/src/sharc.rs

- 21: 1M entries (~40MB GPU memory)
- 84: Hash entries: 1 entry = uint64_t (8 bytes)
- 90: Accumulation: 1 entry = uint4 (16 bytes)
- 96: Resolved: 1 entry = float16_t4 radiance + 2x u32 sampleData (16 bytes)
- 165: Descriptor set layout: b0 = hash_entries, b1 = accumulation, b2 = resolved
- 215: Write descriptor set
- 253: Pipeline
- 413: The buffers are owned by GraphRenderer; the handles are declared so
- 414: the resolve is an explicit reader/writer of the cache in the graph
- 415: and runs after the trainer that wrote it.
- 439: The previous position is what the shader reprojects the cached
- 440: radiance with; it has to be the real previous frame, so it is
- 441: recorded here rather than reconstructed from the current frame.
- 445: The resolve only runs where the cache is actually in use: path-traced
- 446: SHaRC, or the raster ReSTIR GI variant that terminates its bounces in
- 447: the same cache.
- 466: This frame's training dispatch just wrote the hash entries and the
- 467: accumulation buffer that resolve is about to read, and the two are
- 468: separate dispatches. Order them explicitly. With training disabled
- 469: the writes may come from an earlier frame, which this barrier also
- 470: covers.
- 504: The consumer reads the resolved cache from a later pass in the same
- 505: frame, so make the resolve writes visible to compute and fragment.
- 600: 1M entries is exactly 40 MB

## crates/prism-render/src/skybox_pass.rs

- 95: The render pass + color/depth formats are provided at draw time via
- 96: `execute_with` because the skybox must render into the *same*
- 97: Frame buffer the ForwardPass uses. We can't build a fixed pipeline
- 98: here without that render pass, so the pipeline is created lazily
- 99: inside `execute_with` (which has the render pass).
- 119: Lazily (re)build the pipeline if the render pass differs (e.g. after
- 120: A swapchain recreate that rebuilt ForwardPass' render pass
- 123: Drop the old pipeline via GraphicsPipeline::Drop (which destroys
- 124: the pipeline + layout). Do NOT call destroy_pipeline manually -
- 125: that double-frees.
- 149: No vertex buffer: positions come from SV_VertexID in the shader.
- 153: Push constants: SkyboxPush struct, size directly from Slang reflection.
- 159: MRT blend  state  ForwardPass's render  pass now has 2 color
- 160: attachments (color + view-normal). Every pipeline bound inside
- 161: that render pass must declare a matching attachmentCount, so the
- 162: skybox pipeline lists 2 blend states even though it only writes
- 163: SV_Target0. attachment 1's write mask is 0 so the normal target
- 164: is untouched (the cleared value remains for sky pixels).
- 175: Albedo + roughness (SV_Target3): the skybox shader writes none.
- 194: Disable depth write so the sky never occludes scene geometry;
- 195: depth test LEQUAL lets it draw where depth == 1.0 (cleared).
- 217: Bind the IBL set at set 0. The IBL layout has bindings
- 218: 0=envCube, 1=irradiance, 2=prefiltered; the skybox shader
- 219: only reads binding 0 (envCube).
- 229: Push the combined inverse projection/view rotation plus the sky parameters; the block
- 230: layout and size come from the Slang reflection.
- 244: 6 vertices = 2 triangles full-screen quad (see skybox.slang).
- 258: Dropping `pipeline` runs `GraphicsPipeline::drop`, which calls
- 259: `destroy_pipeline` + `destroy_pipeline_layout`.
- 268: `GraphicsPipeline::drop` handles destroy_pipeline + destroy_layout,
- 269: so just drop the Option. We gate on `self.device` so that an
- 270: un-initialized `SkyboxPass` (device=None) doesn't drop a pipeline
- 271: that was never created.

## crates/prism-render/src/sprite_pass.rs

- 138: ---- 2D lighting (#611) ----
- 207: Lighting resources lazily created (`SpritePass::new` has no device). Even if this frame
- 208: has no lights must be built, else `light_count == 0` branch has no set 1 to bind.
- 218: Keep CPU sort order, only merge adjacent same-blend-state into one draw.
- 219: Blend mode not a sort key, so correct exact order preserved.
- 269: 6 vertices = 2 triangles; one instance per sprite.
- 312: Push constant = `SpritePush` (view-projection + light count). The
- 313: fragment stage reads `lightParams`, so both stages see the block.
- 319: ForwardPass' render pass has two color attachments (HDR color +
- 320: view-space normal). Sprites blend into SV_Target0 and leave the
- 321: normal target untouched (write mask empty), mirroring SkyboxPass.
- 333: Alpha channel only accumulates in standard alpha blend; additive / multiply alpha
- 334: keeps target value, avoid turning later sprites transparent.
- 352: Albedo + roughness (SV_Target3): sprites write none.
- 371: 2D order decided by CPU sort: no depth test nor write.
- 396: First allocation happens here (capacity == 0), so there is no old
- 397: buffer to release yet.
- 408: A previous frame's command buffer may still read the old buffer.
- 463: Callers wait for the device before `destroy`, so parked buffers can go.
- 592: 2D lighting resources (#611).

## crates/prism-render/src/streamline.rs

- 280: eInvalid
- 309: 0 = D3D11, 1 = D3D12, 2 = Vulkan
- 369: 0 = Tex2D
- 373: VkImageLayout
- 392: 0 = OnlyValidNow, 1 = ValidUntilPresent, 2 = ValidUntilEvaluate
- 674: SAFETY: Dynamic library handles and resolved entry-point pointers are process-global.
- 676: SAFETY: Dynamic library handles and resolved entry-point pointers are process-global.
- 698: RTLD_LAZY
- 766: SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
- 768: SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
- 930: eDisableCLStateTracking | eUseFrameBasedResourceTagging
- 937: Vulkan
- 968: Keep the binding alive inside `sl.interposer` (Entry::load_from also holds a ref).

## crates/prism-render/src/suballocator.rs

- 119: Hit free range: remove original and insert remaining part after split
- 163: Coalesce adjacent ranges
- 221: First try allocating in existing Chunk of same memory type
- 238: If cannot fit allocate new Chunk
- 318: ---------------------------------------------------------------------------
- 319: Staging Arena
- 320: ---------------------------------------------------------------------------
- 331: Safety: StagingChunk exclusively owns its mapped pointer and Vulkan objects, cross-thread move safe.
- 454: Current Chunk cannot fit, create new Chunk
- 495: ---------------------------------------------------------------------------
- 496: Unit tests
- 497: ---------------------------------------------------------------------------
- 527: Allocation 1: offset 0, size 100
- 531: Allocation 2: offset 112 (align 16), size 200
- 535: Allocation 3: offset 320 (align 64), size 100
- 542: Release a2 (112..312)
- 546: Release a1 (0..100) -> merges with aligned gap (100..112) and already released a2 (112..312) into 0..320
- 550: Release a3 (320..420) -> all free ranges merge back to 0..1024

## crates/prism-render/src/swapchain.rs

- 212: Build the new swapchain first, handing off the old one so the
- 213: implementation can retire it cleanly (avoids NATIVE_WINDOW_IN_USE).
- 232: Old views and per-image render-finished semaphores go with the old
- 233: swapchain; build replacements sized to the new image set.
- 240: Commit: destroy old, install new.
- 247: The old swapchain was retired by create_swapchain; destroy it now.
- 280: Wait for the previous submission using this frame's fence. This ensures
- 281: the frame's command buffer is no longer in use and its acquire semaphore
- 282: has been consumed by the prior submit.
- 286: Reset only after acquire succeeds. `ERROR_OUT_OF_DATE_KHR` returns
- 287: without a submit, so a fence reset beforehand would never be
- 288: signaled and the next `wait_for_fences(..., u64::MAX)` would hang.
- 368: ---------------------------------------------------------------------------
- 369: swapchain creation helpers
- 370: ---------------------------------------------------------------------------
- 398: `set_present_mode` is intentionally a lightweight setter and may be
- 399: called before a surface is recreated (or with a mode unsupported by
- 400: the current driver). Validate at the Vulkan boundary and fall back to
- 401: FIFO, which the spec guarantees for every surface.
- 426: Honor the presentation engine's current orientation. On a landscape app
- 427: running on a portrait-native device this is `ROTATE_90`/`ROTATE_270`, so
- 428: the compositor rotates the (portrait) swapchain buffer to landscape.
- 437: The backdrop filter copies the composited frame into an offscreen image
- 438: before blurring it. That needs the swapchain image as a transfer source,
- 439: which a surface may not support; ask only when it does, and let the
- 440: filter fall back to an unblurred tint otherwise.
- 490: Prefer sRGB B8G8R8A8 for color accuracy; fall back to the first.
- 547: Fallback for some platforms (e.g. some Android configs) that report
- 548: 0xFFFFFFFF; clamp a minimal extent to the allowed range.

## crates/prism-render/src/taa.rs

- 33: GPU layout comes from the Slang reflection; never mirror it by hand.
- 351: Path-trace mode resolves the traced image, whose guides come from
- 352: the path tracer rather than the raster G-buffer. Declared
- 353: unconditionally: the graph skips a read barrier when the tracked
- 354: layout already matches, and these handles are only published while the
- 355: path tracer actually runs.
- 395: Executed every frame: only output at trace level, `RUST_LOG` disabled by default.
- 402: Drop history while TAA is off so re-enabling does not blend a
- 403: frame from before the pause.
- 409: The path tracer renders its own image and publishes its own depth and
- 410: motion guides, so resolve those instead of the raster G-buffer. The
- 411: normal texture is only read for its alpha reactive flag, which the
- 412: raster G-buffer still supplies in path-trace mode.
- 460: Recover the unjittered camera matrix (GraphRenderer applied
- 461: `view_proj_jitter` after building the graph frame).

## crates/prism-render/src/tangents.rs

- 125: Gram-Schmidt: remove normal direction component so TBN is orthogonal.
- 186: U axis along +Y, V axis along −X: tangent should be +Y, not hard-coded +X by import path.
- 198: Note: this quad has all tangents same direction, so `are_placeholders` still thinks placeholder —
- 199: for flat mesh both results agree, re-generation no worse.
- 216: V axis flipped (mirror): tangent still +X but bitangent direction flipped → w = −1.

## crates/prism-render/src/ui_backdrop.rs

- 228: Downsample: mip 0 -> mip 1 -> mip 2 -> mip 3.
- 266: Upsample: mip 3 -> mip 2 -> mip 1 -> mip 0 (50% tent blend over each mip).

## crates/prism-render/src/ui_overlay/gpu.rs

- 49: ---------------------------------------------------------------------------
- 50: Glyph atlas
- 51: ---------------------------------------------------------------------------
- 138: rowPitch of LINEAR tiling decided by driver, may exceed ATLAS_SIZE.
- 147: UNDEFINED → GENERAL: host side directly maps and writes (LINEAR tiling).
- 184: Reserve 2x2 white pixels at (0,0) so linear sampling at uv=(0,0) yields alpha 1.
- 444: Copy to host-visible memory.
- 459: 1px gap prevents adjacent glyph bleeding
- 477: ---------------------------------------------------------------------------
- 478: UiOverlay
- 479: ---------------------------------------------------------------------------
- 615: Until the renderer supplies a real backdrop, this slot points at the
- 616: glyph atlas: it is always alive and already in a sampled layout, and
- 617: the shader only reads the slot for quads that ask for a backdrop.
- 713: Build new first then tear down old, so on create failure old pass still valid.
- 725: Pipelines rebuilt on next record with new render pass; here only swap pass. Previously submitted
- 726: command buffers may still reference this render pass / pipeline, left to recycle queue for delayed destruction.
- 792: Two bindings per set, one set per frame-in-flight: the pool has to
- 793: hold every descriptor those sets will ever use, not just one set's.
- 922: set 0 binding 0 — glyph atlas (see GlyphAtlas::create_descriptors).
- 938: set 1 = scene-shared bindless texture table (UI background images, #591).
- 1010: Pipeline state: no depth, alpha blend.
- 1081: The previous frames' command buffers may still read the old buffer, and
- 1082: growth happens mid-frame while the current one is recording. Park it in
- 1083: the retirement queue instead of stalling the device.
- 1121: Target rect (#988): when `internal_resolution` active UI only draws inside this block,
- 1122: composited with PostPass-upscaled image, not pressed onto letterbox.
- 1126: Scene bindless texture table (set 1); UI background images sampled by `UiQuad::texture`.
- 1162: The frame slot that just waited its fence is free again: release what
- 1163: this overlay parked during the previous pass through the queue.
- 1173: The estimate only sizes the reservation: a character writes vertices
- 1174: only when it has a glyph bitmap (spaces have no rect, and a full atlas
- 1175: or missing font skips entirely), so the real count is usually lower.
- 1178: Build vertices (quads + per-glyph expanded texts).
- 1183: Interleave rects and text by engine-computed layer / order (old path without order degrades to
- 1184: "all rects first, then all text").
- 1188: NDC rect → pixel size. `UiQuad::border_radius` is already
- 1189: normalised by the engine (`radius_px / max(w, h) * 2`), so
- 1190: convert it back to pixels for the SDF.
- 1196: With background image uv covers 0..1; solid quad samples atlas 1×1 white pixel.
- 1246: Upload and draw both use actual vertex count: estimate may exceed `vertices.len()`,
- 1247: copying that much would read uninitialized memory past Vec (UB, stray triangles on GPU).
- 1258: Upload vertex data into this frame-in-flight slot's buffer.
- 1278: Atlas has new glyphs → HOST_WRITE → FRAGMENT_SHADER barrier (GENERAL layout unchanged).
- 1309: Reuse the framebuffer for this swapchain image instead of creating
- 1310: (and retaining) one every frame. Not thread-safe by itself; record
- 1311: runs on the render thread and swapchain recreation clears the cache.
- 1337: Record draw segments, capturing and blurring the backdrop before any
- 1338: segment that introduces frosted-glass surfaces.
- 1392: Map NDC [-1,1] to `dest`: UI and scene scale together and stay centered
- 1393: inside `dest`, while scissor clips any out-of-bounds pixels.
- 1455: 1. Line breaking (#527): `\n` always breaks; with `wrap` also greedy break by rect width.
- 1456: Without `wrap` nor `\n` it's original single line, behavior unchanged.
- 1472: Lines beyond rect bottom not drawn (no half-glyph clipping).
- 1477: 2. Measure this line's width (sum of advances).
- 1486: 3. Alignment decides starting x (per-line alignment, not whole paragraph).
- 1493: 4. Emit per-glyph quads (shadow pass first, then main face + optional SDF outline).
- 1602: Byte offset → char index, and cumulative advance per char: line width for any `[from, to)` is
- 1603: O(1), no need to re-measure for every candidate break point.
- 1624: Farthest break point from `pos` such that whole line width still fits.
- 1640: No break point fits: consume at least one char, truncate by char.
- 1746: `GraphRenderer::destroy` waits for the device before dropping overlays.
- 1775: ---------------------------------------------------------------------------
- 1776: One-off command buffer / memory type helpers (same mode as ibl.rs)
- 1777: ---------------------------------------------------------------------------
- 1901: Subtract 0.5 so the 0-contour sits midway between adjacent inside/outside
- 1902: pixel centers with exact unit slope across the boundary.
- 1917: Vertical pass (columns)
- 1924: Horizontal pass (rows)
- 2153: #592: #FF8020 sRGB components are 1.0 / 0.502 / 0.125.
- 2162: Black / white are fixed points.
- 2258: Center of the filled 4x4 square should be well inside (> 128).
- 2260: Far corner should be well outside (< 128).
- 2266: Vulkan NDC y downward: pixel top is -1, pixel bottom is +1, center is 0.
- 2274: Monotonically increasing: larger pixel y must land lower (larger NDC y).
- 2275: This is the property old implementation (`h - py`) violated.

## crates/prism-render/src/ui_overlay/text/coverage.rs

- 30: BGRA: B G R A — grayscale AA three channels equal, take R (offset 2).
- 40: Empty white glyph (empty grid, fully corner gaps): does not occupy atlas, but advance must be kept.
- 73: Same meaning as fontdue `Metrics::ymin`: bitmap **bottom edge** upward offset relative baseline.
- 99: BGRA: grayscale glyph three channels equal.
- 118: A 2x2 ink block, upper-left corner (3, 4), grayscale 0 and 128.
- 124: Baseline origin (4, 6): ink left edge 1px left of origin, bottom edge 0px below baseline.
- 136: A row of ink at y = 5, baseline at y = 12.
- 141: Bottom edge relative baseline: 12 - (5 + 1) = 6 px upward → +6 in fontdue convention.

## crates/prism-render/src/ui_overlay/text/directwrite.rs

- 109: ClearType level 0 = grayscale antialiasing (single-channel coverage, exactly what R8 atlas needs).
- 121: Fall back to system default parameters: may be ClearType (R/B channels have color fringes),
- 122: taking R channel as coverage would introduce slight color tint — warn first, don't silently blur.
- 158: UI coords are pixels: 1 dip = 1 px, system scale handled by window.
- 173: First release old one: it may still be held by previous readback.
- 182: 1. char → glyph index + design units.
- 202: 2. Temporary bitmap: 2em square enough for ink of any single glyph (incl. negative bearing and descenders).
- 206: Integer origin: origin not on half pixels, so clipped xmin/ymin are whole pixels.
- 207: In 2em square place pen at 60% from left: negative bearing / descenders / right overflow have margin.
- 218: 3. Clear white + transparent background mode + black glyph → coverage = 255 - R.
- 220: Clear failure would leave previous glyph's ink, clipped into new glyph — must handle as error.
- 231: `ManuallyDrop`: binding field convention, cloned references must be manually Released after call.
- 253: Pair with clone from above, avoid leaking one AddRef per rasterization.
- 257: 4. Read back BGRA (BitBlt into our own top-down DIB, see ReadbackSurface).
- 267: 2em square temp bitmap should not clip normal glyphs; clipping means exceptional oversized glyph.
- 276: DirectWrite backend has no project font registry (`register_font` uses trait default
- 277: returning false), all text uses system fonts (#583).
- 293: Each (char, size) reaches here only once (result cached by atlas), no spam.
- 331: negative height = top-down: row 0 of `bits` is the top screen row.
- 380: Safety: `bits` comes from CreateDIBSection, length is box_px² × 4 bytes BGRA.

## crates/prism-render/src/ui_overlay/text/fontdue_backend.rs

- 24: Windows
- 25: Microsoft YaHei (includes Chinese)
- 26: SimHei (includes Chinese)
- 28: Android
- 33: Linux
- 40: macOS
- 95: Explicit font unusable → still search candidates, but must say so clearly, otherwise
- 96: "font not swapped" and "font never loaded" look identical in logs.
- 137: Do not just give up: project can register bundled font at runtime (#583). But if no font registered in
- 138: whole lifetime, still no text drawn — must keep this line of reasoning.
- 189: fontdue descent is negative number (downward negative), here normalized to "positive distance below baseline".
- 196: First parse font then mutably borrow rasterize: `resolve` read-only.
- 214: Could previously warn "not registered", now usable so do not report again.
- 256: Unregistered font + no default font = cannot get glyph, not panic.
- 267: Use TTF actually present in repo to validate register path (skip if missing, not tying test to
- 268: some system font).
- 288: Re-register same path: directly overwrite, no error.

## crates/prism-render/src/ui_overlay/text/mod.rs

- 65: Without metrics keep legacy behavior (`emit_text` always uses this approximation).
- 96: Only the DirectWrite backend reads back BGRA bitmaps, so the module must
- 97: vanish with it instead of carrying an unused-code suppression.
- 141: Previously hardcoded `font_size * 0.8`: must keep this value when no metrics.

## crates/prism-render/src/ui_overlay.rs

- 25: ---------------------------------------------------------------------------
- 26: Text backend feature combinations
- 27: ---------------------------------------------------------------------------
- 28: 
- 29: The two backends are mutually exclusive: enabling both leaves fontdue and
- 30: ttf-parser in the binary while the author believes the switch took effect.
- 31: Failing the build is better than silently shipping the extra 90 KB.
- 33: `ui-fontdue` and `ui-directwrite` are alternatives, never both.
- 41: DirectWrite is a Windows-only backend; other targets have no `windows` bindings.

## crates/prism-render/src/voxel_dda_pass.rs

- 840: Sort uploaded chunks nearest-to-camera first so `trace_world` tightens `best.t` early.
- 1136: 2 slots * 6 descriptor sets per slot = 12 sets.
- 1672: Sun radiance and IBL ambient pass through verbatim: with no directional
- 1673: light the engine writes zero intensity, and it must stay zero here. Any
- 1674: non-zero floor turns a night scene back into daylight.
- 1746: Transition one-time dummy images if needed.
- 1806: Update all 5 descriptor sets for this slot cleanly.
- 1911: 1. Transition all target images to GENERAL for writing.
- 1949: 2. Pass 1: Primary DDA + 2-bounce GI + Emissive NEE + Temporal accumulation.
- 1967: Transition primary G-buffer outputs to SHADER_READ_ONLY_OPTIMAL before GI + denoise + composite.
- 2017: 2b. Pass 1b: Demodulated 2-bounce voxel GI + emissive NEE + temporal accumulation.
- 2054: 3. Pass 2: 4-iteration À-Trous edge-aware bilateral denoise (`step = 1, 2, 4, 8`).
- 2155: Final denoise pass: transition denoise_b -> SHADER_READ_ONLY_OPTIMAL for composite.
- 2177: 4. Pass 3: Composite denoised GI + direct + glass + cursor into VOXEL_COLOR_H & VOXEL_DEPTH_H.

## crates/prism-render/src/voxel_rt_gi_pass.rs

- 168: Ready to bind VoxelDdaPass G-Buffer & TLAS when GraphRenderer schedules RT GI.

## crates/prism-render/tests/csm_gpu.rs

- 179: Completely outside identity box; when light matrix degenerates it will still be submitted, so this assertion can distinguish
- 180: "cull works" from "matrix cannot build frustum".

## crates/prism-render/tests/debug_pipeline_sponza.rs

- 62: right = fwd x up
- 70: true_up = right x fwd
- 77: In engine convention, camera looks down -Z:
- 78: So camera Z axis = -fwd
- 98: Vulkan NDC +Y down
- 107: Quad at X = -5.0, Y in [0, 4], Z in [-2, 2], facing toward -X (normal = [-1, 0, 0])
- 127: Graph resources not fully in library, but this inside only uses geometry and material factors (graph slot left empty),
- 128: so bypass `gltf::import` graph decode, read document + buffer directly.
- 248: Camera at [-10.0, 2.0, 0.0] looking along +X towards Sponza colonnade
- 272: -----------------------------------------------------------------------
- 273: Pass 1: Empty Scene (Clear to Dark Slate Blue)
- 274: -----------------------------------------------------------------------
- 311: -----------------------------------------------------------------------
- 312: Pass 2: Calibration Quad (Red quad directly in front of camera)
- 313: -----------------------------------------------------------------------
- 399: -----------------------------------------------------------------------
- 400: Pass 3: Real Crytek Sponza Meshes Loading
- 401: -----------------------------------------------------------------------
- 402: Resources submitted with repository, parsed by crate position, avoid hardcoding absolute path of a specific machine.
- 413: Scale 0.01 model matrix (transforms cm to meters)
- 452: -----------------------------------------------------------------------
- 453: Pass 4: Sponza Forward Raster (Solid Opaque)
- 454: -----------------------------------------------------------------------
- 488: -----------------------------------------------------------------------
- 489: Pass 5: Sponza Raster Wireframe
- 490: -----------------------------------------------------------------------
- 511: wireframe = true
- 524: -----------------------------------------------------------------------
- 525: Pass 6: Sponza Depth Buffer (debug_rt = 1)
- 526: -----------------------------------------------------------------------
- 535: debug_rt = 1 (Depth)
- 563: -----------------------------------------------------------------------
- 564: Pass 7: Sponza View-Space Normals (debug_rt = 2)
- 565: -----------------------------------------------------------------------
- 574: debug_rt = 2 (Normals)
- 602: -----------------------------------------------------------------------
- 603: Pass 8: Sponza Path Tracing (Direct Lighting, 4 iterations)
- 604: -----------------------------------------------------------------------
- 658: -----------------------------------------------------------------------
- 659: Pass 9: Sponza Path Tracing with RTXGI SHARC (32 iterations)
- 660: -----------------------------------------------------------------------

## crates/prism-render/tests/dof_gpu.rs

- 29: The box sits 3 m from the camera; focusing at 0.2 m puts it far
- 30: outside the depth of field.

## crates/prism-render/tests/golden_frame.rs

- 79: VK_MAKE_VERSION(1, 3, 280) = 1 << 22 | 3 << 12 | 280.

## crates/prism-render/tests/golden_support/mod.rs

- 20: This module is compiled into several test binaries; each one exercises a
- 21: different subset, so per-binary dead-code expectations cannot all be met.
- 191: Path-traced frames resolve materials from the SSBO, so give the draw
- 192: list the registered material instead of the raster default.
- 210: The path tracer does not read the raster shadow/light path; without an
- 211: analytic light the traced frame is pure ambient and reads back black.
- 212: Mirror the frame's directional light so path-trace frames are lit.
- 226: The path tracer traces its own acceleration structures, so the draw
- 227: list has to be uploaded before the frame that consumes it.
- 229: The tracer reads the bindless material table, which the raster
- 230: path fills lazily; flush it before uploading geometry.
- 243: debug flags
- 244: ACES tonemap
- 245: AA off: keep the rasterizer deterministic
- 246: debug target
- 251: pt_max_bounces
- 252: pt_ray_max_distance: the box sits 3 m out
- 253: pt_max_iterations: keep accumulating
- 285: Vulkan NDC +Y points down.
- 373: ---------------------------------------------------------------------------
- 374: Golden comparison
- 375: ---------------------------------------------------------------------------

## crates/prism-render/tests/gpu_timings.rs

- 45: debug flags
- 46: tonemap: passthrough
- 47: AA off
- 48: debug_rt
- 49: proj22
- 50: proj32
- 61: bone matrices
- 73: First frame results are not yet readable (readback intentionally delayed by one frame).
- 99: Full frame interval covers all passes; timestamp quantization allows pass sum to be slightly larger.

## crates/prism-render/tests/gtao_gpu.rs

- 27: A camera move changes the velocity field; the temporal descriptor set
- 28: must follow it without validation errors or a dropped frame.

## crates/prism-render/tests/headless_clear.rs

- 22: If the error is "no driver" / "no compatible device",
- 23: skip instead of failing.
- 35: Any other error is unexpected.
- 45: graceful skip
- 48: Clear the offscreen target to pure red.
- 54: Read back the pixel data.
- 57: Offscreen target is 256 × 256 × RGBA8 = 262 144 bytes.
- 60: Every pixel should be (255, 0, 0, 255) — pure red.
- 61: Sample the four corners + center to confirm uniformity.
- 144: bone matrices
- 202: bone matrices
- 225: A headless context never enables `VK_KHR_swapchain`, so ending the pass in
- 226: PRESENT_SRC_KHR is invalid there (VUID-VkAttachmentDescription-finalLayout-parameter).

## crates/prism-render/tests/kernel_benchmark_gpu.rs

- 39: Match the two separate operations in harness_bench.slang so the
- 40: reference does not depend on FMA contraction.
- 83: Relative tolerance: kernel accumulates hundreds-level values, GPU fma/order differs from CPU reference
- 84: implementation. Fixed 1e-5 absolute tolerance at success value ~3e2 leaves only ~3e-8 relative precision,
- 85: changing driver or slangc version will cause false report (#818).

## crates/prism-render/tests/lut_gpu.rs

- 98: passthrough: only the clear color + LUT matter
- 163: One 8-bit step of trilinear/rounding noise is expected; anything larger
- 164: means the identity LUT is not being sampled at texel centres.
- 195: And the swap must actually be visible for this non-grey clear color.

## crates/prism-render/tests/mobile_upscaler_gpu.rs

- 155: 1. Enclosed atrium: travertine floor & ceiling (with skylight opening),
- 156: terracotta left wall (bounces warm red GI), jade right wall (bounces cool green GI),
- 157: travertine back wall with recessed dark slate niches.
- 168: Floor (y = 0): travertine with central slate inlay.
- 180: Ceiling (y = 9): solid except a 5x4 skylight opening so sun casts crisp shafts + soft GI fill.
- 189: Left wall (x = 0): warm terracotta bounce wall.
- 194: Right wall (x = 15): jade-green bounce wall.
- 199: Back wall (z = 15) and front wall behind camera (z = 0).
- 207: 2. Stepped altar & metallic bronze colonnade framing the center.
- 216: Central bronze pedestal + floating cyan crystal core.
- 222: Carved L3 floating octahedron-like crystal on the central pedestal.
- 235: Twin bronze columns with L3 fluted capitals on left & right.
- 251: 3. Recessed emissive GI braziers in the deep shaded side alcoves:
- 252: Left alcove (x=1, z=10): warm amber brazier behind L3 stone grate.
- 265: Right alcove (x=14, z=10): cool cyan brazier behind L3 stone grate.
- 271: 4. Overhead L3 skylight lattice to cast delicate shadows & showcase upscaler edge reconstruction.

## crates/prism-render/tests/motion_blur_gpu.rs

- 23: Establish a previous camera matrix for the moving renderer.
- 35: Same camera position, effect off. This is the reference frame.

## crates/prism-render/tests/multi_camera_gpu.rs

- 154: Camera 1 is `Nothing`: it must not clear the red background.
- 163: Both cameras clear colour: the later (higher depth) one wins.
- 177: Empty camera list = legacy path: `clear_color` from the frame input.

## crates/prism-render/tests/occlusion_gpu.rs

- 61: Box mesh is identity cube, model matrix only translation + uniform scale.
- 191: 4 identity large boxes at z = −5, nearly covering the screen; another box at z = −30 fully hidden behind.
- 197: First frame has no previous-frame depth: nothing culled.
- 201: Second frame: near boxes write depth that fully covers far box.

## crates/prism-render/tests/post_grade_gpu.rs

- 50: no draw items
- 52: light view-proj
- 53: inverse projection
- 54: debug flags
- 55: tonemap: passthrough, so only grading changes the pixel
- 56: AA off
- 57: debug_rt
- 58: proj22
- 59: proj32
- 60: no lights
- 62: pt bounces
- 63: pt ray distance
- 64: pt iterations
- 65: exposure
- 66: no PT lights
- 70: bone matrices
- 123: Center barely moves (smoothstep starts at 0.35), corners darken noticeably.
- 139: The wrapper clears to a neutral grey, so saturation 0 alone cannot prove
- 140: much on its own — but it must at least keep RGB equal (the luma blend is
- 141: a no-op on grey and must not introduce a tint).
- 183: Monochrome grain: all three colour channels move together.

## crates/prism-render/tests/prt_gpu.rs

- 102: Run 3 frames so 1-frame latency gi_volume is populated and sampled by ForwardPass.

## crates/prism-render/tests/pt_analytic_gpu.rs

- 81: Without ray query there's no `PathTracePass`, whole test suite has nowhere to land.
- 184: `to_gpu` only sends emissive[0] into SSBO, and in shader
- 185: `s.emissive = ei * es`, so this value is expected radiance.
- 315: Don't freeze: one sample per frame enough — every scene's estimator below is zero variance
- 316: (constant environment, constant emissive, single NEE), no convergence noise to store.
- 318: `result *= push.camera_pos.w`: exposure multiplied at output, parsed value only equals radiance itself when
- 319: exposure == 1.
- 322: `reset = true` accumulates from zero, so "one frame == one Monte Carlo sample"; false
- 323: lets accumulation converge across frames, for low-variance quantitative comparisons.
- 426: White furnace: constant environment 0.25, nothing in scene blocks raycast, every pixel should only see environment.
- 435: Same scene with doubled radiance.
- 442: Linearity: doubled radiance, read-back must double. This assertion proves "read back before tonemap" —
- 443: post-tonemap 8-bit path compresses 0.25 and 0.5 to about 0.19 and 0.31,
- 444: ratio only 1.6x.
- 454: Constant environment 0.375 (mantissa byte 96). Use radiance value different from above, so IBL disk
- 455: cache hash differs — two tests running in parallel won't fight for same cache file.
- 462: Two segments: main raycast → box surface (one scatter) → miss → environment.
- 466: Background: zero-segment scatter, result is environment radiance itself (miss propagation).
- 473: Front face: one diffuse scatter then miss. Per-segment throughput is
- 474: (brdf·cos) / pdf; with roughness = 1 pdf always cosine pdf (cos/π), so
- 475: throughput = (albedo/π + specular)·cos / (cos/π) = albedo + π·specular.
- 476: With normal incidence D = 1/π, V = 0.25, F = 0.1024, therefore π·specular = 0.1024·0.25
- 477: = 0.0256. Expected value of whole path is (albedo + 0.0256) · 0.375.
- 488: Convex outward cosine sampling can never hit itself, so raising bounce count shouldn't
- 489: change any pixel: extra segments are all empty.
- 505: Black environment (`new_empty` is 1x1x6 all-zero cube), only light source in whole scene is material's own
- 506: emissive term.
- 513: pt_max_bounces = 1: bounce 0 only accumulates emissive, then NEE (sun off,
- 514: no glow triangles), loop never entered. Expected value is emissive itself.
- 526: `GpuMaterial` only packs `emissive[0]` into SSBO (`metallic_roughness_emissive.z`),
- 527: then multiplies `emissive_strength` so entire RGB takes effect — glow body is gray body, all three channels
- 528: equal same scalar. This assertion pins "gray body" fact onto interface.
- 536: Box ~48% of image height, corner raycast hits black environment, must be strictly 0.
- 552: Single directional light, pt_max_bounces = 1: only light transport term is bounce 0 NEE.
- 553: L_dir same direction as +Z face normal (n_dot_l = 1), L_rad = light_color.rgb · 1,
- 554: occlusion raycast cast to outside side of box, necessarily unoccluded, so
- 555: L = (albedo/π + F·D·V) · intensity
- 556: where F = 0.1024 (metallic 0), D = 1/π, V = 0.25.
- 567: Linearity: doubled intensity, radiance doubles. This assertion guards "NEE missing or extra 1/pdf
- 568: factor" class of errors — uniform sampling one light's 1/pdf is exactly 1.
- 582: Last item of §7.2: "NaN/Inf and empty hash". When hash capacity 0 cache table can't be
- 583: built, but query bit in push constants still gets set, so shader truly queries a
- 584: degenerate table. Image must still be finite and non-negative — one out-of-bounds read or NaN propagation would make
- 585: PT_COLOR_H dirty, which downstream tonemap can't hide.
- 601: Only light source is constant environment 0.25; diffuse box only attenuates it, so no pixel should
- 602: exceed environment itself (plus one GGX highlight point outside).
- 608: Remaining three items of §7.2 measured together here: on cache hit must copy `cached_radiance` then
- 609: directly `break` (`pt_render.slang:771-810`), cannot add another local NEE; training side
- 610: judging "this voxel is credible enough" (`SharcUpdateHit` returns false) ends path,
- 611: cannot double deposited measurable amount; while main raycast miss sky term completely unrelated to cache. Three share same,
- 612: observable consequence: same scene with cache on/off, frame-level amount must match.
- 613: 
- 614: SHaRC is biased (voxel resolution, material demodulation, firefly suppression), so §7.2 only requires
- 615: conventional diffuse ROI relative average luminance error ≤5%, and "no stable double counting" — hit
- 616: then adding NEE again typical result is doubled indirect term, in this scene showing >30% overall
- 617: offset, far from 5% threshold.
- 629: Reverse check: turning off gi_mode these two counters must not move. Without this, below
- 630: "counter > 0" assertion can't exclude "counter always 1" bad case.
- 641: Voxel edge length is `2^level / sharc_scene_scale`, and `level` determined by distance from hit point to camera
- 642: (in this scene 2–6 m → level 1–2). With sceneScale = 5 voxels are 0.4–0.8 m,
- 643: same order as line segment length between two boxes, so `valid_hit`'s two geometry thresholds pass;
- 644: factory value 50 gives 4 cm voxels, hits sparse to only a few pixels.
- 650: Cache truly ran, otherwise below quantitative comparison is empty talk. Timers showing
- 651: "SharcTrain" / "SharcResolve" only means those two GPU ranges were opened
- 652: (`GraphRenderer::execute` unconditional `begin_pass`), inside range
- 653: `dispatch_training` may still silently early-return via guard; renderer's own recorded real dispatch
- 654: count is evidence.
- 678: Miss propagation: main raycast from image top-left corner grazes past two boxes, only sees constant sky.
- 679: Cache must not participate here — no matter how long it trained.
- 687: Training and parsing truly dispatched is only necessary condition — training runs but no voxel
- 688: falls into queried position, image still won't change. Real evidence is image: cache hit copies
- 689: `cached_radiance` and ends path directly, same pixel's estimate must differ from pure path tracing.

## crates/prism-render/tests/selection_outline_gpu.rs

- 41: Camera at (0, 0, 3) looking down -Z at a unit cube centred on origin.
- 54: Vulkan NDC +Y points down.
- 156: bone matrices
- 209: Fill still present: wireframe is not the only content.

## crates/prism-render/tests/sharc_lifecycle_gpu.rs

- 27: Small capacity + default scene scale: cache fits, VRAM use controlled.
- 197: First frame creates cache and clears once (initial allocation).
- 208: Static camera: world cache has no reason to invalidate.
- 222: Resize only invalidates screen history (motion vector / accumulation); world cache stores radiance by world position,
- 223: resolution change only needs reprojection.
- 256: Draw list rebuilt each frame, signature changes with camera / cull; it cannot count as geometry invalidate, otherwise camera
- 257: moving slightly would clear world cache.
- 276: Light changed: radiance in old voxels came from no-longer-present incident light, hard clear per §8.
- 295: Invalidate reason is one-shot: not cleared again next frame.
- 322: Changing environment = changing training incident light; radiance in old voxels no longer belongs to this scene.
- 357: Scene unload happens in raster frame: no SHaRC to clear, reason must persist across frames.
- 377: First frame back to SHaRC consumes it — also the frame old voxels become visible.
- 404: Run one more frame with same scale: no format change, cache kept.
- 414: Changing voxel scale equals changing hash key: old entries stay but now describe other
- 415: voxels (§8 "cache format change clears").
- 430: One-shot: continuing with same scale does not clear again.

## crates/prism-render/tests/sharc_sponza_golden.rs

- 27: Normal from winding
- 62: Looking down -Z towards [0, 1.5, 0]
- 75: Vulkan NDC +Y down
- 147: 1. Create Sponza atrium architecture:
- 205: Register Materials
- 229: Build DrawItems
- 232: Floor & Ceiling
- 246: Walls
- 266: Sponza-like colonnade rows
- 282: Center pedestal
- 315: -----------------------------------------------------------------------
- 316: Pass 1: Standard Rasterization
- 317: -----------------------------------------------------------------------
- 350: -----------------------------------------------------------------------
- 351: Pass 2: Path Tracing without SHARC (gi_mode = Off)
- 352: -----------------------------------------------------------------------
- 357: Run 4 accumulation iterations
- 392: -----------------------------------------------------------------------
- 393: Pass 3: Path Tracing with RTXGI SHARC (gi_mode = Sharc)
- 394: -----------------------------------------------------------------------
- 398: reset accumulation
- 402: Run 4 accumulation iterations with SHARC active
- 437: Training and resolve are their own stages: each must show up as a GPU
- 438: interval of its own, separate from the consumer pass. An open interval
- 439: alone proves nothing though -- `dispatch_training` returns early when one
- 440: of its preconditions is missing and the "SharcTrain" interval stays open
- 441: either way -- so the renderer's own dispatch counters are what actually
- 442: pins the work down. The SHaRC graph work is behind the `sharc` feature,
- 443: so this only applies to builds that enable it.
- 468: -----------------------------------------------------------------------
- 469: Pass 4: cache-free reference variant (no cache, no ReSTIR, no clamp)
- 470: -----------------------------------------------------------------------
- 516: The reference only differs by the disabled firefly clamp in this
- 517: sun-only scene, so its mean must track the consumer closely.
- 525: -----------------------------------------------------------------------
- 526: Pass 5: standalone SHaRC trainer. It deposits radiance and writes no
- 527: screen output, so the only assertion is that the dispatch runs clean.
- 528: -----------------------------------------------------------------------
- 561: -----------------------------------------------------------------------
- 562: Golden Frame Comparison & Metrics
- 563: -----------------------------------------------------------------------
- 642: Sanity checks: Non-empty, non-black renders

## crates/prism-render/tests/skinned_shadow_gpu.rs

- 168: No palette but declares skinning: this caster cannot draw (otherwise garbage outside bind pose).
- 198: Shadow channel did not run in this build (config outside `ShadowMode::Raster`).
- 209: Non-identity palette: no panic, still submitted. Per-pixel correctness cannot be done here — need a
- 210: receiver scene sampling shadow texture; skinned vertex blend itself covered by `skinning_gpu.rs`.

## crates/prism-render/tests/skinned_shadow_pixels_gpu.rs

- 77: aspect = 1
- 78: Vulkan y-flip
- 301: Baseline: no shadows, no shadow on receiver face.
- 306: Enable shadows + place bone at x = -1.2: a shadow appears on receiver face.
- 314: Move bone to x = +1.2: shadow must follow, not stay at original position.

## crates/prism-render/tests/skinning_gpu.rs

- 36: projection (column-major, [col][row])
- 38: aspect = 1
- 39: Vulkan y-flip
- 43: camera sits at z = +3, so view = translate(0, 0, -3)
- 46: view_proj = proj * view  (column-major multiply)
- 148: wireframe
- 149: shaded wireframe
- 160: The clear colour is a flat value; anything that differs from the corner
- 161: pixel is geometry.
- 186: Every vertex fully weighted to joint 0, whose matrix is the identity.
- 236: Slide the bone 0.6 units along +X: the silhouette has to move.

## crates/prism-render/tests/skinning_heatmap_gpu.rs

- 38: aspect = 1
- 39: Vulkan y-flip
- 151: wireframe
- 152: shaded wireframe
- 189: Half vertices attached to joint 0, half to joint 1, weights all 1 — both joint matrices are identity,
- 190: so geometry is completely identical, any color difference can only come from heatmap shading.
- 202: Normal shading: default material is linear color, no obvious color cast.
- 215: Heatmap: hue = joint × golden angle. joint 0 → red (1, .15, .15),
- 216: joint 1 (0.618 turn) → blue (.15, .4, 1).
- 245: Weight all 0 = bad import / static data: cannot paint it as some bone color.

## crates/prism-render/tests/taa_gpu.rs

- 28: A rotated box gives diagonal edges, so subpixel jitter changes coverage.
- 59: A camera move must not crash and must keep producing a frame.
- 69: Turning TAA off returns to the legacy path.
- 83: Move the box sideways for several frames. With per-object velocity the
- 84: resolve must keep producing a stable frame instead of accumulating the
- 85: old silhouette.
- 92: Transparent draws set the reactive mask (normal.w), so TAA must trust the
- 93: current frame rather than blending in stale history. This mainly guards
- 94: the render-pass/pipeline wiring (three MRT attachments + mask flag).
- 119: The tracer accumulates, so let it settle before comparing frames.
- 126: A traced frame must not be a flat clear: the box has to be visible.
- 135: Turning TAA on in path-trace mode must still produce a full frame. This
- 136: is the regression guard for the `!is_pt` gate that used to keep the
- 137: temporal resolve out of path-trace mode entirely.
- 155: Disabling TAA must fall back to the raw traced image again.

## crates/prism-render/tests/voxel_dda_gpu.rs

- 104: Build a 4x4x4 L1 chunk centered around (2, 2, 2):
- 105: - L1 (0, 1, 2): uniform COW orange slab (`0xFFFFFFFF`).
- 106: - L1 (2, 1, 2): split L2 brick containing one L2 wall at local (0, 2, 0) [x:2.0..2.25, y:1.5..1.75, z:2.0..2.25]
- 107: with its center 2x2 L3 tunnel along Z carved out (`mask` omits center mx in 1..3, my in 1..3).
- 113: L1 uniform orange/copper block at (1, 1, 2) -> x=[1..2], y=[1..2], z=[2..3].
- 118: L1 split brick at (2, 1, 2) -> brick index 0 (`ref = 1`).
- 120: local L2 (0, 2, 0) -> world [2.0..2.25, 1.5..1.75, 2.0..2.25]
- 125: Build 64-bit mask missing the center 2x2 tunnel along Z (mx in 1..3, my in 1..3).
- 138: Transient WET flag (`1 << 0`) + METALLIC (`1 << 12`).
- 222: Center pixel looks through the carved 2x2 L3 hole (dark clear color ~ [85, 85, 85, 255]).
- 224: Offset pixel (-28 px from center) hits the solid cyan-metallic L3 frame.
- 226: Right-side pixel (+82 px -> x ≈ 1.84 in [1..2]) hits the uniform COW L1 orange slab at (1, 1, 2).
- 273: Place off-camera inside the enclosed dark room to the right of view frustum.
- 384: Test transient BURNING flag (emissive byte == 0) contributing colored indirect GI.
- 396: With no sun and no sky light the baseline is exactly zero, so the emissive
- 397: voxel is the only light source: the shaded wall averages ~5.3/255. A 3.0
- 398: threshold leaves room for driver/hardware variation while still catching an
- 399: NEE regression (falls back to 0) or a denoiser flattening the GI away.

## crates/prism-render-shaders/build.rs

- 104: Nothing goes to stdout on failure: cargo prints build-script stdout above the
- 105: error, and a watch list there would bury the message we want the reader to see.
- 248: `tests/support/mod.rs` documents the pattern with an ellipsis placeholder.

## crates/prism-runtime-host/src/lib.rs

- 586: Try runtime components first (the registry would only log an "unknown"
- 587: warning and reject them).
- 616: The type name is pinned to the `#[reflect(name = ...)]` value.
- 1049: Kept solely to keep the module loaded while its callbacks are registered.
- 1197: `module` is deliberately dropped after shutdown so no callback can
- 1198: point into an unloaded library.
- 1217: The target is always `project`: the editor Console groups module logs under
- 1218: the `Game` source by target (`logging::source_for`), keeping them apart from
- 1219: engine / editor logs.
- 1346: The callback itself is always present (the host implements it), but it reports
- 1347: UNSUPPORTED when nothing was injected.
- 1368: Capability not declared: the callback is not exposed, so project code does not
- 1369: assume the switch took effect.
- 1396: The additive flag is passed through.
- 1412: Empty and non-UTF-8 paths report an error instead of recording a half request.
- 1487: A miss returns 0; a null pointer is an argument error.
- 1725: Two-pass read: ask for the length, then fetch the bytes.
- 1733: A missing key returns a negative code, not a zero length.
- 1961: Query: probe the length first, then fill.
- 1995: Read: the pointer is valid until the next host call, so copy immediately.
- 2053: A second despawn must be rejected instead of silently succeeding.

## crates/prism-scene/src/animation.rs

- 121: Last index with `time <= keys[i].time` (`time > keys[0].time` guarantees exists).
- 139: Two keyframes at same moment (or out of order): use first one, avoid division by zero amplification.

## crates/prism-scene/src/animation_tests.rs

- 57: NaN / Infinity are treated as 0: better to stop at the initial pose than write NaN transforms.

## crates/prism-scene/src/components.rs

- 31: ---------------------------------------------------------------------------
- 32: SceneAssetId
- 33: ---------------------------------------------------------------------------
- 68: ---------------------------------------------------------------------------
- 69: Hierarchy
- 70: ---------------------------------------------------------------------------
- 105: ---------------------------------------------------------------------------
- 106: Transform
- 107: ---------------------------------------------------------------------------
- 192: ---------------------------------------------------------------------------
- 193: Render references
- 194: ---------------------------------------------------------------------------
- 234: ---------------------------------------------------------------------------
- 235: Authoring-time helpers
- 236: ---------------------------------------------------------------------------
- 297: Engine opaque default: One / Zero / Add (see `blends`).
- 365: ---------------------------------------------------------------------------
- 366: Distance LOD
- 367: ---------------------------------------------------------------------------
- 477: ---------------------------------------------------------------------------
- 478: 2D Sprite
- 479: ---------------------------------------------------------------------------
- 1100: ---------------------------------------------------------------------------
- 1101: Particles
- 1102: ---------------------------------------------------------------------------
- 1126: Order consistent with declaration; these strings are values in `.scene.json`.
- 1618: ---------------------------------------------------------------------------
- 1619: Lighting
- 1620: ---------------------------------------------------------------------------
- 1866: 0 = night, 1 = day; sunrise / sunset each have half transition.
- 1989: ~25°
- 1990: ~50°
- 2042: Engine `new Light()` defaults to point light source.
- 2190: ---------------------------------------------------------------------------
- 2191: Camera
- 2192: ---------------------------------------------------------------------------
- 2504: ---------------------------------------------------------------------------
- 2505: Prefab instances (editor metadata)
- 2506: ---------------------------------------------------------------------------
- 2521: ---------------------------------------------------------------------------
- 2522: Identification
- 2523: ---------------------------------------------------------------------------
- 2534: ---------------------------------------------------------------------------
- 2535: Skybox
- 2536: ---------------------------------------------------------------------------
- 2640: ---------------------------------------------------------------------------
- 2641: Physics
- 2642: ---------------------------------------------------------------------------
- 2848: ---------------------------------------------------------------------------
- 2849: 2D Physics
- 2850: ---------------------------------------------------------------------------
- 3084: ---------------------------------------------------------------------------
- 3085: Skinning
- 3086: ---------------------------------------------------------------------------
- 3152: ---------------------------------------------------------------------------
- 3153: Skinning Animation
- 3154: ---------------------------------------------------------------------------
- 3361: 0.2s ≈ UE default blend time: short enough not to drag, covers most pose differences.
- 3392: ---------------------------------------------------------------------------
- 3393: Terrain
- 3394: ---------------------------------------------------------------------------
- 3504: ---------------------------------------------------------------------------
- 3505: Audio
- 3506: ---------------------------------------------------------------------------
- 3660: ---------------------------------------------------------------------------
- 3661: Scene management
- 3662: ---------------------------------------------------------------------------

## crates/prism-scene/src/components_tests.rs

- 29: Vulkan y-flip: p[1][1] is negative.
- 65: half height 2 → NDC 1/2; after y-flip the top edge (y=+2) lands at NDC y = -1.
- 67: half width = half height × aspect = 4.
- 132: Document with only old fields (pre-v2 fast-path camera) must still load — new fields use serde defaults.
- 161: ---------------------------------------------------------------------------
- 162: Light
- 163: ---------------------------------------------------------------------------
- 169: `new Light()` defaults to point light; cone angles and color temperature use engine defaults.
- 196: Document containing only known fields must also serialize correctly (`#[serde(default)]`).
- 207: #611: 2D lighting uses "artistic units" — intensity 1.0 = original color.
- 290: #611: default does not participate in lighting, old scenes' palette unchanged.
- 311: Color temperature disabled: return as-is.
- 316: 2000 K is warm: red component kept, blue visibly reduced.
- 328: Engine's 60° full cone = 30° half cone.
- 337: When inner cone larger than outer cone, clamp to outer cone, no negative cosine interval.
- 362: ---------------------------------------------------------------------------
- 363: SpriteRenderer (2D sprite)
- 364: ---------------------------------------------------------------------------
- 421: Old document with only path: remaining fields use serde defaults.
- 437: #582: clipping disabled by default, old documents not suddenly clipped.
- 458: #529: old documents have no Tilemap, new fields all use serde defaults.
- 489: 2 columns → 3 rows (last row has only one cell).
- 495: tileset 64x32 + 16x16 tile = 4 columns 2 rows.
- 499: Index beyond tileset clamped to last tile.
- 502: Row 0 at top: larger row number means smaller y.
- 520: Entire texture 64x32, PPU 16 -> 4x2 world units.
- 527: sprite sheet single frame takes priority over texture size.
- 536: Invalid PPU falls back to 100, no NaN/Inf.
- 605: Old document with only mesh/material: all new fields must fall back to engine's opaque default.
- 616: One / Zero / Add = opaque.
- 625: Author format uses variant names (consistent with `CameraClearFlags` / `TextAlign`),
- 626: queue is numeric (`Material.renderQueue` int form).
- 652: Standard alpha blend = transparent.
- 668: One-to-one correspondence uses submesh lower bound.
- 673: Empty string does not masquerade as material, continue matching by glTF material index.
- 678: When not found fall back to singular field.
- 684: When length is not submesh count, match by source material index (Unity shared-material usage).
- 708: Author format uses variant names (consistent with `CameraClearFlags` / `TextAlign`).
- 721: Missing fields keep defaults (cross-version compatibility).
- 763: Pure 2D: distance has no effect.
- 771: Half blend: falloff only applies halfway.
- 778: Degenerate range / out-of-range volume will not produce >1 or NaN.
- 792: #604: old document has only points / width, new fields must be serde defaults.
- 831: Old document / only one field written: rest use serde defaults.
- 839: Degenerate direction / non-finite value falls back to downward, not NaN velocity.

## crates/prism-scene/src/cook_stamp.rs

- 179: Future-version stamp: do not guess semantics, leave to "no stamp" branch.
- 213: mtime second-level granularity / network drive clock rollback: within 2 seconds not stale.
- 225: Release pack has no `.scene.json`, scan returns 0, only schema compared.
- 246: After deleting documents only pak remains, returns 0 (release form).

## crates/prism-scene/src/helpers.rs

- 122: 1. Remove from old parent's Child list

## crates/prism-scene/src/helpers_tests.rs

- 107: a -> b -> c, so making a a child of c would close a cycle.

## crates/prism-scene/src/json_tests.rs

- 31: Historical doc wrote transform as a component. On read should bake into `transform` not keep component,
- 32: otherwise loader overwrites `LocalTransform` again after `LocalToWorld` computed.
- 58: cook goes through same `validate`, so it also fails before encode.
- 110: Still must stay in type table: RTTIBin type table / editor field descriptions depend on it.
- 142: Old impl scanned full table per node (O(n²)), 20k nodes would do 400M comparisons.
- 256: v2: `parent` is array index, no id.
- 271: Identity map: old parent index value directly becomes id, level unchanged.
- 288: Deleting id 9 does not make next id fall back to 9.
- 305: Index not id: 40 → 0, 12 → 1, 3 → 2.
- 311: Track is "struct list", most likely to lose data in RSCN positional encoding (#490).
- 366: Emitter has an enum field (`shape`): RSCN writes enum as small integer, restores on decode,
- 367: most likely to lose value in positional encoding (#501).
- 431: Audio component only a string path + scalars; RSCN positional encoding most likely to lose
- 432: string (`sound_path`) and boolean (#508).
- 487: Physics component two enums (ColliderShape / BodyType) use hand-written ReflectType with "JSON variant name, RSCN small integer"
- 488: most likely to go wrong in round trip (#522).
- 617: New fields must tolerate old document: missing `blend_duration` uses 0.2s, not 0 (hard cut).
- 647: Terrain component is "path + scalars", RSCN positional encoding most likely to lose string (heightmap)
- 648: and `[f32; 2]` (size) (#517).
- 689: Distance LOD only one `Vec<f32>` field (#491).
- 728: Old scene only wrote `distances`; new fields must stay compatible via serde default.
- 764: Skeleton and skin renderer only have path + one `Vec<String>` (#560).

## crates/prism-scene/src/manifest.rs

- 89: Another table (should not appear): close the current entry and
- 90: ignore the keys that follow.

## crates/prism-scene/src/migrate.rs

- 72: Version-independent normalization:
- 73: - Missing entity id filled by array order (v3+ `parent` references id);
- 74: - Derived components not allowed in author document.
- 216: `transform` object in author shape.
- 243: would truncate to 1 with `as u32`
- 365: "new_added" at index 2 must NOT receive id 2 (which already belongs to "c")

## crates/prism-scene/src/references.rs

- 24: MeshRenderer
- 25: MeshRenderer
- 26: MeshRenderer (Vec<String>, multi-submesh materials)
- 27: SpriteRenderer
- 28: Tilemap
- 29: Skybox
- 30: AudioSource
- 31: Terrain (does not end with _path, must be registered explicitly)
- 32: SkeletonRoot
- 33: SkeletonRoot
- 34: SkinnedMeshRenderer
- 35: Animator
- 36: Animator (.rasm)
- 37: Animator (Vec<String>, 1D blend space)
- 38: Animator (Vec<String>, 2D blend space)
- 39: Animator (additive layer clip)
- 40: Animator (.rag animation graph)
- 41: EnvironmentLighting
- 42: GiConfig
- 43: PrefabInstance
- 44: UI Text (project-provided font, #583)
- 281: A GUID array can outlive a shortened path array. Recover only
- 282: slots that actually resolve; unresolved entries must not grow
- 283: the path array with empty strings.
- 326: GUID arrays always mirror the path array's slots.

## crates/prism-scene/src/references_tests.rs

- 114: Unknown path: no GUID to add.
- 117: Invalid GUID with an unresolvable path is left alone rather than guessed.

## crates/prism-scene/src/registry.rs

- 16: ---------------------------------------------------------------------------
- 17: ComponentRegistry
- 18: ---------------------------------------------------------------------------
- 192: Default author JSON for editor "add component": serde shape of `C::default()`.
- 197: Reflection object → author JSON (`Object -> C -> serde JSON`).
- 392: ---------------------------------------------------------------------------
- 393: built-in component register
- 394: ---------------------------------------------------------------------------
- 429: ── scene base components ──
- 432: Name uses custom JSON shape (string, not {"0": ...}).
- 452: ── light source  ──
- 461: ── declarative lighting (scene-driven IBL / GI, avoid engine startup unconditional load) ──
- 469: ── camera  ──
- 480: ── skybox / render bundle ──
- 486: #491: distance LOD (child entity is LOD level).
- 490: ── 2D sprite (author state; GPU handle held by runtime `SpriteRef`) ──
- 497: #611: 2D sprite lighting (same group as SpriteRenderer, meaningless without sprite).
- 499: #983: day-night clock. It only writes color into ambient light of same entity, 2D scenes use it often,
- 500: but can also do logical timing in pure 3D projects, so not limiting by `group-2d` is more sensible;
- 501: currently registered with Light2D, keeping consistency "no 2D means no day-night appearance".
- 503: #1160: 2D camera and 2D physics
- 510: ── local transform  ──
- 511: Register **types only** (RTTIBin type table / editor field descriptions), not as attachable component:
- 512: Author only writes `EntityJson::transform`, baked by loader / cooker into
- 513: `LocalTransform` + `LocalToWorld` (the non-removable Transform in author state).
- 514: If registered as component, `registry.apply` would overwrite `LocalTransform` after
- 515: `LocalToWorld` computed, leaving world matrix calculated from old transform.
- 519: ── keyframe animation ──
- 520: Tracks contain nested structs, first register type (RTTIBin type table / editor field descriptions),
- 521: then register component itself.
- 526: ── particle emitter (author state; live particles in runtime `ParticleEmitter`) ──
- 529: Screen-space emitter: simulated in the engine, emitted as UI quads (#1057).
- 532: ── audio (author state; decode and mixing in host layer `prism-app::scene_audio`) ──
- 541: `Animator.layers` is nested struct; only after registering type can RSCN / editor schema parse.
- 556: ── prefab instance (editor metadata) ──
- 559: ── UI component  ──
- 565: `ComputedLayout` is output of layout system (`rect` rewritten every frame),
- 566: ensured by `ui::layout`; only register type, don't let author attach.
- 569: #528: components of interactive UI must be savable in scene / prefab.
- 572: #463: canvas scale (reference resolution → actual screen).
- 576: ── nested types (type table / cook needed) ──
- 586: Nested type of `Style.layout` (#527 / #709): missing registration makes all scenes with `Style`
- 587: fail RSCN encode with `type ... is not registered`.
- 638: export goes through same schema.
- 657: Project modules and the engine must hash the same schema, so the
- 658: registry is built once and handed out by reference.
- 671: Each built-in component has "add component" default value, and can deserialize back to component.
- 674: schema_fingerprint is deterministic and non-zero
- 688: Export via reflection path, shape should match default (Name uses custom converter).
- 698: Name default shape is bare string (custom JSON shape).
- 704: Types nested in author components must also be registered, otherwise RSCN encode will
- 705: fail with `type ... is not registered` (#709 `Style.layout`).
- 721: Each component can round-trip "author JSON -> reflection object -> author JSON".

## crates/prism-scene/src/rscn.rs

- 51: ---------------------------------------------------------------------------
- 52: Errors
- 53: ---------------------------------------------------------------------------
- 114: ---------------------------------------------------------------------------
- 115: Header
- 116: ---------------------------------------------------------------------------
- 140: ---------------------------------------------------------------------------
- 141: Decoded scene
- 142: ---------------------------------------------------------------------------
- 194: v1 has no env path
- 208: ---------------------------------------------------------------------------
- 209: Encoding
- 210: ---------------------------------------------------------------------------
- 226: Original index → topological order position. Precompute once to avoid scanning `order` linearly for each entity.
- 231: `parent` stores entity id; parse into array index here.
- 250: Sort by name so repeated cooking produces byte-identical output.
- 480: ---------------------------------------------------------------------------
- 481: Decoding
- 482: ---------------------------------------------------------------------------
- 524: `entity_count` is attacker-controlled until the first entity is read.
- 525: Clamp the reservation to what the remaining input could actually hold.
- 648: ---------------------------------------------------------------------------
- 649: Byte helpers
- 650: ---------------------------------------------------------------------------

## crates/prism-scene/src/terrain.rs

- 221: Center keeps its height; the edge gets (1 - 0.25)² = 0.5625 of it and
- 222: the corner (1 - 0.5)² = 0.25 — the brush never overshoots the target.

## crates/prism-scene/src/ui.rs

- 11: ── flag component  ─────────────────────────────────────────────────
- 20: ── anchor system ─────────────────────────────────────────────────
- 107: ── layout group (#527) ──────────────────────────────────────────────
- 230: ── Style ─────────────────────────────────────────────────────
- 317: ── ComputedLayout ────────────────────────────────────────────
- 370: ── Text ──────────────────────────────────────────────────────
- 446: ── Canvas / scale  ─────────────────────────────────────────────
- 565: ── Interaction ───────────────────────────────────────────────
- 583: ── Button ───────────────────────────────────────────────────
- 702: #583: legacy document without `font` → empty string = system default font.
- 718: #463: 1920×1080 design at 960×540 should be 0.5.
- 725: When aspect differs, match interpolates in log space: 960×1080 width ratio 0.5, height ratio 1.0.
- 729: When matching one side only, take that side directly.
- 743: ConstantPixelSize = 1, degenerate input also returns 1 not NaN/0.
- 764: Old `.scene.json` has no `scale` field: treat as 1, font size not shrunk.
- 801: #527: legacy document without `layout` → direction = None, children positioned by anchors.
- 847: #527: when `wrap` not written keep single line; only wrap when written.

## crates/prism-scene/src/voxel.rs

- 753: Convert to chunk-local L1 space (`1 unit = 1 m`).
- 1143: Compute exact inward sub-cell coordinates near surface entry when hitting uniform tier.
- 1518: Carve every single L3 micro-voxel inside L2 (lx=4, ly=8, lz=4).
- 1531: Iron Law 1: clearing all L3 bits clears parent L2 occupancy automatically.
- 1533: Adjacent L2 cell inside the split L1 brick remains intact.
- 1546: Carve a 2x2 L3 pinhole through the center of this 0.25m L2 cell along Z.
- 1555: Ray straight through the carved L3 hole misses the front face!
- 1559: Offset ray hits solid L3 rim and inherits L2 transient WET|METALLIC flags (Iron Law 2).
- 1580: Water should not block solid-only player collision.
- 1584: Add one L3 bar at mx=0..4, my=0, mz=0 -> [0.0..0.25, 0.0..0.0625, 0.0..0.0625]
- 1605: Inside the box of key 2 only ([16..24) on X).
- 1609: Straddling the 0/1 boundary returns both neighbours, never the far ones.
- 1614: Above the chunks: nothing overlaps on Y.
- 1628: Travelling +X from x = -1 enters key 100 first, then 101, then 102.
- 1636: A short ray stops before the second chunk: it would only reach x = 7.5.
- 1643: A degenerate direction yields no candidates.
- 1655: Only the middle chunk holds solid voxels.
- 1668: Inside chunk 1's solid L2 cell at [4..5) on every axis.
- 1684: Empty space above the chunks agrees with the brute-force walk too.
- 1694: A ray down -X from beyond the far chunk hits the middle one first.
- 1719: Same key, moved box: the entry is replaced, not duplicated.

## crates/xtask/src/shader_bindgen.rs

- 237: The emitted Rust type must never be larger than what the shader
- 238: reserved for the field; when it is smaller (e.g. a std140 matrix with
- 239: 16-byte column stride) the reserved size wins.
- 271: std140 rounds every struct up to a 16-byte boundary; matching that in
- 272: `repr(C)` keeps `align_of` honest for records used as SSBO elements.
- 312: Static layout assertions: compile-time guarantee of bit-identical std140 layout
- 435: LightingPushConstants: 4×u32 + 4×f32 = 32 bytes.
- 437: SharcQueryPushConstants: 7 fields padded to 48.
- 461: Only emit descriptor-set slots and push-constant buffers. Slang also
- 462: reflects vertex-shader `in` parameters (kind "vertexInput" / no
- 463: descriptor binding) — those are not Vulkan descriptor bindings and must
- 464: be skipped.
- 470: Distinguish UBO vs texture via the type shape.
- 486: vertex inputs, stage inputs, etc. — not descriptor bindings
- 547: Gather global params + entry-point params.
- 561: Dedup by (set, binding, name).
- 566: File header
- 574: Entry points.
- 587: Descriptor bindings grouped by set.
- 590: std140 size of the emitted structs. This is the authoritative fallback for
- 591: shaders whose slangc reflection omits the block size on the
- 592: `pushConstantBuffer` parameter.
- 629: --- Auto-generated GPU buffer / push-constant struct definitions ---
- 630: Scan global and entry-point parameters for buffer types (PushConstants and
- 631: UniformBuffers) whose types carry struct field info; emit a #[repr(C)] Rust
- 632: struct with std140 layout padding and compile-time offset assertions for each.
- 634: Skip types Rust cannot lay out yet (e.g. `float16`, which is still an
- 635: unstable primitive). Such structs stay hand-written or unmodelled.
- 686: Write per-shader file: {out_dir}/{mod_name}.rs
- 690: Add to mod.rs
- 694: Write mod.rs
- 781: Every reflected field must land on its own reflected offset.
- 800: And the struct must extend to the last reflected byte.
- 818: std140 rounds the vec4 after a u32 up to 16: bytes 4..16 are a hole that
- 819: a dense `#[repr(C)]` layout would silently collapse.
- 852: `model`(64) + 14 packed scalars. The block used to carry a
- 853: `float3 _padding` after `skin_mode`; #491 / #883 / #517 replaced it
- 854: with real fields (instance_*, prev_draw_index, reactive_mask,
- 855: splat_*), so no filler is left and the size is exactly 120.
- 901: Derive the expected constant from the reflected layout instead of
- 902: pinning a byte count: the previous literal (108) went stale when the
- 903: block grew to 120, and nothing noticed because `crates/xtask` is
- 904: excluded from the default workspace.

## editor/crates/prism-editor-core/examples/player_build.rs

- 67: `required` is the **executable in the release directory**, only present after copy.

## editor/crates/prism-editor-core/src/anim_graph_view.rs

- 3121: Any state (`*`) is not affected, nor does it mistakenly hurt other states.
- 3525: String connections never had a mask, clearing is a no-op.

## editor/crates/prism-editor-core/src/asset_edit.rs

- 98: Only single-file edits participate in drag coalescing; a multi-file transaction,
- 99: once committed, is an independent step.

## editor/crates/prism-editor-core/src/asset_meta_tests.rs

- 47: Second time, even with a different importer, does not rewrite the existing GUID.
- 71: A GUID with wrong shape is also treated as corrupt.

## editor/crates/prism-editor-core/src/assets.rs

- 137: Absolute path / drive letter: not a project asset, don't guess.
- 159: Relative to the `assets/` root: `sprites/water.png` / `textures/wood.png`.
- 163: Bare file name: same-named assets must be unique, otherwise don't guess.
- 269: path -> child nodes (directories use BTreeMap for stable order: dirs first, then
- 270: same category by name).
- 286: `assets/` is the project asset root, not shown as a collapsible directory.
- 737: Root directory files are also in the root node.
- 880: Relative to project root
- 885: Relative to document directory (a form allowed by #465)
- 890: Relative to assets/ root (the most common form in the project)
- 895: Bare file name (when unique)
- 909: Can't guess when same-named files aren't unique.
- 911: Empty value = not set, not a broken reference.
- 913: Relative path runs out of assets/.

## editor/crates/prism-editor-core/src/build.rs

- 146: The plan must be reproducible: sort by file name so `read_dir` order doesn't affect the
- 147: order of build artifacts.
- 185: cwd must be the project root: `build-scenes` finds `assets/scenes.json` by cwd.
- 198: The editor parses these two outputs into Problems panel entries; `cargo run` without
- 199: them would pollute game stdout.
- 209: Leaving it empty must not degenerate into "project root" — that would copy assets/
- 210: into itself.
- 228: The artifact layout must let `asset_paths` recognize `assets/`: the executable goes in
- 229: `output_dir/`, assets go in `output_dir/assets/` (consistent with the Web editor and the
- 230: game directory packing layout).
- 272: Copying onto itself (output_dir points back at project root) errors, just skip it.
- 348: Cook still runs in the project root, otherwise it would cook the engine repo's own assets/.
- 367: The temp dir has no `assets/`, so only one copy item (the exe).
- 393: Authoring documents and loose assets must not be copied into the release directory.

## editor/crates/prism-editor-core/src/components.rs

- 160: serde feeds entries in file order, so author order is naturally restored here.
- 219: At head / tail / nonexistent names all stay put.

## editor/crates/prism-editor-core/src/doc.rs

- 201: v1/v2: parent is an index.
- 228: Drop parents pointing to nonexistent ids (cycle check is done separately at reparent).
- 368: Orphaned children become roots.
- 508: Moving a whole subtree: root + descendants, keep array order (parent before child).
- 518: Slots the target shifts forward after removal = number of moved entities before it.
- 561: Collect subtree (root + descendants), keep array order, guarantee parent before child.
- 697: Root entities can't keep a parent pointing to an external parent.
- 1067: Bare file name: compare against the old file's name, and stay a bare file name after rewrite.
- 1076: Can't resolve: only worth flagging if "looks like this very file".
- 1086: Write back in the author's original style: `assets/...` stays absolute, `../` stays relative.
- 1407: Non-defaults must be retained.
- 1450: Grand is Child's child, should be deleted together.
- 1456: Root's children Child/Grand deleted together, only Sibling remains.
- 1495: ids are globally unique.
- 1567: New ids must not overlap old ones.
- 1579: Sibling scope: children can reuse root-level names.
- 1581: Strip the suffix first, then find a free slot.
- 1583: Non-numeric suffixes are part of the name itself.
- 1714: #465: entity names and UI text that happen to read the same path must not be changed along.
- 1772: Relative form resolved against **this document's directory**, stays relative.

## editor/crates/prism-editor-core/src/dock.rs

- 213: Neither side fits the minimum: don't force it, split in half (at least no panic, and
- 214: the splitter isn't drawn outside the frame).
- 517: - left / right / top / bottom: open a new leaf next to the target.
- 537: Reorder tabs within the same leaf.
- 565: The target leaf disappeared because we dragged away its only panel; put it back in place.
- 585: First remove the tab from its old position; if the target leaf disappears as a result,
- 586: we're dragging the target leaf's own only tab — edge-splitting onto itself would just
- 587: duplicate it, so do nothing.
- 1132: Unknown tab / unknown leaf doesn't change anything.
- 1145: Drag c to the far left (index 0).
- 1156: Drag from right leaf to left leaf: right leaf becomes empty, pruned, split collapses.
- 1197: The remaining subtree still keeps a.
- 1203: Normal width: a ratio split with a floor.
- 1205: Ratio at the extreme: stops at the minimum, not squeezed to 0.
- 1212: #498: `2by3` dragging the middle column to ~95px wide, inner Row wants `clamp(60.0, 30.0)`.
- 1231: Negative / non-finite available space (parent split already flattened) must not
- 1232: produce weird values.
- 1266: Utility windows default to joining the inspector leaf, not opening a new column.
- 1350: Dragging left beyond the minimum width clamps width to the minimum, and x follows the anchor.
- 1355: The top edge cannot cross the toolbar.
- 1376: Right edge sticks to the viewport edge: x + width = 1600 - 8.
- 1383: Aligns with the other panel's top edge.

## editor/crates/prism-editor-core/src/expr.rs

- 28: Trailing unconsumed tokens (`1 2`, extra fragment of `(1+2`).
- 46: ---------------------------------------------------------------------------
- 47: Lexer
- 48: ---------------------------------------------------------------------------
- 108: The number already advanced to the end; don't let the trailing `index += 1` consume another character.
- 128: ---------------------------------------------------------------------------
- 129: Grammar: expr := term (('+'|'-') term)*
- 130: term := unary (('*'|'/'|'%') unary)*
- 131: unary := ('+'|'-') unary | power
- 132: power := primary ('^' unary)?      (right-associative: 2^3^2 = 2^9)
- 133: primary := number | ident | ident '(' args ')' | '(' expr ')'
- 134: ---------------------------------------------------------------------------
- 243: '('
- 342: Result must be finite: expressions overflowing to inf are also rejected.

## editor/crates/prism-editor-core/src/gizmo.rs

- 28: Ramamoorthi & Hanrahan order-2 real SH constants (must match `prism-render/src/gi.rs`).
- 652: ---------------------------------------------------------------------------
- 653: Built-in component gizmo drawers
- 654: ---------------------------------------------------------------------------
- 1108: ---------------------------------------------------------------------------
- 1109: PRT Probe Volume (PRPV) parsing, SH9 evaluation & Scene View visualization
- 1110: ---------------------------------------------------------------------------
- 1393: L1 basis order in PRPV: idx 1 = Y_1^-1 (y), idx 2 = Y_1^0 (z), idx 3 = Y_1^1 (x).
- 1525: Draw the active Y-slice horizontal highlight rectangle if slicing by layer.
- 1557: Collect (dist_sq_from_camera, probe_idx, pos) so far probes render before near probes.
- 1624: Trilinear tethers connecting the focused/selected world position to its 8 probes.
- 1700: Pure isotropic irradiance E = [PI, PI*0.5, 0] requires c0 = E / (SH_C0 * PI) = 1 / SH_C0.
- 1747: (0,0,0)
- 1748: (1,0,0)

## editor/crates/prism-editor-core/src/keys.rs

- 10: File
- 19: Edit
- 33: Build
- 37: Run
- 42: Window
- 55: Help
- 58: Viewport
- 335: Panel switching (engine's Ctrl+1..5).
- 592: #421: After rebinding old key no longer valid; alias should not trigger same command either.
- 630: Binding back to own original key is not a conflict.
- 633: Override table participates in judgment: after Play changed to Ctrl+Shift+P, other commands can't take it.
- 649: Same chord cannot bind two commands.

## editor/crates/prism-editor-core/src/logbuf.rs

- 417: Out-of-bounds requests past the bottom are clamped.

## editor/crates/prism-editor-core/src/manifest.rs

- 451: Same name (different directory) gets a suffix added automatically.

## editor/crates/prism-editor-core/src/notify_view.rs

- 257: 100px for 2 seconds: midpoint 1 second, both ends clamped.
- 305: 0..200px for 2 seconds → A at 50px, B at 150px.

## editor/crates/prism-editor-core/src/palette.rs

- 155: Parent entity is `id` not index, and may form cycle (corrupted doc), max entities.len() steps.
- 196: Shorter ranks higher within same level: `Save` before `Save As`.
- 274: Shorter ranks first within same level: `water` before `water_tile.png`.
- 294: b…o…a…t is subsequence of "boat", but not "lake".
- 306: sample()'s document has entities, entity entries should carry hierarchy path or component name.

## editor/crates/prism-editor-core/src/project.rs

- 125: `.rmeta` is asset's sidecar, not importable asset: don't show in asset browser,
- 126: and don't generate another `<...>.rmeta.rmeta` layer for it.
- 158: `.mat.json` is material authoring data (`MaterialDef` / cooker's `.mat.json` schema),
- 159: must not fall into `json` branch below to become "data" (#488).
- 354: Material validator is "any valid JSON", not `SceneDoc` (#488).
- 386: Scene document write must be readable back as-is (rollback on truncation / bad write).
- 419: Windows rename can't overwrite existing target, so previous step moved original file away.
- 428: Read-back validation: what was written must parse back into `SceneDoc`, otherwise rollback.
- 482: #466: Asset identity is in `<path>.rmeta`, sidecar must move along on rename,
- 483: otherwise GUID points to nonexistent old file, zero-rewrite references fail to resolve.
- 599: On Windows `assets/C:/x` `C:` is just a regular component, block separately.
- 625: Half-written form: strip trailing `.json` and `.scene` / `.prefab`, then complete.
- 658: #495 item 5: version stamp includes length, same-second overwrites can still be detected.
- 683: #864: when mtime precision insufficient, same-length overwrites must still be detected via digest.
- 742: Unknown fields must be preserved as-is (hand-edited materials must not be swallowed by editor).
- 757: Only assert "current directory not mistakenly flagged as root" (temp dir itself has no assets/).
- 850: target already exists
- 858: allowed to move to new directory (auto-create).
- 870: nested allowed
- 939: #459: missing suffix (or partial) must still be completed.
- 960: Path traversal and absolute paths all rejected.
- 977: Derived files don't generate sidecar.
- 1003: second time no rewrite, GUID stable.

## editor/crates/prism-editor-core/src/recovery.rs

- 52: Snapshot is the last insurance, must really flush to disk before rename.
- 78: Directory may contain user / other process files, delete only when empty.

## editor/crates/prism-editor-core/src/render_graph.rs

- 219: On old report / missing field status is None, panel shows placeholder not empty string.
- 239: Old report has no stats: whole section hidden, do not fake with 0 values.

## editor/crates/prism-editor-core/src/schema.rs

- 205: RGBA / color array uses color block + components.
- 589: Non-existent path does not create node.

## editor/crates/prism-editor-core/src/settings.rs

- 538: Same-length / same-second overwrites also need detecting (compare content, not mtime).
- 658: Write side only outputs settings.ini.

## editor/crates/prism-editor-core/src/state.rs

- 226: Panels may have been deleted (old archive / panel renamed); pointing to a missing tab
- 227: would blank the whole dock area, so filter against the current dock once.
- 286: #418: presets live in editor state, reopen editor after and they're still there.
- 321: Archived panel no longer exists (renamed / layout changed): pointing at it must not
- 322: blank out the whole dock area.
- 404: Old archive without `scene` section: defaults (outline defaults on).
- 415: Has `scene` section but missing `showSelectionOutline` (written by old version): default on.
- 466: Unknown panel is dropped, falls back to default layout (at least sceneView present).

## editor/crates/prism-editor-core/src/sync_marker_view.rs

- 300: 0..200px -> 0.25 at 50px, 0.75 at 150px.

## editor/crates/prism-editor-core/src/textinput.rs

- 225: If cursor is at whitespace/punct, skip it first, then skip whole word; otherwise skip only current word.
- 260: Finish current word first, then eat following whitespace/punctuation, stop at next word start.

## editor/crates/prism-editor-core/src/theme.rs

- 371: bg
- 372: bgSunken
- 373: panel
- 374: panelAlt
- 375: panelRaised
- 376: chrome
- 377: border
- 378: borderStrong
- 379: borderSubtle
- 380: text
- 381: textStrong
- 382: textSoft
- 383: muted
- 384: mutedDim
- 385: rowHover
- 386: menuHover
- 387: menuActive
- 388: selection
- 389: accent
- 390: accentSolid
- 391: onAccent
- 392: accentInk
- 393: accentAlt
- 394: success
- 395: successSoft
- 396: successBg
- 397: successBorder
- 398: danger
- 399: dangerSoft
- 400: dangerBg
- 401: dangerBgSubtle
- 402: dangerSolid
- 403: dangerBorder
- 404: onDanger
- 405: warning
- 406: warningStrong
- 407: warningBorder
- 408: panic
- 419: bg
- 420: bgSunken
- 421: panel
- 422: panelAlt
- 423: panelRaised
- 424: chrome
- 425: border
- 426: borderStrong
- 427: borderSubtle
- 428: text
- 429: textStrong
- 430: textSoft
- 431: muted
- 432: mutedDim
- 433: rowHover
- 434: menuHover
- 435: menuActive
- 436: selection
- 437: accent
- 438: accentSolid
- 439: onAccent
- 440: accentInk
- 441: accentAlt
- 442: success
- 443: successSoft
- 444: successBg
- 445: successBorder
- 446: danger
- 447: dangerSoft
- 448: dangerBg
- 449: dangerBgSubtle
- 450: dangerSolid
- 451: dangerBorder
- 452: onDanger
- 453: warning
- 454: warningStrong
- 455: warningBorder
- 456: panic
- 467: bg
- 468: bgSunken
- 469: panel
- 470: panelAlt
- 471: panelRaised
- 472: chrome
- 473: border
- 474: borderStrong
- 475: borderSubtle
- 476: text
- 477: textStrong
- 478: textSoft
- 479: muted
- 480: mutedDim
- 481: rowHover
- 482: menuHover
- 483: menuActive
- 484: selection
- 485: accent
- 486: accentSolid
- 487: onAccent
- 488: accentInk
- 489: accentAlt
- 490: success
- 491: successSoft
- 492: successBg
- 493: successBorder
- 494: danger
- 495: dangerSoft
- 496: dangerBg
- 497: dangerBgSubtle
- 498: dangerSolid
- 499: dangerBorder
- 500: onDanger
- 501: warning
- 502: warningStrong
- 503: warningBorder
- 504: panic
- 515: bg
- 516: bgSunken
- 517: panel
- 518: panelAlt
- 519: panelRaised
- 520: chrome
- 521: border
- 522: borderStrong
- 523: borderSubtle
- 524: text
- 525: textStrong
- 526: textSoft
- 527: muted
- 528: mutedDim
- 529: rowHover
- 530: menuHover
- 531: menuActive
- 532: selection
- 533: accent
- 534: accentSolid
- 535: onAccent
- 536: accentInk
- 537: accentAlt
- 538: success
- 539: successSoft
- 540: successBg
- 541: successBorder
- 542: danger
- 543: dangerSoft
- 544: dangerBg
- 545: dangerBgSubtle
- 546: dangerSolid
- 547: dangerBorder
- 548: onDanger
- 549: warning
- 550: warningStrong
- 551: warningBorder
- 552: panic
- 563: bg
- 564: bgSunken
- 565: panel
- 566: panelAlt
- 567: panelRaised
- 568: chrome
- 569: border
- 570: borderStrong
- 571: borderSubtle
- 572: text
- 573: textStrong
- 574: textSoft
- 575: muted
- 576: mutedDim
- 577: rowHover
- 578: menuHover
- 579: menuActive
- 580: selection
- 581: accent
- 582: accentSolid
- 583: onAccent
- 584: accentInk
- 585: accentAlt
- 586: success
- 587: successSoft
- 588: successBg
- 589: successBorder
- 590: danger
- 591: dangerSoft
- 592: dangerBg
- 593: dangerBgSubtle
- 594: dangerSolid
- 595: dangerBorder
- 596: onDanger
- 597: warning
- 598: warningStrong
- 599: warningBorder
- 600: panic
- 611: bg
- 612: bgSunken
- 613: panel
- 614: panelAlt
- 615: panelRaised
- 616: chrome
- 617: border
- 618: borderStrong
- 619: borderSubtle
- 620: text
- 621: textStrong
- 622: textSoft
- 623: muted
- 624: mutedDim
- 625: rowHover
- 626: menuHover
- 627: menuActive
- 628: selection
- 629: accent
- 630: accentSolid
- 631: onAccent
- 632: accentInk
- 633: accentAlt
- 634: success
- 635: successSoft
- 636: successBg
- 637: successBorder
- 638: danger
- 639: dangerSoft
- 640: dangerBg
- 641: dangerBgSubtle
- 642: dangerSolid
- 643: dangerBorder
- 644: onDanger
- 645: warning
- 646: warningStrong
- 647: warningBorder
- 648: panic

## editor/crates/prism-editor-core/src/thumbnails.rs

- 268: Rewriting the file invalidates the entry (different length).

## editor/crates/prism-editor-core/src/undo.rs

- 228: The last element of the redo stack is the next step to redo, so reversing the whole
- 229: thing places it right after the undo steps in chronological order.
- 249: Clearing history must not silently wash the dirty flag: a clean document stays clean
- 250: after clearing history, but a dirty document stays dirty just because the history is gone.
- 624: Undo back to the initial 60.
- 778: During interaction: an 800ms+ pause still coalesces into one step.
- 789: Release then drag again immediately: must open a new step even though interval < 800ms.
- 858: Undoing then making a *different* edit at the same depth: stack length is 1 again,
- 859: but content differs from save point.

## editor/crates/prism-editor-core/src/viewport_math.rs

- 111: Ring lies in plane perpendicular to axis; radius approximated by average of two orthogonal radius screen lengths.
- 376: Rotation matrix taken by column: `m[c][r]` consistent with below `m00..m22` row-major indexing.
- 384: Gimbal lock: when Y near ±90° X/Z couple, move roll to Z (Unity also takes 0).
- 395: `R = Rz * Ry * Rx`: rotate X first, then Y, then Z (fixed axes).
- 623: Use real local AABB if available: unit cube differs from 2D sprite by a whole
- 624: "texture pixel size", framing sticks to face (#991).
- 641: When local center non-zero (sprite pivot / collider center) first carried to world by rotation-scale.
- 682: Radius takes "center to farthest corner" not AABB half-diagonal: latter grows with rotation,
- 683: same object turns and framing distance changes.
- 737: Axis projected length is 0 when facing camera, cannot click.
- 770: Derive orthonormal basis directly from yaw/pitch to avoid gimbal collapse at +-90 deg.
- 833: Orthographic: screen position independent of depth, half-height is world units.
- 874: Orthographic: all rays same direction, origin spread on near plane by half-width / half-height.
- 1342: Corner index arranged as "dx/dy/dz each one bit": z changes fastest.
- 1347: along z
- 1351: along y
- 1355: along x
- 1680: half-height = distance = 10 -> 30px per world unit in 600px-high viewport:
- 1681: +1 X right moves 30px, +0.5 Y lifts 15px.
- 1703: Center ray hits box; the one shifted horizontally 10 world units misses directly
- 1704: (orthographic no near-large-far-small, does not hit just because "closer to camera").
- 1722: half-height 10 -> 30px per world unit in 600px-high viewport.
- 1733: bottom-edge pivot `[0.5, 0.0]`: rect overall above entity by half height.
- 1738: screen y down: lifting half world unit = screen y decreases 15px.
- 1767: 2D: front-facing XY, 30px/world unit, screen (430, 285) = world (+1, +0.5).
- 1779: 3D: top-down center ray lands on ground at origin (target).
- 1793: Default camera looks horizontally at −Z, parallel to y = 0 -> no landing point.
- 1796: Parallel rays / rays flying away from plane all rejected; vertically downward ray normally projects to ground.
- 1826: eye at +Z 10; further forward is +Z 20 (behind camera).
- 1838: Offset to screen corner does not hit.
- 1866: Right shift hits second one.
- 1874: #416: narrow FOV pushes same world offset closer to screen edge.
- 1887: Picking ray and projection are inverses: same screen point ray direction consistent.
- 1896: #416: candidate set sorted by distance, first click picks nearest.
- 1952: Thin plate 2 × 0.1 × 2 rotated 45° around Z: visually slanted, axis-aligned box still upright.
- 1959: Empty point in axis-aligned box (old impl would mis-select) now must miss.
- 1964: Real end of thin plate (old impl instead missed it).
- 1975: One 40 × 40 wall, projected covers whole viewport, but four corners all outside viewport.
- 1995: Box far from wall still selects nothing.
- 2042: Y = 90° is gimbal lock: only guarantees overall rotation consistent, not component consistent.
- 2047: Degenerate quaternion must not produce NaN.
- 2049: Non-normalized input normalized first.
- 2060: Screen pixel length fixed; taller viewport means same pixel corresponds to smaller world length.
- 2081: Away from all axes -> no hit.
- 2095: Composition: rotate X 90° then Y 90°.
- 2151: Rotate Y 90°: X -> −Z, Y unchanged, Z -> X.
- 2221: Viewport left has hierarchy panel origin non-zero (#422): screen coordinates must subtract it first.
- 2236: Unit cube at origin, camera backed off along +Z by 10: center should fall near screen center.
- 2242: Box behind camera not drawn (otherwise draws lines penetrating screen).
- 2284: Unit cube: half-diagonal = sqrt(3)/2 ≈ 0.866.
- 2292: Parent scale enlarges bounding box.
- 2300: Rotation does not change cube bounding sphere radius (only changes orientation).
- 2325: 64×16 texture @1 ppu -> 32×8 half size, then add ±0.05 thickness.
- 2332: Falls back to unit cube when `frame_size` not written (only engine knows whole texture size).
- 2428: Regression #403: old Tauri impl divided slab param again by |M⁻¹·dir|,
- 2429: causing returned distance proportional to entity scale (scale 4 -> distance ×4).
- 2449: Two boxes overlapping at same screen point: near large box (scale 4, front surface z = 2)
- 2450: must beat far small box (scale 1, front surface z = -9.5).
- 2520: Camera is at z = +10 looking down -Z; point z = +12 is behind camera, z = 0 is in front.

## editor/crates/prism-editor-core/src/workspace/asset_ops.rs

- 99: `rewrite_moved_references` runs after the filesystem move, so the
- 100: assets are already under `to`; match both sides for robustness.
- 227: Every project document failed to parse. Keep an empty scene instead
- 228: of the sample fixture: otherwise the hierarchy and the Game view
- 229: show the editor's demo scene while the status bar says the project,
- 230: which reads as "my project looks like this" (#1020).

## editor/crates/prism-editor-core/src/workspace/mod.rs

- 788: Template documents (prefabs + scenes). Replace defaults if fixture supplies any.

## editor/crates/prism-editor-egui/src/app/chrome.rs

- 104: ---- File ----
- 262: ---- Edit ----
- 360: ---- Assets ----
- 485: ---- GameObject ----
- 590: ---- Component ----
- 630: ---- Window ----
- 756: ---- Help ----
- 943: Segmented gizmo tool pill group matching UE5 / Unity 6 tool strips.
- 1574: Reload engine from the recovered document on the next sync pass.

## editor/crates/prism-editor-egui/src/app/dock_ui.rs

- 142: Paint background before close hover indicator by deferring close glyph below.
- 169: Top 2px accent bar matches UE5 / Unity 6 active tab chrome.

## editor/crates/prism-editor-egui/src/app/shortcuts.rs

- 122: Console Clear on Play runs before Play starts so initial runtime logs stay visible.
- 132: Write current preview snapshot so Game view starts from clean disk state.
- 191: Play edits were discarded by `workspace.stop()`; flush snapshot before reloading.
- 200: Stop must clear the live runtime immediately; do not wait for sync_document.
- 438: Keep the control protocol locale-independent. The toolbar may use
- 439: localized labels, but MCP clients need stable enum-like values.

## editor/crates/prism-editor-egui/src/app.rs

- 815: Keep Play isolated from structural document edits until Stop restores the snapshot.
- 2074: PCM
- 2075: mono
- 2078: block align
- 2079: bits

## editor/crates/prism-editor-egui/src/build.rs

- 81: Only one implementation (data layer), player build and ScenePack both take from here.
- 166: Player build reuses data layer `BuildPlan` (profile / output_dir / product layout all there),
- 167: here only translates its steps to cargo params.
- 277: cargo occasionally mixes non-JSON lines; JSON events already covered by diagnostics / summary.
- 341: whether this step is structured diagnostic mode (only `cargo build` has this param).
- 346: diagnostic span relative to workspace root (parent dir of `--manifest-path`).
- 355: one read thread each for stdout / stderr, guarantee both outputs do not starve each other.
- 376: kill only sends signal; below wait() reaps, read thread also waits.
- 403: if previous step fails, do not run subsequent steps (e.g. cook failure should not package old scene).
- 411: Player build: place exe / `assets/` into `[build].output_dir`, run product directly when needed.
- 625: Real project cooked once has these products; product list expands by existence; test also needs this premise.
- 670: User project without `projects/template` falls back to its own manifest.
- 695: Package step uses real repo root (temp root has no pipeline workspace).

## editor/crates/prism-editor-egui/src/control.rs

- 162: `Drop` sets flag then connects to this port, to wake up blocked accept here.
- 196: Wake up accept thread so listener truly closes (only deleting port file leaves port bound,
- 197: bound, reopening old connections queue into unprocessed queue).
- 257: UI thread replies within 5s max; timeout means editor stuck / not running UI loop.
- 388: accept thread exits only after receiving wake connection; give it a short time.

## editor/crates/prism-editor-egui/src/egui_host/renderer.rs

- 391: Free retired resources from the previous use of this frame slot.
- 447: Upload texture deltas before starting the render pass.
- 450: Prepare merged vertex and index buffers for all clipped meshes.
- 632: Retire freed textures to the current frame slot so they are destroyed
- 633: only after `fence` signals on the next cycle.
- 648: Defer freeing until this frame slot's fence completes so in-flight draws stay valid.

## editor/crates/prism-editor-egui/src/engine_host.rs

- 119: In 2D view XZ mesh is a line seen edge-on (XY mesh see #602), don't draw first.
- 455: All entity handles of old world invalidated: project module discards cached handles by this generation.
- 457: All entity handles of old world invalidated, helper entities need recreating.
- 460: Asset parser (with already-resolved `.pak` and uploaded handles) reused across reloads: previously each
- 461: edit called `GpuAssetResolver::new()` + re-read pak (#435). Only re-read manifest when pak file
- 462: itself was rebuilt.
- 482: Loader only guarantees root `LocalToWorld`; propagate full hierarchy before first frame.
- 610: Keep the rendered camera on the same orbit as `OrbitCamera::eye()`:
- 611: yaw is world-Y, while positive pitch raises the eye toward +Y.
- 631: 2D view: orthographic half height directly reuses `distance`, wheel zoom feel unchanged (#641).
- 636: Editor "View..." settings (#416): FOV and clip planes apply to both render and picking,
- 637: otherwise gizmos / point selection on screen won't match image.
- 646: Animation graph debug snapshot only maintained during Play (#560 editor-runtime bridge).
- 661: On stopping Play discard transient key / mouse state: otherwise next Play
- 662: would inherit previous "just pressed".
- 692: After size change editor camera aspect must follow.
- 829: ECS UI layout depends on `ScreenSize`; offscreen host has no winit window, writes it itself.
- 843: A paused step advances exactly one simulation tick. Do not
- 844: consume wall-clock time accumulated while the editor was idle.
- 881: User project gameplay code runs before engine systems: components it writes
- 882: and UI quads seen by hierarchy / ui::render in same frame.
- 894: Scene change requested by project code executed after world borrow ends (#482).
- 901: Edit mode (or paused): don't run game systems, only propagate layer level.
- 936: On open failure (pak being written) wait next round, don't swap reader to half file.
- 970: Project-bundled fonts (#583): read bytes from `.pak` hand to UI atlas. Only on new path
- 971: read once, same failure-cache strategy as textures / meshes.
- 977: Skeletal animation assets (#560 P0-2): idempotent, only new `.rskel` / `.ranim` read from disk.
- 984: Invalidated GPU resources released synchronously (editor has no render thread, #495).
- 1001: Edit mode doesn't run `tick()` (only called during Play), but producing a frame also needs
- 1002: those two "end of simulation step" steps: layer propagation + asset upload. Otherwise `LocalToWorld` stays at load value,
- 1003: and `SpriteRef::generation == 0` makes 2D render system filter out all sprites —
- 1004: edit-mode Scene / Game view would show only clear color (#579).
- 1040: Edit mode does not run update(); recompute UI layout for Scene anchor handles.
- 1044: #613: internal render resolution follows same path as game process, pixel ratio seen by
- 1045: editor Game view is actual device ratio.
- 1055: #978: sub-pixel lines widened to 1 pixel by actual render height.
- 1060: Local post-processing volume (#502 Phase 2): shares same parse function with player path.
- 1100: #502: LUT strength follows global setting, path resolved under assets/.
- 1143: Editor only redraws on "changes", so each render treated as fresh accumulation start.
- 1172: Keep headless/window fence signaled cleanly when a render pass aborts.
- 1183: Screenshot must read offscreen image current layout (frame end is `COLOR_ATTACHMENT_OPTIMAL`),
- 1184: so must do before transitioning to sample layout.
- 1192: COLOR_ATTACHMENT_OPTIMAL → SHADER_READ_ONLY_OPTIMAL: egui sampling descriptor requires
- 1193: latter, and step happens on same queue, no extra semaphore.

## editor/crates/prism-editor-egui/src/gpu.rs

- 55: Temporary surface: only for "physical device / queue family can present". egui-ash builds
- 56: surface + swapchain per viewport window itself; used then destroyed here.
- 78: reclaim temporary surface first to avoid leak then return error.
- 101: handle already obtained, temporary surface no longer needed.
- 109: Consistent with features enabled by `VulkanContext::from_host`: device address only
- 110: declared when actually enabled, otherwise gpu-allocator requests unenabled feature.

## editor/crates/prism-editor-egui/src/logging.rs

- 56: Only send levels "editor itself cares about" to panel, Trace would flood window.
- 86: Placeholder first, avoid recursive initialization.
- 114: Keep original behavior (stderr / RUST_BACKTRACE), terminal troubleshooting still usable.
- 138: Capture stack in hook: unwind not started yet, frames complete (including project module frames).

## editor/crates/prism-editor-egui/src/loose_assets.rs

- 44: Hidden files / temp files (vim, VSCode atomic save, system thumbnail cache).
- 197: No change: never trigger.
- 201: File changed -> only record pending, do not trigger immediately.
- 207: Quiet for DEBOUNCE -> trigger once, then return to idle.
- 212: Continuous writes re-time debounce.
- 220: New file / deletion also counts as change.

## editor/crates/prism-editor-egui/src/main.rs

- 63: Global control channel switch: `--no-control` builds no listener, writes no `control.port`,
- 64: instead of rejecting each message after accept --- otherwise agent sees "connected but nothing works".

## editor/crates/prism-editor-egui/src/palette.rs

- 46: Starts from empty query each open: previous search term usually useless.

## editor/crates/prism-editor-egui/src/panels/console.rs

- 331: CREATE_NO_WINDOW

## editor/crates/prism-editor-egui/src/panels/hierarchy.rs

- 766: Hierarchy keeps a lightweight ghost under the pointer while dragging.
- 767: It makes the operation legible even when the pointer is between rows or over
- 768: the empty tail of the scroll area.

## editor/crates/prism-editor-egui/src/panels/inspector.rs

- 3453: 960×16 sheet with 64×16 frames = 15 cells in one row.
- 3474: 100 is not a multiple of 30, so the last column would be clipped.

## editor/crates/prism-editor-egui/src/panels/settings_and_tools.rs

- 383: ---- Add scenes that exist on disk but are missing from the manifest ----
- 407: ---- The active document may not be part of the build yet ----

## editor/crates/prism-editor-egui/src/panels/tests.rs

- 65: Isolate appears before the lock/find buttons, so re-read their rects
- 66: after every state-changing click.
- 73: The new Show All button shifts the following controls; lay out one
- 74: more frame before reading their rects.
- 110: The popup opens below the combo: Name first, Type second.
- 122: Tree order is camera, cube, cube/Detail, light.
- 142: A Scene/Project selection change must not move the locked highlight.
- 146: Clicking inside the hierarchy re-syncs the lock to the new selection.
- 827: Frame 1: pointer press inside inspector panel initiates interaction bracketing.
- 844: Frame 2: held drag keeps interaction open across edits (even outside panel rect).
- 856: Frame 3: pointer release commits interaction so the next edit is a distinct step.

## editor/crates/prism-editor-egui/src/project_module.rs

- 196: Release voices that finished and aren't looping, avoid id table growing unbounded.
- 251: Note: `ProjectAudio` must be created when **constructing** driver, cannot create in module tick.
- 252: `AudioEngine::new` inside tick (opens cpal stream) would freeze editor — world is
- 253: borrowed by module, once audio callback thread starts UI thread can't get control back. Create once at construction is safest.
- 421: Source changed but not compiled (when Auto Refresh off hint user press Ctrl+R);
- 422: only stable Failed if source hasn't changed again after failure.
- 505: Module already judged faulty this frame: wait for reload, don't jump into same hole again.
- 509: Field-level borrows: session and components/assets don't overlap.
- 518: Project save (#984): Play save under `<project>/target/play-data/`,
- 519: separate from player's real save; don't declare feature when no writable dir.
- 524: One-shot audio (#987): when no sound card `ProjectAudio::new` returns `None`, feature
- 525: not declared, project code gets `play_sound() == None` rather than silent success.
- 530: Project code can call `GameWorld::save_scene_state()` (#484); engine is here,
- 531: runtime-host only holds function pointer.
- 542: Scene change request can't execute in callback (world borrowed), first store for host (#482).
- 547: Stop here after one crash: project panicking each frame would burn stderr and GPU time,
- 548: only reloading module (save source / Ctrl+R) makes it try again.
- 623: ------------------------------------------------------------------
- 624: compile  + load
- 625: ------------------------------------------------------------------
- 632: Prebuilt module preferred: only try once, on failure stay in error state (Ctrl+R won't reinstall).
- 651: During Play don't auto-swap module: whole reload would drop project state (no script layer to migrate).
- 652: Placed before source scan, avoid traversing `src/` every frame.
- 679: Record timestamp regardless of outcome: syntax error shouldn't make editor rerun cargo every 350ms.
- 758: First swap session then delete old copy: old code pointers must stay valid until new module takes effect.
- 767: Make next frame rescan immediately: source changed during compile shouldn't be treated as already compiled.
- 804: ---------------------------------------------------------------------------
- 805: input / UI quads
- 806: ---------------------------------------------------------------------------
- 853: Quads submitted by project module are solid-color only, no background image (#591).
- 867: ---------------------------------------------------------------------------
- 868: Loose assets
- 869: ---------------------------------------------------------------------------
- 909: ---------------------------------------------------------------------------
- 910: compile
- 911: ---------------------------------------------------------------------------
- 922: Structured diagnostics: Problems panel needs file:line:col, not a colored stderr blob.
- 930: CREATE_NO_WINDOW
- 947: First error is what user needs to fix; only fall back to stderr when no compiler diagnostics (link errors etc).
- 998: Uniqueness on **directory**, filename must stay: dbghelp finds symbol file by name recorded in PE
- 999: (`fishing.pdb`) in module directory, won't infer from `fishing-<nanos>.dll`.
- 1000: Renamed dll makes project frame symbols degrade to export names (`prism_project_get_api`),
- 1001: panic call stack would be wasted.
- 1015: Symbol file in same directory as dll: without it project frames only have addresses.
- 1062: ---------------------------------------------------------------------------
- 1063: Manifest / source mtime
- 1064: ---------------------------------------------------------------------------
- 1229: `begin_frame` clears just_pressed, but held state must remain:
- 1230: fishing uses `input.key(keys::SPACE)` to detect continuous line retrieval.
- 1278: On frame window opens: held bitmap cleared, release edge set.
- 1279: Host calls `begin_frame` at start of each frame, do same here, else previous frame's press-below edge remains.
- 1316: When fully "ended" don't count as having touch point pressed again.
- 1380: Intro raindrops submitted via `GameWorld::draw_quad`; able to come back across ABI proves
- 1381: component read/write and UI quad channel both work.
- 1450: Failure also records compile timestamp: source unchanged shouldn't rerun cargo every 350 ms.
- 1458: After changing back source (or manual rebuild), next compile period starts fresh.

## editor/crates/prism-editor-egui/src/screenshot.rs

- 165: Desktop DC only used to build compatible DC / DIB, DIB itself is device-independent.
- 172: Negative height = top-to-bottom, rows read back are screen order.
- 199: PrintWindow is async GDI call, must flush before reading DIB.
- 207: DIB is BGRA; PNG needs RGBA.

## editor/crates/prism-editor-egui/src/selection_history.rs

- 72: Navigation itself is not "new selection": next frame `observe` sees same state and returns directly.
- 148: Simulate observe at end of next frame (sees exactly navigation target).
- 162: Oldest 20 dropped, earliest can undo back to index 20.

## editor/crates/prism-editor-egui/src/skeleton_overlay.rs

- 41: world = entity_local_to_world * palette * bind, bind = inverse_bind^-1.
- 88: bind at +1m with identity palette places the joint at (1, 0, 0).
- 126: Shorter palette clamps to the shortest input length.
- 136: NaN joints are excluded from bone segments.

## editor/crates/prism-editor-egui/src/test_harness.rs

- 1: Helper surface: key_press/context stay available for future scenarios.

## editor/crates/prism-editor-egui/src/theme.rs

- 47: ---- surfaces ----
- 57: Web `.menu-popup` shadow: 0 6px 18px rgba(0,0,0,.55)
- 75: ---- controls ----
- 76: Note: egui default `weak_bg_fill` (button / selectable label) is blueish gray,
- 77: here all changed to neutral gray in tokens; blue appears only in `active` (pressed / selected).
- 117: ---- metrics ----
- 134: Default handle takes `fg_stroke` (text gray) -> scrollbar turns bright white; change to `bg_fill`.
- 199: Windows
- 201: Linux (Web editor uses system UI font, pick closest here)
- 205: macOS
- 215: Android / Termux
- 219: Linux
- 222: Windows
- 225: macOS
- 237: Phosphor Icons (MIT/Apache-2.0) as icon glyphs, inserted after body UI font,
- 238: so constants `PLAY` / `EYE` / `FOLDER` can be drawn directly as text.

## editor/crates/prism-editor-egui/src/viewport/gizmos.rs

- 902: Inner mid-latitude ring (r = 0.58 R) + outer rim ring (r = 1.0 R).

## editor/crates/prism-editor-egui/src/viewport/mod.rs

- 753: 3) gizmo / selection overlays.
- 909: Latch fly mode when RMB starts inside viewport so cursor exits stay active.
- 1196: Cube + Cube/Detail
- 1589: Point handle at (200, 100) → drag to (250, 125).
- 1599: Stretch rectangle: right-edge handle at (300, 100) → (250, 100).

## editor/crates/prism-editor-egui/src/widgets.rs

- 122: Component order is part of document (#512): these two arrows take one undo step.
- 144: `…` menu (#418): Copy / Paste Values / order / Reset / Remove.
- 145: Paste enabled state judged by caller at apply time (clipboard may not be this type).
- 165: Reset writes back schema defaultJson (whole component = one undo step).
- 180: Component preset (#418): save current value / apply existing preset.
- 266: #583: UI text can use project bundled font.
- 298: Editing: every keystroke does not touch document.
- 307: Lose focus = submit (click elsewhere case).
- 312: Esc discards, even if Enter pressed same frame.
- 317: Enter in multi-line box is newline, only blur submit.
- 340: Stretch changes only one axis, other axis keeps current center.
- 351: Highlight judgment: horizontal stretch cannot count "full stretch" as itself.
- 357: Keys written back to JSON must be snake_case in schema.
- 455: #418: pasted value does not type-cast, button stays disabled when shape inconsistent.
- 488: #418: `+ Add` default value comes from element schema, does not guess JSON.
- 511: Recursion has limit, corrupted schema will not stack overflow.
- 527: Out-of-bounds / in-place move does not move array.
- 751: Multi-line text Enter is newline, only blur submit.
- 794: Not editing (or focus moved after a handled commit): mirror the document value.
- 1122: `Option<T>`: a checkbox says whether the value exists, then the inner
- 1123: scalar is edited in place. Authors used to have to hand-edit JSON to
- 1124: set `Style.width` / `Style.height`.
- 1151: Nested struct (`Style.anchors`, `Style.pivot`, `Margin` …): recurse
- 1152: through the registry so every leaf keeps its real widget and path.
- 1154: UI anchor: add Unity `RectTransform` style preset grid above numeric field (#429).
- 1287: Stretch preset only guarantees "other axis is point anchor", center value changes with current value.
- 1458: List defaults empty; `Option` defaults absent.
- 1524: element path = `field[i]`; struct subfields again by `field_row_at`
- 1525: Concatenate into `field[i].child`.

## launcher/src-tauri/src/game_launcher.rs

- 102: Still alive: do not launch second one.
- 104: Already exited or query failed: clear handle, allow relaunch.
- 124: Do not swallow game output: only connect stdin to null, stdout/stderr inherit launcher console.
- 167: Repo: <root>/repo/{Cargo.toml, projects/template/Cargo.toml}

## launcher/src-tauri/src/lib.rs

- 18: ==================== Android: custom  Native Plugin ====================
- 19: 
- 20: Call Android native API via Kotlin:
- 21: - toggle_immersive / set_status_bar_color / vibrate / get_device_info
- 22: - launch_game: start GameActivity
- 77: Desktop stub impl: let commands compile on desktop, return hint when called.

## launcher/src-tauri/src/main.rs

- 1: Prevents additional console window on Windows in release, DO NOT REMOVE!!

## projects/demos/echowell/src/game/platformer.rs

- 17: Function keys use the project API's raw virtual-key slots.
- 851: Allow wall jump at wall; true collision direction determined by this frame horizontal parse.
- 862: Jump hold keeps phase with halved gravity; releasing jump immediately cuts upward speed.
- 1005: Preserve forward momentum while pushing the player clear of the
- 1006: hurtbox so one dash cannot repeatedly hit the same enemy.
- 1098: Physics stores the player position at the body centre. Keep the
- 1099: sprite on the same centre and scale the source frame to the body
- 1100: bounds instead of compensating with a foot pivot.
- 1224: The pickup AABB is 16x16 and is centred on `geometry.crystal`.
- 1225: The source art is 16x20, so fit the quad to the gameplay bounds.
- 1846: Keep simulation coordinates on the collision-box centre.
- 1850: Keep hot-reloaded player dimensions aligned with the
- 1851: 16x24 source frame used by the sprite sheet.
- 2269: Platforms are drawn only by the tilemap, so every solid that acts as a
- 2270: platform (not the floor or the side walls) must have a matching tile.

## projects/demos/emergence/src/commands.rs

- 296: Noon by default
- 388: 20 ticks/sec = 20-minute full Minecraft day cycle.
- 408: Apply active weather stimuli (rain wets exposed blocks; thunder occasionally strikes).
- 434: 0 = sunrise (06:00), 6000 = noon, 12000 = sunset, 18000 = midnight.
- 436: 0 = east horizon, PI/2 = zenith, PI = west horizon
- 461: Primary directional light follows the sun by day and switches to the
- 462: moon above the horizon at night so shadows + GI stay active 24h.
- 476: Exact inverse of `prism_engine::euler_xyz_deg_to_dir`:
- 477: +X pitch lifts light vector toward +Y (above horizon).
- 616: Home
- 622: End
- 628: PageUp
- 634: PageDown
- 644: Backspace (VK_BACK = 0x08) and Delete (VK_DELETE = 0x2E).
- 712: Gather visible messages newest-at-bottom, oldest-above.
- 815: Sync input bar and command suggestions when console is open.
- 1586: Verify full time-of-day lighting: noon -> high warm sun, midnight -> cool moonlight above horizon.
- 1635: F1 no longer opens the command console.
- 1639: `T` opens console with an empty buffer and does not inject 't'.
- 1644: `Escape` cancels and closes console.
- 1648: `Enter` when closed opens console empty and does not execute immediately.
- 1655: `/` opens console prefilled with `/`.
- 1678: Open with `T` and type `/ti` + Tab -> `/time`, then ` set day` -> Enter.
- 1691: Cursor navigation + editing: add `x` at start via Home, then Delete it.
- 1739: Open with `Enter`, test relative tilde (`Shift + backquote`) + minus + dot, then chat msg.
- 1759: Sync chat UI spawns pooled bottom-left entities and ages out unfocused lines after 10s.
- 1778: `/clear` clears the chat log.

## projects/demos/emergence/src/debug_hud.rs

- 184: SAFETY: Both structs have their exact byte-size headers initialized and valid writable pointers.

## projects/demos/emergence/src/grid.rs

- 147: Bottom-up 2-phase parity sweep avoids double-moving a cell in one pass.

## projects/demos/emergence/src/lib.rs

- 271: Draw probe selection outline.
- 446: Distinct geological & ore palette for terrain and biome slices.
- 450: bedrock
- 456: diamond_ore
- 461: redstone_ore
- 466: gold_ore
- 471: coal_ore
- 477: iron/copper ore
- 481: magma_lava
- 485: sand
- 488: snow_block
- 494: grass_block
- 497: ice_block
- 500: dirt
- 506: deepslate
- 511: stone
- 719: Bootstrap starts with F3 overlay and chat console hidden.
- 729: Pressing `T` opens chat console and does not flip player TierMode.
- 738: Pressing `F3` while typing in chat must not toggle the debug overlay.
- 743: Close chat with `Escape`, then press `F3` to toggle overlay on and back off.
- 756: Top-center tooltip activates when aiming at blocks and [B] toggles NEI only when chat is closed.
- 794: Close NEI if open by default so only the command console controls gating.
- 797: Open command console with `T`.
- 809: Hold `W` and `SPACE` while console is open — player must not move horizontally or jump.
- 826: Close console with `Escape` -> next frame camera input gate should lift.

## projects/demos/emergence/src/materials.rs

- 208: Dynamic TransientFlags visual projection (Iron Law 2 source of truth on parent L2 cell).

## projects/demos/emergence/src/nei_ui.rs

- 176: Check hotbar slot hover/click.
- 188: Check NEI palette hover/click.
- 220: Map cursor to world grid cell if not hovering UI.
- 254: 1. Aimed-at Inspector (top-left).
- 266: 2. Bottom 9-slot Hotbar.
- 289: 3. Right-side NEI Palette.
- 665: Only `CATALOG` `'static` strings reach here in practice.
- 975: Right click equips directly without triggering world placement.
- 1153: Spawn in strict painter order:
- 1154: 1. Top-center tooltip (`border -> bg -> swatch_border -> swatch -> texts`)
- 1165: 2. Bottom-center hotbar caption & 10 slots
- 1185: 3. Bottom-right NEI panel & 16 item slots
- 1245: 1. Top-Center WAILA / NEI Tooltip
- 1334: 2. Bottom-Center Hotbar
- 1468: 3. Bottom-Right Toggleable NEI Catalog Panel
- 1859: Aim at world geometry -> top-center tooltip populates from aimed voxel hit.
- 1868: Clicking an NEI slot consumes pointer input and equips active hotbar slot.
- 1887: Clicking ENT tab filters items and clicking [>] / [<] paginates cleanly.
- 1898: Switching [ALL] and clicking page next advances page.
- 1917: Syncing ECS UI activates tooltip, hotbar, and NEI panel without panics.
- 1928: Toggling B hides the NEI panel while keeping hotbar & top tooltip intact.

## projects/demos/emergence/src/pbr_materials.rs

- 63: 1. Conductive Wire — LabPBR Copper Block (drawn metallic wire).
- 68: 2. Superconductor Coil — Cryo-cooled metallic coil with subtle cyan glow.
- 76: 3. Water — BSL/Complementary-style refractive liquid (IOR 1.333).
- 85: 4. Reinforced Glass — LabPBR crown glass (IOR 1.52, crisp specular).
- 94: 5. Insulated Bulkhead — Composite ceramic-steel bulkhead.
- 99: 6. Volatile Fuel Drum — Industrial enamel-coated steel drum.
- 105: 7. Absorbent Foam — High-porosity LabPBR sponge (darkens & glosses when wet).
- 111: 8. Corrosive Acid — Bioluminescent/caustic refractive pool.
- 121: 9. Cryo-Spore Specimen — LabPBR packed-ice + subsurface glow lichen.
- 131: 10. Hyper-Neo Mutant — Wet biofilm organic SSS + bioluminescence.
- 140: 11. Piezo Crystal Node — LabPBR Amethyst / Quartz (IOR 1.544).
- 151: 12. Pressurized Valve Conduit — Brushed brass/steel industrial pipework.
- 156: 13. Overclocked Conveyor — Anisotropic rubber-steel track with amber chevrons.
- 163: 14. Radioactive Isotope — Heavy actinide metal with strong Cherenkov glow.
- 169: 15. Stasis Field Core — LabPBR Beacon/Conduit glass-metal resonator.
- 177: 16. Plasma Arc Cell — High-HDR-bloom plasma containment core.
- 185: 17. Bedrock — Indestructible basaltic floor (LabPBR rough dark stone).
- 189: 18. Stone — Classic Minecraft matte silicate rock.
- 194: 19. Deepslate — Stratified dark metamorphic slate with subtle anisotropy.
- 200: 20. Dirt — High-porosity earthy soil.
- 205: 21. Grass Block — LabPBR foliage/grass SSS + damp porosity.
- 211: 22. Sand — Granular quartz sand (darkens cleanly when wet).
- 216: 23. Sandstone — Compressed desert sedimentary rock.
- 221: 24. Glacial Ice — LabPBR refractive ice (IOR 1.31, crisp clearcoat).
- 231: 25. Snow — Powdery snow with LabPBR subsurface scattering.
- 237: 26. Magma Lava — Incandescent molten rock (BSL/SEUS-style HDR lava).
- 244: 27. Coal Ore — Matte carbon seams embedded in stone.
- 249: 28. Iron Ore — Raw metallic iron veins in silicate host rock.
- 254: 29. Copper Ore — LabPBR copper-malachite mineral veins.
- 259: 30. Gold Ore — High-reflectance LabPBR gold flecks in deep rock.
- 263: 31. Redstone Ore — Piezoelectric glowing crimson mineral.
- 269: 32. Diamond Ore — LabPBR high-IOR (2.417) crystalline carbon in deepslate.

## projects/demos/emergence/src/perf_report.rs

- 283: Walk up from executable and working directory looking specifically for the `emergence` crate root.
- 298: Fallback for standalone packaged builds with `<dir>/assets` next to the binary.
- 384: SAFETY: Valid mutable pointer to initialized repr(C) `SYSTEMTIME`.
- 414: Howard Hinnant civil-from-days algorithm in Z >= 0 domain.
- 788: World & display metadata
- 804: Statistics accumulators
- 910: Main-loop frame cadence is measured from wall clock between frame ticks (matching `timeline.frames`),
- 911: never mixed with the render thread's fence-inclusive `frame_time_ms`.
- 917: Deduplicate render-thread & Vulkan GPU timings strictly per completed `render_seq`.
- 980: World telemetry updates
- 1014: Maintain top-N slowest frames across the session.
- 1037: Aggregate 1-second timeline bucket (capped to keep output compact).
- 1237: Pull one final render stats snapshot if exit occurred before the next tick read it back.
- 1572: Launched from repo root (`cargo run --manifest-path projects/demos/emergence/Cargo.toml`).
- 1576: Launched directly from target/debug even with an unrelated cwd.
- 1615: Intentionally emit duplicate `render_seq` values (only 3 distinct GPU frames across 6 main ticks)
- 1616: to verify render-thread & GPU pass metrics are deduplicated strictly per completed Vulkan frame.

## projects/demos/emergence/src/player.rs

- 208: Look northeast from (6.5, 6.5) across water pond toward Research Outpost.
- 343: Use authored courtyard overlook on initial frame while snapping cleanly above terrain.
- 370: Push feet above ground if spawn overlapped solid terrain.
- 394: Only the centered crosshair stays permanently visible; status/probe readouts live in F3.
- 446: Look via mouse delta while locked, plus drag-look and arrow-key fallback when unlocked.
- 483: Keep any attached FlyCameraController locked to the player's heading while UI owns input.
- 491: Track unlocked RMB click vs drag-look so dragging does not spuriously trigger placement.
- 513: Toggle Walk / Fly with Tab or V.
- 519: Switch 3-tier voxel editing granularity: 1 = L1 (1m), 2 = L2 (0.25m), 3 = L3 (0.0625m), press again or T = Auto.
- 534: Smoothly glide camera eye height over stepped ledges without abrupt pops.
- 542: Hotbar slots via digits 4..=9, 0 or Q / scroll wheel.
- 571: Aiming raycast across L1/L2/L3 chunks & cursor wireframe update from live camera basis.
- 611: Interaction: Carve (Left Click), Place / Tool (Right Click), Quick Stimuli (F/E/R/G).
- 721: Depenetrate ONLY on the transition from Fly -> Walk so walking into walls
- 722: never triggers vertical climbing or floor jitter.
- 734: Walk mode kinematics with gravity, coyote time, jump buffer, and L2 step-up.
- 759: Compensate camera eye height so stepping up glides smoothly across render frames.
- 778: Respawn above ground only if player fell below bedrock floor.
- 823: Find the minimal vertical clearance so L2/L3 steps never overshoot or bounce.
- 847: Binary refine remaining gap cleanly up to surface face.
- 1019: Simulate falling onto the flat spawn courtyard (top at y = 4.0m) using a standalone world.
- 1028: Walking into the hand-placed research outpost must stop at its west wall (x = 12.0m).
- 1030: facing +X
- 1064: facing -Z
- 1076: Step W while facing -Z.
- 1088: Rotate right 90 degrees via unlocked RMB drag (e.g. Editor Game View) to face +X.
- 1117: Stepping W must now move in +X matching rotated camera angle.
- 1143: -X
- 1156: Externally rotate camera entity to look +Z (yaw = PI).
- 1177: Attach FlyCameraController facing +X (yaw = -PI/2) and verify physics & aiming follow it.
- 1210: Stand on the spawn courtyard aiming straight down at natural L1 COW terrain.
- 1228: Press `2` to override cursor to L2 (`0.25m`) sub-cell granularity on the same L1 block.
- 1243: Press `2` again to return to Auto and verify the outpost window mullion draws a 0.0625m L3 outline.
- 1248: Stand in front of the hand-placed outpost window grate (L2 `x = 55`, `y = 19..24`, `z = 52`).
- 1271: Build a wide 0.25m L2 step platform ahead (`x in 9.5..12.5, y in 4.0..4.25, z in 10.5..11.5`)
- 1272: on the clean open courtyard (`y = 4.0`).
- 1293: +X
- 1353: Switch back to Walk mode and step physics: position must NOT be clamped to 31.6m.

## projects/demos/emergence/src/sim.rs

- 133: 1. Unload distant chunks beyond `STREAM_RADIUS_CHUNKS + 1`.
- 145: 2. Collect missing chunks within `STREAM_RADIUS_CHUNKS`, sorted closest-first.
- 196: O(1) unload drops the chunk's HashMap and frees its bucket allocation immediately.
- 366: Iron Law 1: shattered glass/depleted cells with zero durability turn into empty void.
- 428: Brittle / Glass cells shatter neighbors when kinetically struck.
- 466: Iron Law 1: clearing the final L3 bit inside an L2 cell clears its L2 physics cell.
- 713: Power generators continually recharge adjacent conductive paths.
- 720: Thermal combustion / ignition / structural burnout.
- 750: Conductive electrical pulse propagation through wires, alloy, and WET paths.
- 772: Transient discharge decays slowly unless driven by an active power source.
- 779: Acid corrosion onto adjacent organic / metallic materials.
- 802: Cryo chilling spreads cold and freezes adjacent liquids.
- 819: Gravity & horizontal fluid spread for unfrozen liquids.
- 843: Lateral trickle when resting on a solid or liquid floor (avoid infinite flood).
- 896: Resolve any volatile tank / isotope explosions.
- 989: Place a dedicated alloy L2 cell and carve all 64 L3 micro-voxels.
- 1080: Move far away to `(160m, 4m, 160m)` -> chunk `(10, 10)` and stream in real time.

## projects/demos/emergence/src/structures.rs

- 320: Base pad & hollow interior (36 x 13 x 32 L2 = 9m x 3.25m x 8m).
- 323: Limestone perimeter walls.
- 328: Roof + glass skylight.
- 331: Front doorway + L3 steps.
- 340: Front glass window + L3 grate mullion.
- 349: Interior Isotope reactor & Cryo-Biomass bay along back corners.
- 364: 12 x 22 x 12 L2 (3m x 5.5m x 3m).
- 371: Observation deck + glass parapet + glowing Isotope beacon.
- 386: Stepped ancient altar 12 x 14 x 12 L2.
- 408: 14 x 12 x 14 L2 (3.5m x 3m x 3.5m).
- 439: Broadleaf tree 12 x 18 x 12 L2 (3m x 4.5m x 3m) with an L3-trimmed canopy edge.
- 460: Conifer 10 x 22 x 10 L2 with tiered L3 needle skirts.
- 493: Low 4 x 3 x 4 L2 bush built entirely from L3 micro-voxels.
- 518: 16 x 6 x 8 L2 floating timber dock with alloy pilings.
- 646: Fill only the gap between the natural surface and the plot level.
- 721: The foundation pad is a full L2 brick, never an L1 COW cell.
- 726: The front window mullion is stamped as an L3 micro-voxel mask, not a solid L2 cube.

## projects/demos/emergence/src/tags.rs

- 618: Geological vitrification: sand (`GRANULAR | GLASS`) fuses into solid glass at PlasmaHeat.
- 626: Geological melting: snow (`GRANULAR | ABSORBENT`) or un-welded ice (`GLASS`) melts into water.
- 646: Magma/lava quenching: superheated liquid solidifies into stone/obsidian when cooled.
- 664: Vacuum boiling and overpressure liquefaction enforce macroscopic phase invariants.
- 727: Ongoing combustion either extinguishes in wet/anoxic conditions or consumes durability.
- 744: Four-stage Bio-Activity state machine transitions.
- 916: Freezing pauses decay and puts vital organic matter into cryptosis.
- 921: Warming back to Ambient while WET revives Cryptosis -> Vital.
- 928: Radiation drives Vital -> HyperNeo when unshielded.

## projects/demos/emergence/src/terrain.rs

- 177: Compute surface y (smaller y = higher elevation).
- 190: Fill ocean water above seafloor when surface is below sea_level.
- 206: Carve caves in deeper stone/deepslate layers (never breach ocean floor).
- 211: Deepest caverns occasionally pool magma lava.
- 219: Depth-stratified Minecraft-style ore veins in stone/deepslate layers.

## projects/demos/emergence/src/world_gen.rs

- 216: Gentle starter courtyard plateau at 4.0 m covering the whole spawn plot, so the player, the
- 217: camera and every hand-placed blueprint sit on one flat L1 surface.
- 260: 1. Copy-On-Write L1 bedrock & deep limestone basement (`y in 0..2m`).
- 268: 2. Sculpt terrain, water bodies, snow peaks, and underground caves (`y >= 2.0m`).
- 295: Flat 1 m surface block (`top_l2y == block_top_l2y` across all 16 columns): emit pure L1 COW.
- 414: 3. Spawn courtyard: flatten, carve the pond, then hand-place one of every blueprint.
- 416: 4. Scatter natural vegetation (trees, pines, shrubs) over untouched ground.
- 452: Clear the courtyard volume above the 4.0 m plateau so buildings and the pond read cleanly.
- 464: Carve the pond basin and fill it with water up to sea level.
- 587: Negative chunk coordinates stream seamlessly for infinite worlds.
- 591: Mountains and water basins exist across a 256 m x 256 m sample.

## projects/demos/emergence/tests/headless_dlss_e2e.rs

- 13: Linear pre-upscale 5% -> DLSS min

## projects/demos/emergence/tests/upscaler_render_gpu.rs

- 54: Bootstrap the real fixed-seed (2026) Emergence world, step once so daylight/sky/chunks sync,
- 55: and propagate LocalToWorld matrices via hierarchy_system.

## projects/demos/fishing/src/game/boat.rs

- 54: Slight sway so pixel art looks floating on water.
- 63: Camera follows half amplitude of boat, keeping some parallax.

## projects/demos/fishing/src/game/fishing.rs

- 100: #984: progress restored from player save (key is `caught`, project-defined schema).
- 120: --- input (input snapshot sent down by host each frame) ---
- 123: Cast and hook-set are edge-triggered: holding the key counts once, like `left_clicked`.
- 125: `FISHING_AUTOPLAY=1` makes state machine advance itself: for demos and automated screenshots,
- 126: normal play does not set this var, fully input-driven.
- 132: --- boat anchor ---
- 155: Parabola: horizontal linear interpolation, vertical adds an arc.
- 180: Bobber sinks and shakes on bite.
- 200: Pure function of `t` (smoothstep), so a given elapsed time yields the
- 201: same point whatever the frame rate is (see #1059).
- 212: Splash sound on hook (#987): both player and editor Play can hear.
- 217: #984: save immediately on catch, quitting game will not lose progress.
- 231: --- HUD: fish count + current time -------------------------------------------------
- 232: 
- 233: Time from engine `TimeOfDay` (#983), project no longer maintains own timer.
- 250: --- Write back bobber position and appearance ---
- 257: Flash warm color on bite so player sees window.
- 272: --- Fishing line: rod tip → bobber (sags slightly in middle) ---

## projects/demos/fishing/src/game/world.rs

- 135: Bottom edge aligned to `y` (pivot convention: (0.5, 0.0) = bottom).
- 142: Water reflection: flip same texture downward, pivot to top center,
- 143: position mirrored by tick as `y = 2 * water_line - source.y`.
- 162: Mirror layer likewise aligns top edge to water mirror line, expands downward.
- 165: The art buries each layer's bottom edge under the water so
- 166: the texture cut is hidden. Flipping that edge makes it
- 167: surface above the water line, so the mirror is clipped at
- 168: the line instead of being moved to sit on it (#1052).
- 176: Mirror must press onto water surface for reflection feel: water at 20, boat at 50.
- 293: #984: On first entry write saved time back to `TimeOfDay` (restart continues day/night).
- 313: --- Day/night rotation -------------------------------------------------------
- 314: 
- 315: #983: time from engine `TimeOfDay` component, project no longer computes own 48-second cycle —
- 316: ambient light (`Light2D` on same entity) also advanced by engine system on same clock,
- 317: so "background change" and "darkening" always sync.
- 335: --- Save time (every 2 seconds, so quitting does not lose day/night) -----------------------------
- 349: --- Parallax -----------------------------------------------------------

## projects/demos/fishing/src/lib.rs

- 15: ===========================================================================
- 16: Player form
- 17: ===========================================================================
- 22: Audio subsystem (`prism-app/audio`, feature default off, enabled in project `[features] player`
- 23: ): one-shot sounds (hook splash) go through `GameWorld::play_sound`, need mixer.
- 29: `fishing::tick` reads the boat transform, so the boat must run first. The
- 30: preview module calls the same ticks in this order; keep the two in sync.
- 36: ===========================================================================
- 37: Preview module loaded by editor
- 38: ===========================================================================
- 69: First tick spawns all texture layers from `data/lake_layers.json`:
- 70: this log is most direct evidence that "project code actually ran".
- 88: ===========================================================================
- 89: Android JNI entry point
- 90: ===========================================================================

## projects/demos/fishing/tests/gameplay.rs

- 92: First tick finds boat/bobber and creates fishing line.
- 99: Click = cast.
- 104: Bobber splashes (CAST_SECONDS = 0.55).
- 109: Wait for bite (`bite_at` between 1.4~3.6s).
- 120: Click again within bite window = hook.
- 125: Reel (REEL_SECONDS = 0.9).
- 162: Held without a new press edge: no cast.
- 173: A real press edge this frame: cast once.
- 178: Keep holding (no new edge): the bobber lands and waits for a bite.
- 193: Still only held during the bite window: the bite is missed instead of auto-hooked.
- 208: Wait for the next bite and hook it with a real press edge.
- 273: Identical lead-in for both frame rates: same ticks, same inputs.
- 288: Hook set, then reel for `steps` frames.
- 310: 0.5 s of reeling: 15 frames at 30 FPS, 30 frames at 60 FPS.

## projects/demos/sponza/src/lib.rs

- 8: ===========================================================================
- 9: Player form (Standalone Player)
- 10: ===========================================================================
- 17: Previously no child systems registered, `App` thinks rendering is off → takes headless branch:
- 18: No window created, no render thread started, keyboard events unhandled (all shortcuts invalidated).
- 26: Disable tonemap curve compression in raster mode: passthrough to exposure only.
- 52: The tuning hook owns its input stream; do not let engine PBR
- 53: isolation shortcuts silently replace the render debug flags.
- 55: PBR component isolator, same masks the Digit1..0 / Shift+Digit
- 56: shortcuts set. Bit order matches `PBR_FLAG_NAMES`.
- 67: ===========================================================================
- 68: Editor preview module (Editor Play Dynamic Module)
- 69: ===========================================================================
- 91: ===========================================================================
- 92: Android JNI entry point
- 93: ===========================================================================

## projects/demos/sponza/src/tuning_ui.rs

- 55: Parameters matching RTXGI Pathtracer UI
- 83: ECS UI entities
- 164: Raster <-> Path Trace. Either direction invalidates the path tracing
- 165: accumulation.
- 244: Bounces: 1 .. 24
- 253: Ray max distance: 100.0 .. 5000.0
- 262: Tonemapper: 0=Reinhard, 1=ACES, 2=Passthrough
- 278: Scene scale: 5.0 .. 100.0
- 287: Roughness threshold: 0.0 .. 1.0
- 298: Downscale factor: 1 .. 10
- 307: Accumulation frames: 1 .. 120
- 317: Stale frames: 1 .. 120
- 326: Cache capacity: 2^16 .. 2^23 entries (doubles per step).
- 327: Resizing rebuilds the buffers, so the cache restarts empty.
- 336: DLSS Mode
- 497: Panel toggle: no occlusion key may steal it, else panel opens then cannot close.
- 502: GI mode is Sponza signature operation, panel should allow it even when hidden (consistent with
- 503: top status bar "[1] Ref [2] SHaRC [3] NRC").
- 534: M / R / V are panel own function keys, camera never reads them
- 535: (`camera_controller_system` only looks at WASD / QE / Space / Ctrl / Shift),
- 536: so lock state does not affect key semantics, both states receive.
- 537: Previously added `!self.locked` guard for "locked → camera", but window auto-locks on focus,
- 538: effectively making these keys unreachable in game — M not responding came from this.
- 551: Tone map also panel own setting: available even when panel hidden (status bar always shows it).
- 556: Next panel full is **menu operation**, only belongs to unlocked state. Locked = camera takes
- 557: mouse/keyboard, menu should receive no keys (previously only blocked WASD, arrows / Space / Enter still eaten,
- 558: locked pressing empty slot neither fires nor flips menu, pure key swallow).
- 560: Unlocked: WASD is menu navigation. If panel manually hidden by F1, re-expand then navigate,
- 561: else keys act on invisible list.
- 567: Panel closed then next panel meaningless: pass back to App as-is.
- 614: Lock state ask window directly: `on_tick` cache is previous-frame, while "click screen to lock"
- 615: and "Esc unlock" happen between frames, using old value would misroute this key.
- 620: false = not consumed: App runs shortcuts as usual and routes key into InputManager (under lock
- 621: WASD goes this path: panel ignores, camera moves normally). Note consumption does not block
- 622: InputManager — so "unlocked WASD does not move camera" enforced by engine-side pointer lock check.
- 634: Pointer lock = camera takes over keyboard/mouse. Platform layer is only true source (App copy
- 635: `InputManager::pointer_locked` not reachable by hook), synced each frame for key routing.
- 638: Sync runtime settings to engine
- 646: After lock cursor hidden, UI should not handle hover / click: otherwise when cursor clamped at
- 647: window center, pointer over a button would be "held", border glowing pressed during lock.
- 651: Pointer lock = full-screen view, panel hidden (F1 / Tab can bring back anytime).
- 652: Only hide on **just locked** frame: writing `show && !locked` every frame makes F1 useless
- 653: — on press hook sets show true, next frame on_tick wipes false, panel never opens.
- 698: Initialize UI entities once
- 713: Panel background quad
- 737: Panel multiline text
- 770: Top status banner (visible when panel is closed)
- 804: Format and update text

## projects/editor/crates/editor-app/src/about.rs

- 110: The fill is the glass tint: the shader mixes the blurred backdrop
- 111: toward it, so the same colour reads as "how visible is the scene
- 112: through this card".

## projects/editor/crates/editor-app/src/bridge.rs

- 66: A surface whose material samples the backdrop asks the renderer
- 67: for its filter pass; the tint stays the shader own colour.

## projects/editor/crates/editor-app/tests/about_dialog.rs

- 40: The dialog mounts on the frame after the command is handled.

## projects/editor/crates/editor-app/tests/menu_alignment.rs

- 128: Level 2: the "New" row inside the File panel.

## projects/editor/crates/editor-app/tests/menu_frame.rs

- 47: Click the "File" trigger.
- 65: A menu opens on click, which needs the release edge.
- 78: "New" and "Open Recent" are submenu rows; flat entries sit beside them.

## projects/editor/crates/editor-app/tests/menu_interaction.rs

- 131: Walk in one-pixel steps from the row across the panel's padding into the
- 132: panel's first item. No frame may drop the branch on the way.

## projects/editor/crates/editor-app/tests/theme_switch.rs

- 37: Light means a bright background; guard against an inverted preset.
- 51: Open View and check the Light entry exists and is not checked yet.
- 75: The dropdown is mounted on the frame *after* the menu opens, so it needs
- 76: one more build before its nodes exist.
- 79: Theme is a submenu now: its rows only mount once the Theme row is entered,
- 80: so assert on the definition rather than on the open panel.

## projects/editor/crates/prism-ui/src/dock.rs

- 481: Fallback: replace empty root
- 649: 1. Check floating windows from top to bottom (Z-order).
- 677: Check if clicking a tab for tear-off/docking
- 706: 2. Check docked splitters and tabs.
- 830: Floating windows mount on the overlay layer in Z-order.
- 1075: macOS-style titlebar: traffic lights on the left, centered title
- 1076: (or compact pill tabs when multiple panels are grouped), 1px hairline.
- 1173: Right balance spacer matching the 52px traffic-light cluster
- 1174: so the center title/pills sit at the true window midpoint.
- 1181: Docked Tab bar
- 1261: Active window body

## projects/editor/crates/prism-ui/src/elements.rs

- 723: (id, has_children, expanded) with depth encoded via render

## projects/editor/crates/prism-ui/src/icons.rs

- 149: #FF5F57
- 151: #FEBC2E
- 153: #28C840

## projects/editor/crates/prism-ui/src/layout.rs

- 197: 1. Arrange absolute children directly inside parent rect.

## projects/editor/crates/prism-ui/src/lib.rs

- 110: Test toggle_button

## projects/editor/crates/prism-ui/src/menu.rs

- 92: While a menu is open, hovering a different trigger switches to it —
- 93: that is what makes the bar feel like one control rather than a set of
- 94: independent buttons. Hover alone never opens a closed menu; that stays
- 95: a click, so passing over the bar does not leave menus behind.
- 116: Crossing from a row into the panel it opened passes over panel space
- 117: that enters nothing, so an empty resolution means "in transit", not
- 118: "left the branch". Only a real target ends the branch.
- 198: Items of a closed menu cannot be clicked: the popup is not
- 199: laid out, so a stale hit would be a layout bug, not intent.
- 216: Submenu rows open on hover; a click is intentionally inert
- 217: so the menu stays open while the pointer crosses the row.
- 471: Popups are mounted on the popup layer so they escape the bar's clip
- 472: and stacking order.
- 478: The branch was resolved last frame, after input, while the previous
- 479: frame's nodes still existed; re-resolving here would query a tree that
- 480: `begin_frame` has already cleared.
- 483: A full-viewport backdrop under the popups: clicking anywhere the
- 484: popups do not cover closes the menu. It is mounted first so the
- 485: dropdowns hit-test above it.
- 486: It starts below the bar on purpose: the popup layer hit-tests
- 487: before everything else, so a backdrop covering the bar would
- 488: swallow the triggers and hover-switching between menus would stop
- 489: working.
- 513: Each open submenu level is anchored to its row in the parent
- 514: panel. Depth comes from the menu definition, never from input.
- 549: A submenu row is open when the pointer is on the row itself, or inside the
- 550: panel it spawned, or on any row below it. Those three cases collapse into
- 551: one question — "is the pointer in this row's subtree?" — so the branch is
- 552: simply the ancestor path of the deepest entered row.
- 553: 
- 554: Panels are siblings of rows on the popup layer, so subtree containment is
- 555: decided by scope rather than by the node tree.
- 600: A deeper row of this same submenu also counts as "entered", which
- 601: is what keeps the ancestors alive as the pointer moves outward.
- 612: Descend so a deeper entered row extends the same path. The recursion's
- 613: result is deliberately ignored: once this level is entered, the trail
- 614: keeps it even when nothing deeper is hovered.
- 684: The panel's padding and the gaps between its rows are part of the
- 685: panel, not holes: without this, crossing one of them reports "no
- 686: hover" for a frame and collapses the branch the pointer is entering.
- 690: The pointer lands on the first item, not on the frame, so the item is
- 691: lifted onto the row's top edge; the frame then starts one inset above
- 692: the row. The inset itself is unchanged.
- 714: Width stays Auto so the rule stretches to the panel's inner width; a
- 715: Percent would resolve against the viewport and blow the panel open.
- 779: Submenu rows are hover targets, not click targets: hovering opens
- 780: the child panel and moving away closes it.

## projects/editor/crates/prism-ui/src/menu_fullscreen_tests.rs

- 44: Hover the submenu row.
- 54: Resolve the branch from the hover, then rebuild so the panel mounts. The
- 55: host does these two steps on consecutive frames.

## projects/editor/crates/prism-ui/src/menu_geometry_tests.rs

- 197: Multi-level behaviour is covered in `menu_nested_tests`; this keeps the
- 198: single-level contract: hovering the row mounts exactly one panel beside it.
- 239: Resolve after input, while this frame's nodes still exist.

## projects/editor/crates/prism-ui/src/menu_nested_tests.rs

- 130: Level 2: "New" row spawns a panel whose first item must sit level with it.
- 140: Level 3.
- 150: Level 4.
- 206: The pointer moves onto an item inside the level-2 panel; the branch must
- 207: survive or the panel would vanish the moment the pointer left its row.

## projects/editor/crates/prism-ui/src/svg.rs

- 28: 1-pixel antialiasing feather in viewBox units.

## projects/editor/crates/prism-ui/src/theme.rs

- 18: ---- surfaces (back to front) ----
- 34: ---- borders ----
- 41: ---- text ----
- 50: ---- interaction ----
- 64: ---- metrics: menu bar ----
- 72: ---- metrics: menu dropdown ----
- 89: ---- metrics: typography ----
- 166: Metrics are shared with the light preset; only colours differ,
- 167: so a preset switch cannot relayout the UI.
- 223: Metrics match the dark preset; generous radii are what make this
- 224: theme read as a system surface rather than a tool panel.
- 272: A hard shadow reads as a cutout on light surfaces.
- 291: Surfaces must differ, or switching presets would be invisible.
- 294: Metrics are shared: a theme switch must not relayout the UI.

## projects/editor/crates/prism-ui/src/tree.rs

- 226: Orthogonal behavior tables (zero Rc<RefCell<T>>, zero inheritance)
- 242: Retained cross-frame interaction state
- 345: Popups size to their content. The default cross-axis alignment
- 346: stretches children to the container, which would make every
- 347: dropdown as tall as the window.
- 426: Resolve in dependency order. Popup panels are siblings on the popup
- 427: layer, so tree depth cannot order them; what matters is that a panel
- 428: anchored to a row *inside another panel* is resolved after that other
- 429: panel. Sorting by the anchor target's depth is not enough — measure the
- 430: dependency chain explicitly.
- 464: Align the panel's first row with the row that opened it,
- 465: which is what makes a chain of menus readable.
- 474: Flip a side panel that would leave the viewport: a nested menu
- 475: opening off the right edge is unreachable, so open it leftwards
- 476: instead of clamping it over its parent.
- 480: Keep the popup on screen; a dropdown that runs off an edge is
- 481: unusable, so clamp rather than clip it.
- 484: Translate the whole subtree: children were arranged relative to the
- 485: node's provisional position, so moving only the parent would leave
- 486: the items behind.
- 505: Walk up until we find an ancestor that is itself anchored.
- 519: Continue from the outer panel's own anchor target.
- 649: The popup layer wins over everything below it, mirroring draw order.
- 672: Overlays first, then normal children in reverse draw order.
- 774: Active pointer drag (e.g. DraggableValue scrub).
- 838: Scroll routing: bubble from hit node up to nearest Scrollable ancestor.
- 856: Keyboard / text routing to focused node.
- 883: Refresh node interaction flags so draw list reflects latest state.
- 899: Popups draw last so they sit above every panel.
- 930: A material owns the surface, so it draws even when the fill is
- 931: clear: a frosted panel with no flat colour still has to composite.
- 1004: Normal children first, overlays last.

## projects/template/src/intro.rs

- 33: ---------------------------------------------------------------------------
- 34: Rain (project-bundled minimal 2D particle)
- 35: ---------------------------------------------------------------------------
- 119: ---------------------------------------------------------------------------
- 120: Animation state
- 121: ---------------------------------------------------------------------------
- 238: 1. Lazy init: find entities by Name on first frame.
- 246: 2. Skip detection (input snapshot sent down by engine each frame).
- 249: 3. Advance animation, extract this-frame values to write back and controlled entities.
- 282: 4. Splash rain: intensity follows title fade in/out, submitted as UI quads.
- 289: 5. Write back components (read-modify-write: plugin form has no `&mut` borrow below).
- 370: Black fades out from full black, title floats up simultaneously: both should already be written to components.

## projects/template/src/lib.rs

- 15: ===========================================================================
- 16: static  Player
- 17: ===========================================================================
- 24: Audio subsystem (`prism-app/audio`, default off) not enabled here: this project has no AudioSource,
- 25: enabling would only create an empty engine + decode thread. To play sounds, enable feature and this line together.
- 31: ===========================================================================
- 32: Preview module loaded by editor
- 33: ===========================================================================
- 63: ===========================================================================
- 64: Android JNI entry point
- 65: ===========================================================================
- 74: `run_on_android` stages APK assets and the launch config before calling
- 75: `build_app`, so the engine can use the same `assets/...` paths as desktop.

## projects/template/src/main.rs

- 16: Build app (complete engine init), register intro scene then launch.
