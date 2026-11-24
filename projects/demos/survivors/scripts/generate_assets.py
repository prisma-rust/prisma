#!/usr/bin/env python3
"""Generate pixel-art sprite atlas, data-driven slice metadata, and WAV SFX for Potato Survivors."""

from pathlib import Path
import json
import math
import random
import struct
import wave

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
SPRITES_DIR = ROOT / "assets" / "sprites"
AUDIO_DIR = ROOT / "assets" / "audio"
DATA_DIR = ROOT / "assets" / "data"

OUTLINE = (22, 18, 30, 255)


def put_px(img: Image.Image, x: int, y: int, color: tuple[int, int, int, int]) -> None:
    if 0 <= x < img.width and 0 <= y < img.height:
        img.putpixel((x, y), color)


def draw_pixel_sprite(
    atlas: Image.Image,
    ox: int,
    oy: int,
    rows: list[str],
    palette: dict[str, tuple[int, int, int, int]],
) -> None:
    for dy, row in enumerate(rows):
        for dx, ch in enumerate(row):
            if ch != "." and ch in palette:
                put_px(atlas, ox + dx, oy + dy, palette[ch])


def draw_potato_hero(atlas: Image.Image) -> None:
    pal = {
        "O": OUTLINE,
        "B": (212, 162, 102, 255),
        "L": (242, 198, 134, 255),
        "S": (168, 118, 66, 255),
        "H": (220, 62, 74, 255),
        "h": (165, 36, 48, 255),
        "E": (28, 24, 36, 255),
        "W": (250, 250, 255, 255),
        "F": (122, 82, 46, 255),
    }
    idle_frames = [
        [
            "................",
            ".....OOOOOO.....",
            "....OLLLLLLO....",
            "...OLBBBBBBBO...",
            "...OHHHHHHHHH...",
            "..OhHHHHHHHHh...",
            "..OBBOWBBOOWBO..",
            "..OBBEOBBOOEBO..",
            "..OBBBBBBBBBBO..",
            "..OBBSSSSSSBSO..",
            "...OSSSSSSSSO...",
            "....OOOOOOOO....",
            "....OFO..OFO....",
            "....OOO..OOO....",
            "................",
            "................",
        ],
        [
            "................",
            "................",
            ".....OOOOOO.....",
            "....OLLLLLLO....",
            "...OLBBBBBBBO...",
            "...OHHHHHHHHH...",
            "..OhHHHHHHHHh...",
            "..OBBOWBBOOWBO..",
            "..OBBEOBBOOEBO..",
            "..OBBBBBBBBBBO..",
            "..OBBSSSSSSBSO..",
            "...OSSSSSSSSO...",
            "....OFO..OFO....",
            "....OOO..OOO....",
            "................",
            "................",
        ],
    ]
    run_frames = [
        idle_frames[0],
        [
            "................",
            ".....OOOOOO.....",
            "....OLLLLLLO....",
            "...OLBBBBBBBO...",
            "...OHHHHHHHHHh..",
            "..OhHHHHHHHHh...",
            "..OBBOWBBOOWBO..",
            "..OBBEOBBOOEBO..",
            "..OBBBBBBBBBBO..",
            "..OBBSSSSSSBSO..",
            "...OSSSSSSSSO...",
            "....OOOOOOOO....",
            "...OFO....OFO...",
            "...OOO....OOO...",
            "................",
            "................",
        ],
        idle_frames[1],
        [
            "................",
            ".....OOOOOO.....",
            "....OLLLLLLO....",
            "...OLBBBBBBBO...",
            "..hOHHHHHHHHH...",
            "..OhHHHHHHHHh...",
            "..OBBOWBBOOWBO..",
            "..OBBEOBBOOEBO..",
            "..OBBBBBBBBBBO..",
            "..OBBSSSSSSBSO..",
            "...OSSSSSSSSO...",
            "....OOOOOOOO....",
            ".....OFO.OFO....",
            ".....OOO.OOO....",
            "................",
            "................",
        ],
    ]
    for i in range(4):
        draw_pixel_sprite(atlas, i * 16, 0, idle_frames[i % 2], pal)
        draw_pixel_sprite(atlas, i * 16, 16, run_frames[i], pal)


def draw_enemies(atlas: Image.Image) -> None:
    # Slime (y=32, x=0..64)
    slime_pal = {
        "O": OUTLINE,
        "G": (84, 214, 108, 255),
        "L": (152, 245, 148, 255),
        "D": (46, 152, 74, 255),
        "E": (24, 20, 32, 255),
        "R": (255, 224, 110, 255),
    }
    slime_0 = [
        "................",
        "................",
        ".....OOOOOO.....",
        "....OLLLLLLO....",
        "...OLGGGGGGGO...",
        "..OLGGGGGGGGGO..",
        "..OGGERGGERGGD..",
        "..OGGEOGGOEGDD..",
        "..OGGGGGGGGGDD..",
        "..ODDDDDDDDDDO..",
        "...OOOOOOOOOO...",
        "................",
        "................",
        "................",
        "................",
        "................",
    ]
    slime_1 = [
        "................",
        "................",
        "................",
        "....OOOOOOOO....",
        "...OLLLLLLLLO...",
        "..OLGGGGGGGGGO..",
        ".OLGGERGGERGGDO.",
        ".OGGGEOGGOEGDDO.",
        ".OGGGGGGGGGDDDO.",
        ".ODDDDDDDDDDDDO.",
        "..OOOOOOOOOOOO..",
        "................",
        "................",
        "................",
        "................",
        "................",
    ]
    for i in range(4):
        draw_pixel_sprite(atlas, i * 16, 32, slime_0 if i % 2 == 0 else slime_1, slime_pal)

    # Bat charger (y=32, x=64..128)
    bat_pal = {
        "O": OUTLINE,
        "P": (156, 72, 196, 255),
        "L": (208, 122, 236, 255),
        "D": (98, 42, 134, 255),
        "E": (255, 86, 86, 255),
    }
    bat_up = [
        "................",
        ".OO..........OO.",
        ".OLOO......OOLO.",
        ".OPLPO.OO.OPLPO.",
        "..OPPPOPPOPPPO..",
        "...ODPPPPPPDO...",
        "....OPPEEPPO....",
        "....OPPPPPPO....",
        ".....ODPPDO.....",
        "......OOOO......",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
    ]
    bat_dn = [
        "................",
        "......OOOO......",
        ".....OPLPPO.....",
        "....OPPEEPPO....",
        "...OOPPPPPPOO...",
        "..OPLODPPDOLPO..",
        ".OPLO..OO..OLPO.",
        ".OO..........OO.",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
    ]
    for i in range(4):
        draw_pixel_sprite(atlas, 64 + i * 16, 32, bat_up if i % 2 == 0 else bat_dn, bat_pal)

    # Skeleton (y=48, x=0..64)
    skel_pal = {
        "O": OUTLINE,
        "W": (236, 238, 228, 255),
        "S": (182, 186, 178, 255),
        "E": (32, 24, 38, 255),
        "R": (220, 74, 74, 255),
    }
    skel_0 = [
        ".....OOOOOO.....",
        "....OWWWWWWO....",
        "...OWWWWWWWWO...",
        "...OWEEWWEEWO...",
        "...OWRWWWWWRW...",
        "....OWWWWWWO....",
        ".....OOOOOO.....",
        "....OWSSSSWO....",
        "...OW.OWWO.WO...",
        "...OO.OWWO.OO...",
        "......OSSO......",
        ".....OW..WO.....",
        ".....OW..WO.....",
        ".....OO..OO.....",
        "................",
        "................",
    ]
    skel_1 = [
        "................",
        ".....OOOOOO.....",
        "....OWWWWWWO....",
        "...OWWWWWWWWO...",
        "...OWEEWWEEWO...",
        "...OWRWWWWWRW...",
        "....OWWWWWWO....",
        ".....OOOOOO.....",
        "....OWSSSSWO....",
        "....O.OWWO.O....",
        "......OSSO......",
        "....OW....WO....",
        "....OO....OO....",
        "................",
        "................",
        "................",
    ]
    for i in range(4):
        draw_pixel_sprite(atlas, i * 16, 48, skel_0 if i % 2 == 0 else skel_1, skel_pal)

    # Goblin shooter (y=48, x=64..128)
    gob_pal = {
        "O": OUTLINE,
        "C": (66, 134, 92, 255),
        "D": (42, 86, 62, 255),
        "G": (122, 206, 98, 255),
        "Y": (255, 222, 84, 255),
        "B": (140, 86, 48, 255),
    }
    gob_0 = [
        ".....OOOOOO.....",
        "....OCCCCCCCO...",
        "...OCCCCCCCCCO..",
        "..OOCGGGGGGCOO..",
        "..ODCGYGGYGCD...",
        "...OCGGGGGGCO...",
        "...ODCCCCCCDO...",
        "..ODCCCCCCCCDO..",
        "..OBBCCCCCCBBO..",
        "...OODDDDDDOO...",
        "....OGG..GGO....",
        "....OOO..OOO....",
        "................",
        "................",
        "................",
        "................",
    ]
    gob_1 = [
        "................",
        ".....OOOOOO.....",
        "....OCCCCCCCO...",
        "...OCCCCCCCCCO..",
        "..OOCGGGGGGCOO..",
        "..ODCGYGGYGCD...",
        "...OCGGGGGGCO...",
        "...ODCCCCCCDO...",
        "..ODCCCCCCCCDO..",
        "..OBBCCCCCCBBO..",
        "...OODDDDDDOO...",
        "...OGG....GGO...",
        "...OOO....OOO...",
        "................",
        "................",
        "................",
    ]
    for i in range(4):
        draw_pixel_sprite(atlas, 64 + i * 16, 48, gob_0 if i % 2 == 0 else gob_1, gob_pal)

    # Orc bruiser (24x24 at y=64, x=0..96)
    draw = ImageDraw.Draw(atlas)
    for i in range(4):
        bx = i * 24
        by = 64
        bob = i % 2
        draw.rounded_rectangle((bx + 4, by + 3 + bob, bx + 19, by + 20 + bob), radius=4, fill=(186, 78, 58, 255), outline=OUTLINE)
        draw.rectangle((bx + 2, by + 9 + bob, bx + 6, by + 16 + bob), fill=(116, 124, 144, 255), outline=OUTLINE)
        draw.rectangle((bx + 17, by + 9 + bob, bx + 21, by + 16 + bob), fill=(116, 124, 144, 255), outline=OUTLINE)
        draw.rectangle((bx + 7, by + 5 + bob, bx + 16, by + 11 + bob), fill=(214, 98, 72, 255))
        draw.point((bx + 9, by + 8 + bob), fill=(255, 232, 92, 255))
        draw.point((bx + 14, by + 8 + bob), fill=(255, 232, 92, 255))
        draw.rectangle((bx + 8, by + 12 + bob, bx + 9, by + 14 + bob), fill=(246, 240, 224, 255))
        draw.rectangle((bx + 14, by + 12 + bob, bx + 15, by + 14 + bob), fill=(246, 240, 224, 255))
        leg_dx = 1 if i == 1 else (-1 if i == 3 else 0)
        draw.rectangle((bx + 6 + leg_dx, by + 19, bx + 10 + leg_dx, by + 23), fill=(96, 48, 42, 255), outline=OUTLINE)
        draw.rectangle((bx + 13 - leg_dx, by + 19, bx + 17 - leg_dx, by + 23), fill=(96, 48, 42, 255), outline=OUTLINE)

    # Demon Boss (32x32 at y=96, x=0..128)
    for i in range(4):
        bx = i * 32
        by = 96
        bob = i % 2
        # Horns
        draw.polygon([(bx + 6, by + 10 + bob), (bx + 2, by + 2 + bob), (bx + 11, by + 6 + bob)], fill=(238, 198, 92, 255), outline=OUTLINE)
        draw.polygon([(bx + 25, by + 10 + bob), (bx + 29, by + 2 + bob), (bx + 20, by + 6 + bob)], fill=(238, 198, 92, 255), outline=OUTLINE)
        # Torso & armor
        draw.rounded_rectangle((bx + 6, by + 6 + bob, bx + 25, by + 26 + bob), radius=6, fill=(164, 34, 52, 255), outline=OUTLINE)
        draw.rounded_rectangle((bx + 9, by + 10 + bob, bx + 22, by + 19 + bob), radius=3, fill=(214, 56, 72, 255))
        # Glowing eyes & crown gem
        draw.rectangle((bx + 14, by + 6 + bob, bx + 17, by + 9 + bob), fill=(255, 220, 96, 255))
        draw.rectangle((bx + 10, by + 12 + bob, bx + 13, by + 14 + bob), fill=(255, 246, 130, 255))
        draw.rectangle((bx + 18, by + 12 + bob, bx + 21, by + 14 + bob), fill=(255, 246, 130, 255))
        # Claws
        draw.rectangle((bx + 2, by + 15 + bob, bx + 7, by + 22 + bob), fill=(92, 24, 36, 255), outline=OUTLINE)
        draw.rectangle((bx + 24, by + 15 + bob, bx + 29, by + 22 + bob), fill=(92, 24, 36, 255), outline=OUTLINE)
        # Legs
        draw.rectangle((bx + 9, by + 25, bx + 14, by + 30), fill=(86, 22, 34, 255), outline=OUTLINE)
        draw.rectangle((bx + 17, by + 25, bx + 22, by + 30), fill=(86, 22, 34, 255), outline=OUTLINE)


def draw_weapons_and_items(atlas: Image.Image) -> None:
    draw = ImageDraw.Draw(atlas)

    # Pistol [0, 128, 16, 16]
    draw.rectangle((2, 133, 13, 136), fill=(176, 188, 206, 255), outline=OUTLINE)
    draw.rectangle((3, 136, 7, 141), fill=(142, 88, 52, 255), outline=OUTLINE)
    draw.point((12, 134), fill=(246, 248, 255, 255))

    # SMG [16, 128, 16, 16]
    draw.rectangle((18, 132, 30, 136), fill=(76, 86, 104, 255), outline=OUTLINE)
    draw.rectangle((20, 136, 23, 141), fill=(48, 54, 68, 255), outline=OUTLINE)
    draw.rectangle((25, 136, 27, 142), fill=(120, 132, 152, 255), outline=OUTLINE)

    # Shotgun [32, 128, 16, 16]
    draw.rectangle((33, 133, 47, 136), fill=(148, 158, 174, 255), outline=OUTLINE)
    draw.rectangle((33, 136, 39, 139), fill=(156, 94, 52, 255), outline=OUTLINE)
    draw.rectangle((40, 136, 44, 138), fill=(104, 64, 38, 255), outline=OUTLINE)

    # Arcane Wand [48, 128, 16, 16]
    draw.rectangle((49, 135, 59, 137), fill=(134, 84, 52, 255), outline=OUTLINE)
    draw.ellipse((57, 132, 63, 138), fill=(108, 232, 255, 255), outline=(42, 124, 198, 255))
    draw.point((59, 134), fill=(255, 255, 255, 255))

    # Rusty Blade / Sword [64, 128, 24, 16]
    draw.rectangle((65, 134, 70, 137), fill=(146, 94, 54, 255), outline=OUTLINE)
    draw.rectangle((70, 131, 72, 140), fill=(224, 184, 86, 255), outline=OUTLINE)
    draw.polygon([(72, 133), (86, 134), (87, 136), (72, 138)], fill=(208, 222, 238, 255), outline=OUTLINE)

    # Spear [96, 128, 28, 16]
    draw.rectangle((97, 135, 117, 137), fill=(142, 92, 54, 255), outline=OUTLINE)
    draw.polygon([(116, 133), (123, 136), (116, 139)], fill=(214, 230, 246, 255), outline=OUTLINE)

    # Projectiles (y=144)
    # Yellow bullet [0, 144, 8, 8]
    draw.ellipse((1, 145, 6, 150), fill=(255, 228, 92, 255), outline=OUTLINE)
    draw.point((3, 147), fill=(255, 255, 240, 255))

    # Orange pellet [16, 144, 8, 8]
    draw.ellipse((17, 145, 22, 150), fill=(255, 136, 56, 255), outline=OUTLINE)

    # Magic orb [32, 144, 12, 12]
    draw.ellipse((33, 145, 42, 154), fill=(92, 232, 255, 255), outline=(66, 112, 224, 255))
    draw.ellipse((36, 148, 39, 151), fill=(240, 255, 255, 255))

    # Enemy fireball [48, 144, 10, 10]
    draw.ellipse((49, 145, 56, 152), fill=(255, 72, 86, 255), outline=(128, 18, 34, 255))
    draw.ellipse((51, 147, 54, 150), fill=(255, 212, 116, 255))

    # XP Gem small [64, 144, 10, 10]
    draw.polygon([(69, 145), (73, 149), (69, 153), (65, 149)], fill=(72, 224, 255, 255), outline=OUTLINE)
    draw.point((68, 148), fill=(224, 255, 255, 255))

    # XP Gem large [80, 144, 12, 12]
    draw.polygon([(86, 145), (91, 150), (86, 155), (81, 150)], fill=(196, 104, 255, 255), outline=OUTLINE)
    draw.point((85, 148), fill=(250, 220, 255, 255))

    # Gold Coin 2 frames [96, 144, 10, 10] and [106, 144, 10, 10]
    draw.ellipse((97, 145, 104, 152), fill=(255, 206, 56, 255), outline=OUTLINE)
    draw.rectangle((100, 147, 101, 150), fill=(255, 246, 168, 255))
    draw.ellipse((109, 145, 112, 152), fill=(255, 206, 56, 255), outline=OUTLINE)

    # Heal drop [128, 144, 12, 12]
    draw.ellipse((130, 146, 134, 150), fill=(242, 68, 86, 255))
    draw.ellipse((134, 146, 138, 150), fill=(242, 68, 86, 255))
    draw.polygon([(130, 149), (138, 149), (134, 154)], fill=(242, 68, 86, 255))

    # Melee Slash Arc [0, 160, 24, 32]
    for y in range(162, 190):
        dy = (y - 176) / 14.0
        if abs(dy) <= 1.0:
            x_outer = int(6 + 15 * math.sqrt(max(0.0, 1.0 - dy * dy)))
            x_inner = max(4, x_outer - 5)
            for x in range(x_inner, x_outer + 1):
                alpha = 235 if x >= x_outer - 2 else 160
                color = (232, 248, 255, alpha) if x >= x_outer - 2 else (128, 212, 255, alpha)
                put_px(atlas, x, y, color)

    # Melee Thrust Streak [32, 160, 32, 16]
    draw.polygon([(33, 167), (63, 168), (33, 169)], fill=(240, 250, 255, 245))
    draw.polygon([(36, 165), (61, 168), (36, 171)], fill=(128, 216, 255, 145))

    # Spawn Warning Marker [64, 160, 16, 16]
    draw.line((66, 162, 77, 173), fill=(255, 72, 72, 220), width=2)
    draw.line((66, 173, 77, 162), fill=(255, 72, 72, 220), width=2)
    draw.ellipse((65, 161, 78, 174), outline=(255, 92, 92, 180))

    # Arena Floor Tile [80, 160, 32, 32]
    draw.rectangle((80, 160, 111, 191), fill=(32, 36, 48, 255))
    draw.rectangle((80, 160, 111, 191), outline=(42, 47, 62, 255))
    draw.line((84, 168, 96, 168), fill=(25, 29, 40, 255), width=1)
    draw.line((96, 182, 108, 182), fill=(25, 29, 40, 255), width=1)
    draw.point((88, 176), fill=(48, 54, 72, 255))
    draw.point((102, 165), fill=(48, 54, 72, 255))

    # Arena Crate / Pillar Prop [112, 160, 24, 24]
    draw.rectangle((113, 161, 134, 182), fill=(118, 76, 48, 255), outline=OUTLINE)
    draw.rectangle((115, 163, 132, 180), fill=(144, 96, 62, 255), outline=(82, 52, 32, 255))
    draw.line((115, 163, 132, 180), fill=(96, 62, 38, 255), width=2)


def emit_sprites_json() -> None:
    sprites = {
        "player_idle": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 0, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 6.0,
            "looped": True,
        },
        "player_run": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 16, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 12.0,
            "looped": True,
        },
        "enemy_slime": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 32, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 8.0,
            "looped": True,
        },
        "enemy_bat": {
            "texture": "sprites/atlas.png",
            "rect_px": [64, 32, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 12.0,
            "looped": True,
        },
        "enemy_skeleton": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 48, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 8.0,
            "looped": True,
        },
        "enemy_goblin": {
            "texture": "sprites/atlas.png",
            "rect_px": [64, 48, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 8.0,
            "looped": True,
        },
        "enemy_orc": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 64, 24, 24],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 4,
            "fps": 8.0,
            "looped": True,
        },
        "enemy_boss": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 96, 32, 32],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 14.0,
            "frames": 4,
            "fps": 6.0,
            "looped": True,
        },
        "weapon_pistol": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 128, 16, 16],
            "pivot": [0.35, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "weapon_smg": {
            "texture": "sprites/atlas.png",
            "rect_px": [16, 128, 16, 16],
            "pivot": [0.35, 0.5],
            "pixels_per_unit": 17.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "weapon_shotgun": {
            "texture": "sprites/atlas.png",
            "rect_px": [32, 128, 16, 16],
            "pivot": [0.3, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "weapon_wand": {
            "texture": "sprites/atlas.png",
            "rect_px": [48, 128, 16, 16],
            "pivot": [0.3, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "weapon_blade": {
            "texture": "sprites/atlas.png",
            "rect_px": [64, 128, 24, 16],
            "pivot": [0.2, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "weapon_spear": {
            "texture": "sprites/atlas.png",
            "rect_px": [96, 128, 28, 16],
            "pivot": [0.18, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "proj_bullet": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 144, 8, 8],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 20.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "proj_pellet": {
            "texture": "sprites/atlas.png",
            "rect_px": [16, 144, 8, 8],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 22.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "proj_orb": {
            "texture": "sprites/atlas.png",
            "rect_px": [32, 144, 12, 12],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 20.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "proj_enemy_fire": {
            "texture": "sprites/atlas.png",
            "rect_px": [48, 144, 10, 10],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "pickup_xp_small": {
            "texture": "sprites/atlas.png",
            "rect_px": [64, 144, 10, 10],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "pickup_xp_large": {
            "texture": "sprites/atlas.png",
            "rect_px": [80, 144, 12, 12],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "pickup_coin": {
            "texture": "sprites/atlas.png",
            "rect_px": [96, 144, 10, 10],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 2,
            "fps": 6.0,
            "looped": True,
        },
        "pickup_heal": {
            "texture": "sprites/atlas.png",
            "rect_px": [128, 144, 12, 12],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 18.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "fx_slash": {
            "texture": "sprites/atlas.png",
            "rect_px": [0, 160, 24, 32],
            "pivot": [0.15, 0.5],
            "pixels_per_unit": 12.0,
            "frames": 1,
            "fps": 0.0,
            "looped": False,
        },
        "fx_thrust": {
            "texture": "sprites/atlas.png",
            "rect_px": [32, 160, 32, 16],
            "pivot": [0.1, 0.5],
            "pixels_per_unit": 10.0,
            "frames": 1,
            "fps": 0.0,
            "looped": False,
        },
        "fx_spawn_ring": {
            "texture": "sprites/atlas.png",
            "rect_px": [64, 160, 16, 16],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 14.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "arena_tile": {
            "texture": "sprites/atlas.png",
            "rect_px": [80, 160, 32, 32],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
        "arena_crate": {
            "texture": "sprites/atlas.png",
            "rect_px": [112, 160, 24, 24],
            "pivot": [0.5, 0.5],
            "pixels_per_unit": 16.0,
            "frames": 1,
            "fps": 0.0,
            "looped": True,
        },
    }
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    (DATA_DIR / "sprites.json").write_text(
        json.dumps({"sprites": sprites}, indent=2) + "\n", encoding="utf-8"
    )


def synth_wave(samples: list[float], path: Path, sample_rate: int = 22050) -> None:
    pcm = bytearray()
    for s in samples:
        clamped = max(-1.0, min(1.0, s))
        pcm.extend(struct.pack("<h", int(clamped * 32767)))
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(bytes(pcm))


def tone(freq: float, duration: float, sweep: float = 0.0, amp: float = 0.25, square: bool = False) -> list[float]:
    sr = 22050
    n = int(sr * duration)
    out = []
    for i in range(n):
        t = i / sr
        env = (1.0 - i / n) ** 1.8
        phase = 2.0 * math.pi * (freq * t + 0.5 * sweep * t * t)
        val = math.sin(phase)
        if square:
            val = 1.0 if val >= 0.0 else -1.0
        out.append(val * env * amp)
    return out


def noise_burst(duration: float, amp: float = 0.22, seed: int = 7) -> list[float]:
    rng = random.Random(seed)
    sr = 22050
    n = int(sr * duration)
    return [(rng.uniform(-1.0, 1.0) * ((1.0 - i / n) ** 2.2) * amp) for i in range(n)]


def generate_audio() -> None:
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    synth_wave(tone(740.0, 0.09, -3200.0, 0.24), AUDIO_DIR / "shoot.wav")
    whoosh = [a + b for a, b in zip(tone(220.0, 0.14, 650.0, 0.15), noise_burst(0.14, 0.18))]
    synth_wave(whoosh, AUDIO_DIR / "melee_swing.wav")
    synth_wave(tone(310.0, 0.06, -900.0, 0.22), AUDIO_DIR / "hit_enemy.wav")
    synth_wave(tone(260.0, 0.14, -1100.0, 0.25, square=True), AUDIO_DIR / "enemy_die.wav")
    synth_wave(tone(150.0, 0.22, -320.0, 0.30, square=True), AUDIO_DIR / "player_hurt.wav")
    synth_wave(tone(880.0, 0.08, 1200.0, 0.18), AUDIO_DIR / "pickup_gem.wav")
    synth_wave(tone(988.0, 0.06, 0.0, 0.18) + tone(1318.5, 0.10, 0.0, 0.20), AUDIO_DIR / "pickup_coin.wav")
    synth_wave(
        tone(523.25, 0.09, 0.0, 0.22)
        + tone(659.25, 0.09, 0.0, 0.22)
        + tone(783.99, 0.09, 0.0, 0.22)
        + tone(1046.5, 0.20, 0.0, 0.26),
        AUDIO_DIR / "level_up.wav",
    )
    synth_wave(tone(587.3, 0.07, 450.0, 0.22), AUDIO_DIR / "shop_buy.wav")
    synth_wave(tone(440.0, 0.06, 800.0, 0.18) + tone(520.0, 0.06, 600.0, 0.18), AUDIO_DIR / "shop_reroll.wav")
    synth_wave(tone(330.0, 0.12, 220.0, 0.24) + tone(495.0, 0.22, 0.0, 0.26), AUDIO_DIR / "wave_start.wav")
    synth_wave(
        tone(440.0, 0.10, 0.0, 0.22)
        + tone(554.37, 0.10, 0.0, 0.22)
        + tone(659.25, 0.12, 0.0, 0.24)
        + tone(880.0, 0.24, 0.0, 0.26),
        AUDIO_DIR / "wave_clear.wav",
    )


def main() -> None:
    SPRITES_DIR.mkdir(parents=True, exist_ok=True)
    atlas = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    draw_potato_hero(atlas)
    draw_enemies(atlas)
    draw_weapons_and_items(atlas)
    atlas.save(SPRITES_DIR / "atlas.png")
    emit_sprites_json()
    generate_audio()
    print(f"Generated atlas ({atlas.size}), sprites.json, and {len(list(AUDIO_DIR.glob('*.wav')))} WAV files")


if __name__ == "__main__":
    main()
