# Prisma showcase video

This project contains automatically generated engine showcase videos.

## Generated content

### Video: `target/showcase/prisma_showcase.mp4`

- **Duration**: about 96 seconds (1 min 36 sec)
- **Resolution**: 1920×1080 @ 30fps
- **Encoding**: H.264 (libx264) + AAC audio

### Video structure

1. **Title card** (4 sec)
   - Prisma brand title
   - "A Modern Mobile-First Game Engine" subtitle

2. **Feature showcase clips** (10 scenes, each 9-10 sec)
   - **Mobile-First Architecture** — TBDR optimization, transient attachment, bandwidth optimization
   - **Modular Render Graph** — RenderPassNode architecture, automatic barriers, pluggable passes
   - **Advanced Lighting** — PBR materials, IBL, CSM shadows, baked GI
   - **Data-Oriented ECS** — archetype query, reflection serialization, RSCN v4
   - **Asset Pipeline** — glTF import, texture compression, .pak packaging
   - **Native Rust Editor** — egui + ash zero-copy sampling, split-tree docking, reflection inspector
   - **Bindless Everything** — descriptor indexing, dynamic batching
   - **Slang Shaders** — unified shader language, reflection-driven bindings
   - **Built-In Automation** — local TCP control channel, MCP server, native window and offscreen capture

3. **End card** (5 sec)
   - Prisma logo
   - "Modern · Mobile-First · Modular" slogan
   - GitHub link

### Visual design

**Color scheme**:
- Base background: deep space blue (#05070C → #0F131C)
- Accent color: teal (#38BDF8, #6EE7B7)
- Text: white (#F0F2F5) + gray (#A0AAB4)

**Animation**:
- Particle background (slowly moving light dots)
- Text fade-in effect (layered cascade)
- Gradient background transition

### Music

Programmatically generated ambient drone music:
- Multi-layer overtone superposition (A minor chord)
- Low-frequency sub-bass rumble
- 1.5 sec fade in/out

## Usage

### Generate video

```bash
python scripts/showcase_video_simple.py
```

Output location: `target/showcase/prisma_showcase.mp4`

### Dependencies

Auto-installed:
- `moviepy` — video synthesis
- `numpy` — math operations

### System requirements

- Python 3.8+
- FFmpeg (moviepy auto-downloads imageio-ffmpeg)
- About 2GB temporary space

## Customization

Edit `scripts/showcase_video_simple.py`:

### Modify colors

```python
# Colors (deep space blue + teal accent)
BG = np.array([5, 7, 12], dtype=np.uint8)
ACCENT_CYAN = np.array([56, 189, 248], dtype=np.uint8)
```

### Add/modify scenes

Add to the `scenes` list in `main()`:

```python
("feature", "Your Title",
 "Subtitle",
 [
     "Feature 1",
     "Feature 2",
     "Feature 3"
 ], 10.0),  # duration (sec)
```

### Adjust resolution

```python
W, H = 1920, 1080  # modify to desired resolution
FPS = 30           # frame rate
```

## Technical details

### Background generation

- **Gradient background**: numpy row-by-row color interpolation
- **Particle background**: 40 random particles, each with independent position, velocity, size, and brightness
- Particle movement speed: 15px/s (slow movement)

### Audio synthesis

- Sample rate: 44.1kHz
- Frequency: 55Hz (A1) and its harmonics (110, 165, 220, 330, 440Hz)
- Envelope modulation: 0.08Hz slow LFO
- Sub-bass: 27.5Hz (A0)

### Render parameters

```python
codec='libx264'      # H.264 encoding
audio_codec='aac'    # AAC audio
preset='medium'      # encoding preset (speed/quality balance)
bitrate='6000k'      # 6Mbps video bitrate
threads=4            # parallel encoding threads
```

## Future extensions

Possible improvement directions:

1. **Real-time screenshot integration**
   - Use `PRISMA_SCREENSHOT` to capture actual engine rendered frames
   - Currently pure animation demo

2. **More complex animations**
   - Code rain effect (Matrix style)
   - 3D wireframe animation
   - Shader effect overlay

3. **Music upgrade**
   - MIDI synthesis
   - Real instrument sampling
   - Dynamic music (changes with scene)

4. **Multilingual version**
   - Chinese/English subtitle switching
   - Dubbing support

## License

This video generation script uses the same license as Prisma engine (Apache 2.0 / MIT dual license).

Generated videos can be used for project promotion, technical talks, teaching, etc.
