# Celeste-like Metroidvania: complete feature breakdown and engine requirement mapping

Nov 9, 2026 · @Jason Gu

## 1. Project positioning and design pillars

This is a 2D pixel Metroidvania with ~3–4 hours of playtime: movement feel benchmarked against Celeste, world structure benchmarked against Metroidvania (ability-as-key, backtracking exploration). Its primary purpose is not to be a hit, but to push your engine to "good enough" with a set of real, interlocking requirements.

**One-line definition (working title "Echo Well")**: player descends an abandoned vertical giant well; each movement ability gained opens rooms previously unreachable, finally reaching the bottom to face the final boss.

**Four design pillars**

1. **Feel first**: movement precise, forgiving, predictable. Every detail that "feels fair" (coyote time, input buffer, corner correction) is hard requirement, not polish.
2. **Ability as key**: map gates determined by movement abilities, not stats or key items. This naturally couples levels with engine physics.
3. **Room as puzzle, death nearly free**: after death return to current room entrance within 0.5s, encouraging repeated trial and error.
4. **Small but complete**: prefer 6 abilities and 5 areas done thoroughly over 15 abilities half-finished.

**Technical profile (premise determining all later engine requirements)**

| Item | Value | Reason |
| --- | --- | --- |
| Internal resolution | 320×180, integer scaling | Pixel-perfect, consistent with Celeste |
| Logic frame rate | Fixed 60 Hz time step, render may exceed 60 | Feel depends on determinism |
| View | Side-view horizontal, single-screen rooms + some scrollable large rooms | Room camera simplest |
| Collision | AABB + tile grid, no general physics engine | Platforming needs fully controllable "cheat-style" physics |
| Platform | PC first (Windows/macOS/Linux), gamepad + keyboard/mouse | No mobile for now |
| Players | Single player | Networking out of scope |

**Explicitly not doing**: networking, procedural maps, inventory equipment system, complex dialogue trees, skill trees, economy, localization beyond fixed-resolution UI adaptation.

## 2. Character movement system (Celeste-style feel)

Movement system is heart of whole project, should be built first and finalized last. It is an explicit state machine, all "feel" comes from transition details between states, not a single parameter.

**Player state machine**

- **Normal**: regular ground / air movement and jump
- **Dash**: dash, with startup freeze frames, direction lock, end deceleration
- **Climb**: wall climbing, consumes stamina
- **WallSlide**: wall sliding (sub-state of Normal but velocity cap handled separately)
- **Hurt / Dead**: hit stun, death, respawn
- **Cutscene / Locked**: input taken over by script during story / cutscene

**Feel details that must be implemented**

| Mechanic | Behavior | Why hard requirement |
| --- | --- | --- |
| Coyote time | Can still jump ~0.1s after leaving platform edge | Eliminates frustration of "pressed but didn't jump" |
| Jump buffer | Press jump ~0.08s before landing triggers jump on landing | Makes chained jumps smooth |
| Variable jump height | Hold jump key halves gravity briefly, release falls normally | Jump height controlled by player |
| Apex half-gravity | Halve gravity near jump apex when vertical velocity low | Creates "floating" hang time |
| Corner correction | When jump hits ceiling corner, shift horizontally few pixels to "slide" past | Avoid being stuck by one-pixel overhead |
| Ground / platform edge correction | When dashing near platform edge auto-raise few pixels | Prevent dash blocked by "one pixel short" |
| Dash direction | 8 directions, diagonal velocity normalized; ~3 freeze frames before dash | Impact feel, also gives player reaction time |
| Dash count | 1 in air, restored on landing or touching specific object | Basic unit of level design |
| Super-dash techniques | Ground dash then immediately jump grants extra horizontal speed | Optional advanced technique, decide early whether to keep |
| Wall jump | Jump while on wall, gain horizontal velocity away from wall | Combines with wall climbing |
| Wall detection tolerance | Can jump / wall-jump 1–2 pixels off wall | Forgiveness |
| Stamina system | Wall climb consumes stamina, less when still, more upward, restores on landing | Limits wall-climb duration |
| Grab and carry | Grab and carry objects (can be deferred) | Introduces parent-child relation between entities |

**Starting reference parameters** (scale reference, at 320×180, pixels/second; from public community analysis of Celeste feel, approximate values needing tuning in your engine, not exact)

| Parameter | Reference value |
| --- | --- |
| Max horizontal run speed | ~90 px/s |
| Ground acceleration | ~1000 px/s² |
| Air acceleration factor | ~0.65× ground |
| Gravity | ~900 px/s² |
| Max fall speed | ~160 px/s (hold down to ~240) |
| Jump velocity | ~105 px/s |
| Variable jump hold time | ~0.2 s |
| Coyote time / input buffer | ~0.1 s / ~0.08 s |
| Dash speed / duration | ~240 px/s / ~0.15 s |
| Dash end speed | ~160 px/s |
| Wall climb speed | ~45 px/s (up) / 80 px/s (down) |
| Stamina cap | ~110 units |

**Engineering requirements**

1. All numbers in data files (not hardcoded), support runtime hot reload, immediate feel test after change.
2. Player movement uses **fixed time step + integer / sub-pixel position**, horizontal and vertical **axis-separated movement with per-pixel collision detection**, foundation of determinism and feel.
3. Provide "input replay": record an input sequence, stably reproduce same trajectory, for feel regression tests.
4. Physics debug overlay: show collision boxes, velocity vector, current state name, coyote / buffer timers.

## 3. Combat and interaction systems

Celeste has no combat, Metroidvanias usually do. Suggest "light combat": combat is extension of movement system, not second gameplay system, keeping workload controllable.

**Minimal combat set (suggested)**

- **Melee slash**: one short-range attack hitbox, ~0.15s duration, usable in air, short cooldown.
- **Dash attack**: dashing into specific enemies deals damage or bounces (turning movement ability into combat ability, no extra button).
- **Hit and invincibility frames**: brief stun + ~1s flashing invincibility after hit; knockback direction depends on source.
- **Health**: integer cells (e.g. 5), restored at checkpoints or items; death respawns, no "game over" screen.

**Interaction system**

- Generic interact key: open doors, dialogue, use save points, pull levers.
- Interaction prompt: show icon near interactable, fade out on leave.
- Player-entity contact rules: damage contact, pushing, standing (vehicles / moving platforms), trigger volumes.

**Three collision box types that must be distinguished in implementation**

| Type | Purpose | Example |
| --- | --- | --- |
| Hurtbox | Area that can be damaged | Player body, enemy body |
| Hitbox | Area that deals damage | Slash, enemy claw |
| Solid / Trigger | Block / trigger event | Wall, spring, teleport zone |

**Optional extensions (if capacity allows)**: ranged projectiles, block / parry, upgradeable attack power. These significantly increase enemy design and balance workload, not recommended for first milestone.

## 4. Ability unlock and gating

5 core abilities (plus 1 optional) form map "locks and keys". Each ability must satisfy three points: changes movement rules, blocked by instantly-understandable gate, combinable with later abilities.

| Order | Ability | Change to movement system | Typical gate (what it unlocks) |
| --- | --- | --- | --- |
| 0 | Basic movement | Run, jump, variable jump, coyote, buffer | Starting area |
| 1 | Wall jump | Jump from wall, gain reverse horizontal velocity | Overly tall shafts, narrow gaps |
| 2 | Air dash | 1 eight-direction dash in air, restored on landing | Wide pits, hazard zones to cross |
| 3 | Climbing | Grab wall, consume stamina | Long vertical walls, stamina-limited routes |
| 4 | Double jump or hover | One extra jump in air (or brief slow descent) | Suspended platforms needing precise landing |
| 5 | Dash break | Dash shatters specific fragile objects | Sealed shortcuts and hidden rooms |
| 6 | Water / airflow movement (optional) | Movement rules in special media | Underwater areas, airflow wells |

**Design rules**

1. After each ability gained, **immediately** arrange a room passable only with it (tutorial room), then 2–3 rooms requiring combination.
2. Once player has ability, should see at least one previously-seen but impassable place — motivation for backtracking.
3. Avoid "hard lock": at no point should player be unable to return to last save point due to missing ability.
4. Ability switches represented by \*\*game state flags\*\*, engine needs global savable flag system; doors, mechanisms, NPC dialogue all read same flags.

**Data-layer requirement**: ability list, unlock conditions, corresponding tutorial rooms maintained in tabular data (e.g. JSON/CSV), not in code. Lets level designers reorder unlocks without code changes.

## 5. World and map structure

World is chain of 5 areas arranged downward along a vertical well, each exit opened by ability from previous area.

&#91;embedded content: World topology · 5 areas, 4 ability gates\]

Each level deeper, player brings new ability back to open previously-seen impassable places; final gate requires two abilities simultaneously.

**Room system**

- One room = one tile map + entity list + camera bounds + exit connections. Single-screen room 320×180, large rooms integer multiples.
- Rooms connected via **exits** (door or screen-edge trigger), exit records target room and landing point, player velocity preserved on transition.
- Total ~60–100 rooms, 12–20 per area; each area has own tileset, background, music and ambient sound.
- Each room has unique ID for save, map, script references.

**Map and navigation**

- Minimap drawn from "visited rooms" set, room positions on map from editor grid coords.
- Map fragment item unlocks outline of whole area, unvisited rooms shown dark.
- Fast travel between save points if both activated; travel is "load room + transition", no extra special path.
- Each area has at least 1 **backtracking shortcut** (one-way door or elevator), avoid long walk for backtracking collectibles.

## 6. Mechanisms and environmental elements

Each mechanism is independent Entity with own state, collision and trigger rules. This section is most direct source of engine "entity/component system" and "collision system" requirements.

| Mechanism | Behavior | Engine requirement |
| --- | --- | --- |
| Spikes / lava | Instant death or damage on contact, directional | Directional damage zones, death events |
| Moving platform | Back-and-forth along path, player standing gets carried | **Riding**: platform movement carries entities on it, handle crushing |
| One-way platform | Pass through from below, stand on top | Directional collision-layer detection, drop-through action |
| Spring / bounce mushroom | Fixed launch velocity on step | Velocity override, interaction with jump state |
| Dash-refresh crystal | Restores dash on touch, briefly disappears then respawns | Entity visibility, timers, communication with player state |
| Falling platform | Drops after delay when stepped on, restores after time | State machine, timers, entity reset |
| Breakable wall | Breaks on dash or attack | Hit events, tile / entity replacement, save record "broken" |
| Switch + door | Lever / pressure plate controls door open-close | Global flags, inter-entity references, animation |
| Wind / airflow zone | Continuously applies force in area | Continuous effects in trigger volume |
| Water body | Changes gravity and movement speed | Physics parameter switching in area |
| Portal / room exit | Switch to adjacent room preserving player velocity | Room-switch data and transition |
| Collectible | Disappears on touch and recorded in save | Persistence of one-time entities |

**Common requirements**

1. All mechanisms **reset to room initial state on player death / respawn** (except permanent changes recorded in save, e.g. opened doors, broken shortcuts).
2. Mechanism parameters (speed, delay, path points) settable directly in level editor, no code changes.
3. Entity lifecycle explicit: created on room load, destroyed on leaving room, reset on respawn. This decides whether "room" is first-class concept in your engine.

## 7. Enemies and bosses

Prefer fewer enemies: 6 normal + 3 bosses enough to cover most engine requirements. Each enemy simple "one movement pattern + one threat method" combination, teaching player one thing.

**Normal enemies (suggested 6)**

| Enemy | Behavior | Main engine capability tested |
| --- | --- | --- |
| Patroller | Back-and-forth on platform, turns at edge / wall | Ground detection, edge detection |
| Flying wanderer | Flies along sine or path points | Path / curve movement |
| Chaser | Chases player after entering perception range | Line-of-sight / distance perception, simple pathfinding |
| Fixed turret | Periodically fires projectiles | Projectile entities, object pool |
| Jumper | Charges then parabolic jump at player | Same physics as player, state machine |
| Shielded soldier | Front immune, need flank or dash to break shield | Directional hit detection |

**Unified enemy AI structure**: each enemy small state machine (Idle → Alert → Attack → Recover → Dead), transitions driven by timers and perception events. Engine needs: state machine tool, timers, line-of-sight detection (ray or area), reusable movement behavior components.

**Boss fights (suggested 3)**

1. **Area guardian boss** (tutorial type): 1 phase, 3 attacks, fixed predictable patterns, tests freshly learned ability.
2. **Mid-game boss**: 2 phases, attack pattern changes past half health, mechanisms appear in arena.
3. **Final boss**: 3 phases, requires all movement abilities, short cutscene at end of each phase.

**General boss-fight requirements**

- **Boss room lock**: on entry doors close, camera locks, boss health bar shows, unlock after fight.
- **Phase scripts**: describe attack sequences with **coroutines or timeline scripts** (wait → telegraph → attack → recover), easier than pure state machine.
- **Attack telegraph**: every attack has clear visual / audio warning, source of fairness.
- **Failure retry**: on death immediately return to boss room entrance, boss state fully reset, no long walk.

## 8. NPCs, narrative and collectibles

Keep narrative restrained: tell story with environment and sparse dialogue, avoid complex branching. Value here is forcing engine's **dialog box, text, script events** three base capabilities.

**NPCs and dialogue**

- 8–12 NPCs, all linear dialogue (at most few lines switched by flags).
- Dialog box: character-by-character display, skippable, supports line breaks and simple rich text (color, pause, shake).
- Player input taken over during dialogue, returned after.
- Speaker portrait or name tag, positioned bottom of screen or above character.

**Cutscenes and script events**

- Simple "event scripts": move camera, walk character to point, play animation, wait, show dialogue, set flag.
- Write as coroutine / timeline sequence of ordered steps, not hardcoded state machines in game code.
- Skippable during cutscene (hold a key).

**Collectibles and progress**

| Category | Count | Purpose |
| --- | --- | --- |
| Ability items | 5 (optional 6th) | Main progression |
| Health-capacity fragments | ~10 | 4 fragments = 1 health cell, encourages exploration |
| Hidden collectibles | ~20 | Pure achievement, maps to completion percentage |
| Map fragments | 1 per area | Unlocks full map display of that area |

**Achievements and stats**: record death count, play time, collection completion, shown in save screen. Achievement system itself can be deferred, but stats recommended from day one.

**Localization**: all text via string table (key → text), don't write strings in code. Even if first release single language, doing this right from day one worth it, backfilling expensive.

## 9. Saves, checkpoints and death / respawn

This system distinguishes two completely different "remembering": **temporary room state** (reset on death) and **world permanent state** (written to save). Confusing them is most common source of Metroidvania bugs.

**Death and respawn flow (target total ~0.5s)**

1. Player death: freeze ~3–5 frames, play short death effect.
2. Screen fast transition (wipe or zoom).
3. All entities in room reset to initial state, player appears at room entrance (or nearest in-room checkpoint).
4. Immediately restore control, no key confirmation.

**What save points store**

| Data | Notes |
| --- | --- |
| World flag table | Doors opened, bosses defeated, abilities gained |
| Collected item set | Stable entity IDs, avoid reappearing |
| Player state | Current max health, unlocked abilities, current room and save point |
| Map exploration | Visited room set, for drawing map |
| Stats | Death count, play time |

**Design points**

- Save point is an entity in room, player interacts to restore health and write disk; checkpoints (automatic, this-play-only) can be denser.
- Save format versioned text / binary structure (e.g. JSON + version field), migration function on version change. Engine "serialization" requirement.
- Entities need **stable IDs**: assigned when placed in editor, unchanged at runtime, so save can point to "that chest in room 3".
- Support 3 save slots, manual delete; disk write "temp file then atomic rename", avoid power-loss corruption.
- Auto-save on entering new area, defeating boss, gaining ability.

## 10. Camera, image, audio and effects

This section decides "whether it looks and sounds like a complete game", main requirement source for render and audio modules.

**Camera**

- **Room camera**: each room own bounds, camera clamped within; smooth transition or hard cut on room switch.
- **Follow and look-ahead**: horizontal slight look-ahead toward facing, vertical smoothed on land / jump, avoid jitter.
- **Camera zones**: room can have multiple camera zones (different bounds or fixed shot), for boss rooms and large rooms.
- **Screen shake**: adjustable intensity, duration, decay, with "reduce shake" accessibility toggle.

**Rendering**

| Requirement | Notes |
| --- | --- |
| Tile map render | Multi-layer (background, collision, foreground), camera-culled, batched |
| Sprite and animation | Sprite atlas, frame animation, event frames (e.g. footstep trigger), flipping |
| Parallax background | Multi-layer backgrounds scrolling at different speeds |
| Pixel-perfect output | Render to fixed-resolution offscreen buffer then integer upscale |
| Particle system | Dash afterimages, landing dust, hit sparks, needs object pool |
| Lighting / post-processing (optional) | Simple screen flash, tint change, vignette; complex lighting later |
| Hitstop | Global pause few frames on dash start / hit, key to feel |

**Audio**

- Sound effects: multiple simultaneous, volume / pitch randomization, positional attenuation (optional).
- Music: looping, crossfade on area switch, supports **layered music** (same track adds more parts with progress).
- Buses and volume settings: music, SFX, ambience three buses, independently adjustable.
- Event-driven: all SFX triggered by game events ("jump", "dash", "land"), not direct filename calls in logic code.

**Camera-language details**: dash afterimage and slight screen zoom, landing dust, death burst particles — all should have rough version in first milestone, directly affecting feel judgment.

## 11. UI and menus

UI system needn't be general GUI framework, only support concrete screens below, with full gamepad operability.

| Screen | Content | Key requirement |
| --- | --- | --- |
| HUD | Health cells, current area name (fade in / out on entry), collection prompts | Screen-space draw, camera-independent |
| Main menu | Start, continue, settings, quit, save slot selection | Focus navigation (up/down/left/right + confirm/back) |
| Pause menu | Continue, map, settings, back to main menu | Game logic frozen while UI runs |
| Map screen | Explored rooms, current position, save point marks, area coloring | Dynamically drawn from "visited rooms" data, zoom / scroll |
| Settings | Volume, display (window/fullscreen/scale), input rebinding, accessibility | Immediate effect, written to config |
| Dialog box | Character-by-character text, few options | Linked with script system |
| Item / ability acquisition popup | One-time show of new ability and control hints | Cutscene-style, skippable |
| Results / stats | Time, death count, collection rate after completion | Reads stats data |

**Technical points**

1. UI and game world rendered in layers: world → effects → HUD → menu → transition mask.
2. Menus use **focus graph** navigation rather than mouse-click-first; mouse support additive.
3. Text rendering supports pixel fonts, auto-wrap, multi-language char sets (at least Chinese and English).
4. Transitions (fade, wipe) independent fullscreen effect layer, reusable by room switch, death, cutscene.

**Assist mode (ref Celeste)**: game speed adjust, infinite dash, invincibility, skip boss fights, in settings; engine requirement "key params overridable at runtime".

## 12. Input and accessibility

Platforming extremely sensitive to input latency, input layer must be thin and stable, separating "actions" from "physical keys" from day one.

**Input abstraction**

- Game logic reads only **actions** (jump, dash, attack, grab, interact, pause), not keys directly.
- Each action bindable to multiple physical inputs (keyboard keys, gamepad buttons, stick directions).
- Direction input unified into one vector with **dead zone** handling and 8-direction quantization (for dash).
- Each action provides three state queries: frame pressed, held, frame released.
- Input sampled on **fixed logic step**, same input sequence produces same result (supports replay and tests).

**Device support**

| Item | Requirement |
| --- | --- |
| Keyboard | Default + fully rebindable |
| Gamepad | Mainstream plug-and-play, stick dead zone adjustable, buttons rebindable |
| Hot-plug | Plug / unplug gamepad at runtime no crash, auto-switch prompt icons |
| Key prompts | Key icons in dialogue / tutorial change with current input device |

**Accessibility checklist**

1. All keys rebindable; "hold" actions (wall grab) offer "toggle" mode.
2. Assist mode: adjust game speed, infinite dash, invincibility, etc. (see §11).
3. Reduce screen shake and flashing options.
4. Subtitle and dialogue text size adjustable; don't convey info by color alone.
5. Three audio buses independently adjustable.

## 13. Content pipeline and tools

This project's content volume (dozens to 100+ rooms) decides: **without handy tools, content can't be made**. Toolchain itself is major engine requirement source and best reveals data-format design problems.

**Level editing**

- Can use existing editor (e.g. Tiled / LDtk) export then import into engine, or build own. **Suggest existing first**, save energy for engine itself.
- Editor must express: multi-layer tiles, entity placement (with params), trigger areas, room bounds, connection points between rooms.
- Room data format stable, versionable, text-diffable (like JSON), for collaboration and rollback.
- Engine supports **runtime hot reload**: after saving level file, current room in game refreshes immediately.

**Asset pipeline**

| Asset | Processing |
| --- | --- |
| Images | Auto-atlas, keep originals for artists |
| Animation | Read from exported frame data (e.g. Aseprite JSON) |
| Audio | Compressed format, music streamed, SFX loaded whole |
| Fonts | Bitmap font or TTF pre-rendered |
| Config / numbers | Text files, hot reload supported |

**Debug and developer tools (strongly recommended, all cheap)**

1. Debug overlay: collision boxes, entity IDs, state names, frame time.
2. Console / commands: teleport to room, set flag, unlock all abilities, toggle invincibility.
3. Time control: pause, single-step one frame, slow motion.
4. Input recording and replay (see §2).
5. Performance panel: per-frame time, entity count, draw call count.
6. Logging system: leveled, filterable by module.

## 14. Engine requirement mapping table

Below translates game features into engine modules with verifiable acceptance criteria. Treat as engine todo list: each row done, verify with corresponding game feature.

| Engine module | Driven by game features | Acceptance criteria |
| --- | --- | --- |
| Main loop and fixed time step | Feel, replay | Logic fixed 60 Hz, render frame rate variation doesn't affect jump height |
| Input system | All actions, rebinding | Same input sequence replay, trajectory frame-identical |
| Tile collision | Run, jump, wall climb, corner correction | Player never stuck in wall, no jitter on flat ground |
| Entity / component system | Mechanisms, enemies, player, collectibles | Adding new mechanism only needs one class / component and one data file |
| Inter-entity communication (events) | Crystal restores dash, switch opens door | Interaction without direct reference to other side |
| State machine tool | Player, enemies, bosses | Transitions have enter / exit callbacks, current state printable |
| Coroutine / timeline script | Boss phases, cutscenes, dialogue | 20-line script expresses "walk to A → talk → open door" |
| Room system | Room switch, reset, camera bounds | No flicker on room switch, room fully reset after death |
| Rendering (tile / sprite / atlas) | All visuals | Stable 60 FPS on target machine, no seams between pixels |
| Animation system | Characters, enemies, effects | Animation event frames trigger SFX / hitboxes |
| Particle system | Dash, dust, hits | Thousands of particles without visible frame drop, object pool used |
| Camera | Room camera, shake, lock | No jump on room switch or boss lock |
| Audio | SFX, layered music | 16+ simultaneous SFX, no popping on music switch |
| UI / text | Menus, dialogue, map | Gamepad fully operates all menus, Chinese displays correctly |
| Serialization / save | Saves, flags, collectibles | After load world matches pre-exit state, old saves migrate |
| Resource management | Levels, atlases, audio loading | No hitch on room switch, no resource leaks |
| Hot reload | Numbers, levels, script debug | Takes effect in game within 1s after file save |
| Debug tools | Dev efficiency | Can teleport, set flags, single-step in game |
| Packaging and release | Delivery | One command produces build runnable on clean machine |

**How to use**: don't "finish" modules one by one then make game. Follow next section's milestones, advance only the engine slice game needs, verify with game, then abstract.

## 15. Milestones and vertical slice

Development in 5 phases, each ends with a gate that must pass, otherwise don't enter next phase.

&#91;embedded content: Development roadmap · 5 phases, 4 gates\]

Time not written in diagram: depends on weekly effort, suggest calibrating later phases with M0 actual duration.

**Execution principles**

1. **M2 vertical slice most important node**: first time almost all engine modules work together, exposes most problems.
2. Each phase only engine capabilities game of that phase needs, verify with game then abstract.
3. Graybox first: don't batch-produce art before feel and levels finalized, replace with real assets after M2.
4. At end of each phase leave runnable version (tag), for rollback and feel comparison.

## 16. Scope control and risks

Biggest risk not failing a feature, but **always working on engine, never having playable game**. Below are items most likely to spiral and suggested handling.

| Risk | Why spirals | Suggested handling |
| --- | --- | --- |
| Feel never tuned right | Parameters interlocked, no replay means no comparison | Build input replay and hot reload first, then tune numbers |
| Room switch and reset bugs | Temporary and permanent states confused | Strictly separate two state kinds in §9, write test room first |
| Level content drags progress | Many rooms, tools clumsy | First build 15-room vertical slice, then expand |
| Map screen and exploration % | Seems simple but touches save and UI | Put after milestone 4 |
| Boss balance tuning | Phase scripts, telegraphs, retry all need polish | First version only 1 boss, rest deferred |
| Premature generalization | Designing abstractions for "future games" | Only abstract things used twice+ in this project |
| Art asset gap | Large animation and tile volume | Graybox placeholder first, art after feel and levels fixed |

**Suggested cut or defer**

- Water / airflow special media (ability 6)
- Grab and carry objects
- Complex layered music (first do simple switching)
- Achievement system (keep stats only)
- Lighting and complex post-processing
- Mouse menu support

**Three questions for deciding whether to build a feature**

1. Does it push some engine module to be truly used for first time?
2. Without it, can vertical slice still be playable?
3. Can it reach verifiable state within one weekend?

If two of three answers are "no", put it in backlog first.
