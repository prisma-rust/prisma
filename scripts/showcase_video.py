#!/usr/bin/env python3
"""
Prisma engine showcase video production script

Uses moviepy to generate a video showcasing Prisma engine features:
- Title sequence
- Engine screenshot slideshow (from pre-rendered keyframes)
- Feature annotation overlays
- Background music (procedurally generated ambient music)
"""

import os
import sys
import subprocess
import json
from pathlib import Path
from typing import List, Tuple

try:
    import moviepy
    from moviepy import (
        VideoClip, ImageClip, TextClip, CompositeVideoClip,
        AudioFileClip, concatenate_videoclips, vfx
    )
    import numpy as np
except ImportError:
    print("Error: moviepy not found. Installing...")
    subprocess.check_call(
        [sys.executable, "-m", "pip", "install", "moviepy", "numpy"],
        text=True,
        encoding="utf-8",
    )
    import moviepy
    from moviepy import (
        VideoClip, ImageClip, TextClip, CompositeVideoClip,
        AudioFileClip, concatenate_videoclips, vfx
    )
    import numpy as np

# Video config
VIDEO_WIDTH = 1920
VIDEO_HEIGHT = 1080
FPS = 30
TOTAL_DURATION = 120  # 2 minutes

# Color scheme (referencing Prisma engineering property: Vulkan blue + amber accent)
BG_COLOR = (5, 7, 12)  # Deep space blue
SURFACE_1 = (10, 13, 18)
SURFACE_2 = (15, 19, 28)
ACCENT_CYAN = (56, 189, 248)
ACCENT_AMBER = (233, 165, 104)
TEXT_PRIMARY = (240, 242, 245)
TEXT_SECONDARY = (160, 170, 180)


class VideoScene:
    """Video scene definitions"""
    def __init__(self, duration: float, title: str, subtitle: str,
                 features: List[str], screenshot_path: str = None):
        self.duration = duration
        self.title = title
        self.subtitle = subtitle
        self.features = features
        self.screenshot_path = screenshot_path


def generate_ambient_audio(duration: float, output_path: str):
    """Generate ambient audio (simple ambient drone with numpy)"""
    print(f"Generating ambient audio ({duration}s)...")

    sample_rate = 44100
    t = np.linspace(0, duration, int(sample_rate * duration))

    # Multi-layer sine wave blending (simulating ambient pad)
    frequencies = [55, 110, 165, 220, 330]  # A chord overtones
    audio = np.zeros_like(t)

    for i, freq in enumerate(frequencies):
        # Each frequency has different envelope and phase
        envelope = 0.3 * (1 + 0.3 * np.sin(2 * np.pi * 0.1 * (i + 1) * t))
        phase = 2 * np.pi * freq * t + 0.5 * np.sin(2 * np.pi * 0.05 * t)
        audio += envelope * np.sin(phase) / len(frequencies)

    # Add low-frequency rumble
    rumble = 0.15 * np.sin(2 * np.pi * 27.5 * t)
    audio += rumble

    # Normalize
    audio = audio / np.max(np.abs(audio)) * 0.7

    # Fade in/out
    fade_samples = int(sample_rate * 2)
    audio[:fade_samples] *= np.linspace(0, 1, fade_samples)
    audio[-fade_samples:] *= np.linspace(1, 0, fade_samples)

    # Convert to 16-bit PCM
    audio_int16 = (audio * 32767).astype(np.int16)

    # Save as WAV (using scipy or directly write WAV format)
    try:
        from scipy.io import wavfile
        wavfile.write(output_path, sample_rate, audio_int16)
    except ImportError:
        # Manually write WAV header
        import struct
        with open(output_path, 'wb') as f:
            # RIFF header
            f.write(b'RIFF')
            f.write(struct.pack('<I', 36 + len(audio_int16) * 2))
            f.write(b'WAVE')
            # fmt chunk
            f.write(b'fmt ')
            f.write(struct.pack('<I', 16))  # chunk size
            f.write(struct.pack('<H', 1))   # PCM
            f.write(struct.pack('<H', 1))   # mono
            f.write(struct.pack('<I', sample_rate))
            f.write(struct.pack('<I', sample_rate * 2))  # byte rate
            f.write(struct.pack('<H', 2))   # block align
            f.write(struct.pack('<H', 16))  # bits per sample
            # data chunk
            f.write(b'data')
            f.write(struct.pack('<I', len(audio_int16) * 2))
            f.write(audio_int16.tobytes())

    print(f"Audio saved to {output_path}")


def capture_engine_screenshots(scenes: List[str], output_dir: Path) -> List[Path]:
    """Capture engine scene screenshots"""
    output_dir.mkdir(parents=True, exist_ok=True)
    screenshot_paths = []

    print("Capturing engine screenshots...")

    # Check project binary
    project_root = Path(__file__).parent.parent
    exe_path = project_root / "projects" / "template" / "target" / "release" / "template.exe"

    if not exe_path.exists():
        exe_path = project_root / "projects" / "template" / "target" / "debug" / "template.exe"

    if not exe_path.exists():
        print(f"Warning: Engine executable not found at {exe_path}")
        print("Please build the project first: cd projects/template && cargo build --release")
        return []

    # Capture multiple angles for each scene
    for i, scene in enumerate(scenes):
        for frame in [30, 60, 90, 120, 150]:
            screenshot_path = output_dir / f"scene_{i}_frame_{frame}.png"

            env = os.environ.copy()
            env['PRISMA_SCREENSHOT'] = str(screenshot_path)
            env['PRISMA_SCREENSHOT_FRAME'] = str(frame)
            env['PRISMA_SCREENSHOT_SIZE'] = f"{VIDEO_WIDTH}x{VIDEO_HEIGHT}"
            env['RUST_LOG'] = 'warn'

            try:
                print(f"  Capturing {scene} at frame {frame}...")
                result = subprocess.run(
                    [str(exe_path)],
                    env=env,
                    cwd=str(project_root),
                    timeout=60,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )

                if screenshot_path.exists():
                    screenshot_paths.append(screenshot_path)
                    print(f"    [OK] Saved {screenshot_path.name}")
                else:
                    print(f"    [FAIL] Failed to capture {screenshot_path.name}")

            except subprocess.TimeoutExpired:
                print(f"    [TIMEOUT] {scene}")
            except Exception as e:
                print(f"    [ERROR] {e}")

    return screenshot_paths


def create_title_card(text: str, subtitle: str, duration: float) -> VideoClip:
    """Create title card"""

    def make_frame(t):
        # Create gradient background
        frame = np.zeros((VIDEO_HEIGHT, VIDEO_WIDTH, 3), dtype=np.uint8)

        # Vertical gradient (deep blue to deeper blue)
        for y in range(VIDEO_HEIGHT):
            ratio = y / VIDEO_HEIGHT
            color = tuple(int(BG_COLOR[i] * (1 + 0.3 * ratio)) for i in range(3))
            frame[y, :] = color

        return frame

    bg_clip = VideoClip(make_frame, duration=duration)

    # Title text (with fade in)
    title_clip = TextClip(
        text,
        fontsize=80,
        color='white',
        font='Arial-Bold',
        size=(VIDEO_WIDTH - 200, None),
        method='caption'
    ).set_position('center').set_duration(duration)

    # Subtitle
    subtitle_clip = TextClip(
        subtitle,
        fontsize=36,
        color=tuple(TEXT_SECONDARY),
        font='Arial',
        size=(VIDEO_WIDTH - 200, None),
        method='caption'
    ).set_position(('center', VIDEO_HEIGHT * 0.6)).set_duration(duration)

    # Fade in effect
    title_clip = title_clip.crossfadein(0.5)
    subtitle_clip = subtitle_clip.crossfadein(1.0)

    return CompositeVideoClip([bg_clip, title_clip, subtitle_clip])


def create_feature_clip(scene: VideoScene, screenshot_paths: List[Path]) -> VideoClip:
    """Create feature showcase clip"""
    clips = []

    # If screenshots available use them; otherwise use solid color background
    if screenshot_paths:
        # Slideshow screenshots (each holds 2-3 seconds)
        screenshot_duration = scene.duration / max(1, len(screenshot_paths))
        for path in screenshot_paths:
            if path.exists():
                img_clip = ImageClip(str(path)).set_duration(screenshot_duration)
                img_clip = img_clip.resize((VIDEO_WIDTH, VIDEO_HEIGHT))
                clips.append(img_clip)

    # If no screenshots, create code background
    if not clips:
        def make_code_bg(t):
            frame = np.zeros((VIDEO_HEIGHT, VIDEO_WIDTH, 3), dtype=np.uint8)
            for y in range(VIDEO_HEIGHT):
                ratio = y / VIDEO_HEIGHT
                color = tuple(int(SURFACE_1[i] * (1 + 0.2 * ratio)) for i in range(3))
                frame[y, :] = color
            return frame

        bg_clip = VideoClip(make_code_bg, duration=scene.duration)
        clips.append(bg_clip)

    # Concatenate all backgrounds
    if len(clips) > 1:
        base_clip = concatenate_videoclips(clips)
    else:
        base_clip = clips[0]

    # Add title overlay layer
    title_clip = TextClip(
        scene.title,
        fontsize=56,
        color=tuple(ACCENT_CYAN),
        font='Arial-Bold',
        stroke_color=tuple(BG_COLOR),
        stroke_width=2
    ).set_position(('center', 80)).set_duration(scene.duration).crossfadein(0.3)

    # Add subtitle
    subtitle_clip = TextClip(
        scene.subtitle,
        fontsize=32,
        color=tuple(TEXT_PRIMARY),
        font='Arial',
        stroke_color=tuple(BG_COLOR),
        stroke_width=1
    ).set_position(('center', 160)).set_duration(scene.duration).crossfadein(0.5)

    # Add feature list (bottom left)
    if scene.features:
        feature_text = "\n".join(f"• {f}" for f in scene.features)
        features_clip = TextClip(
            feature_text,
            fontsize=24,
            color=tuple(TEXT_SECONDARY),
            font='Arial',
            align='West',
            size=(VIDEO_WIDTH - 100, None),
            method='caption'
        ).set_position((50, VIDEO_HEIGHT - 250)).set_duration(scene.duration).crossfadein(1.0)

        return CompositeVideoClip([base_clip, title_clip, subtitle_clip, features_clip])

    return CompositeVideoClip([base_clip, title_clip, subtitle_clip])


def create_end_card(duration: float) -> VideoClip:
    """Create ending card"""
    def make_frame(t):
        frame = np.zeros((VIDEO_HEIGHT, VIDEO_WIDTH, 3), dtype=np.uint8)
        frame[:, :] = BG_COLOR
        return frame

    bg_clip = VideoClip(make_frame, duration=duration)

    # GitHub link
    github_text = "github.com/yourusername/Prisma"
    github_clip = TextClip(
        github_text,
        fontsize=40,
        color=tuple(ACCENT_CYAN),
        font='Arial-Bold'
    ).set_position('center').set_duration(duration).crossfadein(0.5)

    # Tagline
    tagline = "Modern · Mobile-First · Modular"
    tagline_clip = TextClip(
        tagline,
        fontsize=32,
        color=tuple(TEXT_SECONDARY),
        font='Arial'
    ).set_position(('center', VIDEO_HEIGHT * 0.65)).set_duration(duration).crossfadein(1.0)

    return CompositeVideoClip([bg_clip, github_clip, tagline_clip])


def main():
    output_dir = Path(__file__).parent.parent / "target" / "showcase"
    output_dir.mkdir(parents=True, exist_ok=True)

    screenshots_dir = output_dir / "screenshots"
    audio_path = output_dir / "ambient.wav"
    video_path = output_dir / "prisma_showcase.mp4"

    print("=" * 60)
    print("Prisma Showcase Video Generator")
    print("=" * 60)

    # 1. Capture screenshots
    print("\n[1/4] Capturing engine screenshots...")
    available_scenes = ["default", "intro"]
    screenshot_paths = capture_engine_screenshots(available_scenes, screenshots_dir)

    if not screenshot_paths:
        print("Warning: No screenshots captured. Using placeholder backgrounds.")
    else:
        print(f"Captured {len(screenshot_paths)} screenshots")

    # 2. Generate audio
    print("\n[2/4] Generating ambient audio...")
    generate_ambient_audio(TOTAL_DURATION, str(audio_path))

    # 3. Define video scenes
    print("\n[3/4] Composing video scenes...")

    scenes = [
        # Opening title
        VideoScene(
            duration=4.0,
            title="Prisma",
            subtitle="A Modern Mobile-First Game Engine",
            features=[],
            screenshot_path=None
        ),

        # Core features
        VideoScene(
            duration=12.0,
            title="Mobile-First Architecture",
            subtitle="Optimized for TBDR GPUs · Vulkan Powered",
            features=[
                "Tile-based deferred rendering",
                "Transient attachment optimization",
                "Automatic bandwidth reduction",
                "Android + Desktop unified pipeline"
            ]
        ),

        VideoScene(
            duration=12.0,
            title="Modular Render Graph",
            subtitle="Extensible Pass-Based Design",
            features=[
                "RenderPassNode architecture",
                "Automatic resource barriers",
                "Pluggable feature passes",
                "Runtime capability detection"
            ]
        ),

        VideoScene(
            duration=12.0,
            title="Advanced Lighting",
            subtitle="PBR · IBL · Cascaded Shadows",
            features=[
                "Physically-based materials",
                "Image-based lighting",
                "Cascaded shadow maps",
                "Baked probe-volume GI"
            ]
        ),

        VideoScene(
            duration=12.0,
            title="Data-Oriented ECS",
            subtitle="High-Performance Entity Component System",
            features=[
                "Cache-friendly queries",
                "Reflection-driven serialization",
                "Component versioning",
                "Cross-platform scenes (RSCN v4)"
            ]
        ),

        VideoScene(
            duration=12.0,
            title="Asset Pipeline",
            subtitle="Offline Cooker · Runtime Pak Loading",
            features=[
                "glTF 2.0 mesh import",
                "Texture compression (BC7/ASTC)",
                "Scene package (.pak)",
                "Hot-reload ready"
            ]
        ),

        VideoScene(
            duration=12.0,
            title="Web-Based Editor",
            subtitle="Tauri 2 + Vue 3 · Live Editing",
            features=[
                "Component inspector",
                "Hierarchy drag-and-drop",
                "Prefab system",
                "Undo/redo with field merging"
            ]
        ),

        VideoScene(
            duration=10.0,
            title="Bindless Everything",
            subtitle="Modern Descriptor Management",
            features=[
                "Descriptor indexing",
                "Dynamic material batching",
                "GPU-driven rendering ready",
                "Fallback to legacy descriptors"
            ]
        ),

        VideoScene(
            duration=10.0,
            title="Slang Shaders",
            subtitle="Cross-Platform Shader Compilation",
            features=[
                "Unified shader language",
                "Reflection-driven bindings",
                "Auto-generated Rust structs",
                "SPIR-V output"
            ]
        ),

        VideoScene(
            duration=10.0,
            title="Screenshot & Automation",
            subtitle="Built-In Capture System",
            features=[
                "PRISMA_SCREENSHOT env var",
                "Configurable resolution/frame",
                "MCP server for editor automation",
                "CDP-driven UI testing"
            ]
        ),

        VideoScene(
            duration=8.0,
            title="Open Source",
            subtitle="Built with Rust · Vulkan · Love",
            features=[
                "Apache 2.0 / MIT dual license",
                "Contributions welcome",
                "Active development",
                "Transparent design docs"
            ]
        ),
    ]

    # Create title card
    title_card = create_title_card(
        "Prisma",
        "A Modern Mobile-First Game Engine",
        4.0
    )

    # Create all feature clips
    video_clips = [title_card]

    # Assign screenshots to each scene
    screenshots_per_scene = max(1, len(screenshot_paths) // len(scenes))
    for i, scene in enumerate(scenes):
        start_idx = i * screenshots_per_scene
        end_idx = start_idx + screenshots_per_scene
        scene_screenshots = screenshot_paths[start_idx:end_idx]

        clip = create_feature_clip(scene, scene_screenshots)
        video_clips.append(clip)

    # Ending card
    end_card = create_end_card(6.0)
    video_clips.append(end_card)

    # 4. Concatenate and output
    print("\n[4/4] Rendering final video...")

    final_video = concatenate_videoclips(video_clips, method="compose")

    # Add audio
    if audio_path.exists():
        audio = AudioFileClip(str(audio_path))
        final_video = final_video.set_audio(audio)

    # output
    print(f"\nWriting video to {video_path}...")
    final_video.write_videofile(
        str(video_path),
        fps=FPS,
        codec='libx264',
        audio_codec='aac',
        preset='medium',
        bitrate='8000k'
    )

    print("\n" + "=" * 60)
    print(f"[SUCCESS] Showcase video created: {video_path}")
    print(f"  Duration: {final_video.duration:.1f}s")
    print(f"  Resolution: {VIDEO_WIDTH}x{VIDEO_HEIGHT}")
    print("=" * 60)


if __name__ == '__main__':
    main()
